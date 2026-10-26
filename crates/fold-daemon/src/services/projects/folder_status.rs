use fold_core::folder::{
    Blocker, FolderFacts, FolderPlace, UnrecordedKind, blockers, place, unrecorded_changes,
};
use fold_core::tree::EntryKind;
use fold_core::{FoldError, RevisionId, Timestamp};
use fold_store::StoreError;
use fold_store::workspaces::{StashState, conflicted_human_change, open_unstash, stash_stack};
use fold_vcs::checkout::hold;
use fold_vcs::compare::load_flat;
use fold_vcs::git::Git;
use fold_vcs::git_remote::BranchState;
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::index::StatIndex;
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::RepoPath;
use fold_vcs::platform::Capabilities;
use fold_vcs::scan::{ScanOptions, Skipped, scan};
use fold_vcs::stash::changed_paths;

use super::ProjectError;
use super::folder::{compare_folder, read_folder};
use super::remote::blocking;
use crate::projects::ProjectHandle;

/// What `fld status` shows of the project folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderStatus {
    /// Main's head when the folder was read.
    pub head: RevisionId,
    /// The main revision the folder holds, apart from the changes no revision records.
    pub folder: RevisionId,
    pub place: FolderPlace,
    /// The `[git]` branch while Git sync is on.
    pub branch: Option<String>,
    /// What `fld record` would record now, in path order.
    pub changes: Vec<FolderChange>,
    /// The files the read leaves out, as a record leaves them out.
    pub skipped: Vec<FolderSkipped>,
    /// The files the record tracks that an ignore rule matches, in path order: they stay tracked until `fld untrack` removes them from main.
    pub ignored_tracked: Vec<String>,
    pub blockers: Vec<Blocker>,
    /// Newest first.
    pub stash: Vec<FolderStashEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderChange {
    pub path: String,
    pub kind: UnrecordedKind,
    pub entry: EntryKind,
    /// Main changed the path too since the folder's revision, so the change holds the folder where it is.
    pub held: bool,
}

/// A file the read of the folder leaves out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderSkipped {
    pub skipped: Skipped,
    /// The record tracks the path, so main keeps the version it recorded.
    pub tracked: bool,
}

/// The state of a stash entry as `fld status` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StashEntryState {
    Stashed,
    /// Its unstash wrote conflict markers that wait in the folder.
    Conflicted,
    /// Its unstash stopped part-way and waits to be finished.
    Unstashing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderStashEntry {
    /// 0 is the newest, `stash@{0}`.
    pub position: usize,
    pub state: StashEntryState,
    pub created_at: Timestamp,
    /// The main revision the folder held when the entry was made.
    pub base: RevisionId,
    /// The event actor label of the human who made it.
    pub actor: String,
    pub files: usize,
    /// `fld unstash` applies this entry, or finishes its unstash, when it runs next.
    pub next: bool,
}

/// Reads the project folder against main as `fld record` and the folder's follow see it, and writes nothing: no record, no follow, no stash change, no object, not even the folder's stat index, which the read uses only to skip hashing files whose metadata it knows.
///
/// The project's folder lock is held from the first read of the rows to the end of the folder scan. Every write of the folder's files, of the checkout record, of the stash and of the unstash journal happens under that lock, so the scan sees the files that the record it compares with describes, never a follow half-way. The integration mutex is not taken: main's head moves only by compare-and-swap after its objects are stored, so the one read of the head is a whole revision, and an integration that lands during the read makes the answer one revision old, never inconsistent. The objects read lock is held while trees are loaded, so a collection cannot remove one meanwhile.
pub async fn folder_status(
    handle: &ProjectHandle,
    git: &Git,
) -> Result<FolderStatus, ProjectError> {
    let _folder = handle.folder_lock.clone().lock_owned().await;
    let read = read_folder(&handle.store, &handle.paths, git)
        .await?
        .ok_or(StoreError::Corrupt("refs.main"))?;
    let (waiting, stack, journal) = {
        let mut conn = handle
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        (
            conflicted_human_change(&mut conn).await?,
            stash_stack(&mut conn).await?,
            open_unstash(&mut conn).await?,
        )
    };
    let objects_guard = handle.objects.clone().read_owned().await;
    let paths = handle.paths.clone();
    let (record_tree, lay, head_tree, max_file_bytes) = (
        read.record_tree,
        read.lay,
        read.head.tree_hash,
        read.max_file_bytes,
    );
    let entries: Vec<_> = stack
        .iter()
        .map(|entry| (entry.base_tree, entry.tree))
        .collect();
    let unstash = read.unstash;
    let may_write = read.may_write();
    let scanned = blocking(move || {
        let _objects = objects_guard;
        let fold_dir = paths.fold_dir();
        let objects = ObjectStore::open_existing(&fold_dir).map_err(FoldError::from)?;
        let caps = Capabilities::probe(&fold_dir).map_err(|error| {
            FoldError::new(
                fold_core::codes::WORKSPACE_IO,
                format!("cannot probe the file system: {}", error.kind()),
            )
        })?;
        let root = paths.root();
        let compared = compare_folder(
            &objects,
            root,
            &record_tree,
            lay,
            &head_tree,
            max_file_bytes,
            &caps,
        )?;
        let mut settled = hold(&compared.record, &compared.main, &compared.changes);
        // A follow that may not write keeps the record as it is, also where the folder holds main's entry already, and `fld record` compares with that record.
        if !may_write {
            settled.record = compared.record;
        }
        let rules = IgnoreRules::new(root);
        let mut index = StatIndex::load(&paths.folder_index(), caps);
        let found = scan(
            root,
            &mut index,
            &ScanOptions {
                rules: &rules,
                max_file_bytes,
                caps,
                recorded: Some(&settled.record),
            },
        )
        .map_err(FoldError::from)?;
        let folder = found.settled(&settled.record);
        let changes = unrecorded_changes(&settled.record, &folder);
        let ignored_tracked: Vec<String> = settled
            .record
            .keys()
            .filter_map(|path| RepoPath::parse(path).ok())
            .filter(|path| rules.is_ignored(path, false))
            .map(|path| path.to_string())
            .collect();
        let skipped: Vec<FolderSkipped> = found
            .skipped
            .into_iter()
            .map(|skipped| FolderSkipped {
                tracked: skipped.path.at_or_below(&settled.record).next().is_some(),
                skipped,
            })
            .collect();
        let files: Vec<usize> = entries
            .iter()
            .map(|(base, tree)| {
                Ok(changed_paths(&load_flat(&objects, base)?, &load_flat(&objects, tree)?).len())
            })
            .collect::<Result<_, FoldError>>()?;
        let unstash_paths = match unstash {
            Some((from, to)) => Some(
                changed_paths(&load_flat(&objects, &from)?, &load_flat(&objects, &to)?)
                    .iter()
                    .map(|path| String::from_utf8_lossy(path).into_owned())
                    .collect::<Vec<String>>(),
            ),
            None => None,
        };
        let held: Vec<String> = settled.held.iter().map(ToString::to_string).collect();
        Ok((
            changes,
            skipped,
            ignored_tracked,
            held,
            files,
            unstash_paths,
        ))
    })
    .await?;
    let (changes, skipped, ignored_tracked, held, files, unstash_paths) = scanned;
    let facts = FolderFacts {
        held: held.clone(),
        human_change: waiting.map(|change| {
            change
                .conflicts
                .iter()
                .map(|row| String::from_utf8_lossy(&row.path).into_owned())
                .collect()
        }),
        unstash_conflicted: read.stash_held.clone(),
        unstash_open: unstash_paths,
        head_moved: match &read.branch {
            Some(BranchState::Moved(moved)) => Some(moved.hint(&read.branch_name)),
            _ => None,
        },
        commits_waiting: matches!(read.branch, Some(BranchState::Ahead { .. })),
    };
    let blockers = blockers(&facts);
    let held: std::collections::BTreeSet<String> = held.into_iter().collect();
    let journal_stash = journal.map(|journal| journal.stash_id);
    let stash = stack
        .into_iter()
        .zip(files)
        .enumerate()
        .map(|(position, (entry, files))| {
            let state = if journal_stash == Some(entry.id) {
                StashEntryState::Unstashing
            } else if entry.state == StashState::Conflicted {
                StashEntryState::Conflicted
            } else {
                StashEntryState::Stashed
            };
            FolderStashEntry {
                position,
                next: position == 0 && state != StashEntryState::Conflicted,
                state,
                created_at: entry.created_at,
                base: entry.revision,
                actor: entry.actor,
                files,
            }
        })
        .collect();
    Ok(FolderStatus {
        head: read.head.number,
        folder: read.from,
        place: place(&read.head.number, &read.from, &blockers),
        branch: read.synced.then_some(read.branch_name),
        changes: changes
            .into_iter()
            .map(|change| {
                let path = String::from_utf8_lossy(&change.path).into_owned();
                FolderChange {
                    held: held.contains(&path),
                    path,
                    kind: change.kind,
                    entry: change.entry,
                }
            })
            .collect(),
        skipped,
        ignored_tracked,
        blockers,
        stash,
    })
}
