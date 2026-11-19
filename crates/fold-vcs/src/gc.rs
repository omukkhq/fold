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
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            return Err(WorkspaceError::OtherFilesystem(
                path.to_string_lossy().into_owned(),
            ));
        }
        Err(error) => return Err(error.into()),
    }
    match fs::remove_dir_all(&dir) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

/// The canonical folder `name` directly below the project's `.fold/workers/`, or `None` when nothing is there. `.fold/workers` must be a real directory and `name` a plain folder name whose entry is a directory, never a symlink; anything else is `InvalidInput`, so a link a worker's shell planted is never followed; a folder on another file system than `.fold/workers` is `CrossesDevices`.
pub fn locate_worker_dir(paths: &ProjectPaths, name: &str) -> io::Result<Option<PathBuf>> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "not a worker folder");
    let mut components = Path::new(name).components();
    if !matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    ) {
        return Err(invalid());
    }
    let workers = paths.workers_dir();
    let workers = match fs::symlink_metadata(&workers) {
        Ok(meta) if meta.is_dir() => fs::canonicalize(&workers)?,
        Ok(_) => return Err(invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let dir = workers.join(name);
    match fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() => {
            if device(&meta) != device(&fs::metadata(&workers)?) {
                return Err(crosses_devices());
            }
            Ok(Some(dir))
        }
        Ok(_) => Err(invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Deletes the directory `dir` and everything below it, opening up directories that refuse removal first and removing symlinks inside without following them, the way [`remove_workspace_dir`] deletes a workspace. False when nothing was at `dir`; an entry at `dir` that is no directory is `InvalidInput`.
pub fn remove_tree(dir: &Path) -> io::Result<bool> {
    match make_removable(dir) {
        Ok(true) => {}
        Ok(false) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a directory",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    }
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error),
    }
}

/// The bytes of the files and symlinks below `dir`, by their length, never following a symlink; zero when nothing is there. An entry removed during the walk counts nothing.
pub fn tree_bytes(dir: &Path) -> io::Result<u64> {
    let mut bytes = 0u64;
    let mut pending = vec![dir.to_owned()];
    while let Some(at) = pending.pop() {
        let entries = match fs::read_dir(&at) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for item in entries {
            let item = item?;
            let meta = match fs::symlink_metadata(item.path()) {
                Ok(meta) => meta,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if meta.is_dir() {
                pending.push(item.path());
            } else {
                bytes = bytes.saturating_add(meta.len());
            }
        }
    }
    Ok(bytes)
}

/// The canonical directory [`remove_workspace_dir`] would delete, or `None` when it is already gone. The path must name a direct child of the project's workspace directory `.fold/ws/` once its parent is resolved, must hold no `..`, and must not be a symlink itself; anything else is `invalid_path`.
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
        Ok(meta) if meta.is_dir() => {
            if device(&meta) != device(&fs::metadata(&canonical_ws)?) {
                return Err(WorkspaceError::OtherFilesystem(
                    path.to_string_lossy().into_owned(),
                ));
            }
            Ok(Some(dir))
        }
        Ok(_) => Err(invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The file system that holds an entry, so a delete never crosses into a mount.
#[cfg(unix)]
fn device(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.dev()
}

#[cfg(not(unix))]
fn device(_meta: &fs::Metadata) -> u64 {
    0
}

/// The error of [`make_removable`] for a directory below the root on another file system.
fn crosses_devices() -> io::Error {
    io::Error::new(
        io::ErrorKind::CrossesDevices,
        "a directory below is on another file system",
    )
}

/// Gives the owner list, enter and write access to `root` and every directory below it, which is all an unlink needs, and returns false when `root` is not a directory. Each directory is opened without following a symlink and changed and listed through that descriptor, so an entry swapped for a symlink during the walk is never followed; files keep their modes. One descriptor pair is open per level, as `remove_dir_all` holds.
#[cfg(target_os = "linux")]
fn make_removable(root: &Path) -> io::Result<bool> {
    let Some(top) = open_dir(root, None)? else {
        return Ok(false);
    };
    let root_device = top.device;
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
        match open_dir(&level.at.join(item.file_name()), Some(root_device)) {
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
    device: u64,
}

/// Opens `path` without following a final symlink, gives the owner `rwx` on it, and lists it. `None` when it is not a directory; a directory on another file system than `root_device` is refused before it is changed. The `/proc/self/fd` link names the inode the descriptor holds, so the change and the listing reach that inode and nothing a path could be swapped to.
#[cfg(target_os = "linux")]
fn open_dir(path: &Path, root_device: Option<u64>) -> io::Result<Option<OpenDir>> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;

    use rustix::fs::{Mode, OFlags};
    // An `O_PATH` descriptor opens a directory its owner may not list, but it takes no `fchmod`, so the change goes through the descriptor's `/proc/self/fd` link.
    let fd = fs::File::from(rustix::fs::open(
        path,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let meta = fd.metadata()?;
    if !meta.is_dir() {
        return Ok(None);
    }
    let device = device(&meta);
    if root_device.is_some_and(|root| root != device) {
        return Err(crosses_devices());
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
        device,
    }))
}

/// `remove_dir_all` refuses entries in a directory without write access, and other platforms refuse read-only files too. Returns false when `root` is not a directory.
#[cfg(not(target_os = "linux"))]
fn make_removable(root: &Path) -> io::Result<bool> {
    let top = fs::symlink_metadata(root)?;
    if !top.is_dir() {
        return Ok(false);
    }
    let root_device = device(&top);
    let mut pending = vec![root.to_owned()];
    while let Some(dir) = pending.pop() {
        let meta = fs::symlink_metadata(&dir)?;
        if !meta.is_dir() {
            continue;
        }
        if device(&meta) != root_device {
            return Err(crosses_devices());
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

    /// No test may mount, so `.fold/ws` resolves to `/`, whose child `/dev` is a mount on most hosts; only the lookup runs, nothing is removed.
    #[test]
    fn a_workspace_directory_on_another_file_system_is_refused() {
        let root_device = device(&fs::metadata("/").unwrap());
        if device(&fs::metadata("/dev").unwrap()) == root_device {
            return;
        }
        let project = tempfile::tempdir().unwrap();
        fs::create_dir(project.path().join(".fold")).unwrap();
        symlink("/", project.path().join(".fold/ws")).unwrap();
        let error = locate_workspace_dir(project.path(), Path::new(".fold/ws/dev")).unwrap_err();
        assert_eq!(error.code(), fold_core::codes::DIRECTORY_OTHER_FILESYSTEM);
    }

    #[test]
    fn a_symlink_entry_below_the_root_is_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("file"), b"outside").unwrap();
        fs::set_permissions(outside.join("file"), fs::Permissions::from_mode(0o400)).unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o500)).unwrap();
        let root = dir.path().join("ws");
        fs::create_dir_all(root.join("sub")).unwrap();
        symlink(&outside, root.join("sub/dir-link")).unwrap();
        symlink(outside.join("file"), root.join("file-link")).unwrap();
        fs::set_permissions(root.join("sub"), fs::Permissions::from_mode(0o500)).unwrap();

        assert!(make_removable(&root).unwrap());
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&root.join("sub")), 0o700);
        assert_eq!(mode(&outside), 0o500);
        assert_eq!(mode(&outside.join("file")), 0o400);
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).unwrap();
    }

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
