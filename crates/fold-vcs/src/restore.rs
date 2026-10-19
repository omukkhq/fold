use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use fold_core::FoldError;
use fold_core::object::ObjectHash;
use fold_core::tree::{FlatEntry, FlatTree};

use crate::compare::load_flat;
use crate::index::StatIndex;
use crate::materialise::{Batch, MaterialiseOptions, Synced};
use crate::objects::{ObjectError, ObjectStore};
use crate::paths::{RepoPath, WorkspaceError, os_bytes, os_path};
use crate::scan::{ChangeKind, ScanOptions, changes, scan};
use crate::snapshot::{Snapshot, snapshot};

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
    bring_to_tree(objects, root, index, index_path, target_tree, opts, true)
}

/// Makes the tracked files of a directory that holds nobody's work equal `target_tree`, as [`restore_tree`] does but without its refusal of unrecorded content: fold's own check directory, where the checks of each integration ran, holds only what fold wrote and what the checks produced. Ignored files, such as installed dependencies, stay. A missing directory is created.
pub fn reset_tree(
    objects: &ObjectStore,
    root: &Path,
    index: &mut StatIndex,
    index_path: &Path,
    target_tree: &ObjectHash,
    opts: &RestoreOptions<'_>,
) -> Result<Restored, FoldError> {
    fs::create_dir_all(root).map_err(WorkspaceError::from)?;
    bring_to_tree(objects, root, index, index_path, target_tree, opts, false)
}

fn bring_to_tree(
    objects: &ObjectStore,
    root: &Path,
    index: &mut StatIndex,
    index_path: &Path,
    target_tree: &ObjectHash,
    opts: &RestoreOptions<'_>,
    keep_unrecorded: bool,
) -> Result<Restored, FoldError> {
    let target = load_flat(objects, target_tree)?;
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
    let lost = if keep_unrecorded {
        unrecorded(objects, root, &scanned.entries, &removals, &writes)?
    } else {
        Vec::new()
    };
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
        batch.remove_empty_dirs(&path)?;
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
            None => !occupied(root, path, &|found| removing.contains(found))?,
        };
        if !recorded {
            lost.push(path.clone());
        }
    }
    lost.sort();
    Ok(lost)
}

/// Whether something the removals do not clear stands at `path` or at a parent of it, so writing a path the recorded side does not have would replace or block it. Nothing there, or only a directory that holds nothing but removals and empty directories, is clear. Links are never followed. A sync and a restore both check a write with it.
pub(crate) fn occupied(
    root: &Path,
    path: &RepoPath,
    removing: &dyn Fn(&RepoPath) -> bool,
) -> Result<bool, WorkspaceError> {
    if let Some(settled) = above(root, path, removing)? {
        return Ok(settled);
    }
    let at = root.join(os_path(path));
    match fs::symlink_metadata(&at) {
        Ok(meta) if meta.is_dir() => Ok(!holds_only(&at, path, removing)),
        Ok(_) => Ok(!removing(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// [`occupied`] for the parents of `path` alone.
pub(crate) fn occupied_above(
    root: &Path,
    path: &RepoPath,
    removing: &dyn Fn(&RepoPath) -> bool,
) -> Result<bool, WorkspaceError> {
    Ok(above(root, path, removing)?.unwrap_or(false))
}

/// Walks the parents of `path` from the root. `Some(false)` when one is missing, so nothing can stand at `path`; `Some(true)` when one is something other than a directory that the removals do not clear; `None` when every parent is a real directory.
fn above(
    root: &Path,
    path: &RepoPath,
    removing: &dyn Fn(&RepoPath) -> bool,
) -> Result<Option<bool>, WorkspaceError> {
    let (parents, _) = path.parents_and_name();
    let mut prefix: Option<RepoPath> = None;
    for name in parents {
        let here = RepoPath::child(prefix.as_ref(), name)?;
        let meta = match fs::symlink_metadata(root.join(os_path(&here))) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Some(false)),
            Err(error) => return Err(error.into()),
        };
        if !meta.is_dir() {
            return Ok(Some(!removing(&here)));
        }
        prefix = Some(here);
    }
    Ok(None)
}

/// Whether every file below `dir` is one of the removals, so only empty directories stay there once they are gone. An unreadable directory or a reserved name holds something that stays.
fn holds_only(dir: &Path, prefix: &RepoPath, removing: &dyn Fn(&RepoPath) -> bool) -> bool {
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
            Ok(_) => removing(&path),
            Err(_) => false,
        };
        if !cleared {
            return false;
        }
    }
    true
}
