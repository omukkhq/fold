use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use fold_core::object::ObjectHash;
use fold_core::tree::{FlatEntry, FlatTree};
use fold_core::{FoldError, RevisionId};

use crate::compare::load_flat;
use crate::materialise::{Batch, MaterialiseOptions, Synced};
use crate::objects::{ObjectError, ObjectStore};
use crate::paths::{RepoPath, WorkspaceError, resolve};
use crate::platform::{Capabilities, DirHandle, sync_dir};
use crate::scan::{ChangeKind, changes};

/// The file in the project's `.fold` directory that names the revision `main/` last finished showing.
pub const MARKER: &str = "main-materialised";

#[derive(Debug, Default)]
pub struct Rewritten {
    pub written: Vec<RepoPath>,
    pub removed: Vec<RepoPath>,
    /// Symlink entries written as regular files, so the caller logs one warning.
    pub degraded: Vec<RepoPath>,
    pub synced: Synced,
}

/// Brings `main/` from the revision the marker in `fold_dir` names to `revision`, then moves the marker. `from` is that revision's tree, `None` when no marker names one, in which case every file of `to` is written. The marker moves only once the files are on disk, so a crash in between leaves the old marker and the next run repeats the rewrite.
pub fn update(
    objects: &ObjectStore,
    root: &Path,
    fold_dir: &Path,
    from: Option<&ObjectHash>,
    to: &ObjectHash,
    revision: RevisionId,
    caps: Capabilities,
) -> Result<Rewritten, FoldError> {
    let old = match from {
        Some(tree) => load_flat(objects, tree)?,
        None => FlatTree::new(),
    };
    let new = load_flat(objects, to)?;
    let done = rewrite(objects, root, &old, &new, caps)?;
    write_marker(fold_dir, revision).map_err(WorkspaceError::from)?;
    Ok(done)
}

/// Rewrites only the paths whose content or kind differs between `from` and `to`: deleted paths are removed first, so a file and a directory can swap, then each changed file or symlink is staged under `main/.fold/tmp`, made read-only and renamed into place. Symlinks are created as recorded and never followed. Nothing is touched until every changed path is a valid path inside `root` and every blob it needs is stored. Each directory on the way to a changed path is made writable for the change and returned to `0555` afterwards where the platform has permission bits.
pub fn rewrite(
    objects: &ObjectStore,
    root: &Path,
    from: &FlatTree,
    to: &FlatTree,
    caps: Capabilities,
) -> Result<Rewritten, WorkspaceError> {
    let mut removals = Vec::new();
    let mut writes: Vec<(RepoPath, FlatEntry)> = Vec::new();
    for change in changes(from, to) {
        let path = RepoPath::parse(&change.path)?;
        match (change.kind, to.get(&change.path)) {
            (ChangeKind::Deleted, _) | (_, None) => removals.push(path),
            (ChangeKind::Added | ChangeKind::Modified, Some(entry)) => writes.push((path, *entry)),
        }
    }
    let removing: BTreeSet<&[u8]> = removals.iter().map(RepoPath::as_bytes).collect();
    for path in removals.iter().chain(writes.iter().map(|(path, _)| path)) {
        check_inside(root, path, &removing)?;
    }
    for (_, entry) in &writes {
        if !objects.contains(&entry.hash)? {
            return Err(ObjectError::Missing(entry.hash).into());
        }
    }
    let mut done = Rewritten::default();
    if removals.is_empty() && writes.is_empty() {
        return Ok(done);
    }
    fs::create_dir_all(root)?;
    let dirs = parent_dirs(removals.iter().chain(writes.iter().map(|(path, _)| path)));
    set_dir_modes(root, &dirs, |mode| mode | 0o700)?;
    let opts = MaterialiseOptions {
        read_only: true,
        caps,
    };
    let mut batch = Batch::open(root)?;
    for path in removals {
        batch.remove_entry(&path)?;
        done.removed.push(path);
    }
    for (path, entry) in writes {
        batch.remove_empty_dirs(&path)?;
        let written = batch.write_entry(objects, &path, &entry, &opts)?;
        if written.degraded {
            done.degraded.push(path.clone());
        }
        done.written.push(path);
    }
    done.synced = batch.finish()?;
    if caps.read_only_bit {
        set_dir_modes(root, &dirs, |_| 0o555)?;
    }
    Ok(done)
}

/// Refuses `path` when one of its parents in `root` is anything but a real directory, a symlink included, unless the rewrite removes that parent first.
fn check_inside(
    root: &Path,
    path: &RepoPath,
    removing: &BTreeSet<&[u8]>,
) -> Result<(), WorkspaceError> {
    let bytes = path.as_bytes();
    let ends = bytes
        .iter()
        .enumerate()
        .filter(|&(_, &byte)| byte == b'/')
        .map(|(at, _)| at);
    for end in ends {
        let parent = &bytes[..end];
        if removing.contains(parent) {
            return Ok(());
        }
        match fs::symlink_metadata(resolve(root, &RepoPath::parse(parent)?)?) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(WorkspaceError::invalid(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// The directories on the way to each of `paths`, as slash-joined names below the root; the empty name is the root.
fn parent_dirs<'a>(paths: impl Iterator<Item = &'a RepoPath>) -> BTreeSet<Vec<u8>> {
    let mut dirs = BTreeSet::from([Vec::new()]);
    for path in paths {
        let bytes = path.as_bytes();
        for (at, _) in bytes.iter().enumerate().filter(|&(_, &byte)| byte == b'/') {
            dirs.insert(bytes[..at].to_vec());
        }
    }
    dirs
}

/// Applies `mode` to the permission bits of each of `dirs` that is a real directory. Each one is reached through handles opened from `root` without following a symlink, so one that a removal pruned, a file replaced or a symlink took the place of is skipped and its target never changes. Removing and replacing an entry needs write access to its directory only, so the read-only files themselves are left as they are.
fn set_dir_modes(
    root: &Path,
    dirs: &BTreeSet<Vec<u8>>,
    mode: impl Fn(u32) -> u32,
) -> Result<(), WorkspaceError> {
    let base = DirHandle::open(root)?;
    'dirs: for dir in dirs {
        let mut current = base.try_clone()?;
        for name in dir
            .split(|&byte| byte == b'/')
            .filter(|name| !name.is_empty())
        {
            match current.child(name) {
                Ok(Some(child)) => current = child,
                Ok(None) => continue 'dirs,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue 'dirs,
                Err(error) => return Err(error.into()),
            }
        }
        let old = current.mode()?;
        if mode(old) != old {
            current.set_mode(mode(old))?;
        }
    }
    Ok(())
}

/// The revision the marker in `fold_dir` names. A missing marker, or one that names no revision, is `None`.
pub fn read_marker(fold_dir: &Path) -> io::Result<Option<RevisionId>> {
    match fs::read_to_string(fold_dir.join(MARKER)) {
        Ok(text) => Ok(text.trim().parse().ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Replaces the marker in one rename, so a reader sees the old revision or the new one and never a partial file.
pub fn write_marker(fold_dir: &Path, revision: RevisionId) -> io::Result<()> {
    let marker = fold_dir.join(MARKER);
    let tmp = fold_dir.join(format!("{MARKER}.tmp"));
    let mut file = File::create(&tmp)?;
    file.write_all(format!("{revision}\n").as_bytes())?;
    file.sync_all()?;
    fs::rename(&tmp, &marker)?;
    sync_dir(fold_dir)
}
