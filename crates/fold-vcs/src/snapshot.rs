use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::ops::Bound;
use std::path::Path;

use fold_core::FoldError;
use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::tree::{EntryKind, FlatEntry, FlatTree, Tree, apply_changes, flatten};

use crate::index::StatIndex;
use crate::objects::ObjectStore;
use crate::paths::{RepoPath, WorkspaceError, os_bytes, resolve};
use crate::scan::{ChangeKind, ScanOptions, ScanResult, SkipReason, changes, open_regular, scan};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningReason {
    Skipped(SkipReason),
    /// The file kept changing while it was read.
    Unstable,
    /// The file changed between the scan and the moment its content was stored.
    ChangedDuringSnapshot,
}

/// A path the snapshot could not record. The snapshot holds what the base tree has at and below this path, or nothing when the base had nothing there, so an unreadable directory keeps every file it had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub path: RepoPath,
    pub reason: WarningReason,
}

#[derive(Debug)]
pub struct Snapshot {
    pub tree: ObjectHash,
    pub entries: FlatTree,
    pub warnings: Vec<Warning>,
}

/// Records the workspace as a tree. It reads the workspace and writes only objects and the stat index file at `index_path`; no workspace file is touched. Every blob is stored before any tree that names it, and the index is saved last.
pub fn snapshot(
    objects: &ObjectStore,
    root: &Path,
    index: &mut StatIndex,
    index_path: &Path,
    base_tree: Option<ObjectHash>,
    opts: &ScanOptions<'_>,
) -> Result<Snapshot, FoldError> {
    let base = match &base_tree {
        Some(tree) => flatten(tree, &|hash| load_tree(objects, hash))?,
        None => FlatTree::new(),
    };
    let opts = ScanOptions {
        recorded: Some(&base),
        ..*opts
    };
    let scanned = scan(root, index, &opts)?;
    record(objects, root, index, index_path, base_tree, &base, scanned)
}

/// The second half of [`snapshot`], for a scan the caller already made. `base` must be the flattened `base_tree`.
pub fn record(
    objects: &ObjectStore,
    root: &Path,
    index: &mut StatIndex,
    index_path: &Path,
    base_tree: Option<ObjectHash>,
    base: &FlatTree,
    scanned: ScanResult,
) -> Result<Snapshot, FoldError> {
    let mut warnings = Vec::new();
    let mut current = scanned.entries;
    let unrecorded = scanned
        .skipped
        .into_iter()
        .map(|skipped| (skipped.path, WarningReason::Skipped(skipped.reason)))
        .chain(
            scanned
                .unstable
                .into_iter()
                .map(|path| (path, WarningReason::Unstable)),
        );
    for (path, reason) in unrecorded {
        keep_base(&mut current, base, &path);
        warnings.push(Warning { path, reason });
    }

    for change in changes(base, &current) {
        if change.kind == ChangeKind::Deleted {
            continue;
        }
        let Some(entry) = current.get(&change.path).copied() else {
            continue;
        };
        let path = RepoPath::parse(&change.path)?;
        let size = index.lookup(&path).map(|stat| stat.size);
        if !store_blob(objects, root, &path, &entry, size)? {
            index.remove(&path);
            keep_base(&mut current, base, &path);
            warnings.push(Warning {
                path,
                reason: WarningReason::ChangedDuringSnapshot,
            });
        }
    }
    // Taken again after the failed stores, which can bring back a base directory whose files the first pass saw deleted.
    let pending: Changes = changes(base, &current)
        .into_iter()
        .map(|change| {
            let entry = current.get(&change.path).copied();
            (change.path, entry)
        })
        .collect();

    let tree = match base_tree.filter(|_| pending.is_empty()) {
        Some(unchanged) => unchanged,
        None => build_tree(objects, base_tree, &pending)?,
    };
    index.save(index_path).map_err(WorkspaceError::from)?;
    Ok(Snapshot {
        tree,
        entries: current,
        warnings,
    })
}

/// Applies the changes to the base tree and stores the new trees, children before parents.
fn build_tree(
    objects: &ObjectStore,
    base_tree: Option<ObjectHash>,
    pending: &Changes,
) -> Result<ObjectHash, FoldError> {
    let built = apply_changes(base_tree, pending, &|hash| load_tree(objects, hash))?;
    store_trees(objects, &built.new_trees)?;
    Ok(built.root)
}

type Changes = BTreeMap<Vec<u8>, Option<FlatEntry>>;

fn store_trees(
    objects: &ObjectStore,
    trees: &[(ObjectHash, Vec<u8>)],
) -> Result<(), WorkspaceError> {
    for (hash, payload) in trees {
        let stored = objects.write(ObjectKind::Tree, payload)?;
        debug_assert_eq!(&stored, hash);
    }
    Ok(())
}

/// Replaces everything `current` has at and below `path` with what `base` has there.
fn keep_base(current: &mut FlatTree, base: &FlatTree, path: &RepoPath) {
    let stale: Vec<Vec<u8>> = at_or_below(current, path)
        .map(|(found, _)| found.clone())
        .collect();
    for found in stale {
        current.remove(&found);
    }
    for (found, entry) in at_or_below(base, path) {
        current.insert(found.clone(), *entry);
    }
}

fn at_or_below<'t>(
    tree: &'t FlatTree,
    path: &RepoPath,
) -> impl Iterator<Item = (&'t Vec<u8>, &'t FlatEntry)> {
    let path = path.as_bytes().to_vec();
    tree.range::<[u8], _>((Bound::Included(path.as_slice()), Bound::Unbounded))
        .take_while(move |(found, _)| {
            found
                .strip_prefix(path.as_slice())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"))
        })
}

/// Stores the blob of one entry unless the object store already holds it. False when the file no longer has the hash the scan found. A file is read only when it is still a regular file of the size the scan recorded, and never past that size.
fn store_blob(
    objects: &ObjectStore,
    root: &Path,
    path: &RepoPath,
    entry: &FlatEntry,
    size: Option<u64>,
) -> Result<bool, WorkspaceError> {
    if objects.refresh(&entry.hash)? {
        return Ok(true);
    }
    let located = resolve(root, path)?;
    let written = if entry.kind == EntryKind::Symlink {
        let target = match fs::read_link(&located) {
            Ok(target) => target,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let Some(bytes) = os_bytes(target.as_os_str()) else {
            return Ok(false);
        };
        objects.write(ObjectKind::Blob, &bytes)?
    } else {
        let (file, meta) = match open_regular(&located) {
            Ok(Some(opened)) => opened,
            Ok(None) => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if size.is_some_and(|size| size != meta.len()) {
            return Ok(false);
        }
        objects.write_blob_from(&mut file.take(meta.len()))?.hash
    };
    Ok(written == entry.hash)
}

pub(crate) fn load_tree(objects: &ObjectStore, hash: &ObjectHash) -> Result<Tree, FoldError> {
    let object = objects
        .read_expecting(hash, ObjectKind::Tree)
        .map_err(FoldError::from)?;
    Ok(Tree::decode(&object.payload)?)
}

#[cfg(test)]
mod tests {
    use crate::ignore::IgnoreRules;
    use crate::platform::Capabilities;
    use crate::scan::{DEFAULT_MAX_FILE_BYTES, with_rewrites_during_reads};

    use super::*;

    const HOST: Capabilities = Capabilities {
        symlinks: true,
        exec_bit: true,
        inodes: true,
        read_only_bit: true,
    };

    #[test]
    fn an_unstable_file_keeps_its_base_entry_and_adds_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let objects = ObjectStore::open(&dir.path().join(".fold")).unwrap();
        let root = dir.path().join("ws");
        let index_path = dir.path().join(".fold/ws-index");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("moving"), b"small").unwrap();
        fs::write(root.join("steady"), b"s").unwrap();
        let rules = IgnoreRules::new(&root);
        let opts = ScanOptions {
            rules: &rules,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            caps: HOST,
            recorded: None,
        };
        let mut index = StatIndex::empty(HOST);
        let first = snapshot(&objects, &root, &mut index, &index_path, None, &opts).unwrap();

        fs::write(root.join("moving"), b"rewritten").unwrap();
        fs::write(root.join("steady"), b"edited").unwrap();
        let next = with_rewrites_during_reads("moving", || {
            snapshot(
                &objects,
                &root,
                &mut index,
                &index_path,
                Some(first.tree),
                &opts,
            )
        })
        .unwrap();
        assert_eq!(next.warnings.len(), 1);
        assert_eq!(next.warnings[0].path.to_string(), "moving");
        assert_eq!(next.warnings[0].reason, WarningReason::Unstable);
        assert_eq!(
            next.entries[b"moving".as_slice()],
            first.entries[b"moving".as_slice()]
        );
        assert_ne!(
            next.entries[b"steady".as_slice()],
            first.entries[b"steady".as_slice()]
        );
    }
}
