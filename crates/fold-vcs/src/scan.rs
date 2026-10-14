use std::collections::BTreeSet;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

use fold_core::object::{ObjectHash, ObjectHasher, ObjectKind, hash_object};
use fold_core::tree::{ChangeCounts, EntryKind, FlatEntry, FlatTree};

use crate::ignore::{FOLDIGNORE, GITIGNORE, IgnoreRules};
use crate::index::{StatEntry, StatIndex, mtime_ns};
use crate::paths::{RepoPath, WorkspaceError, is_reserved_name, os_bytes, resolve};
use crate::platform::{Capabilities, is_executable};

pub const DEFAULT_MAX_FILE_BYTES: u64 = 52_428_800;
const READ_BUFFER: usize = 64 * 1024;
const READ_TRIES: usize = 3;

#[derive(Debug, Clone, Copy)]
pub struct ScanOptions<'a> {
    pub rules: &'a IgnoreRules,
    pub max_file_bytes: u64,
    pub caps: Capabilities,
    /// The entries of the tree the workspace was last known to match. Where the platform has no executable bit, a file that the stat index does not know keeps the kind recorded here.
    pub recorded: Option<&'a FlatTree>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    TooLarge,
    /// A socket, FIFO or device.
    Special,
    /// The file or directory could not be read for lack of permission.
    Unreadable,
}

impl SkipReason {
    /// The name the command line prints next to a skipped path.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
            Self::Special => "special",
            Self::Unreadable => "unreadable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: RepoPath,
    pub reason: SkipReason,
}

#[derive(Debug, Default)]
pub struct ScanResult {
    /// Every tracked file and symlink found. Empty directories do not appear.
    pub entries: FlatTree,
    pub skipped: Vec<Skipped>,
    /// Files whose size or modification time kept changing while they were read.
    pub unstable: Vec<RepoPath>,
    /// Files whose content was read and hashed. A file served from the stat index does not count.
    pub hashed: usize,
    /// Stat index entries dropped because the path is gone, newly ignored or now outside the tracked set.
    pub removed: Vec<RepoPath>,
}

/// Walks the whole workspace. The stat index is updated in place; saving it is the caller's job. A path found in `skipped` or `unstable` has no entry and no index record, so the caller keeps whatever it knew before.
pub fn scan(
    root: &Path,
    index: &mut StatIndex,
    opts: &ScanOptions<'_>,
) -> Result<ScanResult, WorkspaceError> {
    opts.rules.reload();
    let mut walk = Walk::new(index, opts);
    walk.dir(root, None)?;
    walk.prune(None);
    Ok(walk.out)
}

/// Looks only at the given paths and everything below any of them that is a directory. `entries` holds the current state of those paths; a touched path with no entry is gone. A touched `.gitignore` or `.foldignore` changes which paths below its folder are tracked, so that whole folder is looked at in its place, and a touched ignore file at the root makes this a full [`scan`].
pub fn rescan_paths(
    root: &Path,
    index: &mut StatIndex,
    paths: &[RepoPath],
    opts: &ScanOptions<'_>,
) -> Result<ScanResult, WorkspaceError> {
    let mut targets = Vec::with_capacity(paths.len());
    for path in paths {
        match ignore_folder(path) {
            None => targets.push(path.clone()),
            Some(Some(folder)) => targets.push(folder),
            Some(None) => return scan(root, index, opts),
        }
    }
    opts.rules.reload();
    let mut walk = Walk::new(index, opts);
    for path in &targets {
        walk.touched(root, path)?;
    }
    Ok(walk.out)
}

/// For an ignore file, the folder its rules apply to (`None` for the root); `None` for any other path.
fn ignore_folder(path: &RepoPath) -> Option<Option<RepoPath>> {
    let bytes = path.as_bytes();
    let (folder, name) = match bytes.iter().rposition(|&byte| byte == b'/') {
        Some(at) => (Some(&bytes[..at]), &bytes[at + 1..]),
        None => (None, bytes),
    };
    if name != GITIGNORE.as_bytes() && name != FOLDIGNORE.as_bytes() {
        return None;
    }
    Some(folder.and_then(|folder| RepoPath::parse(folder).ok()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathChange {
    pub path: Vec<u8>,
    pub kind: ChangeKind,
}

/// Compares by hash and kind only, in path order.
pub fn changes(base: &FlatTree, current: &FlatTree) -> Vec<PathChange> {
    let mut found = Vec::new();
    for (path, entry) in current {
        match base.get(path) {
            None => found.push(change(path, ChangeKind::Added)),
            Some(old) if old != entry => found.push(change(path, ChangeKind::Modified)),
            Some(_) => {}
        }
    }
    for path in base.keys().filter(|path| !current.contains_key(*path)) {
        found.push(change(path, ChangeKind::Deleted));
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

/// The file counts a changeset stores for its changed paths.
pub fn count_changes(changes: &[PathChange]) -> ChangeCounts {
    let mut counts = ChangeCounts::default();
    for change in changes {
        let count = match change.kind {
            ChangeKind::Added => &mut counts.files_added,
            ChangeKind::Modified => &mut counts.files_modified,
            ChangeKind::Deleted => &mut counts.files_deleted,
        };
        *count = count.saturating_add(1);
    }
    counts
}

fn change(path: &[u8], kind: ChangeKind) -> PathChange {
    PathChange {
        path: path.to_vec(),
        kind,
    }
}

struct Walk<'a, 'o> {
    index: &'a mut StatIndex,
    opts: &'a ScanOptions<'o>,
    out: ScanResult,
    seen: BTreeSet<RepoPath>,
}

pub(crate) enum Reading {
    Hashed(ObjectHash, Box<Metadata>),
    Unstable,
    Gone,
    Denied,
}

impl<'a, 'o> Walk<'a, 'o> {
    fn new(index: &'a mut StatIndex, opts: &'a ScanOptions<'o>) -> Self {
        Self {
            index,
            opts,
            out: ScanResult::default(),
            seen: BTreeSet::new(),
        }
    }

    fn dir(&mut self, folder: &Path, prefix: Option<&RepoPath>) -> Result<(), WorkspaceError> {
        let reader = match fs::read_dir(folder) {
            Ok(reader) => reader,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                let Some(prefix) = prefix else {
                    return Err(error.into());
                };
                self.seen.insert(prefix.clone());
                self.skip(prefix.clone(), SkipReason::Unreadable);
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        for entry in reader {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            let Some(name) = os_bytes(&entry.file_name()) else {
                continue;
            };
            if is_reserved_name(&name) {
                continue;
            }
            let Ok(path) = RepoPath::child(prefix, &name) else {
                continue;
            };
            let meta = match entry.metadata() {
                Ok(meta) => meta,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if self.opts.rules.is_ignored_below(&path, meta.is_dir()) {
                continue;
            }
            if meta.is_dir() {
                self.dir(&entry.path(), Some(&path))?;
            } else {
                self.file(&path, &entry.path(), &meta)?;
            }
        }
        Ok(())
    }

    fn touched(&mut self, root: &Path, path: &RepoPath) -> Result<(), WorkspaceError> {
        let located = resolve(root, path)
            .ok()
            .and_then(|located| match fs::symlink_metadata(&located) {
                Ok(meta) => Some(Ok((located, meta))),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => Some(Err(error)),
            })
            .transpose()?;
        let Some((located, meta)) = located else {
            self.drop_below(path);
            return Ok(());
        };
        if self.opts.rules.is_ignored(path, meta.is_dir()) {
            self.drop_below(path);
        } else if meta.is_dir() {
            self.dir(&located, Some(path))?;
            self.prune(Some(path));
        } else {
            self.file(path, &located, &meta)?;
            self.prune(Some(path));
        }
        Ok(())
    }

    /// Removes the index entries at and below `prefix` that this walk did not see.
    fn prune(&mut self, prefix: Option<&RepoPath>) {
        let stale: Vec<RepoPath> = self
            .index
            .iter()
            .map(|(path, _)| path)
            .filter(|path| prefix.is_none_or(|prefix| is_at_or_below(path, prefix)))
            .filter(|path| !self.seen.contains(*path))
            .cloned()
            .collect();
        for path in stale {
            self.index.remove(&path);
            self.out.removed.push(path);
        }
    }

    fn drop_below(&mut self, prefix: &RepoPath) {
        self.prune(Some(prefix));
    }

    fn skip(&mut self, path: RepoPath, reason: SkipReason) {
        self.index.remove(&path);
        self.out.skipped.push(Skipped { path, reason });
    }

    fn file(&mut self, path: &RepoPath, located: &Path, meta: &Metadata) -> io::Result<()> {
        let kind = meta.file_type();
        if kind.is_symlink() {
            return self.symlink(path, located, meta);
        }
        self.seen.insert(path.clone());
        if !kind.is_file() {
            self.skip(path.clone(), SkipReason::Special);
            return Ok(());
        }
        if meta.len() > self.opts.max_file_bytes {
            self.skip(path.clone(), SkipReason::TooLarge);
            return Ok(());
        }
        let known = self.index.lookup(path).copied();
        let emulated = !self.opts.caps.symlinks;
        if emulated
            && known.is_some_and(|old| old.kind == EntryKind::Symlink)
            && let Some(hash) = self.index.is_fresh(path, meta)
        {
            self.keep(path, EntryKind::Symlink, hash);
            return Ok(());
        }
        let recorded = known
            .map(|old| FlatEntry {
                kind: old.kind,
                hash: old.hash,
            })
            .or_else(|| self.opts.recorded?.get(path.as_bytes()).copied());
        let recorded_kind = recorded.map_or(EntryKind::File, |entry| entry.kind);
        let kind = if is_executable(meta, recorded_kind, &self.opts.caps) {
            EntryKind::Executable
        } else {
            EntryKind::File
        };
        let cached = known
            .filter(|old| old.kind != EntryKind::Symlink)
            .and_then(|_| self.index.is_fresh(path, meta));
        let (hash, meta) = match cached {
            Some(hash) => (hash, meta.clone()),
            None => match hash_stable(located)? {
                Reading::Hashed(hash, meta) => {
                    self.out.hashed += 1;
                    (hash, *meta)
                }
                Reading::Unstable => {
                    self.index.remove(path);
                    self.out.unstable.push(path.clone());
                    return Ok(());
                }
                Reading::Gone => {
                    self.seen.remove(path);
                    return Ok(());
                }
                Reading::Denied => {
                    self.skip(path.clone(), SkipReason::Unreadable);
                    return Ok(());
                }
            },
        };
        // A link emulated as a file that still holds its target text stays a link, even when the index that knew it is lost.
        let kind = match recorded {
            Some(entry) if emulated && entry.kind == EntryKind::Symlink && entry.hash == hash => {
                EntryKind::Symlink
            }
            _ => kind,
        };
        self.record(path, kind, hash, &meta);
        Ok(())
    }

    fn symlink(&mut self, path: &RepoPath, located: &Path, meta: &Metadata) -> io::Result<()> {
        self.seen.insert(path.clone());
        let known = self.index.lookup(path).copied();
        let cached = known
            .filter(|old| old.kind == EntryKind::Symlink)
            .and_then(|_| self.index.is_fresh(path, meta));
        let hash = match cached {
            Some(hash) => hash,
            None => {
                let target = match fs::read_link(located) {
                    Ok(target) => target,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        self.seen.remove(path);
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                };
                let Some(bytes) = os_bytes(target.as_os_str()) else {
                    self.skip(path.clone(), SkipReason::Special);
                    return Ok(());
                };
                self.out.hashed += 1;
                hash_object(ObjectKind::Blob, &bytes)
            }
        };
        self.record(path, EntryKind::Symlink, hash, meta);
        Ok(())
    }

    fn record(&mut self, path: &RepoPath, kind: EntryKind, hash: ObjectHash, meta: &Metadata) {
        self.index.upsert(
            path.clone(),
            StatEntry::from_metadata(meta, kind, hash, &self.opts.caps),
        );
        self.keep(path, kind, hash);
    }

    fn keep(&mut self, path: &RepoPath, kind: EntryKind, hash: ObjectHash) {
        self.out
            .entries
            .insert(path.as_bytes().to_vec(), FlatEntry { kind, hash });
    }
}

fn is_at_or_below(path: &RepoPath, prefix: &RepoPath) -> bool {
    let (path, prefix) = (path.as_bytes(), prefix.as_bytes());
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"))
}

/// Opens the file at `path` for reading only when it is a regular file: a symlink put in its place is not followed and a FIFO does not block the open. `None` when something other than a regular file is there now.
pub(crate) fn open_regular(path: &Path) -> io::Result<Option<(File, Metadata)>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        #[cfg(unix)]
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => return Ok(None),
        Err(error) => return Err(error),
    };
    let meta = file.metadata()?;
    Ok(meta.is_file().then_some((file, meta)))
}

/// Hashes the file as a blob. The read counts only when the size and modification time are the same before and after it, and the path is still a regular file; otherwise it is read again, up to three times in all.
pub(crate) fn hash_stable(path: &Path) -> io::Result<Reading> {
    for _ in 0..READ_TRIES {
        let (mut file, before) = match open_regular(path) {
            Ok(Some(opened)) => opened,
            Ok(None) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Reading::Gone),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                return Ok(Reading::Denied);
            }
            Err(error) => return Err(error),
        };
        let mut hasher = ObjectHasher::new(ObjectKind::Blob);
        let mut buffer = vec![0u8; READ_BUFFER];
        let mut total = 0u64;
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    hasher.update(&buffer[..read]);
                    total += read as u64;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        let after = file.metadata()?;
        let end = match fs::symlink_metadata(path) {
            Ok(end) => end,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Reading::Gone),
            Err(error) => return Err(error),
        };
        let same = |a: &Metadata, b: &Metadata| a.len() == b.len() && mtime_ns(a) == mtime_ns(b);
        if end.is_file() && same(&before, &after) && same(&after, &end) && after.len() == total {
            return Ok(Reading::Hashed(hasher.finish(), Box::new(end)));
        }
    }
    Ok(Reading::Unstable)
}
