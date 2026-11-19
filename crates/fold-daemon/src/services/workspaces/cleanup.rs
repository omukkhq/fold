use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fold_core::events::{EventType, types};
use fold_core::layout::WS_DIR;
use fold_core::removal::{
    ClosedDecision, KeepReason, decide_closed, owner_hold_cutoff, window_cutoff,
};
use fold_core::{Actor, CheckpointId, FoldError, Timestamp, WorkspaceId, codes};
use fold_store::StoreError;
use fold_store::agents;
use fold_store::workspaces::{
    self, CheckpointReason, ClosedWorkspace, WorkspaceKind, WorkspaceRow,
};
use fold_vcs::gc::{locate_workspace_dir, remove_workspace_dir};
use fold_vcs::scan::SkipReason;
use fold_vcs::snapshot::{Warning, WarningReason};
use tokio::sync::Notify;
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};

use super::files::{checkpoint_in_tx, not_found};
use super::lifecycle::io_error;
use super::{WorkspaceError, WorkspaceService, blocking};
use crate::services::agents::{FolderDecision, collect_worker_folders};
use crate::services::events::{BusMessage, EventBuffer, EventBus};

/// What became of the directory of one closed workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectoryRemoval {
    /// The directory is gone and the row records its removal. `checkpoint` is the final checkpoint taken first, when the files held something fold had not recorded.
    Removed { checkpoint: Option<CheckpointId> },
    /// Nothing was done, for this reason.
    Kept(KeepReason),
    /// Nothing was done: the workspace is live or its removal is already recorded.
    NotDue,
}

/// A closed workspace whose directory fold has not removed, or whose directory came back after fold removed it, with what fold does about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedDirectory {
    pub workspace: WorkspaceId,
    /// The directory as `workspaces.path` holds it.
    pub path: String,
    /// The worker row that owns the workspace.
    pub owner: Option<String>,
    /// fold had recorded the directory as removed, and a directory was at its path again.
    pub returned: bool,
    pub decision: ClosedDecision,
}

/// What decides whether a closed workspace's owner still holds its directory back: the cutoff of the `closed_workspace_days` window, the working folders of the open sessions, and the canonical `.fold/ws/`.
#[derive(Debug, Clone, Default)]
pub(crate) struct RemovalWindow {
    pub(crate) cutoff: Option<Timestamp>,
    pub(crate) session_folders: Vec<PathBuf>,
    /// `.fold/ws/` with its links resolved, as a session's canonical folder names a workspace below it; `None` while it is not there.
    pub(crate) canonical_ws: Option<PathBuf>,
}

impl RemovalWindow {
    /// Whether an open session works in the directory `path` (relative to `root`) or below it, by the project's path or by the canonical one.
    pub(crate) fn session_inside(&self, root: &Path, path: &str) -> bool {
        let given = root.join(path);
        let canonical = self
            .canonical_ws
            .as_ref()
            .zip(Path::new(path).file_name())
            .map(|(ws, name)| ws.join(name));
        self.session_folders.iter().any(|folder| {
            folder.starts_with(&given)
                || canonical
                    .as_ref()
                    .is_some_and(|dir| folder.starts_with(dir))
        })
    }
}

/// What fold does with the directory of `closed` under `window` (see [`decide_closed`]).
fn decide(closed: &ClosedWorkspace, window: &RemovalWindow, root: &Path) -> ClosedDecision {
    decide_closed(
        window.session_inside(root, &closed.path),
        closed.hold,
        closed.past_window,
    )
}

impl WorkspaceService {
    /// The window at the service's `now`: `closed_workspace_days` from `config.toml`, read on each call, through [`owner_hold_cutoff`], so 0 never overrides an owner, and the open sessions' folders. A file that cannot be read gives no cutoff, so only the owner rule removes directories until it is fixed.
    pub(crate) async fn removal_window(&self) -> Result<RemovalWindow, WorkspaceError> {
        let cutoff = match crate::services::projects::load_config(&self.paths).await {
            Ok(config) => owner_hold_cutoff(self.clock.now(), config.closed_workspace_days),
            Err(error) => {
                tracing::warn!(
                    code = error.log_code(),
                    "the configuration was not read; closed workspaces wait for their owners"
                );
                None
            }
        };
        let session_folders = agents::open_session_folders(self.store.read())
            .await?
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let ws = self.paths.ws_dir();
        let canonical_ws = blocking(move || Ok(fs::canonicalize(&ws).ok())).await?;
        Ok(RemovalWindow {
            cutoff,
            session_folders,
            canonical_ws,
        })
    }

    /// What fold does under `window` with each of `closed`, the rows of [`workspaces::closed_directories`] read with `window.cutoff`; those of `returned` are marked so. A decision to remove does not look at the directory itself, so a path that names no workspace directory is only refused when the removal runs.
    pub(crate) fn closed_decisions(
        &self,
        closed: Vec<ClosedWorkspace>,
        window: &RemovalWindow,
        returned: &[WorkspaceId],
    ) -> Vec<ClosedDirectory> {
        let root = self.paths.root();
        closed
            .into_iter()
            .map(|closed| ClosedDirectory {
                decision: decide(&closed, window, root),
                returned: returned.contains(&closed.id),
                workspace: closed.id,
                path: closed.path,
                owner: closed.owner,
            })
            .collect()
    }

    /// The closed workspaces that fold recorded as removed while a directory is at their path again, such as one an agent wrote into by its full path after the removal. Only real directories directly below `.fold/ws/` count; a symlink there is never followed.
    pub(crate) async fn returned_directories(&self) -> Result<Vec<WorkspaceId>, WorkspaceError> {
        let ws = self.paths.ws_dir();
        let paths = blocking(move || {
            let entries = match fs::read_dir(&ws) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(error) => return Err(io_error(error)),
            };
            let mut paths = Vec::new();
            for entry in entries {
                let entry = entry.map_err(io_error)?;
                if entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && let Some(name) = entry.file_name().to_str()
                {
                    paths.push(format!("{WS_DIR}/{name}"));
                }
            }
            Ok(paths)
        })
        .await?;
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        Ok(workspaces::removed_directories_at(self.store.read(), &paths).await?)
    }

    /// Forgets the recorded removal of every workspace of [`WorkspaceService::returned_directories`], so its directory goes again the way a closed workspace's does, after a final checkpoint of the files fold has not recorded: they were written after fold removed the directory, so nothing else holds them. Returns those workspaces.
    pub(crate) async fn reopen_returned_directories(
        &self,
    ) -> Result<Vec<WorkspaceId>, WorkspaceError> {
        let returned = self.returned_directories().await?;
        if returned.is_empty() {
            return Ok(returned);
        }
        let reopened = self
            .store
            .write(async |tx| -> Result<Vec<WorkspaceId>, StoreError> {
                let mut reopened = Vec::new();
                for &workspace in &returned {
                    if workspaces::reopen_directory_removal(tx, workspace).await? {
                        reopened.push(workspace);
                    }
                }
                Ok(reopened)
            })
            .await?;
        for workspace in &reopened {
            tracing::info!(workspace = %workspace, "a removed workspace directory came back; fold removes it again");
        }
        Ok(reopened)
    }

    /// Removes the directory of a closed workspace once its owning agent has no worker process that has not ended and no open session, or once it closed longer than `closed_workspace_days` ago, as long as no open session works in it. The tracked files are snapshotted first, the way a checkpoint reads them, so ignored files such as build output are left out; when fold holds no such tree for the workspace (see [`workspaces::tree_recorded`]), a `before_removal` checkpoint commits before anything is deleted, in the write that records that the removal started. A snapshot that could not read a tracked file, or saw one change while it read it, is `unrecorded_changes` and nothing starts, because its tree would hold the recorded version of that path; a file over `max_file_bytes` does not stop the removal, as it is never recorded anywhere. A review checkout takes no checkpoint: its files are a proposal version, and a fix in it becomes a version only through `submit_review_fix`. The path must name a directory directly below `.fold/ws/` that is not a symlink (see [`locate_workspace_dir`]); anything else is refused before a file is read. The directory is deleted in blocking work, without following a symlink inside it, then one write records the removal. A failure after the start only deletes when it runs again, such as in the next `fld gc`, so a directory that was partly deleted never gets a checkpoint; a failure before it starts again from the snapshot.
    pub async fn remove_closed_directory(
        &self,
        workspace: WorkspaceId,
    ) -> Result<DirectoryRemoval, WorkspaceError> {
        let _op = self.ops.lock(workspace).await;
        let window = self.removal_window().await?;
        let due = workspaces::closed_directories(self.store.read(), Some(workspace), window.cutoff)
            .await?;
        let Some(closed) = due.into_iter().next() else {
            return Ok(DirectoryRemoval::NotDue);
        };
        if let ClosedDecision::Keep(reason) = decide(&closed, &window, self.paths.root()) {
            return Ok(DirectoryRemoval::Kept(reason));
        }
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
        #[cfg(feature = "failpoints")]
        fail::fail_point!("workspace_removal_after_delete", |_| {
            Err(WorkspaceError::from(FoldError::new(
                fold_core::codes::FAILPOINT,
                "workspace_removal_after_delete fired",
            )))
        });
        let now = self.clock.now();
        self.store
            .write(async |tx| workspaces::mark_directory_removed(tx, workspace, now).await)
            .await?;
        tracing::debug!(workspace = %workspace, "closed workspace directory removed");
        Ok(DirectoryRemoval::Removed { checkpoint })
    }

    /// [`WorkspaceService::remove_closed_directory`] for every closed workspace that is due, by id, until `stop` holds true, after [`WorkspaceService::reopen_returned_directories`]. A workspace whose removal fails is logged with its id and code and left for a later run; the others go on. Returns how many directories it removed.
    pub async fn remove_closed_directories(&self, stop: &(dyn Fn() -> bool + Sync)) -> u64 {
        if let Err(error) = self.reopen_returned_directories().await {
            tracing::warn!(
                code = error.log_code(),
                "the removed workspace directories that came back were not read"
            );
        }
        let due = match self.removal_window().await {
            Ok(window) => {
                workspaces::closed_directories_due(self.store.read(), None, window.cutoff)
                    .await
                    .map_err(WorkspaceError::from)
            }
            Err(error) => Err(error),
        };
        let due = match due {
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
                Ok(DirectoryRemoval::Kept(_) | DirectoryRemoval::NotDue) => {}
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

/// Removes the directories of the project's closed workspaces as they become due, through [`WorkspaceService::remove_closed_directories`]: after each committed event that closes a workspace, ends a worker process or ends a session, which can make a closed workspace of that agent due, after a missed event, and whenever `catch_up` is notified, as the daemon does when it opens the project, so what a stopped daemon left goes too. A catch-up also removes the folders of the finished task workers and reviewers (see [`collect_worker_folders`]), which only the passing of `closed_workspace_days` makes due; the daily garbage collection removes those that become due while the daemon runs. `events` is subscribed before the call, so nothing published after it is missed. It ends once the bus closes; a removal in progress finishes first.
pub async fn follow_closed_workspaces(
    service: WorkspaceService,
    bus: EventBus,
    mut events: broadcast::Receiver<BusMessage>,
    catch_up: Arc<Notify>,
) {
    let mut closing = bus.closing();
    loop {
        let (due, folders) = tokio::select! {
            biased;
            _ = closing.wait_for(|closed| *closed) => return,
            () = catch_up.notified() => (true, true),
            received = events.recv() => match received {
                Ok(BusMessage::Durable(event)) => (can_make_due(event.event_type), false),
                Ok(BusMessage::Live(_)) => (false, false),
                Err(RecvError::Lagged(_)) => (true, false),
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
        if folders && !stop() {
            service.remove_worker_folders().await;
        }
    }
}

impl WorkspaceService {
    /// [`collect_worker_folders`] for the project, with the window of `config.toml`; a failure is logged and left for the next run.
    async fn remove_worker_folders(&self) {
        let cutoff = match crate::services::projects::load_config(&self.paths).await {
            Ok(config) => window_cutoff(self.clock.now(), config.closed_workspace_days),
            Err(error) => {
                tracing::warn!(code = error.log_code(), "the worker folders were not read");
                return;
            }
        };
        match collect_worker_folders(&self.store, &self.paths, cutoff, false).await {
            Ok(folders) => {
                let removed = folders
                    .iter()
                    .filter(|folder| folder.decision == FolderDecision::Remove)
                    .count();
                if removed > 0 {
                    tracing::info!(removed, "the folders of finished workers were removed");
                }
            }
            Err(error) => {
                tracing::warn!(
                    code = error.log_code(),
                    "the worker folders were not removed; the next run tries again"
                );
            }
        }
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
