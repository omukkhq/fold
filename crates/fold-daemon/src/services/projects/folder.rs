use std::sync::Arc;

use fold_core::tree::FlatTree;
use fold_core::{FoldError, ProjectPaths, RevisionId, Timestamp};
use fold_store::workspaces::{
    Checkout, checkout, genesis, git_sync, head, merged_commits, newest_human_change, set_checkout,
};
use fold_store::{Store, StoreError};
use fold_vcs::checkout::{FolderEntry, folder_changes, follow, overlay};
use fold_vcs::compare::load_flat;
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_remote::{BranchState, branch_state};
use fold_vcs::git_sync_map::SyncMap;
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::index::StatIndex;
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::Capabilities;
use fold_vcs::revision_store::write_initial_tree;
use fold_vcs::scan::{ScanOptions, scan};

use super::commits::{Committed, commit_main};
use super::remote::{RemoteContext, blocking};
use super::{ProjectAttention, ProjectError, load_config};
#[cfg(feature = "failpoints")]
use crate::services::fired;

/// What one follow of main did in the project folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderFollowed {
    /// The main revision the folder holds now, apart from the paths a human changed; `None` when the folder did not follow.
    pub at: Option<RevisionId>,
    /// The paths main changed that a human changed too, which fold left as they are; in path order.
    pub kept: Vec<String>,
    /// The folder has changes that no revision records: the kept paths, or any other path that differs from the checkout record.
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

/// Brings the project folder to main through its checkout record, as [`fold_vcs::checkout::follow`] describes: a path that still holds the entry fold last wrote gets main's, and a path a human changed stays theirs. The newest human change that main holds is laid over the record first, so the human's recorded files count as fold's. While Git sync is on the folder moves only together with its branch: when the branch shows a change fold did not make, or commits fold did not write, or a path main changed holds the human's own change, nothing is written, so `git status` keeps telling the human's changes from main's. The record tree goes into the object store, then the record row, so a stop between the writes and the row leaves files that the next follow finds holding main's entries and takes as written. The caller holds the project's folder lock.
pub(crate) async fn follow_folder(
    store: &Store,
    paths: &ProjectPaths,
    objects: &Arc<tokio::sync::RwLock<()>>,
    git: &Git,
    now: Timestamp,
) -> Result<FolderFollowed, ProjectError> {
    let config = load_config(paths).await?;
    let read = store.read();
    let Some(head) = head(read).await? else {
        return Ok(FolderFollowed::default());
    };
    let recorded = checkout(read).await?;
    let synced = git_sync(read).await?.is_some();
    let newest = {
        let mut conn = read.acquire().await.map_err(StoreError::from)?;
        newest_human_change(&mut conn, true).await?
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
    let may_write = if synced {
        matches!(branch, Some(BranchState::Fold { .. }))
    } else {
        true
    };
    let objects_guard = objects.clone().read_owned().await;
    let (paths_owned, max_file_bytes) = (paths.clone(), config.max_file_bytes);
    let head_tree = head.tree_hash;
    let lay = lay.map(|change| (change.base_tree, change.tree));
    let (tree, kept, unrecorded, wrote) = blocking(move || {
        let _objects = objects_guard;
        let paths = paths_owned;
        let fold_dir = paths.fold_dir();
        let objects = ObjectStore::open_existing(&fold_dir).map_err(FoldError::from)?;
        let mut record = load_flat(&objects, &record_tree)?;
        if let Some((base, tree)) = lay {
            record = overlay(
                &record,
                &load_flat(&objects, &base)?,
                &load_flat(&objects, &tree)?,
            );
        }
        let main = load_flat(&objects, &head_tree)?;
        let caps = Capabilities::probe(&fold_dir).map_err(|error| io_error(&error))?;
        let root = paths.root();
        let changes = folder_changes(root, &record, &main, max_file_bytes, &caps)?;
        let human = changes
            .iter()
            .any(|(path, found)| is_human(found, &record, &main, path.as_bytes()));
        let wrote = may_write && !(synced && human);
        let followed = if wrote {
            follow(&objects, root, &record, &main, &changes, caps)?
        } else {
            fold_vcs::checkout::Followed {
                kept: changes
                    .iter()
                    .filter(|(path, found)| is_human(found, &record, &main, path.as_bytes()))
                    .map(|(path, _)| path.clone())
                    .collect(),
                record,
                written: Vec::new(),
            }
        };
        #[cfg(feature = "failpoints")]
        fail::fail_point!("folder_after_write", |_| Err(fired("folder_after_write")));
        let tree = write_initial_tree(&objects, followed.record.clone())?;
        let unrecorded =
            !followed.kept.is_empty() || differs(&paths, &followed.record, max_file_bytes, caps)?;
        let kept: Vec<String> = followed.kept.iter().map(ToString::to_string).collect();
        Ok((tree, kept, unrecorded, wrote))
    })
    .await?;
    let at = (wrote && (!synced || kept.is_empty())).then_some(head.number);
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
    if !kept.is_empty() {
        tracing::info!(revision = %head.number, kept = kept.len(), "the project folder kept the paths a human changed");
    }
    Ok(FolderFollowed {
        at,
        kept,
        unrecorded,
        branch,
    })
}

/// Whether a file of the folder that the ignore rules keep differs from the record, read through the folder's stat index so an unchanged file is not hashed again. A path the record holds that the rules leave out does not count. Blocking.
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
    let unstable: Vec<&[u8]> = scanned
        .unstable
        .iter()
        .map(|path| path.as_bytes())
        .collect();
    for (path, entry) in record {
        let Ok(repo) = fold_vcs::paths::RepoPath::parse(path) else {
            continue;
        };
        if rules.is_ignored(&repo, false) {
            continue;
        }
        if scanned.entries.get(path) != Some(entry) && !unstable.contains(&path.as_slice()) {
            return Ok(true);
        }
    }
    Ok(scanned
        .entries
        .keys()
        .any(|path| !record.contains_key(path)))
}

/// Whether the folder holds neither the record's nor main's entry at `key`: a human changed it.
fn is_human(found: &FolderEntry, record: &FlatTree, main: &FlatTree, key: &[u8]) -> bool {
    !found.holds(record.get(key)) && !found.holds(main.get(key))
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
