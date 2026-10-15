use std::collections::BTreeSet;
use std::fs::Metadata;
use std::io::{self, Read};
use std::path::Path;

use fold_core::object::{ObjectHash, ObjectHasher, ObjectKind, hash_object};
use fold_core::tree::{ChangeCounts, EntryKind, FlatEntry, FlatTree};

use crate::ignore::{FOLDIGNORE, GITIGNORE, IgnoreRules};
use crate::index::{StatEntry, StatIndex, mtime_ns};
use crate::paths::{RepoPath, WorkspaceError, is_reserved_name, open_parent};
use crate::platform::{Capabilities, DirHandle, is_executable};

pub const DEFAULT_MAX_FILE_BYTES: u64 = 52_428_800;
const READ_BUFFER: usize = 64 * 1024;
pub(crate) const READ_TRIES: usize = 3;

#[cfg(test)]
type ReadHook = Option<Box<dyn FnMut(&[u8], &std::fs::File)>>;

#[cfg(test)]
thread_local! {
    /// Runs in [`hash_stable`] between a read and its re-check, so a test can change the file at exactly that point.
    static AFTER_READ: std::cell::RefCell<ReadHook> = const { std::cell::RefCell::new(None) };
}

/// Runs `body` while every read of a file named `name` on this thread is followed by a new modification time, so each read of it looks unstable.
#[cfg(test)]
pub(crate) fn with_rewrites_during_reads<T>(name: &'static str, body: impl FnOnce() -> T) -> T {
    let mut tick = 0;
    AFTER_READ.set(Some(Box::new(move |read: &[u8], file: &std::fs::File| {
        if read == name.as_bytes() {
            tick += 1;
            let time = std::time::SystemTime::UNIX_EPOCH
                + std::time::Duration::from_secs(1_000_000 + tick);
            file.set_modified(time).unwrap();
        }
    })));
    let out = body();
    AFTER_READ.set(None);
    out
}

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
    match DirHandle::open(root) {
        Ok(handle) => walk.dir(&handle, None)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
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
    match rescan_scope(paths) {
        Some(targets) => rescan_targets(root, index, &targets, opts),
        None => scan(root, index, opts),
    }
}

/// The paths a [`rescan_paths`] over `paths` looks at: each path itself, except that a touched `.gitignore` or `.foldignore` stands for its whole folder. `None` when an ignore file at the root makes it a full [`scan`].
pub fn rescan_scope(paths: &[RepoPath]) -> Option<Vec<RepoPath>> {
    let mut targets = Vec::with_capacity(paths.len());
    for path in paths {
        match ignore_folder(path) {
            None => targets.push(path.clone()),
            Some(Some(folder)) => targets.push(folder),
            Some(None) => return None,
        }
    }
    Some(targets)
}

/// [`rescan_paths`] over targets that [`rescan_scope`] already worked out, so a caller that keeps its own entries knows exactly which paths the result covers.
pub fn rescan_targets(
    root: &Path,
    index: &mut StatIndex,
    targets: &[RepoPath],
    opts: &ScanOptions<'_>,
) -> Result<ScanResult, WorkspaceError> {
    opts.rules.reload();
    let mut walk = Walk::new(index, opts);
    for path in targets {
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

    /// Lists the directory from its handle, so every entry found is inside it. A directory removed since it was opened holds nothing.
    fn dir(&mut self, dir: &DirHandle, prefix: Option<&RepoPath>) -> Result<(), WorkspaceError> {
        let names = match dir.names() {
            Ok(names) => names,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for name in names {
            if is_reserved_name(&name) {
                continue;
            }
            let Ok(path) = RepoPath::child(prefix, &name) else {
                continue;
            };
            let meta = match dir.metadata(&name) {
                Ok(meta) => meta,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if self.opts.rules.is_ignored_below(&path, meta.is_dir()) {
                continue;
            }
            self.entry(dir, &name, &path, &meta)?;
        }
        Ok(())
    }

    /// A directory is entered through a handle opened without following a symlink. One swapped for something else after `meta` was read is looked at as what is there now, and dropped when that is ignored. One that keeps changing between a directory and something else is unstable, so the caller keeps what it knew below it.
    fn entry(
        &mut self,
        parent: &DirHandle,
        name: &[u8],
        path: &RepoPath,
        meta: &Metadata,
    ) -> Result<(), WorkspaceError> {
        if !meta.is_dir() {
            return Ok(self.file(parent, name, path, meta)?);
        }
        for _ in 0..READ_TRIES {
            match parent.child(name) {
                Ok(Some(child)) => return self.dir(&child, Some(path)),
                Ok(None) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    self.seen.insert(path.clone());
                    self.skip(path.clone(), SkipReason::Unreadable);
                    return Ok(());
                }
                Err(error) => return Err(error.into()),
            }
            let now = match parent.metadata(name) {
                Ok(now) => now,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.into()),
            };
            if now.is_dir() {
                continue;
            }
            if self.opts.rules.is_ignored_below(path, false) {
                return Ok(());
            }
            return Ok(self.file(parent, name, path, &now)?);
        }
        self.out.unstable.push(path.clone());
        Ok(())
    }

    fn touched(&mut self, root: &Path, path: &RepoPath) -> Result<(), WorkspaceError> {
        let Ok(Some((parent, name))) = open_parent(root, path) else {
            self.drop_below(path);
            return Ok(());
        };
        let meta = match parent.metadata(name) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.drop_below(path);
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        if self.opts.rules.is_ignored(path, meta.is_dir()) {
            self.drop_below(path);
        } else {
            self.entry(&parent, name, path, &meta)?;
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

    fn file(
        &mut self,
        parent: &DirHandle,
        name: &[u8],
        path: &RepoPath,
        meta: &Metadata,
    ) -> io::Result<()> {
        let kind = meta.file_type();
        if kind.is_symlink() {
            return self.symlink(parent, name, path, meta);
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
            None => match hash_stable(parent, name)? {
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

    fn symlink(
        &mut self,
        parent: &DirHandle,
        name: &[u8],
        path: &RepoPath,
        meta: &Metadata,
    ) -> io::Result<()> {
        self.seen.insert(path.clone());
        let known = self.index.lookup(path).copied();
        let cached = known
            .filter(|old| old.kind == EntryKind::Symlink)
            .and_then(|_| self.index.is_fresh(path, meta));
        let hash = match cached {
            Some(hash) => hash,
            None => {
                let target = match parent.read_link(name) {
                    Ok(Some(target)) => target,
                    Ok(None) => {
                        self.index.remove(path);
                        self.out.unstable.push(path.clone());
                        return Ok(());
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        self.seen.remove(path);
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                };
                self.out.hashed += 1;
                hash_object(ObjectKind::Blob, &target)
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

/// Hashes the file `name` in `dir` as a blob. The read counts only when the size and modification time are the same before and after it, and the name still holds a regular file; otherwise it is read again, up to three times in all.
pub(crate) fn hash_stable(dir: &DirHandle, name: &[u8]) -> io::Result<Reading> {
    read_stable_into(dir, name, None)
}

/// What a stable read of one workspace file found.
#[derive(Debug, PartialEq, Eq)]
pub enum StableRead {
    /// The content and its blob hash.
    Read(Vec<u8>, ObjectHash),
    /// The file kept changing over three reads.
    Unstable,
    Gone,
    Denied,
}

/// Reads the whole file `name` in `dir` with the scanner's stable read: a read counts only when the file did not change while it was read, and a changed file is read again, three tries at most. A symlink in the file's place is not followed.
pub(crate) fn read_stable(dir: &DirHandle, name: &[u8]) -> io::Result<StableRead> {
    let mut content = Vec::new();
    Ok(match read_stable_into(dir, name, Some(&mut content))? {
        Reading::Hashed(hash, _) => StableRead::Read(content, hash),
        Reading::Unstable => StableRead::Unstable,
        Reading::Gone => StableRead::Gone,
        Reading::Denied => StableRead::Denied,
    })
}

/// What reading one file of a workspace found.
#[derive(Debug, PartialEq, Eq)]
pub enum WorkingFile {
    Read(Vec<u8>),
    /// The file holds more than the limit, so it was not read.
    TooLarge(u64),
    /// Nothing is at the path.
    Missing,
    /// A directory, a symlink or another non-regular file is at the path. A symlink is never followed.
    NotAFile,
    /// The file kept changing over three reads.
    Unstable,
}

/// Reads the regular file at `path` under the workspace `root` one directory handle at a time, with no symlink followed, and only when it holds at most `max_bytes` bytes.
pub fn read_working_file(
    root: &Path,
    path: &RepoPath,
    max_bytes: u64,
) -> Result<WorkingFile, WorkspaceError> {
    let Some((dir, name)) = open_parent(root, path)? else {
        return Ok(WorkingFile::Missing);
    };
    let meta = match dir.metadata(name) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(WorkingFile::Missing),
        Err(error) => return Err(error.into()),
    };
    if !meta.is_file() {
        return Ok(WorkingFile::NotAFile);
    }
    if meta.len() > max_bytes {
        return Ok(WorkingFile::TooLarge(meta.len()));
    }
    Ok(match read_stable(&dir, name)? {
        StableRead::Read(bytes, _) if bytes.len() as u64 > max_bytes => {
            WorkingFile::TooLarge(bytes.len() as u64)
        }
        StableRead::Read(bytes, _) => WorkingFile::Read(bytes),
        StableRead::Gone => WorkingFile::Missing,
        StableRead::Unstable => WorkingFile::Unstable,
        StableRead::Denied => {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied).into());
        }
    })
}

fn read_stable_into(
    dir: &DirHandle,
    name: &[u8],
    mut content: Option<&mut Vec<u8>>,
) -> io::Result<Reading> {
    for _ in 0..READ_TRIES {
        if let Some(content) = content.as_deref_mut() {
            content.clear();
        }
        let (mut file, before) = match dir.open_regular(name) {
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
                    if let Some(content) = content.as_deref_mut() {
                        content.extend_from_slice(&buffer[..read]);
                    }
                    total += read as u64;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        #[cfg(test)]
        AFTER_READ.with_borrow_mut(|hook| {
            if let Some(hook) = hook {
                hook(name, &file);
            }
        });
        let after = file.metadata()?;
        let end = match dir.metadata(name) {
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const HOST: Capabilities = Capabilities {
        symlinks: true,
        exec_bit: true,
        inodes: true,
        read_only_bit: true,
    };

    #[test]
    fn a_directory_removed_after_it_was_opened_holds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        fs::create_dir(&gone).unwrap();
        let handle = DirHandle::open(&gone).unwrap();
        fs::remove_dir(&gone).unwrap();
        let rules = IgnoreRules::new(dir.path());
        let opts = ScanOptions {
            rules: &rules,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            caps: HOST,
            recorded: None,
        };
        let mut index = StatIndex::empty(HOST);
        let mut walk = Walk::new(&mut index, &opts);
        let path = RepoPath::parse(b"gone").unwrap();
        walk.dir(&handle, Some(&path)).unwrap();
        assert!(walk.out.entries.is_empty());
    }

    #[test]
    fn a_directory_swapped_for_a_symlink_during_the_scan_is_never_followed() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("secret"), b"outside").unwrap();
        let root = dir.path().join("ws");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/inner"), b"inner").unwrap();
        let rules = IgnoreRules::new(&root);
        let opts = ScanOptions {
            rules: &rules,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            caps: HOST,
            recorded: None,
        };
        let mut index = StatIndex::empty(HOST);
        let sub = root.join("sub");
        let moved = dir.path().join("moved");
        let target = outside.clone();
        let found = crate::platform::with_swap_before_entering(
            b"sub",
            move || {
                fs::rename(&sub, &moved).unwrap();
                std::os::unix::fs::symlink(&target, &sub).unwrap();
            },
            || scan(&root, &mut index, &opts),
        )
        .unwrap();

        let paths: Vec<&[u8]> = found.entries.keys().map(Vec::as_slice).collect();
        assert_eq!(paths, [b"sub".as_slice()]);
        assert_eq!(found.entries[b"sub".as_slice()].kind, EntryKind::Symlink);
        assert!(found.unstable.is_empty() && found.skipped.is_empty());
    }

    #[test]
    fn a_file_rewritten_during_its_read_is_unstable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("moving"), b"moving").unwrap();
        fs::write(root.join("steady"), b"steady").unwrap();
        let rules = IgnoreRules::new(&root);
        let opts = ScanOptions {
            rules: &rules,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            caps: HOST,
            recorded: None,
        };
        let mut index = StatIndex::empty(HOST);
        let found =
            with_rewrites_during_reads("moving", || scan(&root, &mut index, &opts)).unwrap();
        let moving = RepoPath::parse(b"moving").unwrap();
        assert_eq!(found.unstable, std::slice::from_ref(&moving));
        assert_eq!(
            found.entries.keys().cloned().collect::<Vec<_>>(),
            [b"steady".to_vec()]
        );
        assert!(index.lookup(&moving).is_none());
    }

    #[test]
    fn a_working_file_is_read_only_when_it_is_a_small_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("logs")).unwrap();
        fs::write(root.join("logs/run.log"), b"error\n").unwrap();
        fs::write(root.join("big.bin"), [0u8; 16]).unwrap();
        let read = |text: &str| {
            read_working_file(root, &RepoPath::parse(text.as_bytes()).unwrap(), 8).unwrap()
        };
        assert_eq!(read("logs/run.log"), WorkingFile::Read(b"error\n".to_vec()));
        assert_eq!(read("big.bin"), WorkingFile::TooLarge(16));
        assert_eq!(read("absent.txt"), WorkingFile::Missing);
        assert_eq!(read("absent/run.log"), WorkingFile::Missing);
        assert_eq!(read("logs"), WorkingFile::NotAFile);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("logs/run.log"), root.join("link")).unwrap();
            assert_eq!(read("link"), WorkingFile::NotAFile);
            std::os::unix::fs::symlink(root.join("logs"), root.join("dirlink")).unwrap();
            assert!(
                read_working_file(root, &RepoPath::parse(b"dirlink/run.log").unwrap(), 8).is_err()
            );
        }
    }
}
