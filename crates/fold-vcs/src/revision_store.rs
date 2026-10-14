use std::collections::BTreeMap;

use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::revision::{Revision, StoredRevision};
use fold_core::tree::{FlatEntry, FlatTree, Tree, apply_changes};
use fold_core::{FoldError, codes};

use crate::objects::{ObjectError, ObjectStore};

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

/// Refuses a revision whose object the store does not hold. A revision row is inserted only with the result of this or of [`write_revision`].
pub fn require_stored(
    objects: &ObjectStore,
    revision: &Revision,
) -> Result<StoredRevision, ObjectError> {
    let hash = revision.hash();
    if objects.contains(&hash)? {
        Ok(StoredRevision::confirmed(*revision))
    } else {
        Err(ObjectError::Missing(hash))
    }
}

/// Writes the trees of `entries` bottom-up and returns the root. The caller has stored every blob. No entry is added: an empty `entries` gives the empty tree.
pub fn write_initial_tree(
    objects: &ObjectStore,
    entries: FlatTree,
) -> Result<ObjectHash, FoldError> {
    let changes: BTreeMap<Vec<u8>, Option<FlatEntry>> = entries
        .into_iter()
        .map(|(path, entry)| (path, Some(entry)))
        .collect();
    let load = |hash: &ObjectHash| -> Result<Tree, FoldError> {
        let object = objects.read_expecting(hash, ObjectKind::Tree)?;
        Tree::decode(&object.payload)
            .map_err(|error| FoldError::new(codes::TREE_INVALID, error.to_string()))
    };
    let build = apply_changes(None, &changes, &load)?;
    // Children come before the trees that name them.
    for (_, payload) in &build.new_trees {
        objects.write(ObjectKind::Tree, payload)?;
    }
    Ok(build.root)
}
