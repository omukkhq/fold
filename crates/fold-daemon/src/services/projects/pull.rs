use std::sync::Arc;

use fold_core::merge::ConflictKind;
use fold_core::object::ObjectHash;
use fold_core::{Actor, FoldError, RevisionId, TaskId, WorkspaceId, codes};
use fold_store::{Store, StoreError, reviews, tasks, workspaces};
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_import::{GitImportOptions, archive_tree};
use fold_vcs::git_remote::{MIRROR_DIR, fetch_branch, is_ancestor, prepare_mirror};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::integrate::tree_merge;
use fold_vcs::objects::ObjectStore;

use super::remote::{blocking, diverged, remote_not_set};
use super::{ProjectError, load_config};
use crate::projects::ProjectHandle;
use crate::services::reviews::{self as review, PullIntegration, ReviewError, ReviewService};
use crate::services::tasks::TaskError;
use crate::services::workspaces::{SyncWorkspace, WorkspaceService, short_commit};

/// How many times a pull starts again from reading main when main moved between its own merge and the integration's.
const ATTEMPTS: usize = 3;

/// What a pull did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullOutcome {
    /// Main holds the remote branch already, or the remote has no such branch (`remote_commit` is `None`).
    UpToDate {
        branch: String,
        remote_commit: Option<CommitId>,
    },
    /// The remote commit merged cleanly and main moved to `revision`.
    Integrated {
        branch: String,
        remote_commit: CommitId,
        revision: RevisionId,
    },
    /// The merge conflicted, so main did not move: the sync task resolves it in its workspace. `created` is false when an open sync task for the same commit existed already and nothing was created.
    Conflicted {
        branch: String,
        remote_commit: CommitId,
        task: TaskId,
        workspace: Option<WorkspaceId>,
        /// In path order.
        conflicted: Vec<PulledConflict>,
        created: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulledConflict {
    pub path: String,
    pub kind: ConflictKind,
}

/// The newest commit fold main and the remote share, and the revision it was found on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SyncBase {
    commit: CommitId,
    revision: RevisionId,
    /// True when `.fold/git-sync.map` pairs the revision with the commit, so the revision is the commit; false when the revision's integration brought the commit in and holds a merge of it.
    mapped: bool,
}

/// Brings the `[git]` branch of the remote into fold main, for the named human who runs `fold pull`. The branch is fetched into the mirror under the project's remote lock and Git sync lock, which are held to the end, so a push or an export never runs at the same time. The sync base is the newest commit main and the remote share: walking main back from the head, the first revision that `.fold/git-sync.map` pairs with a commit or whose integration brought a remote commit in; without one it is `r1`'s tree.
///
/// A tip that is the sync base or one of its ancestors, or a remote with no such branch, is up to date. A tip that an open sync task already resolves names that task and creates nothing. Otherwise the remote tree is merged onto main against the sync base under the integration mutex: a clean merge goes through the pull entry of the integration procedure, so it is an integration of kind `pull` whose compare-and-swap protects main, and a conflict creates the sync task and its workspace while the mutex is still held. When main had not moved since the sync base was pushed or imported, the new revision is the remote commit and the map gains `r<n> <tip>` after the commit, so the next push writes nothing for it. The pull runs as its own task, so a caller that stops waiting never leaves an integration without its map line.
pub async fn pull(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
) -> Result<PullOutcome, ProjectError> {
    if !actor.is_human() {
        return Err(FoldError::new(
            codes::HUMAN_REQUIRED,
            "only a human brings the remote branch into main",
        )
        .with_hint("ask a human to run fold pull")
        .into());
    }
    let (handle, actor, git) = (Arc::clone(handle), actor.clone(), git.clone());
    tokio::spawn(async move { run(&handle, &actor, &git).await })
        .await
        .unwrap_or_else(|error| {
            Err(FoldError::new(codes::INTERNAL_ERROR, format!("the pull stopped: {error}")).into())
        })
}

async fn run(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
) -> Result<PullOutcome, ProjectError> {
    let config = load_config(&handle.paths).await?;
    let Some(remote) = config.git.remote else {
        return Err(remote_not_set().into());
    };
    let branch = config.git.branch;
    let remote_guard = handle.remote_lock.clone().lock_owned().await;
    let fold_dir = handle.paths.fold_dir();
    let mirror = fold_dir.join(MIRROR_DIR);
    let (held, map, fetched) = {
        let (git, fold_dir, mirror, branch) = (
            git.clone(),
            fold_dir.clone(),
            mirror.clone(),
            branch.clone(),
        );
        blocking(move || {
            let (sync_lock, map) = SyncMap::lock(&fold_dir)?;
            prepare_mirror(&git, &mirror, &remote)?;
            let fetched = fetch_branch(&git, &mirror, &branch)?;
            Ok(((remote_guard, sync_lock), map, fetched))
        })
        .await?
    };
    let Some(tip) = fetched else {
        return Ok(PullOutcome::UpToDate {
            branch,
            remote_commit: None,
        });
    };
    let base = sync_base(&handle.store, &map).await?;
    if let Some(base) = &base {
        let (git, mirror, base, fetched) = (git.clone(), mirror.clone(), base.clone(), tip.clone());
        let current = blocking(move || {
            if !git.has_commit(&mirror, &base.commit)? {
                return Err(diverged(base.revision, &base.commit));
            }
            Ok(fetched == base.commit || is_ancestor(&git, &mirror, &fetched, &base.commit)?)
        })
        .await?;
        if current {
            return Ok(PullOutcome::UpToDate {
                branch,
                remote_commit: Some(tip),
            });
        }
    }
    if let Some(open) = open_sync_task(&handle.store, &branch, &tip).await? {
        return Ok(open);
    }
    let first = RevisionId::from_number(1).ok_or(StoreError::Corrupt("revision number 1"))?;
    let first_tree = workspaces::get_by_number(handle.store.read(), first)
        .await?
        .tree_hash;
    let (remote_tree, base_tree) = {
        // Shared while the trees are written or refreshed, so a collection never deletes an object between its refresh and the end of this read; from then on the grace period keeps them until the revision that names them commits.
        let objects_guard = handle.objects.clone().read_owned().await;
        let (git, fold_dir, mirror, tip) = (git.clone(), fold_dir.clone(), mirror, tip.clone());
        let base_commit = base.as_ref().map(|base| base.commit.clone());
        let opts = GitImportOptions {
            max_file_bytes: config.max_file_bytes,
        };
        blocking(move || {
            let _objects = objects_guard;
            let objects = ObjectStore::open(&fold_dir)?;
            let remote = archive_tree(&objects, &git, &mirror, tip.as_str(), &opts)?.tree;
            let base = match base_commit {
                Some(commit) => archive_tree(&objects, &git, &mirror, commit.as_str(), &opts)?.tree,
                None => first_tree,
            };
            Ok((remote, base))
        })
        .await?
    };
    let base_label = match &base {
        Some(base) => format!("base {}", short_commit(base.commit.as_str())),
        None => format!("base {first}"),
    };
    let workspaces = WorkspaceService::for_project(handle, git.clone());
    let reviews = ReviewService::new(
        handle.store.clone(),
        handle.bus.clone(),
        handle.clock.clone(),
        workspaces.clone(),
        handle.integration.clone(),
    );
    for _ in 0..ATTEMPTS {
        let serial = handle.integration.clone().lock_owned().await;
        let head = workspaces::head(handle.store.read())
            .await?
            .ok_or(StoreError::Corrupt("refs.main"))?;
        let merge = {
            let fold_dir = fold_dir.clone();
            let head_tree = head.tree_hash;
            blocking(move || {
                tree_merge(
                    &ObjectStore::open(&fold_dir)?,
                    &base_tree,
                    &remote_tree,
                    &head_tree,
                )
            })
            .await?
        };
        if !merge.conflicts.is_empty() {
            let sync = SyncWorkspace {
                branch: branch.clone(),
                remote_commit: tip.to_string(),
                remote_tree,
                base_label,
                head: head.number,
                merge,
            };
            let created = workspaces
                .create_sync_workspace(actor, sync)
                .await
                .map_err(|error| project_error(error.into()))?;
            drop(serial);
            return Ok(PullOutcome::Conflicted {
                branch,
                remote_commit: tip,
                task: created.task,
                workspace: Some(created.workspace),
                conflicted: created
                    .conflicted
                    .iter()
                    .map(|conflict| PulledConflict {
                        path: String::from_utf8_lossy(conflict.path.as_bytes()).into_owned(),
                        kind: conflict.kind,
                    })
                    .collect(),
                created: true,
            });
        }
        drop(serial);
        let request = PullIntegration {
            remote_commit: tip.to_string(),
            branch: branch.clone(),
            base_tree,
            remote_tree,
        };
        let revision = match reviews
            .integrate_pull(actor, request)
            .await
            .map_err(project_error)?
        {
            review::PullOutcome::Succeeded { revision } => revision,
            // Main moved after the merge above and the remote now conflicts with it: merge again under the mutex.
            review::PullOutcome::Conflicted(_) => continue,
        };
        if let Some(base) = base.as_ref().filter(|base| base.mapped) {
            map_fast_forward(handle, held, map, base, revision, &tip, remote_tree).await;
        }
        return Ok(PullOutcome::Integrated {
            branch,
            remote_commit: tip,
            revision,
        });
    }
    Err(FoldError::new(
        codes::HEAD_MOVED,
        format!("main moved during each of {ATTEMPTS} pull attempts"),
    )
    .with_hint("run fold pull again")
    .into())
}

/// The newest of the last revision the map pairs with a commit and the last revision whose integration brought a remote commit in. Main is one line of revisions and the map only grows at its end, so this is the first revision a walk back from the head finds. A revision that is both gives its mapped commit, which is newer: the pushed merge of the commit it brought in.
async fn sync_base(store: &Store, map: &SyncMap) -> Result<Option<SyncBase>, ProjectError> {
    let mapped = map.last().map(|(revision, commit)| SyncBase {
        commit: commit.clone(),
        revision: *revision,
        mapped: true,
    });
    let pulled = match reviews::latest_remote_commit(store.read()).await? {
        Some((revision, commit)) => Some(SyncBase {
            commit: commit
                .parse()
                .map_err(|_| StoreError::Corrupt("integrations.remote_commit"))?,
            revision,
            mapped: false,
        }),
        None => None,
    };
    Ok(match (mapped, pulled) {
        (Some(mapped), Some(pulled)) if pulled.revision > mapped.revision => Some(pulled),
        (Some(mapped), _) => Some(mapped),
        (None, pulled) => pulled,
    })
}

/// The newest sync task created for `tip` that is neither Done nor archived, with its live workspace and that workspace's conflicted paths.
async fn open_sync_task(
    store: &Store,
    branch: &str,
    tip: &CommitId,
) -> Result<Option<PullOutcome>, ProjectError> {
    let Some(task) = tasks::open_sync_task(store.read(), tip.as_str()).await? else {
        return Ok(None);
    };
    let live = workspaces::live_task_workspace(store.read(), task).await?;
    let conflicted = match &live {
        Some(live) => workspaces::list_conflicts(store.read(), live.id)
            .await?
            .into_iter()
            .map(|row| PulledConflict {
                path: String::from_utf8_lossy(&row.path).into_owned(),
                kind: row.kind,
            })
            .collect(),
        None => Vec::new(),
    };
    Ok(Some(PullOutcome::Conflicted {
        branch: branch.to_owned(),
        remote_commit: tip.clone(),
        task,
        workspace: live.map(|live| live.id),
        conflicted,
        created: false,
    }))
}

/// Appends `r<n> <tip>` to the map when `revision` is the remote commit itself: its parent is the revision the map pairs with the sync base, so main had not moved, and its tree is the remote tree. The integration has committed, so a failure here is only logged; the next push then writes the revision as a merge of the tip, which the remote accepts as well.
async fn map_fast_forward<H: Send + 'static>(
    handle: &ProjectHandle,
    held: H,
    map: SyncMap,
    base: &SyncBase,
    revision: RevisionId,
    tip: &CommitId,
    remote_tree: ObjectHash,
) {
    let row = match workspaces::get_by_number(handle.store.read(), revision).await {
        Ok(row) => row,
        Err(error) => {
            tracing::warn!(revision = %revision, code = error.code().unwrap_or("store_error"), "the pulled revision could not be read for the sync map");
            return;
        }
    };
    if row.parent != Some(base.revision) || row.tree_hash != remote_tree {
        return;
    }
    let added = [(revision, tip.clone())];
    let appended = blocking(move || {
        let _held = held;
        let mut map = map;
        map.append(&added)
    })
    .await;
    if let Err(error) = appended {
        tracing::warn!(revision = %revision, code = error.code().unwrap_or("store_error"), "the sync map did not gain the pulled revision");
    }
}

/// A refusal of the review or workspace step the pull runs keeps its code.
fn project_error(error: ReviewError) -> ProjectError {
    match error {
        ReviewError::Fold(error) | ReviewError::Task(TaskError::Fold(error)) => error.into(),
        ReviewError::Store(error) | ReviewError::Task(TaskError::Store(error)) => error.into(),
        ReviewError::Event(error) | ReviewError::Task(TaskError::Event(error)) => {
            FoldError::new(codes::INTERNAL_ERROR, error.to_string()).into()
        }
    }
}

#[cfg(test)]
mod tests {
    use fold_core::EventError;

    use super::*;

    #[test]
    fn review_errors_keep_their_code() {
        let refused = FoldError::new(codes::HEAD_MOVED, "moved");
        assert_eq!(
            project_error(ReviewError::Fold(refused.clone())).code(),
            Some(codes::HEAD_MOVED)
        );
        assert_eq!(
            project_error(ReviewError::Task(TaskError::Fold(refused))).code(),
            Some(codes::HEAD_MOVED)
        );
        let corrupt = || StoreError::Corrupt("x");
        assert_eq!(
            project_error(ReviewError::Store(corrupt())).code(),
            ProjectError::from(corrupt()).code()
        );
        assert_eq!(
            project_error(ReviewError::Task(TaskError::Store(corrupt()))).code(),
            ProjectError::from(corrupt()).code()
        );
        for event in [
            ReviewError::Event(EventError::UnknownType),
            ReviewError::Task(TaskError::Event(EventError::UnknownType)),
        ] {
            assert_eq!(project_error(event).code(), Some(codes::INTERNAL_ERROR));
        }
    }
}
