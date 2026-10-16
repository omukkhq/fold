use std::ffi::OsStr;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use fold_core::tree::EntryKind;
use rustix::fs::{AtFlags, Mode, OFlags};
use rustix::io::Errno;

static PROBE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
type ChildHook = Option<(&'static [u8], Box<dyn FnOnce()>)>;

#[cfg(test)]
thread_local! {
    /// Runs once in [`DirHandle::child`] just before the named directory is opened, so a test can swap it at exactly the point between a walk listing it and entering it.
    static BEFORE_CHILD: std::cell::RefCell<ChildHook> = const { std::cell::RefCell::new(None) };
}

/// Runs `body` while the first [`DirHandle::child`] on this thread that opens a directory named `name` runs `swap` just before it opens it.
#[cfg(test)]
pub(crate) fn with_swap_before_entering<T>(
    name: &'static [u8],
    swap: impl FnOnce() + 'static,
    body: impl FnOnce() -> T,
) -> T {
    BEFORE_CHILD.set(Some((name, Box::new(swap))));
    let out = body();
    BEFORE_CHILD.set(None);
    out
}

/// What the host file system can hold. Probed once per open project, by its first command that needs it; tests build one by hand so every fallback runs on any host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub symlinks: bool,
    pub exec_bit: bool,
    pub inodes: bool,
    pub read_only_bit: bool,
}

impl Capabilities {
    /// `dir` is the project's `.fold` directory; the probe files live in a scratch directory under it and are removed again.
    pub fn probe(dir: &Path) -> io::Result<Self> {
        let number = PROBE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let scratch = dir.join(format!("probe-{}-{number}", std::process::id()));
        fs::create_dir_all(&scratch)?;
        let found = probe_in(&scratch);
        let _ = fs::remove_dir_all(&scratch);
        found
    }
}

fn probe_in(scratch: &Path) -> io::Result<Capabilities> {
    let file = scratch.join("file");
    File::create(&file)?.write_all(b"x")?;
    let exec_bit = probe_mode(&file, 0o755, |mode| mode & 0o111 != 0)?;
    let read_only_bit = probe_mode(&file, 0o444, |mode| mode & 0o222 == 0)?;
    let inodes = file_identity(&fs::symlink_metadata(&file)?, &ALL_BITS) != 0;
    let symlinks = create_symlink(b"file", &scratch.join("link"), &ALL_BITS)
        .is_ok_and(|outcome| outcome == SymlinkOutcome::Created);
    Ok(Capabilities {
        symlinks,
        exec_bit,
        inodes,
        read_only_bit,
    })
}

const ALL_BITS: Capabilities = Capabilities {
    symlinks: true,
    exec_bit: true,
    inodes: true,
    read_only_bit: true,
};

fn probe_mode(file: &Path, mode: u32, holds: fn(u32) -> bool) -> io::Result<bool> {
    fs::set_permissions(file, fs::Permissions::from_mode(mode))?;
    Ok(holds(fs::metadata(file)?.permissions().mode()))
}

/// Sets the permissions of an open file, so the change reaches the file that was written and nothing a path could be swapped to: `0o755` or `0o644` regardless of umask when the platform has an executable bit, and no write bit when `read_only`. Without an executable bit the mode is otherwise left alone. Without the read-only bit the write bits are cleared all the same, which is what the platform's read-only attribute does on Linux.
pub(crate) fn set_file_mode(
    file: &File,
    executable: bool,
    read_only: bool,
    caps: &Capabilities,
) -> io::Result<()> {
    let current = file.metadata()?.permissions().mode();
    let mode = entry_mode(current, executable, read_only, caps);
    file.set_permissions(fs::Permissions::from_mode(mode))
}

fn entry_mode(current: u32, executable: bool, clear_write: bool, caps: &Capabilities) -> u32 {
    let mut mode = if caps.exec_bit {
        if executable { 0o755 } else { 0o644 }
    } else {
        current & 0o7777
    };
    if clear_write {
        mode &= !0o222;
    }
    mode
}

/// Gives the owner what deleting the entry and its contents needs: write access to a file, and list, enter and write access to a directory. A symlink is left alone, because a permission change through it would change its target.
pub fn clear_read_only(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    let mut permissions = meta.permissions();
    let wanted = if meta.is_dir() { 0o700 } else { 0o200 };
    let mode = permissions.mode();
    if mode & wanted == wanted {
        return Ok(());
    }
    permissions.set_mode(mode | wanted);
    fs::set_permissions(path, permissions)
}

/// The permission bits the stat index compares.
pub fn file_mode(meta: &Metadata) -> u32 {
    meta.permissions().mode() & 0o7777
}

/// Without an executable bit the recorded entry decides.
pub fn is_executable(meta: &Metadata, recorded: EntryKind, caps: &Capabilities) -> bool {
    if caps.exec_bit {
        file_mode(meta) & 0o111 != 0
    } else {
        recorded == EntryKind::Executable
    }
}

/// `0` when the capabilities give no inode.
pub fn file_identity(meta: &Metadata, caps: &Capabilities) -> u64 {
    if caps.inodes { meta.ino() } else { 0 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymlinkOutcome {
    Created,
    /// The link was written as a regular file holding the target text.
    Emulated,
}

/// Creates `link` pointing at `target`, unchecked and unfollowed. Without symlink support the link is a regular file holding the target text.
pub fn create_symlink(
    target: &[u8],
    link: &Path,
    caps: &Capabilities,
) -> io::Result<SymlinkOutcome> {
    if caps.symlinks {
        std::os::unix::fs::symlink(OsStr::from_bytes(target), link)?;
        return Ok(SymlinkOutcome::Created);
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(link)?;
    file.write_all(target)?;
    Ok(SymlinkOutcome::Emulated)
}

/// A directory held open as a descriptor. Every name given to it is one component looked up in that directory and never followed when it is a symlink, so a parent swapped for a symlink after it was opened cannot send a read or a write out of the workspace.
#[derive(Debug)]
pub(crate) struct DirHandle(OwnedFd);

impl DirHandle {
    /// The one lookup that follows symlinks: `path` is a workspace root, which fold itself chose.
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
        Ok(Self(rustix::fs::open(path, flags, Mode::empty())?))
    }

    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        Ok(Self(self.0.try_clone()?))
    }

    /// `None` when `name` is there but is a symlink or anything else that is not a directory.
    pub(crate) fn child(&self, name: &[u8]) -> io::Result<Option<Self>> {
        #[cfg(test)]
        BEFORE_CHILD.with_borrow_mut(|hook| {
            if hook.as_ref().is_some_and(|(wanted, _)| *wanted == name)
                && let Some((_, swap)) = hook.take()
            {
                swap();
            }
        });
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        match rustix::fs::openat(&self.0, name, flags, Mode::empty()) {
            Ok(fd) => Ok(Some(Self(fd))),
            Err(Errno::LOOP | Errno::NOTDIR) => Ok(None),
            Err(errno) => Err(errno.into()),
        }
    }

    /// [`DirHandle::child`], creating the directory first when nothing is there. The flag is true when this call created it.
    pub(crate) fn child_or_create(&self, name: &[u8]) -> io::Result<Option<(Self, bool)>> {
        match self.child(name) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            found => return found.map(|dir| dir.map(|dir| (dir, false))),
        }
        let created = match rustix::fs::mkdirat(&self.0, name, Mode::from_raw_mode(0o777)) {
            Ok(()) => true,
            Err(Errno::EXIST) => false,
            Err(errno) => return Err(errno.into()),
        };
        Ok(self.child(name)?.map(|dir| (dir, created)))
    }

    /// What `name` itself is, a symlink included. The metadata comes from an `O_PATH` descriptor, which opens neither a link's target nor a FIFO, so it describes exactly what `symlink_metadata` would.
    pub(crate) fn metadata(&self, name: &[u8]) -> io::Result<Metadata> {
        let flags = OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        File::from(rustix::fs::openat(&self.0, name, flags, Mode::empty())?).metadata()
    }

    /// Opens `name` for reading only when it is a regular file: a symlink is not followed and a FIFO does not block the open. `None` when something other than a regular file is there.
    pub(crate) fn open_regular(&self, name: &[u8]) -> io::Result<Option<(File, Metadata)>> {
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        let file = match rustix::fs::openat(&self.0, name, flags, Mode::empty()) {
            Ok(fd) => File::from(fd),
            Err(Errno::LOOP) => return Ok(None),
            Err(errno) => return Err(errno.into()),
        };
        let meta = file.metadata()?;
        Ok(meta.is_file().then_some((file, meta)))
    }

    /// Creates `name` as a new regular file; anything already there, a symlink included, is an error.
    pub(crate) fn create_file(&self, name: &[u8]) -> io::Result<File> {
        let flags =
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let fd = rustix::fs::openat(&self.0, name, flags, Mode::from_raw_mode(0o666))?;
        Ok(File::from(fd))
    }

    /// Creates the symlink `name` pointing at `target`, unchecked.
    pub(crate) fn symlink(&self, target: &[u8], name: &[u8]) -> io::Result<()> {
        Ok(rustix::fs::symlinkat(target, &self.0, name)?)
    }

    /// `None` when `name` is not a symlink, which a symlink replaced since its metadata was read gives.
    pub(crate) fn read_link(&self, name: &[u8]) -> io::Result<Option<Vec<u8>>> {
        match rustix::fs::readlinkat(&self.0, name, Vec::new()) {
            Ok(target) => Ok(Some(target.into_bytes())),
            Err(Errno::INVAL) => Ok(None),
            Err(errno) => Err(errno.into()),
        }
    }

    pub(crate) fn rename(&self, from: &[u8], to_dir: &DirHandle, to: &[u8]) -> io::Result<()> {
        Ok(rustix::fs::renameat(&self.0, from, &to_dir.0, to)?)
    }

    /// Removes a file or symlink; a directory is an error.
    pub(crate) fn remove_file(&self, name: &[u8]) -> io::Result<()> {
        Ok(rustix::fs::unlinkat(&self.0, name, AtFlags::empty())?)
    }

    /// Removes an empty directory.
    pub(crate) fn remove_dir(&self, name: &[u8]) -> io::Result<()> {
        Ok(rustix::fs::unlinkat(&self.0, name, AtFlags::REMOVEDIR)?)
    }

    /// Removes `name` and, when it is a directory, everything below it. Each level is entered through its own handle and no symlink is followed, so nothing outside the directory is removed.
    pub(crate) fn remove_all(&self, name: &[u8]) -> io::Result<()> {
        match self.child(name)? {
            Some(dir) => {
                for child in dir.names()? {
                    dir.remove_all(&child)?;
                }
                self.remove_dir(name)
            }
            None => self.remove_file(name),
        }
    }

    /// Removes the directory `name` and the empty directories below it, entering each through its own handle. Anything else below makes the removal fail.
    pub(crate) fn remove_empty_dirs(&self, name: &[u8]) -> io::Result<()> {
        if let Some(dir) = self.child(name)? {
            for child in dir.names()? {
                if dir.child(&child)?.is_some() {
                    dir.remove_empty_dirs(&child)?;
                }
            }
        }
        self.remove_dir(name)
    }

    /// The names in the directory, without `.` and `..`.
    pub(crate) fn names(&self) -> io::Result<Vec<Vec<u8>>> {
        let mut names = Vec::new();
        for entry in rustix::fs::Dir::read_from(&self.0)? {
            let entry = entry?;
            let name = entry.file_name().to_bytes();
            if name != b"." && name != b".." {
                names.push(name.to_vec());
            }
        }
        Ok(names)
    }

    /// Flushes the directory's entries, with the tolerance of [`sync_dir`].
    pub(crate) fn sync(&self) -> io::Result<()> {
        match rustix::fs::fsync(&self.0) {
            Ok(()) => Ok(()),
            Err(errno) => tolerate_sync_error(errno.into()),
        }
    }
}

/// Flushes a directory entry to disk so a rename survives a crash. Platforms that cannot open or sync a directory lose only that extra guarantee; the write itself still succeeds.
pub fn sync_dir(path: &Path) -> io::Result<()> {
    match File::open(path).and_then(|dir| dir.sync_all()) {
        Ok(()) => Ok(()),
        Err(error) => tolerate_sync_error(error),
    }
}

/// `Unsupported` and `PermissionDenied` mean the platform has no directory sync; every other kind is a real failure.
pub fn tolerate_sync_error(error: io::Error) -> io::Result<()> {
    match error.kind() {
        io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied => Ok(()),
        _ => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_modes_ignore_the_umask_and_a_missing_executable_bit_keeps_the_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        fs::write(&path, b"x").unwrap();
        let file = File::open(&path).unwrap();
        let mode = || fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
        set_file_mode(&file, true, false, &ALL_BITS).unwrap();
        assert_eq!(mode(), 0o755);
        set_file_mode(&file, false, true, &ALL_BITS).unwrap();
        assert_eq!(mode(), 0o444);
        set_file_mode(&file, true, true, &ALL_BITS).unwrap();
        assert_eq!(mode(), 0o555);
        let keep = Capabilities {
            exec_bit: false,
            ..ALL_BITS
        };
        set_file_mode(&file, true, false, &keep).unwrap();
        assert_eq!(mode(), 0o555, "no executable bit leaves the mode alone");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        set_file_mode(&file, false, true, &keep).unwrap();
        assert_eq!(mode(), 0o444);
    }

    #[test]
    fn syncs_a_real_directory() {
        let dir = tempfile::tempdir().unwrap();
        sync_dir(dir.path()).unwrap();
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let error = sync_dir(&dir.path().join("absent")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn a_handle_never_follows_a_symlink_below_it() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("f"), b"outside").unwrap();
        let root = dir.path().join("ws");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("file"), b"inside").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        let handle = DirHandle::open(&root).unwrap();

        assert!(handle.child(b"link").unwrap().is_none());
        assert!(handle.child(b"file").unwrap().is_none());
        assert!(handle.child_or_create(b"link").unwrap().is_none());
        assert!(handle.open_regular(b"link").unwrap().is_none());
        assert!(handle.metadata(b"link").unwrap().file_type().is_symlink());
        assert_eq!(handle.read_link(b"file").unwrap(), None);
        assert_eq!(
            handle.read_link(b"link").unwrap().unwrap(),
            outside.as_os_str().as_encoded_bytes()
        );
        let (_, created) = handle.child_or_create(b"made").unwrap().unwrap();
        assert!(created);
        let (made, created) = handle.child_or_create(b"made").unwrap().unwrap();
        assert!(!created);
        made.create_file(b"x").unwrap();
        assert!(
            made.create_file(b"x").is_err(),
            "an existing name is refused"
        );
        fs::create_dir_all(root.join("empty/inner")).unwrap();
        fs::create_dir(outside.join("kept")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("empty/link")).unwrap();
        assert!(handle.remove_empty_dirs(b"empty").is_err(), "a link stays");
        let empty = handle.child(b"empty").unwrap().unwrap();
        assert_eq!(empty.names().unwrap(), [b"link".to_vec()]);
        empty.remove_file(b"link").unwrap();
        fs::create_dir(root.join("empty/inner")).unwrap();
        handle.remove_empty_dirs(b"empty").unwrap();
        assert!(fs::symlink_metadata(outside.join("kept")).unwrap().is_dir());
        handle.remove_all(b"link").unwrap();
        handle.remove_all(b"made").unwrap();
        let mut left = handle.names().unwrap();
        left.sort();
        assert_eq!(left, [b"file".to_vec()]);
        assert_eq!(fs::read(outside.join("f")).unwrap(), b"outside");
        handle.sync().unwrap();
    }

    #[test]
    fn error_mapping() {
        for kind in [io::ErrorKind::Unsupported, io::ErrorKind::PermissionDenied] {
            assert!(tolerate_sync_error(io::Error::from(kind)).is_ok());
        }
        for kind in [
            io::ErrorKind::NotFound,
            io::ErrorKind::Other,
            io::ErrorKind::StorageFull,
        ] {
            assert_eq!(
                tolerate_sync_error(io::Error::from(kind))
                    .unwrap_err()
                    .kind(),
                kind
            );
        }
    }
}
