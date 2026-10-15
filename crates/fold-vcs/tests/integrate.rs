#![cfg(test)]

use fold_core::merge::ConflictKind;
use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::tree::{EntryKind, FlatEntry, FlatTree, Tree, flatten};
use fold_vcs::integrate::{MergeOutcome, merge_onto_head};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::write_initial_tree;

fn store() -> (tempfile::TempDir, ObjectStore) {
    let dir = tempfile::tempdir().unwrap();
    let objects = ObjectStore::open(dir.path()).unwrap();
    (dir, objects)
}

fn tree(objects: &ObjectStore, files: &[(&str, &str)]) -> ObjectHash {
    let mut flat = FlatTree::new();
    for (path, content) in files {
        let hash = objects.write(ObjectKind::Blob, content.as_bytes()).unwrap();
        flat.insert(
            path.as_bytes().to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash,
            },
        );
    }
    write_initial_tree(objects, flat).unwrap()
}

fn files(objects: &ObjectStore, root: &ObjectHash) -> Vec<(String, String)> {
    let load = |hash: &ObjectHash| -> Result<Tree, fold_core::FoldError> {
        let object = objects.read_expecting(hash, ObjectKind::Tree)?;
        Ok(Tree::decode(&object.payload)?)
    };
    flatten(root, &load)
        .unwrap()
        .into_iter()
        .map(|(path, entry)| {
            let content = objects
                .read_expecting(&entry.hash, ObjectKind::Blob)
                .unwrap()
                .payload;
            (
                String::from_utf8(path).unwrap(),
                String::from_utf8(content).unwrap(),
            )
        })
        .collect()
}

fn object_count(objects: &ObjectStore) -> usize {
    objects.entries().unwrap().len()
}

#[test]
fn fast_forward_uses_the_other_tree() {
    let (_dir, objects) = store();
    let base = tree(&objects, &[("a.txt", "one\n")]);
    let other = tree(&objects, &[("a.txt", "two\n"), ("b.txt", "new\n")]);
    let before = object_count(&objects);

    let outcome = merge_onto_head(&objects, &base, &base, &other).unwrap();

    assert_eq!(outcome, MergeOutcome::Merged { tree_hash: other });
    assert_eq!(object_count(&objects), before);
}

#[test]
fn fast_forward_refuses_an_other_tree_the_store_lacks() {
    let (_dir, objects) = store();
    let base = tree(&objects, &[("a.txt", "one\n")]);
    let missing = ObjectHash::from_bytes([7; 32]);

    let error = merge_onto_head(&objects, &base, &base, &missing).unwrap_err();

    assert_eq!(error.code(), fold_core::codes::OBJECT_MISSING);
}

#[test]
fn non_overlapping_edits_merge_and_write_objects_first() {
    let (_dir, objects) = store();
    let base = tree(
        &objects,
        &[("a.txt", "1\n2\n3\n4\n5\n6\n7\n"), ("keep.txt", "k\n")],
    );
    let head = tree(
        &objects,
        &[
            ("a.txt", "1 main\n2\n3\n4\n5\n6\n7\n"),
            ("keep.txt", "k\n"),
            ("main.txt", "m\n"),
        ],
    );
    let other = tree(
        &objects,
        &[
            ("a.txt", "1\n2\n3\n4\n5\n6\n7 proposal\n"),
            ("keep.txt", "k\n"),
            ("proposal.txt", "p\n"),
        ],
    );

    let MergeOutcome::Merged { tree_hash } =
        merge_onto_head(&objects, &base, &head, &other).unwrap()
    else {
        panic!("expected a clean merge");
    };

    assert!(objects.contains(&tree_hash).unwrap());
    assert_eq!(
        files(&objects, &tree_hash),
        vec![
            ("a.txt".into(), "1 main\n2\n3\n4\n5\n6\n7 proposal\n".into()),
            ("keep.txt".into(), "k\n".into()),
            ("main.txt".into(), "m\n".into()),
            ("proposal.txt".into(), "p\n".into()),
        ]
    );
}

#[test]
fn conflicting_edits_return_conflicted_and_write_no_revision() {
    let (_dir, objects) = store();
    let base = tree(&objects, &[("a.txt", "one\n")]);
    let head = tree(&objects, &[("a.txt", "main\n")]);
    let other = tree(&objects, &[("a.txt", "proposal\n")]);
    let before = object_count(&objects);

    let outcome = merge_onto_head(&objects, &base, &head, &other).unwrap();

    let MergeOutcome::Conflicted(conflicts) = outcome else {
        panic!("expected a conflict");
    };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, b"a.txt");
    assert_eq!(conflicts[0].kind, ConflictKind::BothModifiedText);
    assert_eq!(object_count(&objects), before);
    assert!(
        objects
            .entries()
            .unwrap()
            .iter()
            .all(|entry| objects.read(&entry.hash).unwrap().kind != ObjectKind::Revision)
    );
}
