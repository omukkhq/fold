use std::sync::Arc;

use fold_core::checkpoint_policy::AutoReason;
use fold_core::events::types;
use fold_core::{Actor, CheckpointId, FoldError, NewEvent, RevisionId, WorkspaceId, codes};
use fold_store::workspaces::{self, ConflictRow, WorkspaceRow};
use fold_vcs::sync::{SyncInput, sync};
use serde_json::json;

use super::auto::auto_checkpoint_in_tx;
use super::checkpoints::retry_hint;
use super::files::{owned_in_tx, set_base};
use super::{WorkspaceError, WorkspaceService, blocking};
use crate::services::events::EventBuffer;

/// What a sync left: the new base, the conflicted paths in path order, and the checkpoint that holds the files from before it wrote anything, the one it took or an identical latest one. `checkpoint` is `None` when the base already was main and nothing was read or written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Synced {
    pub base: RevisionId,
    pub conflicted: Vec<String>,
    pub checkpoint: Option<CheckpointId>,
}

impl WorkspaceService {
    /// Brings main into a workspace the actor's worker owns with a three-way merge of its base, its current files and main. A `before_sync` automatic checkpoint commits first, unless the latest checkpoint already holds the same files on the same base, and is marked as the sync in progress; a checkpoint that cannot be taken refuses the sync; the files are written; then the new base, the conflict list and `workspace.synced` commit and clear the mark.
    ///
    /// A mark left by a sync or a restore that stopped after it began to write means the files may be half written. Merging over them would put markers inside markers, so the marked checkpoint is restored first, through a restore with its own safety checkpoint, and the sync runs from clean files.
    pub async fn sync_workspace(
        &self,
        actor: &Actor,
        workspace: WorkspaceId,
    ) -> Result<Synced, WorkspaceError> {
        let _op = self.ops.lock(workspace).await;
        let row = self.owned(actor, workspace).await?;
        self.live_rescan(workspace).await;
        let synced = self.sync_in(actor, row).await;
        self.live_checkpoints(workspace).await;
        synced
    }

    /// The steps of a sync, for a caller that holds the workspace lock.
    async fn sync_in(
        &self,
        actor: &Actor,
        mut row: WorkspaceRow,
    ) -> Result<Synced, WorkspaceError> {
        let workspace = row.id;
        let files = self.files(&row).await?;
        let _objects = self.objects.clone().read_owned().await;
        if let Some(pending) = workspaces::sync_checkpoint(self.store.read(), workspace).await? {
            let target = workspaces::get_checkpoint(self.store.read(), pending)
                .await?
                .ok_or(fold_store::StoreError::Corrupt(
                    "workspaces.sync_checkpoint_id",
                ))?;
            tracing::warn!(workspace = %workspace, checkpoint = %pending, "restoring the checkpoint of a sync that did not finish");
            self.restore_in(actor, &row, &target, &files)
                .await
                .map_err(|error| retry_hint(error, "call sync_workspace again"))?;
            row = self.owned(actor, workspace).await?;
        }
        let head = workspaces::head(self.store.read()).await?.ok_or_else(|| {
            FoldError::new(codes::REVISION_NOT_FOUND, "main has no revision")
                .with_hint("run fold init to create the first revision")
        })?;
        if head.number == row.base {
            let conflicted = workspaces::list_conflicts(self.store.read(), workspace).await?;
            return Ok(Synced {
                base: row.base,
                conflicted: conflicted.iter().map(path_text).collect(),
                checkpoint: None,
            });
        }

        let base_tree = self.tree_of(row.base).await?;
        // Read before the snapshot, so an edit made while it runs is later than its checkpoint.
        let checkpointed_at = self.clock.now();
        let snapshot = blocking({
            let files = Arc::clone(&files);
            move || files.snapshot(base_tree)
        })
        .await?;
        let mut buffer = EventBuffer::new();
        let checkpoint = self
            .store
            .write(async |tx| {
                let row = owned_in_tx(tx, actor, workspace).await?;
                let taken = auto_checkpoint_in_tx(
                    tx,
                    &mut buffer,
                    actor,
                    &row,
                    snapshot.tree,
                    AutoReason::BeforeSync,
                    files.auto_kept,
                    checkpointed_at,
                )
                .await?;
                workspaces::set_sync_checkpoint(tx, workspace, Some(taken.checkpoint)).await?;
                Ok::<_, WorkspaceError>(taken.checkpoint)
            })
            .await?;
        self.bus.publish(buffer);

        let input = SyncInput {
            base_tree,
            ours_tree: snapshot.tree,
            main_tree: head.tree_hash,
            base_number: row.base,
            main_number: head.number,
        };
        let merged = blocking({
            let files = Arc::clone(&files);
            move || {
                #[cfg(feature = "failpoints")]
                fail::fail_point!("workspace_sync_before_merge", |_| {
                    Err(FoldError::new(
                        codes::FAILPOINT,
                        "workspace_sync_before_merge fired",
                    ))
                });
                let mut index = files.index();
                let outcome = sync(files.objects(), files.root(), &mut index, input)?;
                files.save_index(&mut index);
                Ok(outcome)
            }
        })
        .await;
        let outcome = match merged {
            Ok(outcome) => outcome,
            Err(error) => {
                // This refusal comes before any write, so the files need no restore.
                if error.code() == Some(codes::WORKSPACE_CHANGED_DURING_SYNC) {
                    self.store
                        .write(async |tx| {
                            workspaces::set_sync_checkpoint(tx, workspace, None).await
                        })
                        .await?;
                }
                return Err(error);
            }
        };
        #[cfg(feature = "failpoints")]
        fail::fail_point!("workspace_sync_after_write", |_| {
            Err(WorkspaceError::from(FoldError::new(
                codes::FAILPOINT,
                "workspace_sync_after_write fired",
            )))
        });

        let conflicted: Vec<ConflictRow> = outcome
            .conflicted
            .iter()
            .map(|conflict| ConflictRow {
                path: conflict.path.as_bytes().to_vec(),
                kind: conflict.kind,
            })
            .collect();
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let row = owned_in_tx(tx, actor, workspace).await?;
                set_base(tx, workspace, outcome.new_base).await?;
                workspaces::set_conflicts(tx, workspace, &conflicted).await?;
                workspaces::set_sync_checkpoint(tx, workspace, None).await?;
                let event = NewEvent::new(types::WORKSPACE_SYNCED, actor.event_actor())?
                    .task(row.task)
                    .workspace(workspace)
                    .revision(outcome.new_base)
                    .data(json!({
                        "from": row.base.to_string(),
                        "to": outcome.new_base.to_string(),
                        "checkpoint": checkpoint.to_string(),
                        "conflicted": conflicted.len(),
                    }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, WorkspaceError>(())
            })
            .await?;
        self.bus.publish(buffer);
        self.live_base(workspace, outcome.new_base).await;
        Ok(Synced {
            base: outcome.new_base,
            conflicted: conflicted.iter().map(path_text).collect(),
            checkpoint: Some(checkpoint),
        })
    }
}

/// A conflicted path as an agent reads it. A name that is not UTF-8 shows replacement characters.
pub(super) fn path_text(row: &ConflictRow) -> String {
    String::from_utf8_lossy(&row.path).into_owned()
}
