use std::convert::Infallible;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::{CheckpointId, EventActor, FoldError, NewEvent, Timestamp, WorkspaceId, codes};
use fold_store::workspaces::{
    ClosedWorkspace, closed_workspaces_due, gc_roots, prune_auto, prune_workspace,
    workspaces_over_auto_limit,
};
use fold_store::{StoreError, WriteTx};
pub use fold_vcs::gc::GcReport;
use fold_vcs::gc::{self as vcs_gc, GRACE};
use fold_vcs::objects::ObjectStore;
use serde_json::json;

use super::{ProjectError, load_config};
use crate::projects::ProjectHandle;
use crate::services::events::EventBuffer;

/// Collects the garbage of one project. It reads `now` once, prunes the automatic checkpoints of each workspace beyond the configured number, removes the directories of closed workspaces past their retention and prunes all their checkpoints, then marks every object the remaining rows reach and sweeps the rest that is older than the grace period while it holds the project's object lock exclusive. A dry run makes the same database changes in a write that always rolls back and plans the sweep, so its counts are those of a real run and nothing changes. One run per project at a time; a second call is `gc_in_progress`.
pub async fn gc(project: &ProjectHandle, dry_run: bool) -> Result<GcReport, ProjectError> {
    let Ok(_running) = project.gc.try_lock() else {
        return Err(FoldError::new(
            codes::GC_IN_PROGRESS,
            format!("garbage collection of project {} is running", project.slug),
        )
        .with_hint("wait for the running collection to finish, then run fold gc again")
        .into());
    };
    let now = project.clock.now();
    let config = load_config(&project.paths).await?;
    let keep = u32::try_from(config.auto_checkpoints_kept).unwrap_or(u32::MAX);
    let due =
        closed_workspaces_due(project.store.read(), now, config.closed_workspace_days).await?;
    let pruning = if dry_run {
        plan_pruning(project, keep, due, now).await?
    } else {
        prune(project, keep, due, now).await?
    };
    let objects_dir = project.paths.fold_dir();
    let fs_now = file_time(now);
    let roots = pruning.roots;
    let marked = blocking(move || -> Result<_, FoldError> {
        let objects = ObjectStore::open(&objects_dir)?;
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
    let mut report = blocking(move || -> Result<GcReport, FoldError> {
        let (objects, marked) = marked;
        let swept = match exclusive {
            None => vcs_gc::plan_sweep(&objects, &marked, fs_now, GRACE),
            Some(_held) => vcs_gc::sweep(&objects, &marked, fs_now, GRACE),
        };
        Ok(swept?)
    })
    .await??;
    for hash in &report.missing {
        tracing::warn!(project = %project.slug, code = codes::OBJECT_MISSING, object = %hash, "garbage collection reached an object the store does not hold");
    }
    report.workspaces_removed = pruning.workspaces_removed;
    report.checkpoints_pruned = pruning.checkpoints_pruned;
    Ok(report)
}

/// What the pruning step of a run did, and the roots that remain after it.
struct Pruning {
    checkpoints_pruned: u64,
    workspaces_removed: u64,
    roots: Vec<ObjectHash>,
}

async fn prune(
    project: &ProjectHandle,
    keep: u32,
    due: Vec<ClosedWorkspace>,
    now: Timestamp,
) -> Result<Pruning, ProjectError> {
    let (buffer, mut checkpoints_pruned) = project
        .store
        .write(async |tx| -> Result<(EventBuffer, u64), ProjectError> {
            let mut buffer = EventBuffer::new();
            let mut pruned = 0;
            for workspace in workspaces_over_auto_limit(tx.conn(), keep).await? {
                let ids = prune_auto(tx, workspace, keep, now).await?;
                pruned += append_pruned(tx, &mut buffer, workspace, &ids, now).await?;
            }
            Ok((buffer, pruned))
        })
        .await?;
    project.bus.publish(buffer);
    let mut workspaces_removed = 0;
    for workspace in due {
        let root = project.paths.root().to_owned();
        let path = PathBuf::from(&workspace.path);
        let existed = match blocking(move || vcs_gc::remove_workspace_dir(&root, &path)).await? {
            Ok(existed) => existed,
            Err(error) => {
                tracing::warn!(project = %project.slug, workspace = %workspace.id, code = error.code(), "closed workspace directory was not removed");
                continue;
            }
        };
        let (buffer, pruned) = project
            .store
            .write(async |tx| -> Result<(EventBuffer, u64), ProjectError> {
                let mut buffer = EventBuffer::new();
                let ids = prune_workspace(tx, workspace.id, now).await?;
                let pruned = append_pruned(tx, &mut buffer, workspace.id, &ids, now).await?;
                Ok((buffer, pruned))
            })
            .await?;
        project.bus.publish(buffer);
        checkpoints_pruned += pruned;
        if existed || pruned > 0 {
            workspaces_removed += 1;
        }
    }
    let roots = gc_roots(project.store.read()).await?;
    Ok(Pruning {
        checkpoints_pruned,
        workspaces_removed,
        roots,
    })
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
    due: Vec<ClosedWorkspace>,
    now: Timestamp,
) -> Result<Pruning, ProjectError> {
    let mut located = Vec::with_capacity(due.len());
    for workspace in due {
        let root = project.paths.root().to_owned();
        let path = PathBuf::from(&workspace.path);
        match blocking(move || vcs_gc::locate_workspace_dir(&root, &path)).await? {
            Ok(found) => located.push((workspace.id, found.is_some())),
            Err(error) => {
                tracing::warn!(project = %project.slug, workspace = %workspace.id, code = error.code(), "closed workspace directory would not be removed");
            }
        }
    }
    let outcome = project
        .store
        .write(async |tx| -> Result<Infallible, DryRun> {
            let mut checkpoints_pruned = 0;
            for workspace in workspaces_over_auto_limit(tx.conn(), keep).await? {
                checkpoints_pruned += count(prune_auto(tx, workspace, keep, now).await?.len());
            }
            let mut workspaces_removed = 0;
            for (workspace, exists) in &located {
                let pruned = count(prune_workspace(tx, *workspace, now).await?.len());
                checkpoints_pruned += pruned;
                if *exists || pruned > 0 {
                    workspaces_removed += 1;
                }
            }
            let roots = gc_roots(tx.conn()).await?;
            Err(DryRun::Planned(Pruning {
                checkpoints_pruned,
                workspaces_removed,
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
        let event = NewEvent::new(types::CHECKPOINT_PRUNED, EventActor::fold())
            .and_then(|event| {
                event
                    .workspace(workspace)
                    .data(json!({"checkpoint": id.to_string()}))
            })
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
