use std::fs::{File, TryLockError};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fold_core::{FoldError, HostPaths, codes};

/// The folder below fold's data folder that holds one lock file for each project folder that a daemon closed or an init made.
pub const CLOSING_DIR: &str = "closing";

/// How long an init waits for a daemon to close the old project of its folder before it refuses.
pub const INIT_CLOSE_WAIT: Duration = Duration::from_secs(30);

const RETRY: Duration = Duration::from_millis(50);

/// The lock folder of the host whose registry is at `host`.
pub fn closing_dir(host: &HostPaths) -> PathBuf {
    host.registry_db()
        .parent()
        .map_or_else(|| PathBuf::from(CLOSING_DIR), |dir| dir.join(CLOSING_DIR))
}

/// The exclusive lock of one project folder, held by a daemon while it closes a handle of the folder whose `.fold/` or database went away, and by an init while it makes the project. It lives in fold's data folder, outside the project folder, so a deleted `.fold/` never takes it along, and it is an advisory lock on an open file, so a process that ends lets it go. The lock files are never deleted: a file deleted while another process waits on it would give two holders.
#[derive(Debug)]
pub struct FolderLock {
    _file: File,
}

impl FolderLock {
    /// Takes the lock of `root` without waiting: `None` while another process or handle holds it.
    pub async fn try_take(dir: &Path, root: &Path) -> io::Result<Option<Self>> {
        let path = lock_file(dir, root);
        let dir = dir.to_owned();
        let taken = tokio::task::spawn_blocking(move || -> io::Result<Option<Self>> {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)?;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(&path)?;
            match file.try_lock() {
                Ok(()) => Ok(Some(Self { _file: file })),
                Err(TryLockError::WouldBlock) => Ok(None),
                Err(TryLockError::Error(error)) => Err(error),
            }
        })
        .await;
        taken.unwrap_or_else(|error| Err(io::Error::other(error)))
    }

    /// Takes the lock of `root`, trying again until `wait` is over: `None` when another holder kept it all that time.
    pub async fn take_within(dir: &Path, root: &Path, wait: Duration) -> io::Result<Option<Self>> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if let Some(lock) = Self::try_take(dir, root).await? {
                return Ok(Some(lock));
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(None);
            }
            tokio::time::sleep(RETRY).await;
        }
    }
}

/// The lock file of `root`: the BLAKE3 hash of the folder's path, so every path gives a short file name of its own.
fn lock_file(dir: &Path, root: &Path) -> PathBuf {
    let hash = blake3::hash(root.as_os_str().as_encoded_bytes());
    dir.join(format!("{}.lock", hash.to_hex()))
}

/// The refusal of an init whose folder still has an old project that a daemon closes.
pub(crate) fn old_project_closing(root: &Path) -> FoldError {
    FoldError::new(
        codes::PROJECT_UNAVAILABLE,
        format!(
            "fold still closes the old project of {}, whose .fold was deleted",
            root.display()
        ),
    )
    .with_hint(format!(
        "wait until the daemon has closed the old project, then run `fld init` in {} again",
        root.display()
    ))
    .with_details([("reason", serde_json::json!("old_project_closing"))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_held_lock_keeps_a_second_taker_out_until_it_is_let_go() {
        let dir = tempfile::TempDir::new().unwrap();
        let locks = dir.path().join(CLOSING_DIR);
        let root = Path::new("/srv/app");
        let first = FolderLock::try_take(&locks, root).await.unwrap().unwrap();
        assert!(FolderLock::try_take(&locks, root).await.unwrap().is_none());
        assert!(
            FolderLock::take_within(&locks, root, Duration::from_millis(120))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            FolderLock::try_take(&locks, Path::new("/srv/other"))
                .await
                .unwrap()
                .is_some(),
            "another folder has its own lock"
        );
        drop(first);
        assert!(
            FolderLock::take_within(&locks, root, Duration::from_secs(5))
                .await
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn the_lock_folder_is_beside_the_registry() {
        let host = HostPaths::new(Some("/data"), None).unwrap();
        assert_eq!(closing_dir(&host), Path::new("/data/fold/closing"));
    }

    #[test]
    fn the_refusal_names_the_folder_and_the_next_step() {
        let error = old_project_closing(Path::new("/srv/app"));
        assert_eq!(error.code(), "project_unavailable");
        assert!(error.hint().unwrap_or_default().contains("fld init"));
    }
}
