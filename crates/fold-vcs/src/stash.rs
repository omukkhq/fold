use std::path::Path;

use fold_core::FoldError;
use fold_core::merge::{ConflictKind, TreeMerge, contains_stash_marker_line};
use fold_core::object::ObjectKind;
use fold_core::tree::{EntryKind, FlatTree};

use crate::checkout::{FolderEntry, read_folder_entry};
use crate::objects::ObjectStore;
use crate::paths::RepoPath;
use crate::platform::Capabilities;
use crate::sync::{STASH_SIDECAR_SUFFIX, sidecar_with};

/// The paths a change from `base` to `tree` makes, changed, added or removed, in path byte order.
pub fn changed_paths(base: &FlatTree, tree: &FlatTree) -> Vec<Vec<u8>> {
    let mut paths: Vec<Vec<u8>> = base
        .keys()
        .chain(tree.keys())
        .filter(|path| base.get(*path) != tree.get(*path))
        .cloned()
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// The paths of `changed` where `folder`, a tree read from the project folder, still holds a conflict of an unstash: a file with a line that starts a stash marker, or the `.fold-stash` file or directory beside it. In the order given. Blocking.
pub fn open_conflicts(
    objects: &ObjectStore,
    folder: &FlatTree,
    changed: &[Vec<u8>],
) -> Result<Vec<RepoPath>, FoldError> {
    let mut open = Vec::new();
    for path in changed {
        let mut side = path.clone();
        side.extend_from_slice(STASH_SIDECAR_SUFFIX);
        let mut below = side.clone();
        below.push(b'/');
        let sidecar = folder.contains_key(&side)
            || folder
                .range(below.clone()..)
                .next()
                .is_some_and(|(found, _)| found.starts_with(&below));
        let marked = match folder.get(path) {
            Some(entry) if entry.kind != EntryKind::Symlink => {
                let blob = objects.read_expecting(&entry.hash, ObjectKind::Blob)?;
                contains_stash_marker_line(&String::from_utf8_lossy(&blob.payload))
            }
            _ => false,
        };
        if sidecar || marked {
            open.push(RepoPath::parse(path)?);
        }
    }
    Ok(open)
}

/// The paths where an unstash of `merge` into `folder`, a tree read from the project folder at `root`, would write a file or a `.fold-stash` side, and the folder holds something the read left out there, such as a file an ignore rule or the size limit keeps out. A path the merge frees by removing a file or directory of `folder` is not one. In path order. Blocking.
pub fn occupied_paths(
    root: &Path,
    merge: &TreeMerge,
    folder: &FlatTree,
    max_bytes: u64,
    caps: &Capabilities,
) -> Result<Vec<RepoPath>, FoldError> {
    let mut targets: Vec<RepoPath> = Vec::new();
    for path in merge.merged.keys() {
        if !folder.contains_key(path) {
            targets.push(RepoPath::parse(path)?);
        }
    }
    for conflict in &merge.conflicts {
        if conflict.kind != ConflictKind::BothModifiedText
            && (conflict.theirs.is_some() || !conflict.theirs_below.is_empty())
        {
            targets.push(sidecar_with(
                &RepoPath::parse(&conflict.path)?,
                STASH_SIDECAR_SUFFIX,
            )?);
        }
    }
    let mut occupied = Vec::new();
    for path in targets {
        let key = path.as_bytes();
        let mut below = key.to_vec();
        below.push(b'/');
        let freed = folder
            .range(below.clone()..)
            .next()
            .is_some_and(|(found, _)| found.starts_with(&below))
            || key
                .iter()
                .enumerate()
                .any(|(at, byte)| *byte == b'/' && folder.contains_key(&key[..at]));
        if freed {
            continue;
        }
        if read_folder_entry(root, &path, max_bytes, caps, None)? != FolderEntry::Missing {
            occupied.push(path);
        }
    }
    occupied.sort();
    occupied.dedup();
    Ok(occupied)
}
