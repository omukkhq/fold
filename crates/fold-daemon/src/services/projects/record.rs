use std::sync::Arc;

use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::{Actor, FoldError, HumanName, NewEvent, RevisionId, codes, policy};
use fold_store::StoreError;
use fold_store::workspaces::{self, ConflictRow, NewHumanChange};
use fold_vcs::checkout::overlay;
use fold_vcs::compare::load_flat;
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_remote::{BranchState, Moved};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::import::{ImportOptions, import_dir};
use fold_vcs::integrate::tree_merge;
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::Capabilities;
use fold_vcs::scan::Skipped;
use serde_json::json;

use super::commits::commit_main;
use super::folder::follow_folder;
use super::pull::{PulledConflict, project_error};
use super::remote::blocking;
use super::{FolderFollowed, ProjectError, load_config};
use crate::projects::ProjectHandle;
use crate::services::events::EventBuffer;
#[cfg(feature = "failpoints")]
use crate::services::fired;
use crate::services::reviews::{self as review, HumanRecord, RecordIntegration, ReviewService};
use crate::services::workspaces::{WorkspaceError, WorkspaceService};

/// How many times a record starts again from reading main when main moved between its own merge and the integration's.
const ATTEMPTS: usize = 3;

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
        revision: RevisionId,
        /// The Git commit of `revision` while Git sync is on.
        commit: Option<CommitId>,
        /// The paths the human changed, in path order.
        files: Vec<String>,
        touched: Vec<TouchedTask>,
        /// The files of the folder that fold leaves out, as an import does.
        skipped: Vec<Skipped>,
    },
    /// The merge conflicted, so main did not move: the main agent prepares the merge in its main workspace with the human. `created` is false when the change waited already and nothing new was written.
    Conflicted {
        head: RevisionId,
        /// In path order.
        conflicted: Vec<PulledConflict>,
        touched: Vec<TouchedTask>,
        skipped: Vec<Skipped>,
        created: bool,
    },
}

/// Records the human's changes in the project folder on main, for the named human who runs `fld record`. Nothing else records them, so an unfinished edit never reaches a worker.
///
/// The project's remote lock and folder lock are held to the end, so the folder's follow, a push and a pull never run at the same time. The folder first follows main, so the checkout record is fresh, and while Git sync is on main's missing commits are written; a branch or `HEAD` that shows a change fold did not make, or commits fold did not write, is refused with the command that undoes it. The folder is then read as an import reads it: its ignore rules and the size limit apply, and `.fold` and `.git` never count. When it differs from the checkout record, the difference is the human change, based on the record, and merges onto main under the integration mutex: a clean merge is an integration of kind `record` with no review, which records the human change and `human_change.recorded` in its transaction, and the folder then follows main; a conflict writes the human change, its conflicted paths and `human_change.conflicted` under the integration mutex and leaves main as it is, for the main agent to resolve with the human. A stop before the integration commits leaves main and the folder as they were, so the next record finds the same changes.
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
        .with_hint("ask a human to run fld record")
        .into());
    };
    let config = load_config(&handle.paths).await?;
    let _remote = handle.remote_lock.clone().lock_owned().await;
    let _folder = handle.folder_lock.clone().lock_owned().await;
    let followed = sync(handle, git).await?;
    if let Some(BranchState::Moved(moved)) = &followed.branch {
        return Err(head_moved(&config.git.branch, moved).into());
    }
    if let Some(BranchState::Ahead { .. }) = &followed.branch {
        return Err(outside_commits(&config.git.branch).into());
    }
    let record_tree = workspaces::checkout(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("checkout"))?
        .tree;
    let (human_tree, skipped) = snapshot_folder(handle, config.max_file_bytes).await?;
    #[cfg(feature = "failpoints")]
    fail::fail_point!("record_after_snapshot", |_| Err(fired(
        "record_after_snapshot"
    )
    .into()));
    let head = workspaces::head(handle.store.read())
        .await?
        .ok_or(StoreError::Corrupt("refs.main"))?;
    if human_tree == record_tree {
        return Ok(RecordOutcome::UpToDate {
            revision: head.number,
        });
    }
    if let Some(conflicted) = waiting_change(handle, record_tree, human_tree).await? {
        return Ok(RecordOutcome::Conflicted {
            head: head.number,
            conflicted,
            touched: Vec::new(),
            skipped,
            created: false,
        });
    }
    let change = HumanFolderChange {
        human,
        base_tree: record_tree,
        tree: human_tree,
        commit: None,
    };
    let recorded = with_skipped(integrate_change(handle, actor, &change).await?, skipped);
    let RecordOutcome::Recorded {
        revision,
        files,
        touched,
        skipped,
        ..
    } = recorded
    else {
        return Ok(recorded);
    };
    // The folder follows main now rather than at the Git job's next run, so it holds main when the command returns.
    sync(handle, git).await?;
    let fold_dir = handle.paths.fold_dir();
    let commit = blocking(move || SyncMap::load(&fold_dir))
        .await
        .ok()
        .and_then(|map| map.commit_of(revision).cloned());
    tracing::info!(revision = %revision, files = files.len(), "the human change is recorded");
    Ok(RecordOutcome::Recorded {
        revision,
        commit,
        files,
        touched,
        skipped,
    })
}

/// The folder follows main, and while Git sync is on the commit writer writes main's missing commits and moves the branch to the revision the folder holds. The caller holds the folder lock.
async fn sync(handle: &ProjectHandle, git: &Git) -> Result<FolderFollowed, ProjectError> {
    let followed = follow_folder(
        &handle.store,
        &handle.paths,
        &handle.objects,
        git,
        handle.clock.now(),
    )
    .await?;
    let mut committed = commit_main(
        &handle.store,
        &handle.paths,
        git,
        followed.at,
        handle.clock.as_ref(),
    )
    .await?;
    handle.bus.publish(std::mem::take(&mut committed.events));
    Ok(followed)
}

/// A change of the project folder by `human`: what the folder holds against the checkout record, or a Git commit the human made, with its id.
pub(crate) struct HumanFolderChange<'a> {
    pub(crate) human: &'a HumanName,
    pub(crate) base_tree: ObjectHash,
    pub(crate) tree: ObjectHash,
    pub(crate) commit: Option<CommitId>,
}

/// Merges `change` onto main under the integration mutex, three times at most when main moves meanwhile: a clean merge goes through the `record` entry of the integration procedure, and a conflict is written as the waiting human change with `human_change.conflicted`.
pub(crate) async fn integrate_change(
    handle: &ProjectHandle,
    actor: &Actor,
    change: &HumanFolderChange<'_>,
) -> Result<RecordOutcome, ProjectError> {
    let fold_dir = handle.paths.fold_dir();
    let commit_text = change.commit.as_ref().map(CommitId::as_str);
    let workspace_service = WorkspaceService::for_project(handle);
    let reach = workspace_service
        .human_change_reach(change.base_tree, change.tree)
        .await
        .map_err(from_workspace)?;
    let reviews = ReviewService::new(
        handle.store.clone(),
        handle.bus.clone(),
        handle.clock.clone(),
        workspace_service,
        handle.integration.clone(),
    );
    let (base_tree, human_tree) = (change.base_tree, change.tree);
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
            let new = NewHumanChange {
                commit: commit_text,
                human: change.human.as_str(),
                base_tree,
                tree: human_tree,
                now: handle.clock.now(),
            };
            let mut data = reach.event_data(commit_text);
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
            let now = new.now;
            let buffer = handle
                .store
                .write(async |tx| -> Result<EventBuffer, ProjectError> {
                    let mut buffer = EventBuffer::new();
                    workspaces::supersede_conflicted(tx, now).await?;
                    workspaces::insert_conflicted_human_change(tx, &new, &rows).await?;
                    buffer.append(tx, &event, now).await?;
                    Ok(buffer)
                })
                .await?;
            drop(serial);
            handle.bus.publish(buffer);
            tracing::info!(head = %head.number, conflicted = rows.len(), "the human change conflicts with main; main did not move");
            return Ok(RecordOutcome::Conflicted {
                head: head.number,
                conflicted: rows.iter().map(pulled_conflict).collect(),
                touched: reach.touched,
                skipped: Vec::new(),
                created: true,
            });
        }
        drop(serial);
        let request = RecordIntegration {
            commit: change.commit.as_ref().map(ToString::to_string),
            base_tree,
            tree: human_tree,
            human: HumanRecord {
                human: change.human.clone(),
                data: reach.event_data(commit_text),
            },
        };
        let revision = match reviews
            .integrate_record(actor, request)
            .await
            .map_err(project_error)?
        {
            review::PullOutcome::Succeeded { revision } => revision,
            // Main moved after the merge above and the change now conflicts with it: merge again under the mutex.
            review::PullOutcome::Conflicted(_) => continue,
        };
        return Ok(RecordOutcome::Recorded {
            revision,
            commit: None,
            files: reach.files,
            touched: reach.touched,
            skipped: Vec::new(),
        });
    }
    Err(FoldError::new(
        codes::HEAD_MOVED,
        format!("main moved during each of {ATTEMPTS} record attempts"),
    )
    .with_hint("run fld record again")
    .into())
}

fn with_skipped(outcome: RecordOutcome, left: Vec<Skipped>) -> RecordOutcome {
    match outcome {
        RecordOutcome::Recorded {
            revision,
            commit,
            files,
            touched,
            ..
        } => RecordOutcome::Recorded {
            revision,
            commit,
            files,
            touched,
            skipped: left,
        },
        RecordOutcome::Conflicted {
            head,
            conflicted,
            touched,
            created,
            ..
        } => RecordOutcome::Conflicted {
            head,
            conflicted,
            touched,
            skipped: left,
            created,
        },
        up_to_date => up_to_date,
    }
}

/// Reads the project folder as an import does, its objects written under the project's object lock, and returns its tree with the files it leaves out.
async fn snapshot_folder(
    handle: &ProjectHandle,
    max_file_bytes: u64,
) -> Result<(ObjectHash, Vec<Skipped>), ProjectError> {
    let objects_guard = handle.objects.clone().read_owned().await;
    let paths = handle.paths.clone();
    blocking(move || {
        let _objects = objects_guard;
        let fold_dir = paths.fold_dir();
        let objects = ObjectStore::open_existing(&fold_dir)?;
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
        Ok((imported.tree, imported.skipped))
    })
    .await
}

/// The conflicted paths of the human change that waits for the main agent already, when the folder holds exactly its changes on top of the checkout record.
async fn waiting_change(
    handle: &ProjectHandle,
    record_tree: ObjectHash,
    human_tree: ObjectHash,
) -> Result<Option<Vec<PulledConflict>>, ProjectError> {
    let waiting = {
        let mut conn = handle
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        workspaces::conflicted_human_change(&mut conn).await?
    };
    let Some(waiting) = waiting else {
        return Ok(None);
    };
    let fold_dir = handle.paths.fold_dir();
    let (base, tree) = (waiting.base_tree, waiting.tree);
    let same = blocking(move || {
        let objects = ObjectStore::open_existing(&fold_dir)?;
        let laid = overlay(
            &load_flat(&objects, &record_tree)?,
            &load_flat(&objects, &base)?,
            &load_flat(&objects, &tree)?,
        );
        Ok(laid == load_flat(&objects, &human_tree)?)
    })
    .await?;
    Ok(same.then(|| waiting.conflicts.iter().map(pulled_conflict).collect()))
}

/// The branch has commits fold did not write. `fld pull` brings them in first, so one record never mixes them with the folder's changes.
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

/// The branch or `HEAD` shows a change fold did not make, so the folder's files are not the branch's that fold left.
fn head_moved(branch: &str, moved: &Moved) -> FoldError {
    let found = match moved {
        Moved::OtherBranch(Some(other)) => format!(
            "the project folder has the branch {other} checked out, not the project's Git branch {branch}"
        ),
        Moved::OtherBranch(None) => {
            format!("the project folder has a detached HEAD, not the project's Git branch {branch}")
        }
        Moved::Rewritten { .. } => {
            format!("the project's Git branch {branch} moved to a commit that is not one of fold's")
        }
        Moved::Stash => "the project folder has a stash that fold did not make".to_owned(),
    };
    FoldError::new(codes::INVALID_REQUEST, found)
        .with_hint(format!("{}, then fld record again", moved.hint(branch)))
        .with_details([
            ("reason", json!("head_moved")),
            ("moved", json!(moved.reason())),
            ("branch", json!(branch)),
        ])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moved_head_is_refused_with_the_command_that_undoes_it() {
        let commit: CommitId = "a".repeat(40).parse().unwrap();
        for (moved, hint) in [
            (
                Moved::OtherBranch(Some("dev".to_owned())),
                "git switch main",
            ),
            (Moved::OtherBranch(None), "git switch main"),
            (
                Moved::Rewritten {
                    tip: commit.clone(),
                    fold: commit.clone(),
                },
                "git reset --soft aaaaaaaaaaaa",
            ),
            (Moved::Stash, "git stash pop"),
        ] {
            let error = head_moved("main", &moved);
            assert_eq!(error.code(), codes::INVALID_REQUEST);
            assert!(error.hint().unwrap().contains(hint), "{moved:?}");
            assert_eq!(error.details().unwrap()["reason"], "head_moved");
        }
    }
}
