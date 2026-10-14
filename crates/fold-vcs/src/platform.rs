use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use fold_core::tree::EntryKind;

static PROBE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// What the host file system can hold. Probed once when a project opens; tests build one by hand so every fallback runs on any host.
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

#[cfg(unix)]
fn probe_mode(file: &Path, mode: u32, holds: fn(u32) -> bool) -> io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(file, fs::Permissions::from_mode(mode))?;
    Ok(holds(fs::metadata(file)?.permissions().mode()))
}

#[cfg(not(unix))]
fn probe_mode(_file: &Path, _mode: u32, _holds: fn(u32) -> bool) -> io::Result<bool> {
    Ok(false)
}

/// Sets the file's permissions: `0o755` or `0o644` regardless of umask when the platform has an executable bit, and no write bit (or the platform's read-only attribute) when `read_only`. Without an executable bit the mode is otherwise left alone.
pub fn set_mode(
    path: &Path,
    executable: bool,
    read_only: bool,
    caps: &Capabilities,
) -> io::Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut mode = if caps.exec_bit {
            if executable { 0o755 } else { 0o644 }
        } else {
            permissions.mode() & 0o7777
        };
        if read_only && caps.read_only_bit {
            mode &= !0o222;
        }
        permissions.set_mode(mode);
    }
    #[cfg(not(unix))]
    let _ = executable;
    if read_only && !caps.read_only_bit {
        permissions.set_readonly(true);
    }
    fs::set_permissions(path, permissions)
}

/// The permission bits the stat index compares. Platforms without mode bits report the read-only attribute alone.
pub fn file_mode(meta: &Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o7777
    }
    #[cfg(not(unix))]
    {
        if meta.permissions().readonly() {
            0o444
        } else {
            0o644
        }
    }
}

/// Without an executable bit the recorded entry decides.
pub fn is_executable(meta: &Metadata, recorded: EntryKind, caps: &Capabilities) -> bool {
    if caps.exec_bit {
        file_mode(meta) & 0o111 != 0
    } else {
        recorded == EntryKind::Executable
    }
}

/// `0` when the platform or the capabilities give no inode.
pub fn file_identity(meta: &Metadata, caps: &Capabilities) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if caps.inodes { meta.ino() } else { 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = (meta, caps);
        0
    }
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
    #[cfg(unix)]
    if caps.symlinks {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        std::os::unix::fs::symlink(OsStr::from_bytes(target), link)?;
        return Ok(SymlinkOutcome::Created);
    }
    #[cfg(not(unix))]
    let _ = caps;
    let mut file = OpenOptions::new().write(true).create_new(true).open(link)?;
    file.write_all(target)?;
    Ok(SymlinkOutcome::Emulated)
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
