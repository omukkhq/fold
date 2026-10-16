use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rustix::fs::{FlockOperation, flock};

use crate::platform::sync_dir;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The permission bits of a file that does not exist yet, before the umask removes some.
const NEW_FILE_MODE: u32 = 0o644;

/// Replaces `path` with `bytes` so a reader sees the old file or the new one and a crash leaves one of them. The bytes go to a new temporary file beside `path`, named for the process and a counter so no two writers share one, are flushed, renamed into place, and the directory entry is flushed. An existing file keeps its permission bits, and its new content is never readable with wider ones; a new one gets `0o644` less the umask, so a strict umask still applies. A failed write removes its temporary file. The parent directory is created when missing.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().filter(|dir| !dir.as_os_str().is_empty());
    let dir = dir.unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let existing = fs::metadata(path)
        .ok()
        .map(|meta| meta.permissions().mode() & 0o7777);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let number = COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp: PathBuf = dir.join(format!(".save-{name}-{}-{number}.tmp", std::process::id()));
    let written = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(if existing.is_some() {
                0o600
            } else {
                NEW_FILE_MODE
            })
            .open(&temp)?;
        file.write_all(bytes)?;
        if let Some(mode) = existing {
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    sync_dir(dir)
}

/// An exclusive lock on the sibling `<name>.lock` file of a file that more than one process rewrites. The lock is held until the value drops, and the kernel releases it when the process ends. Take it before reading the file and keep it until the rewrite is written, so no rewrite starts from text another rewrite is about to replace.
#[derive(Debug)]
pub struct EditLock {
    _file: File,
}

impl EditLock {
    /// Waits for the lock of `target`, creating the lock file and the parent directory when missing.
    pub fn acquire(target: &Path) -> io::Result<Self> {
        let mut name = target.file_name().unwrap_or_default().to_owned();
        name.push(".lock");
        let lock = target.with_file_name(name);
        if let Some(dir) = lock.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock)?;
        flock(&file, FlockOperation::LockExclusive)?;
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;

    #[test]
    fn a_write_replaces_the_file_keeps_its_mode_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/config.toml");
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        write_atomic(&path, b"one").unwrap();
        assert_eq!(mode(&path), 0o644 & !umask());
        for bits in [0o640, 0o600] {
            fs::set_permissions(&path, fs::Permissions::from_mode(bits)).unwrap();
            write_atomic(&path, b"two").unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"two");
            assert_eq!(mode(&path), bits);
        }
        assert_eq!(fs::read_dir(dir.path().join("sub")).unwrap().count(), 1);
    }

    /// Read from `/proc`, because setting the umask to learn it would change it for the other tests of the process.
    fn umask() -> u32 {
        let status = fs::read_to_string("/proc/self/status").unwrap();
        let line = status
            .lines()
            .find_map(|line| line.strip_prefix("Umask:"))
            .unwrap();
        u32::from_str_radix(line.trim(), 8).unwrap()
    }

    #[test]
    fn a_failed_write_removes_its_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("target");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("inside"), "x").unwrap();
        assert!(write_atomic(&path, b"x").is_err());
        let names: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn two_locks_of_one_file_never_overlap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let first = EditLock::acquire(&path).unwrap();
        let acquired = Arc::new(AtomicBool::new(false));
        let thread = {
            let (path, acquired) = (path.clone(), Arc::clone(&acquired));
            std::thread::spawn(move || {
                let _second = EditLock::acquire(&path).unwrap();
                acquired.store(true, Ordering::SeqCst);
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(!acquired.load(Ordering::SeqCst));
        drop(first);
        thread.join().unwrap();
        assert!(acquired.load(Ordering::SeqCst));
    }
}
