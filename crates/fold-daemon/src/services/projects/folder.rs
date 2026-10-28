use std::sync::Arc;

use fold_core::object::ObjectHash;
use fold_core::tree::FlatTree;
use fold_core::{FoldError, ProjectPaths, RevisionId, Timestamp};
use fold_store::workspaces::{
    Checkout, RevisionRow, StashState, checkout, genesis, git_sync, head, merged_commits,
    newest_human_change, open_unstash, set_checkout, stash_stack,
};
use fold_store::{Store, StoreError};
use fold_vcs::checkout::{FolderEntry, Followed, folder_changes, follow, overlay};
use fold_vcs::compare::load_flat;
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_remote::{BranchState, branch_state};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::index::StatIndex;
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::RepoPath;
use fold_vcs::platform::Capabilities;
use fold_vcs::revision_store::write_initial_tree;
use fold_vcs::scan::{ScanOptions, scan};
use fold_vcs::stash::changed_paths;

use fold_core::events::types;
use fold_core::{Actor, EventError, NewEvent};

use super::commits::{Committed, commit_main};
use super::remote::{RemoteContext, blocking};
use super::{ProjectAttention, ProjectError, load_config};
use crate::services::events::EventBuffer;
#[cfg(feature = "failpoints")]
use crate::services::fired;

/// The most held paths a `git.uncommitted_changes` event lists; the rest are counted.
const LISTED_PATHS: usize = 50;

/// What one follow of main did in the project folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderFollowed {
    /// The main revision the folder holds now, apart from the changes no revision records; `None` when the folder did not follow.
    pub at: Option<RevisionId>,
    /// The paths main changed that hold a change of the human's that no revision records, which keep the whole folder where it is until the human records the change or takes it back; in path order.
    pub held: Vec<String>,
    /// The folder has changes that no revision records: the held paths, or any other path that differs from the checkout record.
    pub unrecorded: bool,
    /// What the `[git]` branch shows, read while Git sync is on.
    pub branch: Option<BranchState>,
}

/// What one run of [`sync_folder`] did.
#[derive(Debug)]
pub struct FolderSync {
    pub followed: FolderFollowed,
    /// The commit writer's run, which writes nothing while Git sync is off.
    pub committed: Result<Committed, ProjectError>,
}

impl FolderSync {
    /// What the run found that needs a person.
    pub fn attention(&self) -> Vec<ProjectAttention> {
        let committed = self.committed.as_ref().ok();
        let mut found = Vec::new();
        // Commits of the human that wait for the main agent account for what the folder holds beyond the record, and Needs you lists them as a human change.
        let ahead = matches!(self.followed.branch, Some(BranchState::Ahead { .. }));
        let unrecorded = self.followed.unrecorded && !ahead;
        if unrecorded || committed.is_some_and(|committed| committed.uncommitted) {
            found.push(ProjectAttention::UncommittedChanges);
        }
        if self.moved().is_some() {
            found.push(ProjectAttention::HeadMoved);
        }
        found
    }

    /// The paths main changed that hold changes of the human's that no revision records and keep the folder where it is. While the branch is not fold's, none is named: the branch keeps the folder where it is.
    pub fn held(&self) -> &[String] {
        &self.followed.held
    }

    /// Whether the changes that no revision records are only in the Git index: a change the human staged on a path main changed, such as a staged deletion, holds the branch back while the folder holds nothing to record, so `git restore --staged` undoes it and `fld record` does not.
    pub fn staged_only(&self) -> bool {
        let ahead = matches!(self.followed.branch, Some(BranchState::Ahead { .. }));
        !(self.followed.unrecorded && !ahead)
            && self
                .committed
                .as_ref()
                .is_ok_and(|committed| committed.uncommitted)
    }

    /// The change of the branch or `HEAD` that fold did not make, when there is one.
    pub fn moved(&self) -> Option<&fold_vcs::git_remote::Moved> {
        match &self.followed.branch {
            Some(BranchState::Moved(moved)) => Some(moved),
            _ => self
                .committed
                .as_ref()
                .ok()
                .and_then(|committed| committed.moved.as_ref()),
        }
    }
}

/// The `git.uncommitted_changes` event: the folder has changes that no revision records, with the paths main changed that hold the folder back, empty when none does.
pub(crate) fn uncommitted_changes_event(
    branch: &str,
    staged: bool,
    held: &[String],
) -> Result<NewEvent, EventError> {
    NewEvent::new(types::GIT_UNCOMMITTED_CHANGES, Actor::System.event_actor()).and_then(|event| {
        event.data(serde_json::json!({
            "branch": branch,
            "staged": staged,
            "held": held.iter().take(LISTED_PATHS).collect::<Vec<_>>(),
            "held_count": held.len(),
        }))
    })
}

/// Notes what a run of [`sync_folder`] found in the project's Git attention and appends, each in its own transaction, an event for each change: `git.uncommitted_changes` with the branch and the paths main changed that hold the folder back, also when only those paths change while the attention stays, `git.head_moved` with the branch and the reason, or `git.attention_cleared` with the kind that ended. With `raise` false the run only ends what was raised: `fld record` reads the folder while it records it, so what it finds there before the record is no news. The held paths are noted only while the attention of changes that no revision records is raised.
pub(crate) async fn note_attention(context: &RemoteContext, synced: &FolderSync, raise: bool) {
    let attention = &context.git_attention;
    let found = synced.attention();
    let change = if raise {
        attention.set(&found)
    } else {
        let raised = attention.kinds();
        let ended: Vec<ProjectAttention> = found
            .into_iter()
            .filter(|kind| raised.contains(kind))
            .collect();
        attention.set(&ended)
    };
    let branch = load_config(&context.paths)
        .await
        .map(|config| config.git.branch)
        .unwrap_or_default();
    let raised = attention.kinds();
    if raise || !raised.contains(&ProjectAttention::HeadMoved) {
        attention.note_moved(
            synced.moved().map(|moved| moved.hint(&branch)),
            context.clock.now(),
        );
    }
    let uncommitted = raised.contains(&ProjectAttention::UncommittedChanges);
    let held = if uncommitted { synced.held() } else { &[] };
    // A raised or cleared attention appends its own event, which tells of the held paths too.
    let told = change
        .raised
        .contains(&ProjectAttention::UncommittedChanges)
        || change
            .cleared
            .contains(&ProjectAttention::UncommittedChanges);
    if attention.note_held(held, context.clock.now()) && !told {
        append_alone(
            context,
            uncommitted_changes_event(&branch, synced.staged_only(), held),
            "git.uncommitted_changes",
        )
        .await;
    }
    // A run that only ends attention leaves the held paths unnoted, so only a raising run counts as a look at the folder.
    if raise {
        attention.mark_observed();
    }
    let actor = Actor::System.event_actor();
    for kind in &change.raised {
        let event = match kind {
            ProjectAttention::UncommittedChanges => {
                let staged = synced.staged_only();
                if staged {
                    tracing::info!(project = %context.slug, "the Git index holds a staged change on a file main changed; git restore --staged undoes it");
                } else {
                    tracing::info!(project = %context.slug, held = held.len(), "the project folder has changes that no revision records; fld record records them");
                }
                uncommitted_changes_event(&branch, staged, held)
            }
            ProjectAttention::HeadMoved => {
                let reason = synced
                    .moved()
                    .map_or("branch_moved", |moved| moved.reason());
                tracing::warn!(project = %context.slug, reason, "the Git branch or HEAD changed in a way fold did not make; the project folder waits");
                NewEvent::new(types::GIT_HEAD_MOVED, actor.clone()).and_then(|event| {
                    event.data(serde_json::json!({
                        "branch": branch,
                        "reason": reason,
                        "hint": synced.moved().map(|moved| moved.hint(&branch)),
                    }))
                })
            }
        };
        append_alone(context, event, "git attention").await;
    }
    for kind in &change.cleared {
        let event = NewEvent::new(types::GIT_ATTENTION_CLEARED, actor.clone()).and_then(|event| {
            event.data(serde_json::json!({"branch": branch, "kind": kind.as_str()}))
        });
        append_alone(context, event, "git.attention_cleared").await;
    }
}

/// Appends a failure or attention event in its own transaction and publishes it.
pub(crate) async fn append_alone(
    context: &RemoteContext,
    event: Result<NewEvent, EventError>,
    name: &'static str,
) {
    let Ok(event) = event else {
        return;
    };
    let now = context.clock.now();
    let recorded = context
        .store
        .write(async |tx| -> Result<EventBuffer, StoreError> {
            let mut buffer = EventBuffer::new();
            buffer.append(tx, &event, now).await?;
            Ok(buffer)
        })
        .await;
    match recorded {
        Ok(buffer) => context.bus.publish(buffer),
        Err(error) => {
            tracing::warn!(project = %context.slug, event = name, code = error.log_code(), "an event was not recorded");
        }
    }
}

/// Brings the project folder to main and, while Git sync is on, writes the commits of the new revisions and moves the branch and the index to the revision the folder holds, under the project's folder lock. The Git job runs it after every integration, at the start and when the branch changes.
pub async fn sync_folder(context: &RemoteContext, git: &Git) -> Result<FolderSync, ProjectError> {
    let _folder = context.folder_lock.clone().lock_owned().await;
    let handle = context.handle.get().and_then(std::sync::Weak::upgrade);
    let followed = match &handle {
        // The commits of the human on the branch are recorded at once.
        Some(handle) => {
            super::record::follow_with_branch_commits(handle, git)
                .await?
                .0
        }
        None => {
            follow_folder(
                &context.store,
                &context.paths,
                &context.objects,
                git,
                context.clock.now(),
            )
            .await?
        }
    };
    drop(handle);
    let committed = commit_main(
        &context.store,
        &context.paths,
        git,
        followed.at,
        context.clock.as_ref(),
    )
    .await;
    Ok(FolderSync {
        followed,
        committed,
    })
}

/// What a follow of the project folder reads before it writes anything, which `fld status` reads too, so both decide from the same facts.
pub(crate) struct FolderRead {
    pub(crate) max_file_bytes: u64,
    /// The `[git]` branch.
    pub(crate) branch_name: String,
    pub(crate) head: RevisionRow,
    /// The checkout record as stored; `None` before fold first wrote one.
    pub(crate) recorded: Option<Checkout>,
    /// The record's tree, or the genesis tree before fold first wrote one.
    pub(crate) record_tree: ObjectHash,
    /// The main revision the folder holds, apart from the changes no revision records.
    pub(crate) from: RevisionId,
    /// The newest human change whose entries the record holds once `lay` is laid over it.
    pub(crate) human_change: Option<i64>,
    /// The base and tree of a human change that main holds and the record does not yet.
    pub(crate) lay: Option<(ObjectHash, ObjectHash)>,
    /// What the `[git]` branch shows, read while Git sync is on; `None` while it is off or could not be read.
    pub(crate) branch: Option<BranchState>,
    pub(crate) synced: bool,
    /// The conflicted paths of the stash entry whose unstash conflict waits in the folder.
    pub(crate) stash_held: Option<Vec<String>>,
    /// The folder's tree as an unstash that stopped part-way read it, and the tree it writes.
    pub(crate) unstash: Option<(ObjectHash, ObjectHash)>,
}

impl FolderRead {
    /// Whether a follow may write the folder: not while the conflict markers of an unstash wait, nor while an unstash that stopped part-way waits to be finished, and while Git sync is on only while the branch is fold's.
    pub(crate) fn may_write(&self) -> bool {
        self.stash_held.is_none()
            && self.unstash.is_none()
            && if self.synced {
                matches!(self.branch, Some(BranchState::Fold { .. }))
            } else {
                true
            }
    }
}

/// Reads what a follow of the project folder decides from: main's head, the checkout record and the newest human change main holds, the stash and the unstash journal, and, while Git sync is on, the `[git]` branch. Writes nothing. `None` while main has no head.
pub(crate) async fn read_folder(
    store: &Store,
    paths: &ProjectPaths,
    git: &Git,
) -> Result<Option<FolderRead>, ProjectError> {
    let config = load_config(paths).await?;
    let read = store.read();
    let Some(head) = head(read).await? else {
        return Ok(None);
    };
    let recorded = checkout(read).await?;
    let synced = git_sync(read).await?.is_some();
    let (newest, stash_held, unstash) = {
        let mut conn = read.acquire().await.map_err(StoreError::from)?;
        let unstash = open_unstash(&mut conn)
            .await?
            .map(|journal| (journal.folder_tree, journal.target_tree));
        let newest = newest_human_change(&mut conn, true).await?;
        let stash_held = stash_stack(&mut conn)
            .await?
            .into_iter()
            .find(|entry| entry.state == StashState::Conflicted)
            .map(|entry| {
                entry
                    .conflicts
                    .iter()
                    .map(|row| String::from_utf8_lossy(&row.path).into_owned())
                    .collect::<Vec<String>>()
            });
        (newest, stash_held, unstash)
    };
    let (record_tree, from, mut human_change) = match recorded.clone() {
        Some(found) => (found.tree, found.revision, found.human_change),
        None => {
            let genesis = genesis(read)
                .await?
                .ok_or(StoreError::Corrupt("revisions.genesis"))?;
            (genesis.tree_hash, genesis.number, None)
        }
    };
    let lay = newest.filter(|change| human_change.is_none_or(|known| change.id > known));
    if let Some(change) = &lay {
        human_change = Some(change.id);
    }
    let branch = if synced {
        let (git, paths, branch) = (git.clone(), paths.clone(), config.git.branch.clone());
        let merged = merged_commits(read).await?;
        let read = blocking(move || {
            let map = SyncMap::load(&paths.fold_dir())?;
            let known = |commit: &CommitId| {
                map.revision_of(commit).is_some() || merged.contains(commit.as_str())
            };
            branch_state(
                &git,
                &paths,
                &branch,
                &known,
                map.last().map(|(_, commit)| commit),
            )
        })
        .await;
        // A branch that cannot be read keeps the folder as it is; the commit writer reports the failure.
        match read {
            Ok(state) => Some(state),
            Err(error) => {
                tracing::warn!(
                    code = error.log_code(),
                    "the Git branch of the project folder could not be read"
                );
                None
            }
        }
    } else {
        None
    };
    Ok(Some(FolderRead {
        max_file_bytes: config.max_file_bytes,
        branch_name: config.git.branch,
        head,
        recorded,
        record_tree,
        from,
        human_change,
        lay: lay.map(|change| (change.base_tree, change.tree)),
        branch,
        synced,
        stash_held,
        unstash,
    }))
}

/// The checkout record with a human change laid over it, main's tree, and what the folder holds at each path where the two differ.
pub(crate) struct Compared {
    pub(crate) record: FlatTree,
    pub(crate) main: FlatTree,
    pub(crate) changes: Vec<(RepoPath, FolderEntry)>,
}

/// The checkout record with `lay` laid over it, main's tree, and what the folder at `root` holds at each path where the two differ. Blocking.
pub(crate) fn compare_folder(
    objects: &ObjectStore,
    root: &std::path::Path,
    record_tree: &ObjectHash,
    lay: Option<(ObjectHash, ObjectHash)>,
    head_tree: &ObjectHash,
    max_file_bytes: u64,
    caps: &Capabilities,
) -> Result<Compared, ProjectError> {
    let mut record = load_flat(objects, record_tree)?;
    if let Some((base, tree)) = lay {
        record = overlay(
            &record,
            &load_flat(objects, &base)?,
            &load_flat(objects, &tree)?,
        );
    }
    let main = load_flat(objects, head_tree)?;
    let changes =
        folder_changes(root, &record, &main, max_file_bytes, caps).map_err(FoldError::from)?;
    Ok(Compared {
        record,
        main,
        changes,
    })
}

/// Brings the project folder to main through its checkout record, as [`fold_vcs::checkout::follow`] describes, the same way with Git sync on or off: when a path main changed holds a change of the human's that no revision records, nothing is written and the folder stays on its revision until `fld record` records the change, a resolution of the human change brings it onto main, or the human takes it back; otherwise every path main changed gets main's entry. Paths main did not change are never touched. The newest human change that main holds is laid over the record first, so the human's recorded files count as fold's. While `fld unstash` left conflict markers that wait for the human, nothing is written either, and the held paths are the conflicted ones; so too while an unstash that stopped part-way waits to be finished, with the paths it writes. While Git sync is on the folder also moves only together with its branch: when the branch shows a change fold did not make, or commits fold did not write, nothing is written, so `git status` keeps telling the human's changes from main's. The record tree goes into the object store, then the record row, so a stop between the writes and the row leaves files that the next follow finds holding main's entries and takes as written. The caller holds the project's folder lock.
pub(crate) async fn follow_folder(
    store: &Store,
    paths: &ProjectPaths,
    objects: &Arc<tokio::sync::RwLock<()>>,
    git: &Git,
    now: Timestamp,
) -> Result<FolderFollowed, ProjectError> {
    let Some(read) = read_folder(store, paths, git).await? else {
        return Ok(FolderFollowed::default());
    };
    let may_write = read.may_write();
    let FolderRead {
        max_file_bytes,
        head,
        recorded,
        record_tree,
        from,
        human_change,
        lay,
        branch,
        stash_held,
        unstash,
        ..
    } = read;
    let objects_guard = objects.clone().read_owned().await;
    let paths_owned = paths.clone();
    let head_tree = head.tree_hash;
    let (tree, held, unrecorded, wrote) = blocking(move || {
        let _objects = objects_guard;
        let paths = paths_owned;
        let fold_dir = paths.fold_dir();
        let objects = ObjectStore::open_existing(&fold_dir).map_err(FoldError::from)?;
        let caps = Capabilities::probe(&fold_dir).map_err(|error| io_error(&error))?;
        let root = paths.root();
        let Compared {
            record,
            main,
            changes,
        } = compare_folder(
            &objects,
            root,
            &record_tree,
            lay,
            &head_tree,
            max_file_bytes,
            &caps,
        )?;
        let followed = if may_write {
            follow(&objects, root, &record, &main, &changes, caps)?
        } else {
            // The folder holds files of a branch fold did not write, which Git puts back when fold's branch returns, or the files of an unstash that wait for the human or for the unstash's end, so they settle no path of the record.
            Followed {
                record,
                ..Followed::default()
            }
        };
        let wrote = may_write && followed.held.is_empty();
        #[cfg(feature = "failpoints")]
        fail::fail_point!("folder_after_write", |_| Err(fired("folder_after_write")));
        let tree = write_initial_tree(&objects, followed.record.clone())?;
        let unrecorded =
            !followed.held.is_empty() || differs(&paths, &followed.record, max_file_bytes, caps)?;
        let held: Vec<String> = match (stash_held, unstash) {
            (Some(held), _) => held,
            (None, Some((from, to))) => {
                changed_paths(&load_flat(&objects, &from)?, &load_flat(&objects, &to)?)
                    .iter()
                    .map(|path| String::from_utf8_lossy(path).into_owned())
                    .collect()
            }
            (None, None) => followed.held.iter().map(ToString::to_string).collect(),
        };
        let unrecorded = unrecorded || !held.is_empty();
        Ok((tree, held, unrecorded, wrote))
    })
    .await?;
    let at = wrote.then_some(head.number);
    let row = Checkout {
        tree,
        revision: at.unwrap_or(from),
        human_change,
    };
    if recorded.as_ref() != Some(&row) {
        store
            .write(async |tx| set_checkout(tx, &row, now).await)
            .await?;
    }
    if !held.is_empty() {
        tracing::info!(revision = %head.number, held = held.len(), "the project folder waits: paths main changed hold changes that are not recorded");
    }
    Ok(FolderFollowed {
        at,
        held,
        unrecorded,
        branch,
    })
}

/// Whether the folder differs from the record as `fld record` reads it, through the folder's stat index so an unchanged file is not hashed again: the record's paths count although an ignore rule matches them, and a file the scan cannot read, such as one over the size limit, keeps its recorded entry. Blocking.
fn differs(
    paths: &ProjectPaths,
    record: &FlatTree,
    max_file_bytes: u64,
    caps: Capabilities,
) -> Result<bool, ProjectError> {
    let root = paths.root();
    let rules = IgnoreRules::new(root);
    let index_path = paths.folder_index();
    let mut index = StatIndex::load(&index_path, caps);
    let scanned = scan(
        root,
        &mut index,
        &ScanOptions {
            rules: &rules,
            max_file_bytes,
            caps,
            recorded: Some(record),
        },
    )
    .map_err(FoldError::from)?;
    // The index only spares work; a failed save makes the next scan read the files again.
    let _ = index.save(&index_path);
    Ok(scanned.settled(record) != *record)
}

fn io_error(error: &std::io::Error) -> ProjectError {
    FoldError::new(
        fold_core::codes::WORKSPACE_IO,
        format!("cannot probe the file system: {}", error.kind()),
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sync(unrecorded: bool, uncommitted: bool) -> FolderSync {
        FolderSync {
            followed: FolderFollowed {
                unrecorded,
                ..FolderFollowed::default()
            },
            committed: Ok(Committed {
                uncommitted,
                ..Committed::default()
            }),
        }
    }

    #[test]
    fn a_change_only_in_the_git_index_is_told_apart_from_one_to_record() {
        assert!(sync(false, true).staged_only());
        assert_eq!(
            sync(false, true).attention(),
            [ProjectAttention::UncommittedChanges]
        );
        assert!(!sync(true, true).staged_only());
        assert!(!sync(true, false).staged_only());
        assert!(!sync(false, false).staged_only());
    }
}
