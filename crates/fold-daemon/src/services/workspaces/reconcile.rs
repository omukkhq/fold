use fold_core::events::types;
use fold_core::{CheckpointId, EventActor, NewEvent, WorkspaceId};
use fold_store::workspaces::{self, WorkspaceKind, WorkspaceRow, WorkspaceState};
use serde_json::json;

use super::{WorkspaceError, WorkspaceService, blocking};
use crate::services::events::EventBuffer;

/// What reconciliation did with one workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconciled {
    /// The directory is there and the row was `active`; the live state is attached.
    Attached,
    /// The directory came back: the row is `active` again and the live state is attached.
    Recovered,
    /// The directory is gone: the row is `missing` now.
    Missing,
    /// The directory is still gone; nothing changed.
    StillMissing,
    /// The workspace is closed or a review checkout, or it moved on since it was listed; nothing changed.
    Skipped,
}

/// One workspace's reconciliation, with the mark of a sync or restore that stopped after it began to write files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reconciliation {
    pub workspace: WorkspaceId,
    pub outcome: Reconciled,
    pub unfinished: Option<CheckpointId>,
}

impl WorkspaceService {
    /// Brings the row and live state of one task workspace, or of the main workspace, in line with its directory when the project opens. A missing directory moves an `active` row to `missing` and a present one moves a `missing` row back to `active`, each in one write transaction with its `workspace.missing` or `workspace.recovered` event; a present directory is then attached to the live state, which rescans it. Both states keep the workspace the task's live one, so the task keeps its column and nothing is recomputed.
    ///
    /// A sync or restore mark left on the row is reported with a warning and returned; finishing the write is the agent's next `sync_workspace` or `restore_checkpoint`, because fold writes into a workspace only on create, sync and restore.
    pub async fn reconcile_workspace(
        &self,
        workspace: WorkspaceId,
    ) -> Result<Reconciliation, WorkspaceError> {
        let _op = self.ops.lock(workspace).await;
        let read = self.store.read();
        let row = workspaces::get_workspace(read, workspace).await?;
        let Some(row) = row.filter(|row| row.kind != WorkspaceKind::Review && row.state.is_live())
        else {
            return Ok(Reconciliation {
                workspace,
                outcome: Reconciled::Skipped,
                unfinished: None,
            });
        };
        let unfinished = workspaces::sync_checkpoint(read, workspace).await?;
        if let Some(checkpoint) = unfinished {
            tracing::warn!(workspace = %workspace, checkpoint = %checkpoint, "a sync or restore stopped after it began to write files; the agent finishes it with sync_workspace or restore_checkpoint");
        }
        let dir = self.paths.root().join(&row.path);
        let present = blocking(move || Ok(dir.is_dir())).await?;
        let outcome = match (present, row.state) {
            (true, WorkspaceState::Missing) => {
                if self.move_state(&row, true).await? {
                    tracing::info!(workspace = %workspace, "the workspace directory is back");
                    self.attach_live(workspace).await;
                    Reconciled::Recovered
                } else {
                    Reconciled::Skipped
                }
            }
            (true, _) => {
                self.attach_live(workspace).await;
                Reconciled::Attached
            }
            (false, WorkspaceState::Active) => {
                if self.move_state(&row, false).await? {
                    tracing::warn!(workspace = %workspace, "the workspace directory is gone");
                    if let Some(live) = &self.live {
                        live.detach(workspace).await;
                    }
                    Reconciled::Missing
                } else {
                    Reconciled::Skipped
                }
            }
            (false, _) => Reconciled::StillMissing,
        };
        Ok(Reconciliation {
            workspace,
            outcome,
            unfinished,
        })
    }

    /// One write transaction that moves `row` to `active` (`present`) or `missing` with its event. False when the row had already left the other state.
    async fn move_state(&self, row: &WorkspaceRow, present: bool) -> Result<bool, WorkspaceError> {
        let event_type = if present {
            types::WORKSPACE_RECOVERED
        } else {
            types::WORKSPACE_MISSING
        };
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let moved = self
            .store
            .write(async |tx| {
                if !workspaces::set_directory_present(tx, row.id, present).await? {
                    return Ok::<_, WorkspaceError>(false);
                }
                let event = NewEvent::new(event_type, EventActor::fold())?
                    .task(row.task)
                    .workspace(row.id)
                    .data(json!({"path": row.path}))?;
                buffer.append(tx, &event, now).await?;
                Ok(true)
            })
            .await?;
        self.bus.publish(buffer);
        Ok(moved)
    }

    async fn attach_live(&self, workspace: WorkspaceId) {
        if let Some(live) = &self.live
            && let Err(error) = live.attach(workspace).await
        {
            tracing::warn!(workspace = %workspace, code = error.log_code(), "the live state did not start; the next read of its changes starts it");
        }
    }
}
