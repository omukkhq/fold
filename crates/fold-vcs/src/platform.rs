use std::fs::File;
use std::io;
use std::path::Path;

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
