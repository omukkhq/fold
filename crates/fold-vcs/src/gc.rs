use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::revision::Revision;
use fold_core::tree::{EntryKind, Tree};
use fold_core::{FoldError, ProjectPaths};

use crate::objects::{ObjectError, ObjectStore};
use crate::paths::WorkspaceError;
#[cfg(not(target_os = "linux"))]
use crate::platform::clear_read_only;

/// How old an unreferenced object or scratch file must be before a sweep removes it. Together with the modification time a rewrite of an existing object refreshes, it protects objects written but not yet referenced and objects a snapshot reuses while a run is in progress.
pub const GRACE: Duration = Duration::from_secs(60 * 60);

/// What one garbage collection run did, or would do in a dry run. A sweep fills the object counts; the service that prunes checkpoints and removes workspace directories fills the last two.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    pub examined: u64,
    pub deleted: u64,
    pub bytes_freed: u64,
    pub tmp_removed: u64,
    /// Objects the walk reached that the store does not hold, each an `object_missing` finding. Sorted.
    pub missing: Vec<ObjectHash>,
    pub workspaces_removed: u64,
    pub checkpoints_pruned: u64,
}

/// Every object reachable from `roots`: a revision reaches its tree and its parent, a tree reaches each entry, and blobs end the walk unread. A reached object the store does not hold stays in the set and the walk goes on; [`sweep`] reports it as missing. A damaged object stops the walk with its error, because the objects below it cannot be known.
pub fn mark(
    objects: &ObjectStore,
    roots: impl IntoIterator<Item = ObjectHash>,
) -> Result<HashSet<ObjectHash>, FoldError> {
    let mut marked = HashSet::new();
    let mut pending: Vec<ObjectHash> = roots.into_iter().collect();
    while let Some(hash) = pending.pop() {
        if !marked.insert(hash) {
            continue;
        }
        let object = match objects.read(&hash) {
            Ok(object) => object,
            Err(ObjectError::Missing(_)) => continue,
            Err(error) => return Err(error.into()),
        };
        match object.kind {
            ObjectKind::Blob => {}
            ObjectKind::Tree => {
                for entry in Tree::decode(&object.payload)?.entries {
                    if entry.kind == EntryKind::Directory {
                        pending.push(entry.hash);
                    } else {
                        marked.insert(entry.hash);
                    }
                }
            }
            ObjectKind::Revision => {
                let revision = Revision::decode(&object.payload)?;
                pending.push(revision.tree);
                pending.extend(revision.parent);
            }
        }
    }
    Ok(marked)
}

/// Removes every object that is not in `marked` and was last modified more than `grace` before `now`, and every scratch file in `objects/tmp/` of that age. `now` is in the file system's clock domain, the one object modification times use.
pub fn sweep(
    objects: &ObjectStore,
    marked: &HashSet<ObjectHash>,
    now: SystemTime,
    grace: Duration,
) -> Result<GcReport, ObjectError> {
    collect(objects, marked, now, grace, true)
}

/// Counts what [`sweep`] would remove with the same arguments and removes nothing.
pub fn plan_sweep(
    objects: &ObjectStore,
    marked: &HashSet<ObjectHash>,
    now: SystemTime,
    grace: Duration,
) -> Result<GcReport, ObjectError> {
    collect(objects, marked, now, grace, false)
}

fn collect(
    objects: &ObjectStore,
    marked: &HashSet<ObjectHash>,
    now: SystemTime,
    grace: Duration,
    remove: bool,
) -> Result<GcReport, ObjectError> {
    let cutoff = now.checked_sub(grace).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut report = GcReport::default();
    let entries = objects.entries()?;
    let mut present = HashSet::with_capacity(entries.len());
    for entry in entries {
        report.examined += 1;
        present.insert(entry.hash);
        if marked.contains(&entry.hash) || entry.modified >= cutoff {
            continue;
        }
        // A snapshot that reused the object after the listing refreshed its time; the second look keeps it.
        if remove
            && !(older_than(&objects.path(&entry.hash), cutoff)? && objects.remove(&entry.hash)?)
        {
            continue;
        }
        report.deleted += 1;
        report.bytes_freed += entry.size;
    }
    report.tmp_removed = collect_scratch(objects.scratch_dir(), cutoff, remove)?;
    report.missing = marked
        .iter()
        .filter(|hash| !present.contains(hash))
        .copied()
        .collect();
    report.missing.sort();
    Ok(report)
}

fn older_than(path: &Path, cutoff: SystemTime) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Ok(meta.modified()? < cutoff),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn collect_scratch(scratch: &Path, cutoff: SystemTime, remove: bool) -> io::Result<u64> {
    let mut removed = 0;
    for item in fs::read_dir(scratch)? {
        let item = item?;
        let meta = match item.metadata() {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if meta.is_dir() || meta.modified()? >= cutoff {
            continue;
        }
        if remove {
            match fs::remove_file(item.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            }
        }
        removed += 1;
    }
    Ok(removed)
}

/// Deletes the workspace directory at `path` (relative to `project_root`, or absolute) and returns whether it existed. See [`locate_workspace_dir`] for what is refused; a directory replaced by anything else between that check and the removal is refused too. Directories inside that refuse removal are opened up first, and symlinks inside are removed without being followed. Callers pass only directories of archived or abandoned workspaces.
pub fn remove_workspace_dir(project_root: &Path, path: &Path) -> Result<bool, WorkspaceError> {
    let Some(dir) = locate_workspace_dir(project_root, path)? else {
        return Ok(false);
    };
    match make_removable(&dir) {
        Ok(true) => {}
        Ok(false) => {
            return Err(WorkspaceError::InvalidPath(
                path.to_string_lossy().into_owned(),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error.into()),
    }
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

/// The canonical directory [`remove_workspace_dir`] would delete, or `None` when it is already gone. The path must name a direct child of the project's `ws/` directory once its parent is resolved, must hold no `..`, and must not be a symlink itself; anything else is `invalid_path`.
pub fn locate_workspace_dir(
    project_root: &Path,
    path: &Path,
) -> Result<Option<PathBuf>, WorkspaceError> {
    let invalid = || WorkspaceError::InvalidPath(path.to_string_lossy().into_owned());
    let target = project_root.join(path);
    if target
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(invalid());
    }
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        return Err(invalid());
    };
    let ws = ProjectPaths::new(project_root).ws_dir();
    let canonical_ws = match fs::canonicalize(&ws) {
        Ok(dir) => dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound && parent == ws => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(invalid()),
        Err(error) => return Err(error.into()),
    };
    match fs::canonicalize(parent) {
        Ok(dir) if dir == canonical_ws => {}
        Ok(_) => return Err(invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(invalid()),
        Err(error) => return Err(error.into()),
    }
    let dir = canonical_ws.join(name);
    match fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() => Ok(Some(dir)),
        Ok(_) => Err(invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Gives the owner list, enter and write access to `root` and every directory below it, which is all an unlink needs, and returns false when `root` is not a directory. Each directory is opened without following a symlink and changed and listed through that descriptor, so an entry swapped for a symlink during the walk is never followed; files keep their modes. One descriptor pair is open per level, as `remove_dir_all` holds.
#[cfg(target_os = "linux")]
fn make_removable(root: &Path) -> io::Result<bool> {
    let Some(top) = open_dir(root)? else {
        return Ok(false);
    };
    let mut stack = vec![top];
    while let Some(level) = stack.last_mut() {
        let Some(item) = level.entries.next() else {
            stack.pop();
            continue;
        };
        let item = item?;
        if !item.file_type()?.is_dir() {
            continue;
        }
        match open_dir(&level.at.join(item.file_name())) {
            Ok(Some(dir)) => stack.push(dir),
            Ok(None) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

/// A directory held by an `O_PATH` descriptor, with its listing read through that descriptor.
#[cfg(target_os = "linux")]
struct OpenDir {
    _fd: fs::File,
    at: PathBuf,
    entries: fs::ReadDir,
}

/// Opens `path` without following a final symlink, gives the owner `rwx` on it, and lists it. `None` when it is not a directory. The `/proc/self/fd` link names the inode the descriptor holds, so the change and the listing reach that inode and nothing a path could be swapped to.
#[cfg(target_os = "linux")]
fn open_dir(path: &Path) -> io::Result<Option<OpenDir>> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let fd = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_NOFOLLOW)
        .open(path)?;
    let meta = fd.metadata()?;
    if !meta.is_dir() {
        return Ok(None);
    }
    let at = PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd()));
    let mode = meta.permissions().mode();
    if mode & 0o700 != 0o700 {
        fs::set_permissions(&at, fs::Permissions::from_mode(mode | 0o700))?;
    }
    let entries = fs::read_dir(&at)?;
    Ok(Some(OpenDir {
        _fd: fd,
        at,
        entries,
    }))
}

/// `remove_dir_all` refuses entries in a directory without write access, and other platforms refuse read-only files too. Returns false when `root` is not a directory.
#[cfg(not(target_os = "linux"))]
fn make_removable(root: &Path) -> io::Result<bool> {
    if !fs::symlink_metadata(root)?.is_dir() {
        return Ok(false);
    }
    let mut pending = vec![root.to_owned()];
    while let Some(dir) = pending.pop() {
        if !fs::symlink_metadata(&dir)?.is_dir() {
            continue;
        }
        clear_read_only(&dir)?;
        for item in fs::read_dir(&dir)? {
            let item = item?;
            let kind = item.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(item.path());
            } else {
                clear_read_only(&item.path())?;
            }
        }
    }
    Ok(true)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    #[test]
    fn a_root_that_became_a_symlink_is_not_walked() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir_all(outside.join("inner")).unwrap();
        fs::set_permissions(outside.join("inner"), fs::Permissions::from_mode(0o500)).unwrap();
        let link = dir.path().join("link");
        symlink(&outside, &link).unwrap();
        assert!(!make_removable(&link).unwrap());
        let mode = fs::metadata(outside.join("inner"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o500);
        fs::set_permissions(outside.join("inner"), fs::Permissions::from_mode(0o700)).unwrap();
    }
}
