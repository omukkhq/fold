use std::sync::Arc;

use fold_core::events::types;
use fold_core::{Actor, ChangesetId, CheckpointId, FoldError, NewEvent, WorkspaceId, codes};
use fold_store::workspaces::{self, CheckpointReason};
use fold_vcs::scan::{changes, count_changes};
use serde_json::json;

use super::files::{checkpoint_in_tx, owned_in_tx};
use super::{WorkspaceError, WorkspaceService, blocking};
use crate::services::events::EventBuffer;

/// The longest changeset message, in characters.
const MAX_MESSAGE_CHARS: usize = 2000;

/// What `record_changes` recorded. `files` counts the paths that differ from the base revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedChanges {
    pub changeset: ChangesetId,
    pub checkpoint: CheckpointId,
    pub files: u32,
}

impl WorkspaceService {
    /// Records the files of a workspace the actor's worker owns as a changeset with `message`, chained to the workspace's previous changeset. A `before_record` checkpoint of the same tree commits in the same transaction. Refused with `unresolved_conflicts` while a conflicted path still holds a marker line or its `<path>.fold-main`; once none does, the conflict list is cleared with the record. Refused with `workspace_write_unfinished` while a sync or restore that stopped part-way has left the files half written.
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
        if let Some(mark) = workspaces::sync_checkpoint(self.store.read(), workspace).await? {
            return Err(FoldError::new(
                codes::WORKSPACE_WRITE_UNFINISHED,
                format!("a sync or restore of {workspace} stopped before it finished"),
            )
            .with_details([
                ("workspace", json!(workspace.to_string())),
                ("checkpoint", json!(mark.to_string())),
            ])
            .with_hint("call sync_workspace, or call restore_checkpoint again")
            .into());
        }
        let files = self.files(&row).await?;
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
        let base_tree = self.tree_of(row.base).await?;
        let _objects = self.objects.clone().read_owned().await;
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
        let now = self.clock.now();
        let created_by = actor.event_actor();
        let mut buffer = EventBuffer::new();
        let recorded = self
            .store
            .write(async |tx| {
                let row = owned_in_tx(tx, actor, workspace).await?;
                let checkpoint = checkpoint_in_tx(
                    tx,
                    &mut buffer,
                    actor,
                    &row,
                    snapshot.tree,
                    CheckpointReason::BeforeRecord,
                    None,
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
                    checkpoint: checkpoint.id,
                    files,
                })
            })
            .await?;
        self.bus.publish(buffer);
        Ok(recorded)
    }
}
