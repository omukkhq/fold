use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use fold_core::object::{ObjectKind, hash_object};
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};

use crate::materialise::{Batch, MaterialiseOptions};
use crate::objects::ObjectStore;
use crate::paths::{RepoPath, WorkspaceError};
use crate::platform::{Capabilities, DirHandle, is_executable};
use crate::scan::{StableRead, read_stable};

/// What the project folder holds at one path of a tree, read without following a symlink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderEntry {
    /// Nothing is there, and no parent stands in the way.
    Missing,
    /// A file, an executable or a symlink, as a tree entry records it.
    Entry(FlatEntry),
    /// Something a tree entry cannot record: a directory, a parent that is not a directory, a special file, or a file that is too large, unreadable or kept changing while it was read.
    Other,
}

impl FolderEntry {
    /// Whether the folder holds exactly `entry`, nothing for `None`.
    pub fn holds(&self, entry: Option<&FlatEntry>) -> bool {
        match (self, entry) {
            (Self::Missing, None) => true,
            (Self::Entry(found), Some(entry)) => found == entry,
            _ => false,
        }
    }
}

/// Reads the entry at `path` below `root` one directory handle at a time, with no symlink followed. `recorded` is the kind fold knows for the path, which decides a file's kind where the file system has no executable bit. Blocking.
pub fn read_folder_entry(
    root: &Path,
    path: &RepoPath,
    max_bytes: u64,
    caps: &Capabilities,
    recorded: Option<EntryKind>,
) -> Result<FolderEntry, WorkspaceError> {
    let mut dir = match DirHandle::open(root) {
        Ok(dir) => dir,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(FolderEntry::Missing),
        Err(error) => return Err(error.into()),
    };
    let (parents, name) = path.parents_and_name();
    for parent in parents {
        dir = match dir.child(parent) {
            Ok(Some(child)) => child,
            Ok(None) => return Ok(FolderEntry::Other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(FolderEntry::Missing);
            }
            Err(error) => return Err(error.into()),
        };
    }
    let meta = match dir.metadata(name) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(FolderEntry::Missing),
        Err(error) => return Err(error.into()),
    };
    if meta.file_type().is_symlink() {
        return Ok(match dir.read_link(name)? {
            Some(target) => FolderEntry::Entry(FlatEntry {
                kind: EntryKind::Symlink,
                hash: hash_object(ObjectKind::Blob, &target),
            }),
            None => FolderEntry::Other,
        });
    }
    if !meta.is_file() || meta.len() > max_bytes {
        return Ok(FolderEntry::Other);
    }
    let StableRead::Read(_, hash) = read_stable(&dir, name, max_bytes)? else {
        return Ok(FolderEntry::Other);
    };
    let kind = if is_executable(&meta, recorded.unwrap_or(EntryKind::File), caps) {
        EntryKind::Executable
    } else {
        EntryKind::File
    };
    Ok(FolderEntry::Entry(FlatEntry { kind, hash }))
}

/// A cheap fingerprint of what the folder holds at `paths` below `root`, from their metadata alone, read one directory handle at a time with no symlink followed and no file opened: it changes when one of them is written, removed, replaced or swapped for another kind, so a watch runs a follow only then. Blocking.
pub fn folder_stamp(root: &Path, paths: &[RepoPath]) -> Vec<u8> {
    let root = DirHandle::open(root).ok();
    let mut stamp = Vec::new();
    for path in paths {
        stamp.extend_from_slice(path.as_bytes());
        stamp.push(0);
        match root.as_ref().map(|root| entry_metadata(root, path)) {
            Some(Ok(Some(meta))) => {
                for value in [
                    u64::from(meta.mode()),
                    meta.len(),
                    meta.ino(),
                    meta.mtime().cast_unsigned(),
                    meta.mtime_nsec().cast_unsigned(),
                    meta.ctime().cast_unsigned(),
                    meta.ctime_nsec().cast_unsigned(),
                ] {
                    stamp.extend_from_slice(&value.to_le_bytes());
                }
            }
            Some(Ok(None)) => stamp.push(1),
            Some(Err(error)) if error.kind() == io::ErrorKind::NotFound => stamp.push(2),
            _ => stamp.push(3),
        }
    }
    stamp
}

/// The metadata of `path` itself below `root`; `None` when a parent is not a directory.
fn entry_metadata(root: &DirHandle, path: &RepoPath) -> io::Result<Option<std::fs::Metadata>> {
    let (parents, name) = path.parents_and_name();
    let mut dir = root.try_clone()?;
    for parent in parents {
        match dir.child(parent)? {
            Some(child) => dir = child,
            None => return Ok(None),
        }
    }
    dir.metadata(name).map(Some)
}

/// What one follow of main did in the project folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Followed {
    /// The checkout record after the follow: for every path, the entry fold wrote there or knows the folder holds.
    pub record: FlatTree,
    /// The paths fold wrote or removed.
    pub written: Vec<RepoPath>,
    /// The paths where main and the record differ and the folder holds neither, which a human changed and did not record. While there is one, fold writes nothing in the folder. In path order.
    pub held: Vec<RepoPath>,
}

/// The paths of `changes` where the folder holds neither the entry of `record` nor that of `main`: a human changed them, and no revision records the change. In the order of `changes`.
pub fn held(
    record: &FlatTree,
    main: &FlatTree,
    changes: &[(RepoPath, FolderEntry)],
) -> Vec<RepoPath> {
    changes
        .iter()
        .filter(|(path, found)| {
            let key = path.as_bytes();
            !found.holds(record.get(key)) && !found.holds(main.get(key))
        })
        .map(|(path, _)| path.clone())
        .collect()
}

/// The paths where `record` and `main` differ, each with what the folder holds there now, in path order. Blocking.
pub fn folder_changes(
    root: &Path,
    record: &FlatTree,
    main: &FlatTree,
    max_bytes: u64,
    caps: &Capabilities,
) -> Result<Vec<(RepoPath, FolderEntry)>, WorkspaceError> {
    let mut paths: Vec<&Vec<u8>> = record
        .keys()
        .chain(main.keys())
        .filter(|path| record.get(*path) != main.get(*path))
        .collect();
    paths.sort();
    paths.dedup();
    let mut found: Vec<(RepoPath, FolderEntry)> = paths
        .into_iter()
        .map(|path| {
            let repo = RepoPath::parse(path)?;
            let recorded = record.get(path).or_else(|| main.get(path)).map(|e| e.kind);
            let found = read_folder_entry(root, &repo, max_bytes, caps, recorded)?;
            Ok((repo, found))
        })
        .collect::<Result<_, WorkspaceError>>()?;
    // A file of fold's that main removes stands where a directory of main's goes, so what goes below it counts as missing.
    let removed: Vec<Vec<u8>> = found
        .iter()
        .filter(|(path, entry)| {
            let key = path.as_bytes();
            main.get(key).is_none() && record.get(key).is_some() && entry.holds(record.get(key))
        })
        .map(|(path, _)| path.as_bytes().to_vec())
        .collect();
    for (path, entry) in &mut found {
        let below_removed = removed.iter().any(|parent| {
            let bytes = path.as_bytes();
            bytes.len() > parent.len() && bytes.starts_with(parent) && bytes[parent.len()] == b'/'
        });
        if *entry == FolderEntry::Other && below_removed {
            *entry = FolderEntry::Missing;
        }
    }
    // A directory that holds only fold's files that main removes stands where a file of main's goes, so the file's place counts as free once they are gone.
    for (path, entry) in &mut found {
        let key = path.as_bytes();
        if *entry == FolderEntry::Other
            && record.get(key).is_none()
            && main.get(key).is_some()
            && holds_only_removed(root, path, &removed)?
        {
            *entry = FolderEntry::Missing;
        }
    }
    Ok(found)
}

/// Whether `path` below `root` is a directory, reached with no symlink followed, that holds nothing but files of `removed` and directories that hold only those. Blocking.
fn holds_only_removed(
    root: &Path,
    path: &RepoPath,
    removed: &[Vec<u8>],
) -> Result<bool, WorkspaceError> {
    let mut dir = DirHandle::open(root)?;
    let (parents, name) = path.parents_and_name();
    for step in parents.into_iter().chain([name]) {
        dir = match dir.child(step) {
            Ok(Some(child)) => child,
            Ok(None) => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
    }
    dir_holds_only(&dir, path.as_bytes(), removed)
}

fn dir_holds_only(
    dir: &DirHandle,
    prefix: &[u8],
    removed: &[Vec<u8>],
) -> Result<bool, WorkspaceError> {
    for name in dir.names()? {
        let mut key = prefix.to_vec();
        key.push(b'/');
        key.extend_from_slice(&name);
        let holds = match dir.child(&name) {
            Ok(Some(child)) => dir_holds_only(&child, &key, removed)?,
            Ok(None) => removed.contains(&key),
            Err(error) if error.kind() == io::ErrorKind::NotFound => true,
            Err(error) => return Err(error.into()),
        };
        if !holds {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Brings the project folder at `root` from `record`, the entries fold last wrote there, to `main`, for every path where the two differ, all at once or not at all. When one of those paths holds a change of the human's (see [`held`]), nothing is written: the folder stays on the record until the human records the change or takes it back, as `git pull` refuses to overwrite local changes. Otherwise a path that still holds the record's entry gets main's, written through a staging file and renamed into place, or removed with the directories it leaves empty. Either way a path that holds main's entry already only moves the record. Paths where the record and main agree are never read or written. `changes` is what [`folder_changes`] read. Removals come before writes, so a directory main puts where a file was, or a file main puts where a directory of fold's files was, finds its place free. The written entries are on disk when this returns. A stop part-way leaves files that hold main's entries with the old record, which the next follow takes as written. Blocking.
pub fn follow(
    objects: &ObjectStore,
    root: &Path,
    record: &FlatTree,
    main: &FlatTree,
    changes: &[(RepoPath, FolderEntry)],
    caps: Capabilities,
) -> Result<Followed, WorkspaceError> {
    let mut followed = hold(record, main, changes);
    if !followed.held.is_empty() {
        return Ok(followed);
    }
    let mut removals = Vec::new();
    let mut writes = Vec::new();
    for (path, found) in changes {
        let key = path.as_bytes();
        if found.holds(main.get(key)) {
            continue;
        }
        match main.get(key) {
            Some(entry) => writes.push((path, *entry)),
            None => removals.push(path),
        }
    }
    if removals.is_empty() && writes.is_empty() {
        return Ok(followed);
    }
    let mut batch = Batch::open(root)?;
    let opts = MaterialiseOptions {
        read_only: false,
        caps,
    };
    for path in removals {
        batch.remove_entry(path)?;
        followed.record.remove(path.as_bytes());
        followed.written.push(path.clone());
    }
    for (path, entry) in writes {
        // A directory that the removals emptied may still stand where the file goes.
        batch.remove_empty_dirs(path)?;
        batch.write_entry(objects, path, &entry, &opts)?;
        followed.record.insert(path.as_bytes().to_vec(), entry);
        followed.written.push(path.clone());
    }
    batch.finish()?;
    followed.written.sort();
    Ok(followed)
}

/// Brings each path of `changes` that still holds the entry of `from` to that of `to`, as [`follow`] writes it, and leaves every other path as it is: one that holds the entry of `to` already, and one that holds a change of the human's. Returns the paths it wrote or removed, in path order. Blocking.
pub fn follow_unchanged(
    objects: &ObjectStore,
    root: &Path,
    from: &FlatTree,
    to: &FlatTree,
    changes: &[(RepoPath, FolderEntry)],
    caps: Capabilities,
) -> Result<Vec<RepoPath>, WorkspaceError> {
    let unchanged: Vec<(RepoPath, FolderEntry)> = changes
        .iter()
        .filter(|(path, found)| found.holds(from.get(path.as_bytes())))
        .cloned()
        .collect();
    Ok(follow(objects, root, from, to, &unchanged, caps)?.written)
}

/// What a follow knows before it writes: `record` with main's entry for every path of `changes` that holds it already, and the [`held`] paths. Reads no file.
fn hold(record: &FlatTree, main: &FlatTree, changes: &[(RepoPath, FolderEntry)]) -> Followed {
    let mut followed = Followed {
        record: record.clone(),
        written: Vec::new(),
        held: held(record, main, changes),
    };
    for (path, found) in changes {
        let key = path.as_bytes();
        if found.holds(main.get(key)) {
            settle(&mut followed.record, key, main.get(key));
        }
    }
    followed
}

fn settle(record: &mut FlatTree, key: &[u8], entry: Option<&FlatEntry>) {
    match entry {
        Some(entry) => {
            record.insert(key.to_vec(), *entry);
        }
        None => {
            record.remove(key);
        }
    }
}

/// `record` with the entries of a human change from `base` to `tree` laid over it: fold knows the folder holds the human's entries for the paths the change made, once it is recorded. Paths the change left alone keep their entry.
pub fn overlay(record: &FlatTree, base: &FlatTree, tree: &FlatTree) -> FlatTree {
    let mut laid = record.clone();
    for path in base.keys().chain(tree.keys()) {
        let new = tree.get(path);
        if base.get(path) != new {
            settle(&mut laid, path, new);
        }
    }
    laid
}
