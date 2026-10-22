use fold_core::object::ObjectHash;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_vcs::batch::{BatchMember, BatchMerge, merge_batch, merged_batch_state};
use fold_vcs::compare::{ChangeKind, State, compare, file_diff, load_flat};
use fold_vcs::diff::LineKind;
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::RepoPath;
use fold_vcs::revision_store::write_initial_tree;

use crate::common::{blob, store};

#[cfg(test)]
fn tree_of(objects: &ObjectStore, files: &[(&str, &str)]) -> ObjectHash {
    let flat: FlatTree = files
        .iter()
        .map(|(path, text)| {
            let entry = FlatEntry {
                kind: EntryKind::File,
                hash: blob(objects, text.as_bytes()),
            };
            (path.as_bytes().to_vec(), entry)
        })
        .collect();
    write_initial_tree(objects, flat).unwrap()
}

const MAIN: &str = "1\n2\n3\n4\n5\n6\n7\n8\n";

/// main's tree and three members on it: the first changes line 1 of `a.txt` and adds `b.txt`, the second changes line 8, and the third changes line 1 as well.
#[cfg(test)]
fn scene(objects: &ObjectStore) -> (ObjectHash, [BatchMember; 3]) {
    let main = tree_of(objects, &[("a.txt", MAIN)]);
    let member = |files: &[(&str, &str)]| BatchMember {
        base: main,
        tree: tree_of(objects, files),
    };
    let first = member(&[("a.txt", "one\n2\n3\n4\n5\n6\n7\n8\n"), ("b.txt", "new\n")]);
    let last_line = member(&[("a.txt", "1\n2\n3\n4\n5\n6\n7\neight\n")]);
    let same_line = member(&[("a.txt", "uno\n2\n3\n4\n5\n6\n7\n8\n")]);
    (main, [first, last_line, same_line])
}

#[cfg(test)]
fn path(text: &str) -> RepoPath {
    RepoPath::parse(text.as_bytes()).unwrap()
}

#[test]
fn the_merged_state_is_the_tree_the_batch_merge_stores_without_storing_it() {
    let (_dir, objects) = store();
    let (main, [first, last_line, _]) = scene(&objects);
    let members = [first, last_line];
    let stored = objects.entries().unwrap().len();

    let merged = merged_batch_state(&objects, &main, &members).unwrap();

    assert_eq!(merged.merged, 2);
    assert_eq!(merged.conflict, None);
    let a = merged.state.entries()[b"a.txt".as_slice()].hash;
    assert!(
        !objects.contains(&a).unwrap(),
        "the line merge stays in memory"
    );
    assert_eq!(objects.entries().unwrap().len(), stored);
    let from = State::recorded(load_flat(&objects, &main).unwrap());
    let change = compare(&objects, &from, &merged.state).unwrap();
    let files: Vec<(String, ChangeKind, u32, u32)> = change
        .files
        .iter()
        .map(|f| (f.path.to_string(), f.change, f.added_lines, f.removed_lines))
        .collect();
    assert_eq!(
        files,
        [
            ("a.txt".to_owned(), ChangeKind::Modified, 2, 2),
            ("b.txt".to_owned(), ChangeKind::Added, 1, 0),
        ]
    );
    let diff = file_diff(&objects, &from, &merged.state, &path("a.txt")).unwrap();
    let added: Vec<&str> = diff
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter(|line| line.kind == LineKind::Added)
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(added, ["one\n", "eight\n"]);

    let BatchMerge::Merged { tree_hash, .. } = merge_batch(&objects, &main, &members).unwrap()
    else {
        panic!("the members merge cleanly");
    };
    assert_eq!(
        &load_flat(&objects, &tree_hash).unwrap(),
        merged.state.entries()
    );
    assert_eq!(merged.tree, tree_hash, "the hash the stored tree gets");
    assert!(objects.contains(&a).unwrap());
}

#[test]
fn a_conflicting_member_stops_the_merge_and_the_state_holds_the_members_before_it() {
    let (_dir, objects) = store();
    let (main, [first, last_line, same_line]) = scene(&objects);
    let stored = objects.entries().unwrap().len();

    let merged = merged_batch_state(&objects, &main, &[first, same_line, last_line]).unwrap();

    assert_eq!(merged.merged, 1);
    let conflict = merged.conflict.unwrap();
    assert_eq!((conflict.member, conflict.with), (1, Some(0)));
    assert_eq!(conflict.conflicts.len(), 1);
    assert_eq!(conflict.conflicts[0].path, b"a.txt");
    assert_eq!(
        merged.state.entries(),
        &load_flat(&objects, &first.tree).unwrap()
    );
    assert_eq!(objects.entries().unwrap().len(), stored);
}

#[test]
fn a_member_that_conflicts_with_main_leaves_main_alone() {
    let (_dir, objects) = store();
    let (main, [_, _, same_line]) = scene(&objects);
    let moved = tree_of(&objects, &[("a.txt", "I\n2\n3\n4\n5\n6\n7\n8\n")]);

    let merged = merged_batch_state(&objects, &moved, &[same_line]).unwrap();

    assert_eq!(merged.merged, 0);
    assert_eq!(merged.conflict.unwrap().with, None);
    assert_eq!(
        merged.state.entries(),
        &load_flat(&objects, &moved).unwrap()
    );
    assert_ne!(moved, main);
}
