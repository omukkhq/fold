use std::path::Path;
use std::sync::Arc;

use fold_core::merge::ConflictKind;
use fold_core::object::ObjectHash;
use fold_core::{Actor, FoldError, RevisionId, TaskId, WorkspaceId, codes, policy};
use fold_store::{Store, StoreError, tasks, workspaces};
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_import::{GitImportOptions, import_commit};
use fold_vcs::git_remote::{fetch_branch, is_ancestor, merge_base, prepare_mirror};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::integrate::tree_merge;
use fold_vcs::objects::ObjectStore;

use super::commits::commit_locked;
use super::remote::{blocking, select_remote};
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
        remote: String,
        branch: String,
        remote_commit: Option<CommitId>,
    },
    /// The remote commit merged cleanly and main moved to `revision`.
    Integrated {
        remote: String,
        branch: String,
        remote_commit: CommitId,
        revision: RevisionId,
    },
    /// The merge conflicted, so main did not move: the sync task resolves it in its workspace. `created` is false when an open sync task for the same commit existed already and nothing was created.
    Conflicted {
        remote: String,
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

/// The newest commit fold main and the remote share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SyncBase {
    pub(super) commit: CommitId,
    /// Main's head revision when the commit is its commit: the remote only added to main, so a clean pull can be the remote commit itself.
    pub(super) fast_forward: Option<RevisionId>,
}

/// Brings the branch of the remote `remote`, or of the only remote without a name, into fold main, for the named human who runs `fld pull`. The project's remote lock and Git sync lock are held to the end, so a push or the commit of an integration never runs at the same time. Main's missing commits are written first, so the project's repository holds main's whole history. The remote branch is fetched into the repository; the sync base is the best common ancestor of the remote tip and main's newest commit, and without one it is the genesis tree. A commit the human makes in the project folder is no remote's: the Git job records it at once as the human's change.
///
/// A tip that is main's newest commit or one of its ancestors, or a remote with no such branch, is up to date. A tip that an open sync task already resolves names that task and creates nothing. Otherwise the remote tree is merged onto main against the sync base under the integration mutex: a clean merge goes through the pull entry of the integration procedure, so it is an integration of kind `pull` whose compare-and-swap protects main, and a conflict creates the sync task and its workspace while the mutex is still held. When the remote only added to main's newest commit, the new revision is the remote commit and the map gains `r<n> <tip>` after the commit, so no commit is written for it; a pull that also resolves a waiting human change gets a commit of its own with the remote commit and then the human's commit as its merge parents, so the project folder that holds the human's files follows main through the human's commit. The pull runs as its own task, so a caller that stops waiting never leaves an integration without its map line.
pub async fn pull(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
    remote: Option<&str>,
) -> Result<PullOutcome, ProjectError> {
    policy::can_pull(actor).map_err(FoldError::from)?;
    let (handle, actor, git) = (Arc::clone(handle), actor.clone(), git.clone());
    let remote = remote.map(str::to_owned);
    tokio::spawn(async move { run(&handle, &actor, &git, remote.as_deref()).await })
        .await
        .unwrap_or_else(|error| {
            Err(FoldError::new(codes::INTERNAL_ERROR, format!("the pull stopped: {error}")).into())
        })
}

async fn run(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
    remote: Option<&str>,
) -> Result<PullOutcome, ProjectError> {
    super::require_git_sync(&handle.store, "fld pull").await?;
    let config = load_config(&handle.paths).await?;
    let local = config.git.branch.clone();
    let remote_guard = handle.remote_lock.clone().lock_owned().await;
    let fold_dir = handle.paths.fold_dir();
    let mirror = handle.paths.git_dir();
    let (held, map) = {
        let fold_dir = fold_dir.clone();
        blocking(move || {
            let (sync_lock, map) = SyncMap::lock(&fold_dir)?;
            Ok(((remote_guard, sync_lock), map))
        })
        .await?
    };
    let (held, map, mut committed) = commit_locked(
        &handle.store,
        &handle.paths,
        git,
        &local,
        held,
        map,
        None,
        handle.clock.now(),
    )
    .await?;
    handle.bus.publish(std::mem::take(&mut committed.events));
    let (name, remote) = select_remote(&config, remote)?;
    let (url, branch) = (remote.url, remote.branch);
    let (held, fetched) = {
        let (git, mirror, name, branch) =
            (git.clone(), mirror.clone(), name.clone(), branch.clone());
        blocking(move || {
            prepare_mirror(&git, &mirror, &name, &url)?;
            let fetched = fetch_branch(&git, &mirror, &name, &branch)?;
            Ok((held, fetched))
        })
        .await?
    };
    let Some(tip) = fetched else {
        return Ok(PullOutcome::UpToDate {
            remote: name,
            branch,
            remote_commit: None,
        });
    };
    let base = {
        let (git, mirror, tip, last) = (
            git.clone(),
            mirror.clone(),
            tip.clone(),
            map.last().cloned(),
        );
        blocking(move || sync_base(&git, &mirror, &tip, last.as_ref())).await?
    };
    let base = match base {
        SyncCheck::Current => {
            return Ok(PullOutcome::UpToDate {
                remote: name,
                branch,
                remote_commit: Some(tip),
            });
        }
        SyncCheck::Base(base) => base,
    };
    if let Some(open) = open_sync_task(&handle.store, &name, &branch, &tip).await? {
        return Ok(open);
    }
    let genesis = workspaces::genesis(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("revisions.genesis"))?;
    let first_tree = genesis.tree_hash;
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
            let objects = ObjectStore::open_existing(&fold_dir)?;
            let remote = import_commit(&objects, &git, &mirror, tip.as_str(), &opts)?.tree;
            let base = match base_commit {
                Some(commit) => {
                    import_commit(&objects, &git, &mirror, commit.as_str(), &opts)?.tree
                }
                None => first_tree,
            };
            Ok((remote, base))
        })
        .await?
    };
    let base_label = match &base {
        Some(base) => format!("base {}", short_commit(base.commit.as_str())),
        None => format!("base {}", genesis.number),
    };
    let workspaces = WorkspaceService::for_project(handle);
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
                    &ObjectStore::open_existing(&fold_dir)?,
                    &base_tree,
                    &remote_tree,
                    &head_tree,
                )
            })
            .await?
        };
        if !merge.conflicts.is_empty() {
            let sync = SyncWorkspace {
                remote: name.clone(),
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
                remote: name,
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
            remote: name.clone(),
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
        if let Some(head) = base.as_ref().and_then(|base| base.fast_forward) {
            map_fast_forward(handle, held, map, head, revision, &tip, remote_tree).await;
        }
        return Ok(PullOutcome::Integrated {
            remote: name,
            branch,
            remote_commit: tip,
            revision,
        });
    }
    Err(FoldError::new(
        codes::HEAD_MOVED,
        format!("main moved during each of {ATTEMPTS} pull attempts"),
    )
    .with_hint("run fld pull again")
    .into())
}

/// Whether the remote tip is new to main and, when it is, the commit to merge it against.
pub(super) enum SyncCheck {
    /// The tip is main's newest commit or one of its ancestors.
    Current,
    Base(Option<SyncBase>),
}

/// Compares the remote tip with `last`, the commit of main's newest revision in the project's repository. The sync base is their best common ancestor, which every pull or sync task that merged a remote commit leaves in main's history as a second parent; without one, `r1`'s tree is the base. Blocking.
pub(super) fn sync_base(
    git: &Git,
    mirror: &Path,
    tip: &CommitId,
    last: Option<&(RevisionId, CommitId)>,
) -> Result<SyncCheck, FoldError> {
    let Some((head, last)) = last else {
        return Ok(SyncCheck::Base(None));
    };
    if tip == last || is_ancestor(git, mirror, tip, last)? {
        return Ok(SyncCheck::Current);
    }
    Ok(SyncCheck::Base(merge_base(git, mirror, tip, last)?.map(
        |commit| SyncBase {
            fast_forward: (commit == *last).then_some(*head),
            commit,
        },
    )))
}

/// The newest sync task created for `tip` that is neither Done nor archived, with its live workspace and that workspace's conflicted paths.
async fn open_sync_task(
    store: &Store,
    remote: &str,
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
        remote: remote.to_owned(),
        branch: branch.to_owned(),
        remote_commit: tip.clone(),
        task,
        workspace: live.map(|live| live.id),
        conflicted,
        created: false,
    }))
}

/// Appends `r<n> <tip>` to the map when `revision` is the remote commit itself: its parent is `head`, whose commit the remote tip descends from, its tree is the remote tree, and it brought onto main no human change other than `tip`. The integration has committed, so a failure here is only logged; the next commit of main then writes the revision as a merge of the tip, which the remote accepts as well.
pub(super) async fn map_fast_forward<H: Send + 'static>(
    handle: &ProjectHandle,
    held: H,
    map: SyncMap,
    head: RevisionId,
    revision: RevisionId,
    tip: &CommitId,
    remote_tree: ObjectHash,
) {
    let row = match workspaces::get_by_number(handle.store.read(), revision).await {
        Ok(row) => row,
        Err(error) => {
            tracing::warn!(revision = %revision, code = error.log_code(), "the pulled revision could not be read for the sync map");
            return;
        }
    };
    // A pull that resolved a waiting human change needs a commit of its own, whose parents keep the human's commit reachable.
    let other_human = row
        .integration
        .as_ref()
        .and_then(|integration| integration.human_commit.as_deref())
        .is_some_and(|human| human != tip.as_str());
    if row.parent != Some(head) || row.tree_hash != remote_tree || other_human {
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
        tracing::warn!(revision = %revision, code = error.log_code(), "the sync map did not gain the pulled revision");
    }
}

/// A refusal of the review or workspace step the pull runs keeps its code.
pub(super) fn project_error(error: ReviewError) -> ProjectError {
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
