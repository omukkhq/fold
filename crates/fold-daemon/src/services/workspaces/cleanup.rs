use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use fold_core::events::{EventType, types};
use fold_core::{Actor, CheckpointId, FoldError, WorkspaceId, codes};
use fold_store::workspaces::{self, CheckpointReason, WorkspaceKind, WorkspaceRow};
use fold_vcs::gc::{locate_workspace_dir, remove_workspace_dir};
use fold_vcs::scan::SkipReason;
use fold_vcs::snapshot::{Warning, WarningReason};
use tokio::sync::Notify;
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};

use super::files::{checkpoint_in_tx, not_found};
use super::{WorkspaceError, WorkspaceService, blocking};
use crate::services::events::{BusMessage, EventBuffer, EventBus};

/// What became of the directory of one closed workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryRemoval {
    /// The directory is gone and the row records its removal. `checkpoint` is the final checkpoint taken first, when the files held something fold had not recorded.
    Removed { checkpoint: Option<CheckpointId> },
    /// Nothing was done: the workspace is live, its removal is already recorded, or its owning agent still has a worker process that has not ended or an open session.
    NotDue,
}

impl WorkspaceService {
    /// Removes the directory of a closed workspace once its owning agent has no worker process that has not ended and no open session. The tracked files are snapshotted first, the way a checkpoint reads them, so ignored files such as build output are left out; when fold holds no such tree for the workspace (see [`workspaces::tree_recorded`]), a `before_removal` checkpoint commits before anything is deleted, in the write that records that the removal started. A snapshot that could not read a tracked file, or saw one change while it read it, is `unrecorded_changes` and nothing starts, because its tree would hold the recorded version of that path; a file over `max_file_bytes` does not stop the removal, as it is never recorded anywhere. A review checkout takes no checkpoint: its files are a proposal version, and a fix in it becomes a version only through `submit_review_fix`. The path must name a directory directly below `.fold/ws/` that is not a symlink (see [`locate_workspace_dir`]); anything else is refused before a file is read. The directory is deleted in blocking work, without following a symlink inside it, then one write records the removal. A failure after the start only deletes when it runs again, such as in the next `fld gc`, so a directory that was partly deleted never gets a checkpoint; a failure before it starts again from the snapshot.
    pub async fn remove_closed_directory(
        &self,
        workspace: WorkspaceId,
    ) -> Result<DirectoryRemoval, WorkspaceError> {
        let _op = self.ops.lock(workspace).await;
        let due = workspaces::closed_directories_due(self.store.read(), Some(workspace)).await?;
        let Some(closed) = due.into_iter().next() else {
            return Ok(DirectoryRemoval::NotDue);
        };
        let row = workspaces::get_workspace(self.store.read(), workspace)
            .await?
            .ok_or_else(|| not_found(workspace))?;
        if let Some(live) = &self.live {
            live.detach(workspace).await;
        }
        let root = self.paths.root().to_owned();
        let path = PathBuf::from(&row.path);
        let located =
            blocking(move || locate_workspace_dir(&root, &path).map_err(FoldError::from)).await?;
        let checkpoint = match located {
            Some(_) if !closed.removal_started => self.start_removal(&row).await?,
            _ => None,
        };
        #[cfg(feature = "failpoints")]
        fail::fail_point!("workspace_removal_after_checkpoint", |_| {
            Err(WorkspaceError::from(FoldError::new(
                fold_core::codes::FAILPOINT,
                "workspace_removal_after_checkpoint fired",
            )))
        });
        let root = self.paths.root().to_owned();
        let path = PathBuf::from(&row.path);
        let index = self.paths.workspace_index(workspace);
        blocking(move || {
            remove_workspace_dir(&root, &path).map_err(FoldError::from)?;
            match fs::remove_file(&index) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => {
                    tracing::debug!(workspace = %workspace, "the stat index of a removed workspace stays");
                }
            }
            Ok(())
        })
        .await?;
        let now = self.clock.now();
        self.store
            .write(async |tx| workspaces::mark_directory_removed(tx, workspace, now).await)
            .await?;
        tracing::debug!(workspace = %workspace, "closed workspace directory removed");
        Ok(DirectoryRemoval::Removed { checkpoint })
    }

    /// [`WorkspaceService::remove_closed_directory`] for every closed workspace that is due, by id, until `stop` holds true. A workspace whose removal fails is logged with its id and code and left for a later run; the others go on. Returns how many directories it removed.
    pub async fn remove_closed_directories(&self, stop: &(dyn Fn() -> bool + Sync)) -> u64 {
        let due = match workspaces::closed_directories_due(self.store.read(), None).await {
            Ok(due) => due,
            Err(error) => {
                tracing::warn!(
                    code = error.log_code(),
                    "the closed workspaces were not read"
                );
                return 0;
            }
        };
        let mut removed = 0;
        for closed in due {
            if stop() {
                break;
            }
            match self.remove_closed_directory(closed.id).await {
                Ok(DirectoryRemoval::Removed { .. }) => removed += 1,
                Ok(DirectoryRemoval::NotDue) => {}
                Err(error) => {
                    tracing::warn!(workspace = %closed.id, code = error.log_code(), "closed workspace directory was not removed; the next gc tries again");
                }
            }
        }
        removed
    }

    /// Records that the removal of the directory starts, with the `before_removal` checkpoint of a closed task or main workspace whose tracked files fold has not recorded in the same write, and returns that checkpoint.
    async fn start_removal(
        &self,
        row: &WorkspaceRow,
    ) -> Result<Option<CheckpointId>, WorkspaceError> {
        let _objects = self.objects.clone().read_owned().await;
        let unrecorded = if row.kind == WorkspaceKind::Review {
            None
        } else {
            let files = self.files(row).await?;
            let base_tree = self.tree_of(row.base).await?;
            let snapshot = blocking({
                let files = Arc::clone(&files);
                move || files.snapshot(base_tree)
            })
            .await?;
            let unread = snapshot.warnings.iter().filter(|w| not_read(w)).count();
            if unread > 0 {
                tracing::warn!(workspace = %row.id, files = unread, "a closed workspace has files the final checkpoint could not read; its directory stays");
                return Err(FoldError::new(
                    codes::UNRECORDED_CHANGES,
                    format!("{} has {unread} files that fold could not record", row.id),
                )
                .with_hint("fold removes the directory once every tracked file can be read; `fld gc` tries again")
                .into());
            }
            let recorded =
                workspaces::tree_recorded(self.store.read(), row.id, &snapshot.tree).await?;
            (!recorded).then_some(snapshot.tree)
        };
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let checkpoint = self
            .store
            .write(async |tx| -> Result<_, WorkspaceError> {
                let checkpoint = match unrecorded {
                    Some(tree) => Some(
                        checkpoint_in_tx(
                            tx,
                            &mut buffer,
                            &Actor::System,
                            row,
                            tree,
                            CheckpointReason::BeforeRemoval,
                            None,
                            now,
                        )
                        .await?,
                    ),
                    None => None,
                };
                workspaces::start_directory_removal(tx, row.id, now).await?;
                Ok(checkpoint.map(|checkpoint| checkpoint.id))
            })
            .await?;
        self.bus.publish(buffer);
        if let Some(checkpoint) = checkpoint {
            tracing::debug!(workspace = %row.id, checkpoint = %checkpoint, "final checkpoint of a closed workspace recorded");
        }
        Ok(checkpoint)
    }
}

/// Removes the directories of the project's closed workspaces as they become due, through [`WorkspaceService::remove_closed_directories`]: after each committed event that closes a workspace, ends a worker process or ends a session, which can make a closed workspace of that agent due, after a missed event, and whenever `catch_up` is notified, as the daemon does when it opens the project, so what a stopped daemon left goes too. `events` is subscribed before the call, so nothing published after it is missed. It ends once the bus closes; a removal in progress finishes first.
pub async fn follow_closed_workspaces(
    service: WorkspaceService,
    bus: EventBus,
    mut events: broadcast::Receiver<BusMessage>,
    catch_up: Arc<Notify>,
) {
    let mut closing = bus.closing();
    loop {
        let due = tokio::select! {
            biased;
            _ = closing.wait_for(|closed| *closed) => return,
            () = catch_up.notified() => true,
            received = events.recv() => match received {
                Ok(BusMessage::Durable(event)) => can_make_due(event.event_type),
                Ok(BusMessage::Live(_)) => false,
                Err(RecvError::Lagged(_)) => true,
                Err(RecvError::Closed) => return,
            },
        };
        if !due {
            continue;
        }
        // Everything queued so far was committed before it was sent, so the run below covers it.
        while !matches!(
            events.try_recv(),
            Err(TryRecvError::Empty | TryRecvError::Closed)
        ) {}
        let stop = || *closing.borrow();
        service.remove_closed_directories(&stop).await;
    }
}

/// Whether the snapshot left out a tracked file it should have recorded: one it could not read or that changed while it was read. A file over `max_file_bytes` and a socket, FIFO or device are never recorded, so they do not count.
fn not_read(warning: &Warning) -> bool {
    matches!(
        warning.reason,
        WarningReason::Skipped(SkipReason::Unreadable)
            | WarningReason::Unstable
            | WarningReason::ChangedDuringSnapshot
    )
}

fn can_make_due(event_type: EventType) -> bool {
    [
        types::WORKSPACE_ARCHIVED,
        types::WORKSPACE_ABANDONED,
        types::WORKER_STOPPED,
        types::WORKER_FAILED,
        types::SESSION_ENDED,
    ]
    .contains(&event_type)
}
