use std::fs::Metadata;
use std::io::{self, Read};
use std::path::Path;

use fold_core::object::ObjectHash;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{FoldError, codes};

use crate::ignore::IgnoreRules;
use crate::objects::{ObjectError, ObjectStore};
use crate::paths::RepoPath;
use crate::platform::{Capabilities, DirHandle, is_executable};
use crate::revision_store::write_initial_tree;
use crate::scan::{READ_TRIES, SkipReason, Skipped};

#[derive(Debug, Clone, Copy)]
pub struct ImportOptions {
    pub max_file_bytes: u64,
    /// Without an executable bit every file imports as a regular file.
    pub caps: Capabilities,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportResult {
    pub tree: ObjectHash,
    /// Files and symlinks recorded in the tree.
    pub files: usize,
    /// The content size of everything recorded, a symlink counting its target text.
    pub bytes: u64,
    /// Sorted by path.
    pub skipped: Vec<Skipped>,
}

/// Stores every tracked file under `source` as a blob, then writes the trees and returns the root. `rules` is built on `source`, so its `.gitignore` and `.foldignore` files decide what is tracked, while the files themselves import like any other. `.git` and `.fold` never import, because no `RepoPath` can name them. Nothing is written into `source`. A file that cannot be read is skipped and the import goes on; a failure of the object store or of reading `source` itself stops it. Blocking: async callers go through `spawn_blocking`.
pub fn import_dir(
    objects: &ObjectStore,
    source: &Path,
    rules: &IgnoreRules,
    opts: &ImportOptions,
) -> Result<ImportResult, FoldError> {
    rules.reload();
    let mut walk = Walk {
        objects,
        rules,
        opts,
        entries: FlatTree::new(),
        bytes: 0,
        skipped: Vec::new(),
    };
    let root = DirHandle::open(source).map_err(|error| source_error(&error))?;
    walk.dir(&root, None)?;
    walk.skipped.sort_by(|a, b| a.path.cmp(&b.path));
    let files = walk.entries.len();
    let tree = write_initial_tree(objects, walk.entries)?;
    Ok(ImportResult {
        tree,
        files,
        bytes: walk.bytes,
        skipped: walk.skipped,
    })
}

struct Walk<'a> {
    objects: &'a ObjectStore,
    rules: &'a IgnoreRules,
    opts: &'a ImportOptions,
    entries: FlatTree,
    bytes: u64,
    skipped: Vec<Skipped>,
}

impl Walk<'_> {
    /// Lists the directory from its handle, so every entry found is inside it.
    fn dir(&mut self, dir: &DirHandle, prefix: Option<&RepoPath>) -> Result<(), FoldError> {
        let names = match dir.names() {
            Ok(names) => names,
            Err(error) if prefix.is_some() && error.kind() == io::ErrorKind::NotFound => {
                return Ok(());
            }
            Err(error) => return Err(source_error(&error)),
        };
        for name in names {
            let Ok(path) = RepoPath::child(prefix, &name) else {
                continue;
            };
            let meta = match dir.metadata(&name) {
                Ok(meta) => meta,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => {
                    self.skip(&path, SkipReason::Unreadable);
                    continue;
                }
            };
            if self.rules.is_ignored_below(&path, meta.is_dir()) {
                continue;
            }
            if meta.is_dir() {
                self.subdir(dir, &name, &path)?;
            } else {
                self.entry(dir, &name, &path, &meta)?;
            }
        }
        Ok(())
    }

    /// A directory is entered through a handle opened without following a symlink. One swapped for something else since it was listed imports as what is there now, unless that is ignored; one that keeps changing between a directory and something else is skipped as unreadable.
    fn subdir(
        &mut self,
        parent: &DirHandle,
        name: &[u8],
        path: &RepoPath,
    ) -> Result<(), FoldError> {
        for _ in 0..READ_TRIES {
            match parent.child(name) {
                Ok(Some(child)) => return self.dir(&child, Some(path)),
                Ok(None) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    self.skip(path, SkipReason::Unreadable);
                    return Ok(());
                }
                Err(error) => return Err(source_error(&error)),
            }
            let now = match parent.metadata(name) {
                Ok(now) => now,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(_) => {
                    self.skip(path, SkipReason::Unreadable);
                    return Ok(());
                }
            };
            if now.is_dir() {
                continue;
            }
            if self.rules.is_ignored_below(path, false) {
                return Ok(());
            }
            return self.entry(parent, name, path, &now);
        }
        self.skip(path, SkipReason::Unreadable);
        Ok(())
    }

    fn entry(
        &mut self,
        parent: &DirHandle,
        name: &[u8],
        path: &RepoPath,
        meta: &Metadata,
    ) -> Result<(), FoldError> {
        if meta.file_type().is_symlink() {
            return match parent.read_link(name) {
                Ok(Some(target)) => self.store(path, EntryKind::Symlink, &mut target.as_slice()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Ok(None) | Err(_) => {
                    self.skip(path, SkipReason::Unreadable);
                    Ok(())
                }
            };
        }
        if !meta.is_file() {
            self.skip(path, SkipReason::Special);
            return Ok(());
        }
        // The size and mode come from the opened file, so a name swapped after the directory listing cannot pass for what was listed.
        let (mut file, meta) = match parent.open_regular(name) {
            Ok(Some(opened)) => opened,
            Ok(None) => {
                self.skip(path, SkipReason::Special);
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(_) => {
                self.skip(path, SkipReason::Unreadable);
                return Ok(());
            }
        };
        if meta.len() > self.opts.max_file_bytes {
            self.skip(path, SkipReason::TooLarge);
            return Ok(());
        }
        let kind = if is_executable(&meta, EntryKind::File, &self.opts.caps) {
            EntryKind::Executable
        } else {
            EntryKind::File
        };
        self.store(path, kind, &mut file)
    }

    /// A failed or over-long read is the source's and skips the path; any other error is the object store's and stops the import.
    fn store(
        &mut self,
        path: &RepoPath,
        kind: EntryKind,
        reader: &mut impl Read,
    ) -> Result<(), FoldError> {
        let mut tracked = TrackedReader::new(reader, self.opts.max_file_bytes);
        let written = self.objects.write_blob_from(&mut tracked);
        match (written, tracked.failed) {
            (Ok(written), _) => {
                self.bytes += written.size;
                self.entries.insert(
                    path.as_bytes().to_vec(),
                    FlatEntry {
                        kind,
                        hash: written.hash,
                    },
                );
                Ok(())
            }
            (Err(ObjectError::Io(_)), Some(reason)) => {
                self.skip(path, reason);
                Ok(())
            }
            (Err(error), _) => Err(error.into()),
        }
    }

    fn skip(&mut self, path: &RepoPath, reason: SkipReason) {
        self.skipped.push(Skipped {
            path: path.clone(),
            reason,
        });
    }
}

/// Fails the read once more than `limit` bytes arrive, so a file that grows while it is read is skipped as too large instead of stored.
struct TrackedReader<'r, R> {
    inner: &'r mut R,
    limit: u64,
    total: u64,
    failed: Option<SkipReason>,
}

impl<'r, R> TrackedReader<'r, R> {
    fn new(inner: &'r mut R, limit: u64) -> Self {
        Self {
            inner,
            limit,
            total: 0,
            failed: None,
        }
    }
}

impl<R: Read> Read for TrackedReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.inner.read(buf) {
            Ok(read) => {
                self.total = self.total.saturating_add(read as u64);
                if self.total > self.limit {
                    self.failed = Some(SkipReason::TooLarge);
                    return Err(io::Error::other("the file grew past the size limit"));
                }
                Ok(read)
            }
            Err(error) => {
                if error.kind() != io::ErrorKind::Interrupted {
                    self.failed = Some(SkipReason::Unreadable);
                }
                Err(error)
            }
        }
    }
}

fn source_error(error: &io::Error) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_IO,
        format!("cannot read the import source: {}", error.kind()),
    )
    .with_hint("check that the source directory exists and is readable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failing_reader_is_marked() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("x"))
            }
        }
        let mut broken = Broken;
        let mut tracked = TrackedReader::new(&mut broken, 4);
        assert!(tracked.read(&mut [0; 4]).is_err());
        assert_eq!(tracked.failed, Some(SkipReason::Unreadable));
    }

    #[test]
    fn an_interrupted_read_is_not_a_failure() {
        struct Interrupted;
        impl Read for Interrupted {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::ErrorKind::Interrupted.into())
            }
        }
        let mut interrupted = Interrupted;
        let mut tracked = TrackedReader::new(&mut interrupted, 4);
        assert!(tracked.read(&mut [0; 4]).is_err());
        assert_eq!(tracked.failed, None);
    }

    #[test]
    fn a_reader_past_the_limit_is_marked_too_large() {
        let mut bytes: &[u8] = b"12345";
        let mut tracked = TrackedReader::new(&mut bytes, 4);
        let mut buf = [0; 3];
        assert_eq!(tracked.read(&mut buf).unwrap(), 3);
        assert!(tracked.read(&mut buf).is_err());
        assert_eq!(tracked.failed, Some(SkipReason::TooLarge));
    }

    #[test]
    fn a_file_that_grows_past_the_limit_is_skipped_and_not_stored() {
        let dir = tempfile::tempdir().unwrap();
        let objects = ObjectStore::open(&dir.path().join(".fold")).unwrap();
        let rules = IgnoreRules::new(dir.path());
        let opts = ImportOptions {
            max_file_bytes: 4,
            caps: Capabilities::probe(&dir.path().join(".fold")).unwrap(),
        };
        let mut walk = Walk {
            objects: &objects,
            rules: &rules,
            opts: &opts,
            entries: FlatTree::new(),
            bytes: 0,
            skipped: Vec::new(),
        };
        let path = RepoPath::parse(b"grown").unwrap();
        walk.store(&path, EntryKind::File, &mut b"12345".as_slice())
            .unwrap();
        assert!(walk.entries.is_empty());
        assert_eq!(
            walk.skipped,
            [Skipped {
                path,
                reason: SkipReason::TooLarge
            }]
        );
    }

    #[test]
    fn a_directory_swapped_for_a_symlink_during_the_walk_is_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret"), b"outside").unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir_all(source.join("sub")).unwrap();
        std::fs::write(source.join("sub/inner"), b"inner").unwrap();
        std::fs::write(source.join("top"), b"top").unwrap();
        let objects = ObjectStore::open(&dir.path().join(".fold")).unwrap();
        let rules = IgnoreRules::new(&source);
        let opts = ImportOptions {
            max_file_bytes: 1024,
            caps: Capabilities::probe(&dir.path().join(".fold")).unwrap(),
        };
        let sub = source.join("sub");
        let moved = dir.path().join("moved");
        let target = outside.clone();
        let imported = crate::platform::with_swap_before_entering(
            b"sub",
            move || {
                std::fs::rename(&sub, &moved).unwrap();
                std::os::unix::fs::symlink(&target, &sub).unwrap();
            },
            || import_dir(&objects, &source, &rules, &opts),
        )
        .unwrap();

        let load = |hash: &ObjectHash| crate::snapshot::load_tree(&objects, hash);
        let entries = fold_core::tree::flatten(&imported.tree, &load).unwrap();
        let paths: Vec<&[u8]> = entries.keys().map(Vec::as_slice).collect();
        assert_eq!(paths, [b"sub".as_slice(), b"top"]);
        assert_eq!(entries[b"sub".as_slice()].kind, EntryKind::Symlink);
        assert!(imported.skipped.is_empty(), "{:?}", imported.skipped);
    }

    #[test]
    fn a_source_error_carries_the_io_code() {
        let error = source_error(&io::Error::from(io::ErrorKind::NotFound));
        assert_eq!(error.code(), codes::WORKSPACE_IO);
    }
}
