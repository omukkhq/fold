use std::sync::Arc;

use fold_core::checkpoint_policy::AutoReason;
use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::tree::ChangeCounts;
use fold_core::{
    Actor, ChangesetId, CheckpointId, FoldError, NewEvent, Timestamp, WorkspaceId, codes,
};
use fold_store::workspaces::{self, CheckpointReason, WorkspaceRow};
use fold_vcs::scan::{changes, count_changes};
use serde_json::json;
use tokio::sync::{OwnedMutexGuard, OwnedRwLockReadGuard};

use super::auto::auto_checkpoint_in_tx;
use super::files::{Files, owned_in_tx};
use super::{WorkspaceError, WorkspaceService, blocking};
use crate::services::events::EventBuffer;

/// The longest changeset message, in characters.
const MAX_MESSAGE_CHARS: usize = 2000;

/// A snapshot of a task workspace taken for a proposal, with the workspace lock and the shared object lock that took it. The caller keeps it until the transaction that names `tree` commits, so the stat index has one writer and garbage collection keeps the new objects.
pub(crate) struct ProposalSnapshot {
    /// Every object of it is in the object store.
    pub tree: ObjectHash,
    /// Against the workspace's base revision.
    pub stats: ChangeCounts,
    /// How many automatic checkpoints of the workspace stay unpruned.
    pub auto_kept: u32,
    /// The clock just before the snapshot, which the proposal command uses as its `now`: an edit made while the snapshot runs is then later than its checkpoint.
    pub taken_at: Timestamp,
    _held: (OwnedMutexGuard<()>, OwnedRwLockReadGuard<()>),
}

/// What `record_changes` recorded. `files` counts the paths that differ from the base revision. `checkpoint` holds the recorded files: the `before_record` checkpoint, or the latest checkpoint when it already held them on the same base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedChanges {
    pub changeset: ChangesetId,
    pub checkpoint: CheckpointId,
    pub files: u32,
}

impl WorkspaceService {
    /// Records the files of a workspace the actor's worker owns as a changeset with `message`, chained to the workspace's previous changeset. A `before_record` automatic checkpoint of the same tree is the first step of the same transaction, so a checkpoint that cannot be taken refuses the record. Refused with `unresolved_conflicts` while a conflicted path still holds a marker line or its `<path>.fold-main`; once none does, the conflict list is cleared with the record. Refused with `workspace_write_unfinished` while a sync or restore that stopped part-way has left the files half written.
    pub async fn record_changes(
        &self,
        actor: &Actor,
        workspace: WorkspaceId,
        message: &str,
    ) -> Result<RecordedChanges, WorkspaceError> {
        let message = message.trim();
        if message.is_empty() || message.chars().count() > MAX_MESSAGE_CHARS {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("the message must be 1 to {MAX_MESSAGE_CHARS} characters"),
            )
            .with_details([("field", json!("message"))])
            .with_hint("call record_changes again with a short message")
            .into());
        }
        let _op = self.ops.lock(workspace).await;
        let row = self.owned(actor, workspace).await?;
        self.live_rescan(workspace).await;
        let files = self.ready_to_record(&row).await?;
        let base_tree = self.tree_of(row.base).await?;
        let _objects = self.objects.clone().read_owned().await;
        // Read before the snapshot, so an edit made while it runs is later than its checkpoint.
        let now = self.clock.now();
        let (snapshot, counts) = blocking({
            let files = Arc::clone(&files);
            move || {
                let snapshot = files.snapshot(base_tree)?;
                let base = files.flat_tree(&base_tree)?;
                let counts = count_changes(&changes(&base, &snapshot.entries));
                #[cfg(feature = "failpoints")]
                fail::fail_point!("workspace_record_after_snapshot", |_| {
                    Err(FoldError::new(
                        codes::FAILPOINT,
                        "workspace_record_after_snapshot fired",
                    ))
                });
                Ok((snapshot, counts))
            }
        })
        .await?;
        let created_by = actor.event_actor();
        let mut buffer = EventBuffer::new();
        let recorded = self
            .store
            .write(async |tx| {
                let row = owned_in_tx(tx, actor, workspace).await?;
                let checkpoint = auto_checkpoint_in_tx(
                    tx,
                    &mut buffer,
                    actor,
                    &row,
                    snapshot.tree,
                    AutoReason::BeforeRecord,
                    files.auto_kept,
                    now,
                )
                .await?;
                let changeset = workspaces::insert_changeset(
                    tx,
                    workspace,
                    row.task,
                    row.base,
                    snapshot.tree,
                    message,
                    counts,
                    created_by.as_str(),
                    now,
                )
                .await?;
                workspaces::set_conflicts(tx, workspace, &[]).await?;
                let files = counts
                    .files_added
                    .saturating_add(counts.files_modified)
                    .saturating_add(counts.files_deleted);
                let event = NewEvent::new(types::CHANGESET_RECORDED, created_by.clone())?
                    .task(row.task)
                    .workspace(workspace)
                    .revision(row.base)
                    .data(json!({
                        "changeset": changeset.id.to_string(),
                        "files": files,
                    }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, WorkspaceError>(RecordedChanges {
                    changeset: changeset.id,
                    checkpoint: checkpoint.checkpoint,
                    files,
                })
            })
            .await?;
        self.bus.publish(buffer);
        self.live_checkpoints(workspace).await;
        Ok(recorded)
    }
}

impl WorkspaceService {
    /// Snapshots the task workspace `row` for a proposal under the same rules as `record_changes`: refused with `workspace_write_unfinished` after a sync or restore that stopped part-way and with `unresolved_conflicts` while a conflicted path is unresolved. The caller has checked who may propose from the workspace. The locks move into the blocking snapshot and come back in the result, so an aborted caller never releases them while the snapshot still writes.
    pub(crate) async fn snapshot_for_proposal(
        &self,
        row: &WorkspaceRow,
    ) -> Result<ProposalSnapshot, WorkspaceError> {
        let op = self.ops.lock(row.id).await;
        self.live_rescan(row.id).await;
        let files = self.ready_to_record(row).await?;
        let base_tree = self.tree_of(row.base).await?;
        let objects = self.objects.clone().read_owned().await;
        let taken_at = self.clock.now();
        blocking(move || {
            let held = (op, objects);
            let snapshot = files.snapshot(base_tree)?;
            let base = files.flat_tree(&base_tree)?;
            Ok(ProposalSnapshot {
                tree: snapshot.tree,
                stats: count_changes(&changes(&base, &snapshot.entries)),
                auto_kept: files.auto_kept,
                taken_at,
                _held: held,
            })
        })
        .await
    }

    /// The files of `row` once nothing stops a record of them: no sync or restore left them half written, no human edit or reviewer's fix waits to be written into them, and every conflicted path is resolved. The caller holds the workspace lock.
    async fn ready_to_record(&self, row: &WorkspaceRow) -> Result<Arc<Files>, WorkspaceError> {
        let workspace = row.id;
        if let Some(mark) = workspaces::sync_checkpoint(self.store.read(), workspace).await? {
            let pending = workspaces::get_checkpoint(self.store.read(), mark)
                .await?
                .map(|checkpoint| checkpoint.reason);
            let (message, hint) = if pending == Some(CheckpointReason::HumanEdit) {
                (
                    format!("{workspace} has a human edit that its files do not hold yet"),
                    "call sync_workspace to write the human edit into the files",
                )
            } else if pending == Some(CheckpointReason::ReviewFix) {
                (
                    format!("{workspace} has a reviewer's fix that its files do not hold yet"),
                    "call sync_workspace to write the reviewer's version into the files",
                )
            } else {
                (
                    format!("a sync or restore of {workspace} stopped before it finished"),
                    "call sync_workspace, or call restore_checkpoint again",
                )
            };
            return Err(FoldError::new(codes::WORKSPACE_WRITE_UNFINISHED, message)
                .with_details([
                    ("workspace", json!(workspace.to_string())),
                    ("checkpoint", json!(mark.to_string())),
                ])
                .with_hint(hint)
                .into());
        }
        let files = self.files(row).await?;
        let conflicts = workspaces::list_conflicts(self.store.read(), workspace).await?;
        if !conflicts.is_empty() {
            let open = blocking({
                let files = Arc::clone(&files);
                move || files.unresolved(&conflicts)
            })
            .await?;
            if !open.is_empty() {
                let paths: Vec<String> = open
                    .iter()
                    .map(|row| String::from_utf8_lossy(&row.path).into_owned())
                    .collect();
                return Err(FoldError::new(
                    codes::UNRESOLVED_CONFLICTS,
                    format!("conflicts are not resolved in {}", paths.join(", ")),
                )
                .with_details([("paths", json!(paths))])
                .into());
            }
        }
        Ok(files)
    }
}
