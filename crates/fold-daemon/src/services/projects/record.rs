use std::sync::Arc;

use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::{Actor, FoldError, HumanName, NewEvent, RevisionId, codes, policy};
use fold_store::workspaces::{self, ConflictRow, NewHumanChange};
use fold_store::{Store, StoreError};
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_export::{ExportBase, ExportCommit, ExportTarget, Signature, write_commit};
use fold_vcs::git_import::{GitImportOptions, import_commit};
use fold_vcs::git_remote::{
    branch_ref, checked_out_branch, is_ancestor, project_repository, set_ref,
};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::import::{ImportOptions, import_dir};
use fold_vcs::integrate::tree_merge;
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::Capabilities;
use fold_vcs::scan::Skipped;
use serde_json::json;

use super::commits::{commit_locked, commit_main, fold_signature};
use super::pull::{
    PulledConflict, SyncCheck, map_fast_forward, own_repository, project_error, sync_base,
};
use super::remote::blocking;
use super::{ProjectError, load_config};
use crate::projects::ProjectHandle;
use crate::services::events::EventBuffer;
#[cfg(feature = "failpoints")]
use crate::services::fired;
use crate::services::reviews::{self as review, HumanRecord, PullIntegration, ReviewService};
use crate::services::workspaces::{WorkspaceError, WorkspaceService};

/// How many times a record starts again from reading main when main moved between its own merge and the integration's.
const ATTEMPTS: usize = 3;

/// The ref that holds the newest commit `fld record` made of the project folder. The `[git]` branch never moves to it; the commit reaches the branch as a parent of the revision that brings it onto main, and the ref keeps it until then.
const RECORD_REF: &str = "refs/fold/record";

/// The most paths a human-change event lists; the rest are counted.
const LISTED_PATHS: usize = 50;

pub use crate::services::workspaces::TouchedTask;

/// What a record did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOutcome {
    /// The project folder holds no change that is not recorded.
    UpToDate { revision: RevisionId },
    /// The human change merged cleanly and main moved to `revision`; the folder follows main.
    Recorded {
        commit: CommitId,
        revision: RevisionId,
        /// The paths the human changed, in path order.
        files: Vec<String>,
        touched: Vec<TouchedTask>,
        /// The files of the folder that fold leaves out, as an import does.
        skipped: Vec<Skipped>,
    },
    /// The merge conflicted, so main did not move: the main agent prepares the merge in its main workspace with the human. `created` is false when the change waited already and nothing new was written.
    Conflicted {
        commit: CommitId,
        head: RevisionId,
        /// In path order.
        conflicted: Vec<PulledConflict>,
        touched: Vec<TouchedTask>,
        skipped: Vec<Skipped>,
        created: bool,
    },
}

/// Records the human's uncommitted changes in the project folder on main, for the named human who runs `fld record`. Nothing else records them, so an unfinished edit never reaches a worker. A Git commit the human made on the `[git]` branch is not a change of the folder: `fld pull` brings it in, and a record is refused until then, so one record never mixes the two.
///
/// The project's remote lock and Git sync lock are held to the end, so the commit writer, a push and a pull never run at the same time, and main's missing commits are written first. The folder is read as an import reads it: its ignore rules and the size limit apply, and `.fold` and `.git` never count. When it differs from the commit it is based on, its tree is written as one commit on top of that commit, under [`RECORD_REF`]; the branch, the index and the files stay as they are. The commit is then brought in as `fld pull` brings a remote commit, against the newest commit it shares with main: a clean merge is an integration of kind `pull` with no review, which records the human change and `human_change.recorded` in its transaction, and the folder then follows main; a conflict writes the human change, its conflicted paths and `human_change.conflicted` under the integration mutex and leaves main as it is, for the main agent to resolve with the human. A stop before the integration commits leaves main and the folder as they were, so the next record finds the same changes.
pub async fn record(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
) -> Result<RecordOutcome, ProjectError> {
    policy::can_record(actor).map_err(FoldError::from)?;
    let (handle, actor, git) = (Arc::clone(handle), actor.clone(), git.clone());
    tokio::spawn(async move { run(&handle, &actor, &git).await })
        .await
        .unwrap_or_else(|error| {
            Err(FoldError::new(
                codes::INTERNAL_ERROR,
                format!("the record stopped: {error}"),
            )
            .into())
        })
}

async fn run(
    handle: &Arc<ProjectHandle>,
    actor: &Actor,
    git: &Git,
) -> Result<RecordOutcome, ProjectError> {
    let Actor::Human { name: human } = actor else {
        return Err(FoldError::new(
            codes::HUMAN_REQUIRED,
            "only a human records the project folder",
        )
        .into());
    };
    let config = load_config(&handle.paths).await?;
    let branch = config.git.branch.clone();
    let remote_guard = handle.remote_lock.clone().lock_owned().await;
    let fold_dir = handle.paths.fold_dir();
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
        &branch,
        held,
        map,
        handle.clock.now(),
    )
    .await?;
    handle.bus.publish(std::mem::take(&mut committed.events));
    if committed.outside.is_some() {
        return Err(outside_commits(&branch).into());
    }
    let snapshot =
        snapshot_folder(handle, git, &branch, &map, human, config.max_file_bytes).await?;
    let skipped = snapshot.skipped;
    #[cfg(feature = "failpoints")]
    fail::fail_point!("record_after_commit", |_| Err(
        fired("record_after_commit").into()
    ));
    let head = workspaces::head(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("refs.main"))?;
    let Some((tip, human_tree)) = snapshot.commit else {
        // The folder holds what was recorded already: a change that waits for the main agent, or one that main holds and the folder did not follow yet.
        if let Some(conflicted) = waiting_change(&handle.store, &snapshot.base).await? {
            return Ok(RecordOutcome::Conflicted {
                commit: snapshot.base,
                head: head.number,
                conflicted,
                touched: Vec::new(),
                skipped,
                created: false,
            });
        }
        drop((held, map));
        follow(handle, git).await;
        return Ok(RecordOutcome::UpToDate {
            revision: head.number,
        });
    };
    let mirror = handle.paths.git_dir();
    let check = {
        let (git, mirror, tip, last) = (
            git.clone(),
            mirror.clone(),
            tip.clone(),
            map.last().cloned(),
        );
        blocking(move || sync_base(&git, &mirror, &tip, last.as_ref())).await?
    };
    let base = match check {
        SyncCheck::Current => {
            return Ok(RecordOutcome::UpToDate {
                revision: head.number,
            });
        }
        SyncCheck::Base(base) => base,
    };
    let first = RevisionId::from_number(1).ok_or(StoreError::Corrupt("revision number 1"))?;
    let first_tree = workspaces::get_by_number(handle.store.read(), first)
        .await?
        .tree_hash;
    let base_tree = {
        // Shared while the trees are written or refreshed, so a collection never deletes an object before the row that names it commits.
        let objects_guard = handle.objects.clone().read_owned().await;
        let (git, fold_dir, mirror) = (git.clone(), fold_dir.clone(), mirror);
        let base_commit = base.as_ref().map(|base| base.commit.clone());
        let opts = GitImportOptions {
            max_file_bytes: config.max_file_bytes,
        };
        blocking(move || {
            let _objects = objects_guard;
            let objects = ObjectStore::open_existing(&fold_dir)?;
            Ok(match base_commit {
                Some(commit) => {
                    import_commit(&objects, &git, &mirror, commit.as_str(), &opts)?.tree
                }
                None => first_tree,
            })
        })
        .await?
    };
    let workspace_service = WorkspaceService::for_project(handle);
    let reach = workspace_service
        .human_change_reach(base_tree, human_tree)
        .await
        .map_err(from_workspace)?;
    let reviews = ReviewService::new(
        handle.store.clone(),
        handle.bus.clone(),
        handle.clock.clone(),
        workspace_service,
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
                    &human_tree,
                    &head_tree,
                )
            })
            .await?
        };
        if !merge.conflicts.is_empty() {
            let rows: Vec<ConflictRow> = merge
                .conflicts
                .iter()
                .map(|conflict| ConflictRow {
                    path: conflict.path.clone(),
                    kind: conflict.kind,
                })
                .collect();
            let change = NewHumanChange {
                commit: tip.as_str(),
                human: human.as_str(),
                base_tree,
                tree: human_tree,
                now: handle.clock.now(),
            };
            let mut data = reach.event_data(tip.as_str());
            let conflicted: Vec<String> = rows
                .iter()
                .map(|row| String::from_utf8_lossy(&row.path).into_owned())
                .collect();
            data["conflicted"] = json!(conflicted.iter().take(LISTED_PATHS).collect::<Vec<_>>());
            data["conflicted_count"] = json!(conflicted.len());
            data["head"] = json!(head.number.to_string());
            let event = NewEvent::new(types::HUMAN_CHANGE_CONFLICTED, actor.event_actor())
                .and_then(|event| event.revision(head.number).data(data))
                .map_err(|error| FoldError::new(codes::INTERNAL_ERROR, error.to_string()))?;
            let now = change.now;
            let buffer = handle
                .store
                .write(async |tx| -> Result<EventBuffer, ProjectError> {
                    let mut buffer = EventBuffer::new();
                    workspaces::supersede_conflicted(tx, now).await?;
                    workspaces::insert_conflicted_human_change(tx, &change, &rows).await?;
                    buffer.append(tx, &event, now).await?;
                    Ok(buffer)
                })
                .await?;
            drop(serial);
            handle.bus.publish(buffer);
            tracing::info!(commit = %tip, head = %head.number, conflicted = rows.len(), "the human change conflicts with main; main did not move");
            return Ok(RecordOutcome::Conflicted {
                commit: tip,
                head: head.number,
                conflicted: rows.iter().map(pulled_conflict).collect(),
                touched: reach.touched,
                skipped,
                created: true,
            });
        }
        drop(serial);
        let request = PullIntegration {
            remote_commit: tip.to_string(),
            remote: own_repository(&handle.paths),
            branch: branch.clone(),
            base_tree,
            remote_tree: human_tree,
            human: Some(HumanRecord {
                human: human.clone(),
                data: reach.event_data(tip.as_str()),
            }),
        };
        let revision = match reviews
            .integrate_pull(actor, request)
            .await
            .map_err(project_error)?
        {
            review::PullOutcome::Succeeded { revision } => revision,
            // Main moved after the merge above and the change now conflicts with it: merge again under the mutex.
            review::PullOutcome::Conflicted(_) => continue,
        };
        match base.as_ref().and_then(|base| base.fast_forward) {
            Some(head) => {
                map_fast_forward(handle, held, map, head, revision, &tip, human_tree).await
            }
            None => drop((held, map)),
        }
        // The folder follows main now rather than at the Git job's next run, so it is clean when the command returns.
        follow(handle, git).await;
        tracing::info!(commit = %tip, revision = %revision, files = reach.files.len(), "the human change is recorded");
        return Ok(RecordOutcome::Recorded {
            commit: tip,
            revision,
            files: reach.files,
            touched: reach.touched,
            skipped,
        });
    }
    Err(FoldError::new(
        codes::HEAD_MOVED,
        format!("main moved during each of {ATTEMPTS} record attempts"),
    )
    .with_hint("run fld record again")
    .into())
}

struct Snapshot {
    /// The human's new commit with its tree; `None` when the folder holds what `base` holds.
    commit: Option<(CommitId, ObjectHash)>,
    /// The commit the folder's changes are based on.
    base: CommitId,
    skipped: Vec<Skipped>,
}

/// Reads the project folder as an import does and, when it differs from the commit its changes are based on, writes it as one commit of `human` on top of that commit under [`RECORD_REF`]. No file, ref of the branch or index entry of the folder is written. The folder must have the branch checked out, so its files are the branch's files.
///
/// The base is the branch tip, except while the folder still holds the newest human change because it did not follow main yet: the branch tip is then an ancestor of that change's commit, and the new commit continues it, so the next merge does not see the recorded change a second time.
async fn snapshot_folder(
    handle: &ProjectHandle,
    git: &Git,
    branch: &str,
    map: &SyncMap,
    human: &HumanName,
    max_file_bytes: u64,
) -> Result<Snapshot, ProjectError> {
    let paths = handle.paths.clone();
    let repo = {
        let (paths, git, branch) = (paths.clone(), git.clone(), branch.to_owned());
        blocking(move || {
            let repo = project_repository(&paths)?;
            let checked_out = checked_out_branch(&git, &repo)?;
            if checked_out.as_deref() != Some(branch.as_str()) {
                return Err(other_branch(&branch, checked_out.as_deref()));
            }
            Ok(repo)
        })
        .await?
    };
    let newest = {
        let mut conn = handle
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        workspaces::newest_human_change(&mut conn, false).await?
    };
    let (tip, newest) = {
        let (git, repo, branch) = (git.clone(), repo.clone(), branch.to_owned());
        blocking(move || {
            let tip = git.ref_commit(&repo, &branch_ref(&branch))?;
            let newest = match (newest, &tip) {
                (Some((commit, tree)), Some(tip)) => {
                    let commit: CommitId = commit.parse()?;
                    (commit != *tip && is_ancestor(&git, &repo, tip, &commit)?)
                        .then_some((commit, tree))
                }
                _ => None,
            };
            Ok((tip, newest))
        })
        .await?
    };
    let tip = tip.ok_or_else(|| {
        FoldError::new(
            codes::INVALID_REQUEST,
            format!("the project's Git branch {branch} has no commit yet"),
        )
        .with_hint("wait until fold has written the commit of r1, then run fld record again")
    })?;
    let (base, base_tree) = match newest {
        Some((commit, tree)) => (commit, Some(tree)),
        None => {
            // The tip is mostly a commit fold wrote, whose revision gives its tree without reading Git.
            let mapped = map
                .entries()
                .iter()
                .find(|(_, commit)| *commit == tip)
                .map(|(revision, _)| *revision);
            let tree = match mapped {
                Some(revision) => Some(
                    workspaces::get_by_number(handle.store.read(), revision)
                        .await?
                        .tree_hash,
                ),
                None => None,
            };
            (tip, tree)
        }
    };
    let objects_guard = handle.objects.clone().read_owned().await;
    let (git, fold_dir) = (git.clone(), paths.fold_dir());
    let (now, author) = (handle.clock.now(), human_signature(human));
    let message = format!("Changes of {} in the project folder\n", human.as_str());
    blocking(move || {
        let _objects = objects_guard;
        let objects = ObjectStore::open_existing(&fold_dir)?;
        let base_tree = match base_tree {
            Some(tree) => tree,
            None => {
                import_commit(
                    &objects,
                    &git,
                    &repo,
                    base.as_str(),
                    &GitImportOptions { max_file_bytes },
                )?
                .tree
            }
        };
        let root = paths.root();
        let caps = Capabilities::probe(&fold_dir).map_err(|error| {
            FoldError::new(
                codes::WORKSPACE_IO,
                format!("cannot probe the file system: {}", error.kind()),
            )
        })?;
        let imported = import_dir(
            &objects,
            root,
            &IgnoreRules::new(root),
            &ImportOptions {
                max_file_bytes,
                caps,
            },
        )?;
        if imported.tree == base_tree {
            return Ok(Snapshot {
                commit: None,
                base,
                skipped: imported.skipped,
            });
        }
        // fast-import moves a ref only forward, so the record ref starts on the base.
        set_ref(&git, &repo, RECORD_REF, Some(&base))?;
        let commit = write_commit(
            &objects,
            &git,
            &ExportTarget {
                repo: &repo,
                refname: RECORD_REF,
            },
            &ExportBase {
                commit: base.clone(),
                tree: base_tree,
            },
            &ExportCommit {
                tree: imported.tree,
                created_at: now,
                message,
                author,
                committer: fold_signature(),
            },
            &fold_dir,
        )?;
        Ok(Snapshot {
            commit: Some((commit, imported.tree)),
            base,
            skipped: imported.skipped,
        })
    })
    .await
}

/// Brings the project folder to main, as the Git job does after an integration, and logs a failure by its code: the job tries again at its next run.
async fn follow(handle: &ProjectHandle, git: &Git) {
    if let Err(error) = commit_main(&handle.store, &handle.paths, git, handle.clock.as_ref()).await
    {
        tracing::warn!(
            code = error.log_code(),
            "the project folder did not follow main yet; the Git job tries again"
        );
    }
}

/// The branch has commits fold did not write. `fld pull` brings them in first, so one record never mixes them with the folder's uncommitted changes.
fn outside_commits(branch: &str) -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!("the project's Git branch {branch} has commits that fold did not write"),
    )
    .with_hint("run fld pull in the project folder to bring them in, then fld record")
    .with_details([
        ("reason", json!("outside_commits")),
        ("branch", json!(branch)),
    ])
}

/// The folder has another branch, or a detached `HEAD`, checked out, so its files are not the `[git]` branch's.
fn other_branch(branch: &str, checked_out: Option<&str>) -> FoldError {
    let found = checked_out.map_or_else(
        || "a detached HEAD".to_owned(),
        |other| format!("the branch {other}"),
    );
    FoldError::new(
        codes::INVALID_REQUEST,
        format!(
            "the project folder has {found} checked out, not the project's Git branch {branch}"
        ),
    )
    .with_hint(format!(
        "run git switch {branch} in the project folder, then fld record again"
    ))
    .with_details([("reason", json!("other_branch")), ("branch", json!(branch))])
}

/// The author of the commit `fld record` makes for a human.
fn human_signature(human: &HumanName) -> Signature {
    Signature {
        name: human.as_str().to_owned(),
        email: format!("human.{}@fold.local", human.as_str()),
    }
}

/// The conflicted paths of the human change of `tip`, when it waits for the main agent already.
async fn waiting_change(
    store: &Store,
    tip: &CommitId,
) -> Result<Option<Vec<PulledConflict>>, ProjectError> {
    let mut conn = store.read().acquire().await.map_err(StoreError::from)?;
    Ok(workspaces::conflicted_human_change(&mut conn)
        .await?
        .filter(|row| row.commit == tip.as_str())
        .map(|row| row.conflicts.iter().map(pulled_conflict).collect()))
}

fn pulled_conflict(row: &ConflictRow) -> PulledConflict {
    PulledConflict {
        path: String::from_utf8_lossy(&row.path).into_owned(),
        kind: row.kind,
    }
}

/// A failure of the workspace step a record runs keeps its code.
fn from_workspace(error: WorkspaceError) -> ProjectError {
    match error {
        WorkspaceError::Fold(error) => error.into(),
        WorkspaceError::Store(error) => error.into(),
        other => FoldError::new(codes::INTERNAL_ERROR, other.to_string()).into(),
    }
}
