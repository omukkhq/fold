use std::sync::Arc;

use fold_core::checkpoint_policy::AutoReason;
use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::{Actor, CheckpointId, EventActor, EventError, NewEvent, Timestamp, WorkspaceId};
use fold_store::WriteTx;
use fold_store::workspaces::{self, WorkspaceRow};
use serde_json::json;

use super::files::{check_auto, checkpoint_in_tx, not_found};
use super::{WorkspaceError, WorkspaceService, blocking};
use crate::services::events::EventBuffer;

/// What an automatic checkpoint step left: the checkpoint that holds the workspace's state, whether this step recorded it, and the automatic checkpoints it pruned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoCheckpoint {
    pub checkpoint: CheckpointId,
    /// `false` when the latest checkpoint already held the same files on the same base, so no row was added.
    pub recorded: bool,
    pub pruned: Vec<CheckpointId>,
}

/// What the quiet-period job got from one workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuietCheckpoint {
    Taken(AutoCheckpoint),
    /// Another operation held the workspace, so the job tries again on a later tick.
    Busy,
}

impl WorkspaceService {
    /// Records the current files of an active workspace as an automatic checkpoint with `reason`, for the owning worker or the daemon. It only reads the workspace. The objects are written before the one transaction that inserts the row, prunes the automatic checkpoints beyond `auto_checkpoints_kept` and appends their events; a workspace whose latest checkpoint holds the same files on the same base gets no new row.
    pub async fn take_auto_checkpoint(
        &self,
        actor: &Actor,
        workspace: WorkspaceId,
        reason: AutoReason,
    ) -> Result<AutoCheckpoint, WorkspaceError> {
        let _op = self.ops.lock(workspace).await;
        self.auto_checkpoint_locked(actor, workspace, reason).await
    }

    /// The `Quiet` checkpoint the daemon takes after an editing burst. It never waits for another operation on the workspace: a running scan, sync, record or restore makes it [`QuietCheckpoint::Busy`].
    pub async fn take_quiet_checkpoint(
        &self,
        workspace: WorkspaceId,
    ) -> Result<QuietCheckpoint, WorkspaceError> {
        let Some(_op) = self.ops.try_lock(workspace) else {
            return Ok(QuietCheckpoint::Busy);
        };
        self.auto_checkpoint_locked(&Actor::System, workspace, AutoReason::Quiet)
            .await
            .map(QuietCheckpoint::Taken)
    }

    async fn auto_checkpoint_locked(
        &self,
        actor: &Actor,
        workspace: WorkspaceId,
        reason: AutoReason,
    ) -> Result<AutoCheckpoint, WorkspaceError> {
        let row = workspaces::get_workspace(self.store.read(), workspace)
            .await?
            .ok_or_else(|| not_found(workspace))?;
        check_auto(actor, &row)?;
        self.live_rescan(workspace).await;
        let files = self.files(&row).await?;
        let base_tree = self.tree_of(row.base).await?;
        let _objects = self.objects.clone().read_owned().await;
        // Read before the snapshot: activity the watcher sees while it runs is later than the checkpoint, so the quiet job does not take it as covered.
        let now = self.clock.now();
        let snapshot = blocking({
            let files = Arc::clone(&files);
            move || {
                let snapshot = files.snapshot(base_tree)?;
                #[cfg(feature = "failpoints")]
                fail::fail_point!("workspace_auto_checkpoint_after_snapshot", |_| {
                    Err(fold_core::FoldError::new(
                        fold_core::codes::FAILPOINT,
                        "workspace_auto_checkpoint_after_snapshot fired",
                    ))
                });
                Ok(snapshot)
            }
        })
        .await?;
        let mut buffer = EventBuffer::new();
        let taken = self
            .store
            .write(async |tx| {
                let row = workspaces::get_workspace(tx.conn(), workspace)
                    .await?
                    .ok_or_else(|| not_found(workspace))?;
                check_auto(actor, &row)?;
                auto_checkpoint_in_tx(
                    tx,
                    &mut buffer,
                    actor,
                    &row,
                    snapshot.tree,
                    reason,
                    files.auto_kept,
                    now,
                )
                .await
            })
            .await?;
        self.bus.publish(buffer);
        if taken.recorded {
            tracing::debug!(workspace = %workspace, checkpoint = %taken.checkpoint, reason = reason.as_str(), "automatic checkpoint recorded");
            self.live_checkpoints(workspace).await;
        }
        Ok(taken)
    }
}

/// The automatic checkpoint step of a command, in the command's own transaction and before any change it makes: unless the latest unpruned checkpoint already holds `tree` on the workspace's base, it inserts a checkpoint with `reason`, prunes the automatic checkpoints beyond `keep` and appends `checkpoint.recorded` and one `checkpoint.pruned` per pruned row. Every object of `tree` is in the store. The checkpoint it returns is never pruned by it, even with `keep` at zero, because a sync marks it as the state to go back to.
#[expect(
    clippy::too_many_arguments,
    reason = "the step of a command, with the inputs its earlier steps produced"
)]
pub(crate) async fn auto_checkpoint_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    row: &WorkspaceRow,
    tree: ObjectHash,
    reason: AutoReason,
    keep: u32,
    now: Timestamp,
) -> Result<AutoCheckpoint, WorkspaceError> {
    // A restore puts back the checkpoint's base with its files, so a checkpoint on another base is a different state.
    if let Some(latest) = workspaces::latest_checkpoint(tx.conn(), row.id).await?
        && latest.tree == tree
        && latest.base == Some(row.base)
    {
        return Ok(AutoCheckpoint {
            checkpoint: latest.id,
            recorded: false,
            pruned: Vec::new(),
        });
    }
    let checkpoint =
        checkpoint_in_tx(tx, buffer, actor, row, tree, reason.into(), None, now).await?;
    let pruned = workspaces::prune_auto(tx, row.id, keep.max(1), now).await?;
    for id in &pruned {
        buffer.append(tx, &pruned_event(row.id, *id)?, now).await?;
    }
    Ok(AutoCheckpoint {
        checkpoint: checkpoint.id,
        recorded: true,
        pruned,
    })
}

/// `checkpoint.pruned` for one checkpoint of `workspace`. Pruning is fold's own housekeeping, whichever command caused it.
pub(crate) fn pruned_event(
    workspace: WorkspaceId,
    checkpoint: CheckpointId,
) -> Result<NewEvent, EventError> {
    NewEvent::new(types::CHECKPOINT_PRUNED, EventActor::fold())?
        .workspace(workspace)
        .data(json!({"checkpoint": checkpoint.to_string()}))
}
