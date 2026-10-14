use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use fold_core::FoldError;
use fold_core::object::ObjectHash;
use fold_core::tree::{FlatEntry, FlatTree, flatten};

use crate::index::StatIndex;
use crate::materialise::{Batch, MaterialiseOptions, Synced};
use crate::objects::{ObjectError, ObjectStore};
use crate::paths::{RepoPath, WorkspaceError, os_bytes, os_path, resolve};
use crate::scan::{ChangeKind, ScanOptions, changes, scan};
use crate::snapshot::{Snapshot, load_tree, snapshot};

#[derive(Debug, Clone, Copy)]
pub struct RestoreOptions<'a> {
    /// How the current state of the workspace is read.
    pub scan: ScanOptions<'a>,
    /// How the target's files are written.
    pub write: MaterialiseOptions,
}

#[derive(Debug, Default)]
pub struct Restored {
    pub written: Vec<RepoPath>,
    pub removed: Vec<RepoPath>,
    /// Symlink entries written as regular files, so the caller logs one warning.
    pub degraded: Vec<RepoPath>,
    pub synced: Synced,
}

/// Records the workspace as the tree of a checkpoint: a [`snapshot`] against `base_tree`. Every object of the tree is in the object store when this returns, so the caller can insert the checkpoint row that names it.
pub fn record_checkpoint_tree(
    objects: &ObjectStore,
    root: &Path,
    index: &mut StatIndex,
    index_path: &Path,
    base_tree: Option<ObjectHash>,
    opts: &ScanOptions<'_>,
) -> Result<Snapshot, FoldError> {
    snapshot(objects, root, index, index_path, base_tree, opts)
}

/// Whether a snapshot holds anything the latest changeset does not, so proposal creation records a new changeset only when it does. A workspace with no changeset always differs.
pub fn differs_from_latest(latest_tree: Option<&ObjectHash>, snapshot_tree: &ObjectHash) -> bool {
    latest_tree != Some(snapshot_tree)
}

/// Makes the tracked files of the workspace equal `target_tree`. Every path whose content or kind differs is written and every tracked path the target lacks is removed; ignored files stay, and so do files the scan skipped (too large, special or unreadable) that the target does not name.
///
/// Nothing is touched unless every blob the target needs is in the object store and every path the restore would remove or replace holds content the object store already has. The caller takes a checkpoint of the current state first, so a path edited after that checkpoint, a file that kept changing during the scan, or an ignored or skipped file standing where the target puts a file refuses the restore with `unrecorded_changes` instead of being lost. Removals run before writes, so a symlink or file that stands where the target has a directory is gone before anything is written below it, and no write ever goes through a link. A directory left where the target has a file holds only empty directories by then, and is removed. The stat index is saved after the batch is flushed.
pub fn restore_tree(
    objects: &ObjectStore,
    root: &Path,
    index: &mut StatIndex,
    index_path: &Path,
    target_tree: &ObjectHash,
    opts: &RestoreOptions<'_>,
) -> Result<Restored, FoldError> {
    let target = flatten(target_tree, &|hash| load_tree(objects, hash))?;
    let scanned = scan(root, index, &opts.scan)?;

    let mut removals = Vec::new();
    let mut writes: Vec<(RepoPath, FlatEntry)> = Vec::new();
    for change in changes(&scanned.entries, &target) {
        let path = RepoPath::parse(&change.path)?;
        match change.kind {
            ChangeKind::Deleted => removals.push(path),
            ChangeKind::Added | ChangeKind::Modified => {
                if let Some(entry) = target.get(&change.path) {
                    writes.push((path, *entry));
                }
            }
        }
    }
    removals.extend(
        scanned
            .unstable
            .into_iter()
            .filter(|path| !target.contains_key(path.as_bytes())),
    );
    for (_, entry) in &writes {
        if !objects
            .contains(&entry.hash)
            .map_err(WorkspaceError::from)?
        {
            return Err(WorkspaceError::from(ObjectError::Missing(entry.hash)).into());
        }
    }
    let lost = unrecorded(objects, root, &scanned.entries, &removals, &writes)?;
    if !lost.is_empty() {
        let paths = lost.iter().map(ToString::to_string).collect();
        return Err(WorkspaceError::Unrecorded(paths).into());
    }

    let mut done = Restored::default();
    let mut batch = Batch::open(root)?;
    for path in removals {
        batch.remove_entry(&path)?;
        index.remove(&path);
        done.removed.push(path);
    }
    for (path, entry) in writes {
        let dest = resolve(root, &path)?;
        if fs::symlink_metadata(&dest).is_ok_and(|meta| meta.is_dir()) {
            remove_empty_dirs(&dest).map_err(WorkspaceError::from)?;
        }
        let written = batch.write_entry(objects, &path, &entry, &opts.write)?;
        if written.degraded {
            done.degraded.push(path.clone());
        }
        index.upsert(path.clone(), written.stat);
        done.written.push(path);
    }
    done.synced = batch.finish()?;
    index.save(index_path).map_err(WorkspaceError::from)?;
    Ok(done)
}

/// The paths the restore would remove or replace whose current content is in no object. A tracked path counts when its scanned hash is not stored, and a path the scan has no entry for (it kept changing) always counts. A path the target writes counts when something untracked stands there or at one of its parents, unless the removals clear it.
fn unrecorded(
    objects: &ObjectStore,
    root: &Path,
    current: &FlatTree,
    removals: &[RepoPath],
    writes: &[(RepoPath, FlatEntry)],
) -> Result<Vec<RepoPath>, WorkspaceError> {
    let removing: BTreeSet<&RepoPath> = removals.iter().collect();
    let mut lost = Vec::new();
    for path in removals.iter().chain(writes.iter().map(|(path, _)| path)) {
        let recorded = match current.get(path.as_bytes()) {
            Some(entry) => objects.contains(&entry.hash)?,
            None if removing.contains(path) => false,
            None => !occupied(root, path, &removing)?,
        };
        if !recorded {
            lost.push(path.clone());
        }
    }
    lost.sort();
    Ok(lost)
}

/// Whether something the removals do not clear stands at `path` or at a parent of it, so writing there would replace or block it. Links are never followed.
fn occupied(
    root: &Path,
    path: &RepoPath,
    removing: &BTreeSet<&RepoPath>,
) -> Result<bool, WorkspaceError> {
    let mut prefix: Option<RepoPath> = None;
    for name in path.components() {
        let here = RepoPath::child(prefix.as_ref(), name)?;
        let at = root.join(os_path(&here)?);
        let meta = match fs::symlink_metadata(&at) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if !meta.is_dir() {
            return Ok(!removing.contains(&here));
        }
        if here == *path {
            return Ok(!holds_only(&at, &here, removing));
        }
        prefix = Some(here);
    }
    Ok(false)
}

/// Whether every file below `dir` is one of the removals, so only empty directories stay there once they are gone. An unreadable directory or a reserved name holds something that stays.
fn holds_only(dir: &Path, prefix: &RepoPath, removing: &BTreeSet<&RepoPath>) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        let Some(path) =
            os_bytes(&entry.file_name()).and_then(|name| RepoPath::child(Some(prefix), &name).ok())
        else {
            return false;
        };
        let cleared = match entry.file_type() {
            Ok(kind) if kind.is_dir() => holds_only(&entry.path(), &path, removing),
            Ok(_) => removing.contains(&path),
            Err(_) => false,
        };
        if !cleared {
            return false;
        }
    }
    true
}

/// Removes `dir` and the empty directories below it, never following a link. Anything else below makes the removal fail.
pub(crate) fn remove_empty_dirs(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_empty_dirs(&entry.path())?;
        }
    }
    fs::remove_dir(dir)
}
