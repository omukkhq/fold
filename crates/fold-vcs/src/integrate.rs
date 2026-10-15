use fold_core::merge::{Conflict, merge_trees};
use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::tree::flatten;
use fold_core::{FoldError, codes};

use crate::objects::{ObjectError, ObjectStore};
use crate::revision_store::write_initial_tree;
use crate::snapshot::load_tree;

/// The result of merging a proposal version or a remote tree onto main.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// Every object of the tree is in the store, written or refreshed by this call.
    Merged { tree_hash: ObjectHash },
    /// In path order. Nothing was written.
    Conflicted(Vec<Conflict>),
}

/// Merges `other` onto `head` against their common `base`, all three root trees in `objects`. When `base` is `head`, main has not moved and the result is `other`, refreshed so garbage collection keeps it until the revision that names it commits. Otherwise the tree merge runs over the stored trees and blobs; a clean merge writes its merged blobs and then its trees bottom-up, and a conflicted one writes nothing. Blocking: call it through `spawn_blocking`.
pub fn merge_onto_head(
    objects: &ObjectStore,
    base: &ObjectHash,
    head: &ObjectHash,
    other: &ObjectHash,
) -> Result<MergeOutcome, FoldError> {
    if base == head {
        if !objects.refresh(other)? {
            return Err(ObjectError::Missing(*other).into());
        }
        return Ok(MergeOutcome::Merged { tree_hash: *other });
    }
    let load = |hash: &ObjectHash| load_tree(objects, hash);
    let base = flatten(base, &load)?;
    let head = flatten(head, &load)?;
    let other = flatten(other, &load)?;
    let blob = |hash: &ObjectHash| -> Result<Vec<u8>, FoldError> {
        Ok(objects.read_expecting(hash, ObjectKind::Blob)?.payload)
    };
    let merge = merge_trees(&base, &other, &head, &blob)?;
    if !merge.conflicts.is_empty() {
        return Ok(MergeOutcome::Conflicted(merge.conflicts));
    }
    for (hash, bytes) in &merge.new_blobs {
        let written = objects.write(ObjectKind::Blob, bytes)?;
        if written != *hash {
            return Err(FoldError::new(
                codes::OBJECT_CORRUPT,
                format!("the merged blob {hash} was stored as {written}"),
            ));
        }
    }
    let tree_hash = write_initial_tree(objects, merge.merged)?;
    Ok(MergeOutcome::Merged { tree_hash })
}
