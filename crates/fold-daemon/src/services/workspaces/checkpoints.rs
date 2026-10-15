use std::sync::Arc;

use fold_core::events::types;
use fold_core::{
    Actor, CheckpointId, FoldError, NewEvent, RevisionId, Timestamp, WorkspaceId, codes,
};
use fold_store::workspaces::{self, Checkpoint, CheckpointKind, CheckpointReason, WorkspaceRow};
use serde_json::json;

use super::files::{Files, checkpoint_in_tx, not_found, owned_in_tx, set_base};
use super::{WorkspaceError, WorkspaceService, blocking};
use crate::services::events::EventBuffer;

/// The longest checkpoint label, in characters.
const MAX_LABEL_CHARS: usize = 200;

/// A checkpoint with public ids only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointView {
    pub id: CheckpointId,
    pub workspace: WorkspaceId,
    pub kind: CheckpointKind,
    pub reason: CheckpointReason,
    pub label: Option<String>,
    pub created_at: Timestamp,
}

impl From<Checkpoint> for CheckpointView {
    fn from(checkpoint: Checkpoint) -> Self {
        Self {
            id: checkpoint.id,
            workspace: checkpoint.workspace,
            kind: checkpoint.kind,
            reason: checkpoint.reason,
            label: checkpoint.label,
            created_at: checkpoint.created_at,
        }
    }
}

/// What a restore did: the checkpoint it restored and the one it took first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Restored {
    pub restored: CheckpointId,
    pub safety: CheckpointId,
    pub base: RevisionId,
}

impl WorkspaceService {
    /// Records the current files of a workspace the actor's worker owns as an explicit checkpoint, with an optional short label.
    pub async fn record_checkpoint(
        &self,
        actor: &Actor,
        workspace: WorkspaceId,
        label: Option<&str>,
    ) -> Result<CheckpointView, WorkspaceError> {
        let label = label.map(str::trim).filter(|label| !label.is_empty());
        if label.is_some_and(|label| label.chars().count() > MAX_LABEL_CHARS) {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("the label must be at most {MAX_LABEL_CHARS} characters"),
            )
            .with_details([("field", json!("label"))])
            .with_hint("call record_checkpoint again with a shorter label")
            .into());
        }
        let _op = self.ops.lock(workspace).await;
        let row = self.owned(actor, workspace).await?;
        let files = self.files(&row).await?;
        let base_tree = self.tree_of(row.base).await?;
        let _objects = self.objects.clone().read_owned().await;
        let snapshot = blocking({
            let files = Arc::clone(&files);
            move || files.snapshot(base_tree)
        })
        .await?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let checkpoint = self
            .store
            .write(async |tx| {
                let row = owned_in_tx(tx, actor, workspace).await?;
                checkpoint_in_tx(
                    tx,
                    &mut buffer,
                    actor,
                    &row,
                    snapshot.tree,
                    CheckpointReason::Explicit,
                    label,
                    now,
                )
                .await
            })
            .await?;
        self.bus.publish(buffer);
        Ok(checkpoint.into())
    }

    /// The checkpoints of a workspace in any state that are not pruned, newest first. Any worker may read them.
    pub async fn list_checkpoints(
        &self,
        workspace: WorkspaceId,
    ) -> Result<Vec<CheckpointView>, WorkspaceError> {
        let read = self.store.read();
        if workspaces::get_workspace(read, workspace).await?.is_none() {
            return Err(not_found(workspace).into());
        }
        let mut checkpoints = workspaces::list_checkpoints(read, workspace, false).await?;
        checkpoints.reverse();
        Ok(checkpoints.into_iter().map(CheckpointView::from).collect())
    }

    /// Puts the files and the base of a checkpoint back into its workspace, which the actor's worker must own. A `before_restore` checkpoint of the current files commits first, so the restore itself can be undone. A pruned checkpoint is `checkpoint_not_found`: garbage collection may already have removed its objects.
    pub async fn restore_checkpoint(
        &self,
        actor: &Actor,
        checkpoint: CheckpointId,
    ) -> Result<Restored, WorkspaceError> {
        let found = self.unpruned_checkpoint(checkpoint).await?;
        let _op = self.ops.lock(found.workspace).await;
        let row = self.owned(actor, found.workspace).await?;
        let files = self.files(&row).await?;
        let _objects = self.objects.clone().read_owned().await;
        // Pruning does not wait for the object lock, but the sweep that follows it does: a checkpoint still unpruned now keeps its objects until this restore ends.
        let target = self.unpruned_checkpoint(checkpoint).await?;
        self.restore_in(actor, &row, &target, &files)
            .await
            .map_err(|error| retry_hint(error, "call restore_checkpoint again"))
    }

    async fn unpruned_checkpoint(
        &self,
        checkpoint: CheckpointId,
    ) -> Result<Checkpoint, WorkspaceError> {
        Ok(workspaces::get_checkpoint(self.store.read(), checkpoint)
            .await?
            .filter(|found| found.pruned_at.is_none())
            .ok_or_else(|| {
                FoldError::new(
                    codes::CHECKPOINT_NOT_FOUND,
                    format!("{checkpoint} does not exist or was pruned"),
                )
                .with_details([("checkpoint", json!(checkpoint.to_string()))])
            })?)
    }

    /// The steps of a restore, for a caller that holds the workspace lock and the object lock: the safety checkpoint commits, the files are written, then the base and `checkpoint.restored` commit together. The safety checkpoint is marked as the workspace's unfinished write unless an earlier sync or restore left a mark, whose checkpoint is then the last complete state; a restore that stops while writing leaves the mark, so `record_changes` refuses the half-written files and the next sync restores that state first. The finished restore clears the mark, because every tracked file is the target's. The conflict list stays as the last sync left it: `record_changes` checks each listed path for markers, so a restore that brings the markers back is still refused.
    pub(super) async fn restore_in(
        &self,
        actor: &Actor,
        row: &WorkspaceRow,
        target: &Checkpoint,
        files: &Arc<Files>,
    ) -> Result<Restored, WorkspaceError> {
        let base_tree = self.tree_of(row.base).await?;
        let snapshot = blocking({
            let files = Arc::clone(files);
            move || files.snapshot(base_tree)
        })
        .await?;
        let workspace = row.id;
        let mut buffer = EventBuffer::new();
        let safety = self
            .store
            .write(async |tx| {
                let row = owned_in_tx(tx, actor, workspace).await?;
                let checkpoint = checkpoint_in_tx(
                    tx,
                    &mut buffer,
                    actor,
                    &row,
                    snapshot.tree,
                    CheckpointReason::BeforeRestore,
                    None,
                    self.clock.now(),
                )
                .await?;
                if workspaces::sync_checkpoint(tx.conn(), workspace)
                    .await?
                    .is_none()
                {
                    workspaces::set_sync_checkpoint(tx, workspace, Some(checkpoint.id)).await?;
                }
                Ok::<_, WorkspaceError>(checkpoint)
            })
            .await?;
        self.bus.publish(buffer);

        let tree = target.tree;
        blocking({
            let files = Arc::clone(files);
            move || {
                #[cfg(feature = "failpoints")]
                fail::fail_point!("workspace_restore_before_write", |_| {
                    Err(FoldError::new(
                        codes::FAILPOINT,
                        "workspace_restore_before_write fired",
                    ))
                });
                files.restore(&tree)?;
                #[cfg(feature = "failpoints")]
                fail::fail_point!("workspace_restore_after_write", |_| {
                    Err(FoldError::new(
                        codes::FAILPOINT,
                        "workspace_restore_after_write fired",
                    ))
                });
                Ok(())
            }
        })
        .await?;

        let base = target.base.unwrap_or(row.base);
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let row = owned_in_tx(tx, actor, workspace).await?;
                set_base(tx, workspace, base).await?;
                workspaces::set_sync_checkpoint(tx, workspace, None).await?;
                let event = NewEvent::new(types::CHECKPOINT_RESTORED, actor.event_actor())?
                    .task(row.task)
                    .workspace(workspace)
                    .revision(base)
                    .data(json!({
                        "checkpoint": target.id.to_string(),
                        "safety": safety.id.to_string(),
                    }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, WorkspaceError>(())
            })
            .await?;
        self.bus.publish(buffer);
        Ok(Restored {
            restored: target.id,
            safety: safety.id,
            base,
        })
    }
}

/// An edit that lands between a restore's safety checkpoint and its scan refuses the restore with `unrecorded_changes`; running the command again takes a new safety checkpoint that records the edit.
pub(super) fn retry_hint(error: WorkspaceError, hint: &str) -> WorkspaceError {
    match error {
        WorkspaceError::Fold(fold) if fold.code() == codes::UNRECORDED_CHANGES => {
            WorkspaceError::Fold(fold.with_hint(hint))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unrecorded_changes_gets_the_retry_hint() {
        let unrecorded = WorkspaceError::Fold(FoldError::new(codes::UNRECORDED_CHANGES, "edited"));
        match retry_hint(unrecorded, "call sync_workspace again") {
            WorkspaceError::Fold(fold) => {
                assert_eq!(fold.hint(), Some("call sync_workspace again"))
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        let other = WorkspaceError::Fold(FoldError::new(codes::WORKSPACE_IO, "gone"));
        match retry_hint(other, "call sync_workspace again") {
            WorkspaceError::Fold(fold) => assert_eq!(fold.hint(), None),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
}
