use std::convert::Infallible;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use fold_core::object::ObjectHash;
use fold_core::{CheckpointId, FoldError, Timestamp, WorkspaceId, codes};
use fold_store::workspaces::{
    closed_checkpoints_due, closed_directories, gc_roots, mark_directory_removed, prune_auto,
    prune_workspace, reopen_directory_removal, workspaces_over_auto_limit,
};
use fold_store::{StoreError, WriteTx};
use fold_vcs::gc::{self as vcs_gc, GRACE};
use fold_vcs::objects::ObjectStore;

use super::{ProjectError, load_config};
use crate::projects::ProjectHandle;
use crate::services::agents::{FolderDecision, WorkerFolder, collect_worker_folders};
use crate::services::events::EventBuffer;
use crate::services::workspaces::{
    ClosedDecision, ClosedDirectory, DirectoryRemoval, KeepReason, WorkspaceError,
    WorkspaceService, pruned_event, window_cutoff,
};

/// What one garbage collection run did, or would do in a dry run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    pub examined: u64,
    pub deleted: u64,
    pub bytes_freed: u64,
    pub tmp_removed: u64,
    /// Objects the walk reached that the store does not hold, each an `object_missing` finding. Sorted.
    pub missing: Vec<ObjectHash>,
    pub workspaces_removed: u64,
    pub checkpoints_pruned: u64,
    /// Folders of finished task workers and reviewers that the run removed.
    pub workers_removed: u64,
    /// The bytes of the files in those folders.
    pub worker_bytes_freed: u64,
    /// Every closed workspace whose directory was still there, or came back, by id, with what the run did about it.
    pub workspaces: Vec<ClosedDirectory>,
    /// Every folder of a task worker or reviewer that stopped before the window, by agent, with what the run did about it.
    pub workers: Vec<WorkerFolder>,
}

/// Collects the garbage of one project. It reads `now` once, prunes the automatic checkpoints of each workspace beyond the configured number, removes every closed workspace directory that is still there, or came back after its removal, once its owning agent has no running worker process and no open session or once it closed longer than `closed_workspace_days` ago (see [`WorkspaceService::remove_closed_directory`]), which retries a removal that failed when the workspace closed, removes the folders of the task workers and reviewers that stopped longer than `closed_workspace_days` ago and can never run again on their own (see [`collect_worker_folders`]), prunes all the checkpoints of the closed workspaces past `closed_workspace_days`, then marks every object the remaining rows reach and sweeps the rest that is older than the grace period while it holds the project's object lock exclusive. A dry run makes the same database changes in a write that always rolls back and plans the sweep, so its counts are those of a real run and nothing changes; it takes no final checkpoint, reads no workspace file and deletes no folder. The report names each closed workspace and worker folder with what the run did, or would do, and why. One run per project at a time; a second call is `gc_in_progress`.
pub async fn gc(project: &ProjectHandle, dry_run: bool) -> Result<GcReport, ProjectError> {
    let Ok(_running) = project.gc.try_lock() else {
        return Err(FoldError::new(
            codes::GC_IN_PROGRESS,
            format!("garbage collection of project {} is running", project.slug),
        )
        .with_hint("wait for the running collection to finish, then run `fld gc` again")
        .into());
    };
    // The sweep opens the object store, which would make the deleted `.fold/` again.
    if project.database_replaced().await {
        return Err(FoldError::new(
            codes::PROJECT_UNAVAILABLE,
            format!(
                "the database of project {} was deleted or replaced",
                project.slug
            ),
        )
        .into());
    }
    let now = project.clock.now();
    let config = load_config(&project.paths).await?;
    let keep = u32::try_from(config.auto_checkpoints_kept).unwrap_or(u32::MAX);
    let days = config.closed_workspace_days;
    let pruning = if dry_run {
        plan_pruning(project, keep, days, now).await?
    } else {
        prune(project, keep, days, now).await?
    };
    let objects_dir = project.paths.fold_dir();
    let fs_now = file_time(now);
    let roots = pruning.roots;
    let marked = blocking(move || -> Result<_, FoldError> {
        let objects = ObjectStore::open_existing(&objects_dir)?;
        let marked = vcs_gc::mark(&objects, roots)?;
        Ok((objects, marked))
    })
    .await??;
    // A dry run deletes nothing, so it does not hold up the writers.
    let exclusive = if dry_run {
        None
    } else {
        Some(project.objects.clone().write_owned().await)
    };
    let swept = blocking(move || -> Result<vcs_gc::GcReport, FoldError> {
        let (objects, marked) = marked;
        let swept = match exclusive {
            None => vcs_gc::plan_sweep(&objects, &marked, fs_now, GRACE),
            Some(_held) => vcs_gc::sweep(&objects, &marked, fs_now, GRACE),
        };
        Ok(swept?)
    })
    .await??;
    for hash in &swept.missing {
        tracing::warn!(project = %project.slug, code = codes::OBJECT_MISSING, object = %hash, "garbage collection reached an object the store does not hold");
    }
    let workers = collect_worker_folders(
        &project.store,
        &project.paths,
        window_cutoff(now, days),
        dry_run,
    )
    .await
    .map_err(|error| FoldError::new(error.log_code(), error.to_string()))?;
    let removed: Vec<&WorkerFolder> = workers
        .iter()
        .filter(|folder| folder.decision == FolderDecision::Remove)
        .collect();
    Ok(GcReport {
        examined: swept.examined,
        deleted: swept.deleted,
        bytes_freed: swept.bytes_freed,
        tmp_removed: swept.tmp_removed,
        missing: swept.missing,
        workspaces_removed: pruning.workspaces_removed,
        checkpoints_pruned: pruning.checkpoints_pruned,
        workers_removed: count(removed.len()),
        worker_bytes_freed: removed.iter().map(|folder| folder.bytes).sum(),
        workspaces: pruning.workspaces,
        workers,
    })
}

/// What the pruning step of a run did, and the roots that remain after it.
struct Pruning {
    checkpoints_pruned: u64,
    workspaces_removed: u64,
    workspaces: Vec<ClosedDirectory>,
    roots: Vec<ObjectHash>,
}

async fn prune(
    project: &ProjectHandle,
    keep: u32,
    days: u64,
    now: Timestamp,
) -> Result<Pruning, ProjectError> {
    let (buffer, mut checkpoints_pruned, over) = project
        .store
        .write(
            async |tx| -> Result<(EventBuffer, u64, Vec<WorkspaceId>), ProjectError> {
                let mut buffer = EventBuffer::new();
                let mut pruned = 0;
                let over = workspaces_over_auto_limit(tx.conn(), keep).await?;
                for &workspace in &over {
                    let ids = prune_auto(tx, workspace, keep, now).await?;
                    pruned += append_pruned(tx, &mut buffer, workspace, &ids, now).await?;
                }
                Ok((buffer, pruned, over))
            },
        )
        .await?;
    project.bus.publish(buffer);
    for workspace in over {
        project.live.reload_checkpoints(workspace).await;
    }
    let service = WorkspaceService::for_project(project);
    let returned = service.reopen_returned_directories().await?;
    let window = service.removal_window().await?;
    let closed = closed_directories(project.store.read(), None, window.cutoff).await?;
    let listed = service.closed_decisions(closed, &window, &returned);
    let mut workspaces_removed = 0;
    let mut workspaces = Vec::with_capacity(listed.len());
    for mut closed in listed {
        if matches!(closed.decision, ClosedDecision::Remove(_)) {
            match service.remove_closed_directory(closed.workspace).await {
                Ok(DirectoryRemoval::Removed { .. }) => workspaces_removed += 1,
                Ok(DirectoryRemoval::Kept(reason)) => {
                    closed.decision = ClosedDecision::Keep(reason)
                }
                Ok(DirectoryRemoval::NotDue) => continue,
                Err(error) => {
                    tracing::warn!(project = %project.slug, workspace = %closed.workspace, code = error.log_code(), "closed workspace directory was not removed");
                    closed.decision = ClosedDecision::Keep(match error.log_code() {
                        codes::INVALID_PATH => KeepReason::InvalidPath,
                        code => KeepReason::Failed(code),
                    });
                }
            }
        }
        workspaces.push(closed);
    }
    let (buffer, pruned) = project
        .store
        .write(async |tx| -> Result<(EventBuffer, u64), ProjectError> {
            let mut buffer = EventBuffer::new();
            let mut pruned = 0;
            for workspace in closed_checkpoints_due(tx.conn(), now, days).await? {
                let ids = prune_workspace(tx, workspace, now).await?;
                pruned += append_pruned(tx, &mut buffer, workspace, &ids, now).await?;
            }
            Ok((buffer, pruned))
        })
        .await?;
    project.bus.publish(buffer);
    checkpoints_pruned += pruned;
    let roots = gc_roots(project.store.read()).await?;
    Ok(Pruning {
        checkpoints_pruned,
        workspaces_removed,
        workspaces,
        roots,
    })
}

impl From<WorkspaceError> for ProjectError {
    fn from(error: WorkspaceError) -> Self {
        match error {
            WorkspaceError::Fold(error) => Self::Fold(error),
            WorkspaceError::Store(error) => Self::Store(error),
            other => Self::Fold(FoldError::new(other.log_code(), other.to_string())),
        }
    }
}

/// How a dry run leaves its write: always through `Err`, so the write rolls back.
enum DryRun {
    Planned(Pruning),
    Failed(ProjectError),
}

impl From<StoreError> for DryRun {
    fn from(error: StoreError) -> Self {
        Self::Failed(error.into())
    }
}

async fn plan_pruning(
    project: &ProjectHandle,
    keep: u32,
    days: u64,
    now: Timestamp,
) -> Result<Pruning, ProjectError> {
    let service = WorkspaceService::for_project(project);
    let returned = service.returned_directories().await?;
    let window = service.removal_window().await?;
    let root = project.paths.root().to_owned();
    let outcome = project
        .store
        .write(async |tx| -> Result<Infallible, DryRun> {
            let mut checkpoints_pruned = 0;
            for workspace in workspaces_over_auto_limit(tx.conn(), keep).await? {
                checkpoints_pruned += count(prune_auto(tx, workspace, keep, now).await?.len());
            }
            for &workspace in &returned {
                reopen_directory_removal(tx, workspace).await?;
            }
            let closed = closed_directories(tx.conn(), None, window.cutoff).await?;
            let mut workspaces = service.closed_decisions(closed, &window, &returned);
            let mut workspaces_removed = 0;
            for closed in &mut workspaces {
                if !matches!(closed.decision, ClosedDecision::Remove(_)) {
                    continue;
                }
                let (root, path) = (root.clone(), PathBuf::from(&closed.path));
                let located = blocking(move || vcs_gc::locate_workspace_dir(&root, &path))
                    .await
                    .map_err(DryRun::Failed)?;
                if located.is_err() {
                    closed.decision = ClosedDecision::Keep(KeepReason::InvalidPath);
                    continue;
                }
                if mark_directory_removed(tx, closed.workspace, now).await? {
                    workspaces_removed += 1;
                }
            }
            for workspace in closed_checkpoints_due(tx.conn(), now, days).await? {
                checkpoints_pruned += count(prune_workspace(tx, workspace, now).await?.len());
            }
            let roots = gc_roots(tx.conn()).await?;
            Err(DryRun::Planned(Pruning {
                checkpoints_pruned,
                workspaces_removed,
                workspaces,
                roots,
            }))
        })
        .await;
    match outcome {
        Ok(never) => match never {},
        Err(DryRun::Planned(pruning)) => Ok(pruning),
        Err(DryRun::Failed(error)) => Err(error),
    }
}

/// Appends one `checkpoint.pruned` event per marked checkpoint and returns how many there were.
async fn append_pruned(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    workspace: WorkspaceId,
    ids: &[CheckpointId],
    now: Timestamp,
) -> Result<u64, ProjectError> {
    for id in ids {
        let event = pruned_event(workspace, *id)
            .map_err(|error| FoldError::new(codes::INTERNAL_ERROR, error.to_string()))?;
        buffer.append(tx, &event, now).await?;
    }
    Ok(count(ids.len()))
}

fn count(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

/// Object modification times live in the file system's clock domain; the run's `now` comes from the project clock, which in production reads the same system time.
fn file_time(now: Timestamp) -> SystemTime {
    u64::try_from(now.as_millis())
        .ok()
        .and_then(|millis| SystemTime::UNIX_EPOCH.checked_add(Duration::from_millis(millis)))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ProjectError> {
    tokio::task::spawn_blocking(work).await.map_err(|_| {
        ProjectError::Fold(FoldError::new(
            codes::INTERNAL_ERROR,
            "a garbage collection step stopped unexpectedly",
        ))
    })
}
