use std::collections::BTreeMap;

use fold_core::FoldError;
use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::revision::{Revision, StoredRevision};
use fold_core::tree::{FlatEntry, FlatTree, TreeBuild, apply_changes};

use crate::objects::{ObjectError, ObjectStore};
use crate::snapshot::load_tree;

/// Stores `revision` flushed. The result carries the hash the revision row records. Storing the same revision again succeeds and changes nothing.
pub fn write_revision(
    objects: &ObjectStore,
    revision: &Revision,
) -> Result<StoredRevision, ObjectError> {
    objects.write(ObjectKind::Revision, &revision.encode())?;
    Ok(StoredRevision::confirmed(*revision))
}

pub fn read_revision(objects: &ObjectStore, hash: &ObjectHash) -> Result<Revision, FoldError> {
    let object = objects.read_expecting(hash, ObjectKind::Revision)?;
    Ok(Revision::decode(&object.payload)?)
}

/// Writes the trees of `entries` bottom-up and returns the root. The caller has stored every blob. No entry is added: an empty `entries` gives the empty tree.
pub fn write_initial_tree(
    objects: &ObjectStore,
    entries: FlatTree,
) -> Result<ObjectHash, FoldError> {
    let build = initial_build(objects, entries)?;
    // Children come before the trees that name them.
    for (_, payload) in &build.new_trees {
        objects.write(ObjectKind::Tree, payload)?;
    }
    Ok(build.root)
}

/// The root [`write_initial_tree`] gives `entries`, built in memory: nothing is written.
pub fn initial_tree_hash(
    objects: &ObjectStore,
    entries: FlatTree,
) -> Result<ObjectHash, FoldError> {
    Ok(initial_build(objects, entries)?.root)
}

fn initial_build(objects: &ObjectStore, entries: FlatTree) -> Result<TreeBuild, FoldError> {
    let changes: BTreeMap<Vec<u8>, Option<FlatEntry>> = entries
        .into_iter()
        .map(|(path, entry)| (path, Some(entry)))
        .collect();
    apply_changes(None, &changes, &|hash| load_tree(objects, hash))
}
