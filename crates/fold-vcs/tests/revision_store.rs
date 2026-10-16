use fold_core::object::{ObjectHash, ObjectKind, hash_object};
use fold_core::revision::Revision;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree, Tree, apply_changes};
use fold_core::{RevisionId, Timestamp, codes};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{read_revision, write_initial_tree, write_revision};

#[cfg(test)]
mod support {
    use super::*;

    pub fn store() -> (tempfile::TempDir, ObjectStore) {
        let dir = tempfile::tempdir().unwrap();
        let objects = ObjectStore::open(dir.path()).unwrap();
        (dir, objects)
    }

    pub fn revision(tree: ObjectHash) -> Revision {
        Revision {
            tree,
            parent: None,
            number: RevisionId::from_number(1).unwrap(),
            integration: None,
            created_at: Timestamp::from_millis(1_700_000_000_000).unwrap(),
        }
    }

    pub fn blob(objects: &ObjectStore, content: &[u8]) -> FlatEntry {
        FlatEntry {
            kind: EntryKind::File,
            hash: objects.write(ObjectKind::Blob, content).unwrap(),
        }
    }

    pub fn empty_tree_hash() -> ObjectHash {
        hash_object(ObjectKind::Tree, &Tree::default().encode().unwrap())
    }
}
use support::*;

#[test]
fn a_revision_is_written_read_back_and_rehashed() {
    let (_dir, objects) = store();
    let original = revision(empty_tree_hash());
    let stored = write_revision(&objects, &original).unwrap();
    assert_eq!(stored.revision(), &original);
    let hash = stored.hash();
    assert_eq!(hash, original.hash());
    assert!(objects.contains(&hash).unwrap());
    let read = read_revision(&objects, &hash).unwrap();
    assert_eq!(read, original);
    assert_eq!(read.hash(), hash);
}

#[test]
fn writing_a_stored_revision_again_succeeds() {
    let (_dir, objects) = store();
    let original = revision(empty_tree_hash());
    let first = write_revision(&objects, &original).unwrap();
    let second = write_revision(&objects, &original).unwrap();
    assert_eq!(first, second);
}

#[test]
fn reading_a_revision_refuses_another_kind() {
    let (_dir, objects) = store();
    let tree = objects
        .write(ObjectKind::Tree, &Tree::default().encode().unwrap())
        .unwrap();
    assert!(read_revision(&objects, &tree).is_err());
    let missing = read_revision(&objects, &revision(tree).hash()).unwrap_err();
    assert_eq!(missing.code(), codes::OBJECT_MISSING);
}

#[test]
fn a_corrupt_revision_payload_is_refused() {
    let (_dir, objects) = store();
    let hash = objects.write(ObjectKind::Revision, b"short").unwrap();
    let error = read_revision(&objects, &hash).unwrap_err();
    assert_eq!(error.code(), codes::REVISION_INVALID);
}

#[test]
fn no_entries_give_the_empty_tree() {
    let (_dir, objects) = store();
    let root = write_initial_tree(&objects, FlatTree::new()).unwrap();
    assert_eq!(root, empty_tree_hash());
    assert!(objects.contains(&root).unwrap());
    assert_eq!(objects.entries().unwrap().len(), 1, "no file is added");
}

#[test]
fn entries_give_the_root_that_apply_changes_gives() {
    let (_dir, objects) = store();
    let mut entries = FlatTree::new();
    entries.insert(b"README.md".to_vec(), blob(&objects, b"hello"));
    entries.insert(b"src/lib.rs".to_vec(), blob(&objects, b"fn main() {}"));
    entries.insert(b"src/deep/mod.rs".to_vec(), blob(&objects, b"mod x;"));
    let blobs = objects.entries().unwrap().len();

    let root = write_initial_tree(&objects, entries.clone()).unwrap();

    let changes = entries
        .iter()
        .map(|(path, entry)| (path.clone(), Some(*entry)))
        .collect();
    let load = |_: &ObjectHash| -> Result<Tree, fold_core::FoldError> { Ok(Tree::default()) };
    let expected = apply_changes(None, &changes, &load).unwrap();
    assert_eq!(root, expected.root);
    // Three trees (root, src, src/deep) were added to the three blobs, and nothing else.
    assert_eq!(
        objects.entries().unwrap().len(),
        blobs + expected.new_trees.len()
    );
    assert_eq!(expected.new_trees.len(), 3);
    for (hash, _) in &expected.new_trees {
        assert!(objects.contains(hash).unwrap());
    }
}
