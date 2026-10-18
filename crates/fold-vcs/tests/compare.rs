use std::fs;

use fold_core::object::{ObjectKind, hash_object};
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_vcs::compare::{
    ChangeKind, FileChange, State, Totals, carried_state, compare, file_diff, load_flat,
};
use fold_vcs::diff::{LineKind, MAX_INLINE_BYTES};
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::RepoPath;
use fold_vcs::revision_store::write_initial_tree;

use crate::common::{blob, sample, store};

#[cfg(test)]
fn flat(objects: &ObjectStore, files: &[(&str, EntryKind, &[u8])]) -> FlatTree {
    files
        .iter()
        .map(|(path, kind, content)| {
            let hash = blob(objects, content);
            (path.as_bytes().to_vec(), FlatEntry { kind: *kind, hash })
        })
        .collect()
}

#[cfg(test)]
fn path(text: &str) -> RepoPath {
    RepoPath::parse(text.as_bytes()).unwrap()
}

#[cfg(test)]
fn summary(files: &[FileChange]) -> Vec<(String, Option<String>, ChangeKind, u32, u32)> {
    files
        .iter()
        .map(|file| {
            (
                file.path.to_string(),
                file.old_path.as_ref().map(ToString::to_string),
                file.change,
                file.added_lines,
                file.removed_lines,
            )
        })
        .collect()
}

const FILE: EntryKind = EntryKind::File;

#[test]
fn every_change_kind_is_classified_and_counted_in_byte_order_of_paths() {
    let (_dir, objects) = store();
    let from = flat(
        &objects,
        &[
            ("gone.txt", FILE, b"bye\nnow\n"),
            ("edit.txt", FILE, b"a\nb\nc\n"),
            ("run.sh", FILE, b"#!/bin/sh\n"),
            ("old/name.txt", FILE, b"moved\n"),
            ("same.txt", FILE, b"same\n"),
        ],
    );
    let to = flat(
        &objects,
        &[
            ("new.txt", FILE, b"hello\n"),
            ("edit.txt", FILE, b"a\nB\nc\nd\n"),
            ("run.sh", EntryKind::Executable, b"#!/bin/sh\n"),
            ("new/name.txt", FILE, b"moved\n"),
            ("same.txt", FILE, b"same\n"),
        ],
    );
    let result = compare(&objects, &State::recorded(from), &State::recorded(to)).unwrap();
    assert_eq!(
        summary(&result.files),
        [
            ("edit.txt".into(), None, ChangeKind::Modified, 2, 1),
            ("gone.txt".into(), None, ChangeKind::Deleted, 0, 2),
            ("new.txt".into(), None, ChangeKind::Added, 1, 0),
            (
                "new/name.txt".into(),
                Some("old/name.txt".into()),
                ChangeKind::Renamed,
                0,
                0
            ),
            ("run.sh".into(), None, ChangeKind::ModeChanged, 0, 0),
        ]
    );
    assert_eq!(
        result.totals,
        Totals {
            files: 5,
            added_lines: 3,
            removed_lines: 3,
        }
    );
    let added = &result.files[2];
    assert_eq!(added.old_hash, None);
    assert_eq!(
        added.new_hash,
        Some(hash_object(ObjectKind::Blob, b"hello\n"))
    );
    let deleted = &result.files[1];
    assert_eq!(deleted.new_hash, None);
    assert!(deleted.old_hash.is_some());
    assert!(result.files.iter().all(|file| !file.binary));
}

#[test]
fn a_rename_with_an_edit_is_a_delete_plus_an_add() {
    let (_dir, objects) = store();
    let from = flat(&objects, &[("a.txt", FILE, b"one\n")]);
    let to = flat(&objects, &[("b.txt", FILE, b"one\ntwo\n")]);
    let result = compare(&objects, &State::recorded(from), &State::recorded(to)).unwrap();
    assert_eq!(
        summary(&result.files),
        [
            ("a.txt".into(), None, ChangeKind::Deleted, 0, 1),
            ("b.txt".into(), None, ChangeKind::Added, 2, 0),
        ]
    );
}

#[test]
fn copies_of_one_blob_pair_in_path_order_and_the_rest_stay_adds() {
    let (_dir, objects) = store();
    let from = flat(
        &objects,
        &[("x1", FILE, b"same\n"), ("x2", FILE, b"same\n")],
    );
    let to = flat(
        &objects,
        &[
            ("y1", FILE, b"same\n"),
            ("y2", FILE, b"same\n"),
            ("y3", FILE, b"same\n"),
        ],
    );
    let result = compare(&objects, &State::recorded(from), &State::recorded(to)).unwrap();
    assert_eq!(
        summary(&result.files),
        [
            ("y1".into(), Some("x1".into()), ChangeKind::Renamed, 0, 0),
            ("y2".into(), Some("x2".into()), ChangeKind::Renamed, 0, 0),
            ("y3".into(), None, ChangeKind::Added, 1, 0),
        ]
    );
}

#[test]
fn a_rename_pairs_only_the_same_kind_and_never_empty_files() {
    let (_dir, objects) = store();
    let from = flat(&objects, &[("a.txt", FILE, b"target"), ("e1", FILE, b"")]);
    let to = flat(
        &objects,
        &[("link", EntryKind::Symlink, b"target"), ("e2", FILE, b"")],
    );
    let result = compare(&objects, &State::recorded(from), &State::recorded(to)).unwrap();
    assert_eq!(
        summary(&result.files),
        [
            ("a.txt".into(), None, ChangeKind::Deleted, 0, 1),
            ("e1".into(), None, ChangeKind::Deleted, 0, 0),
            ("e2".into(), None, ChangeKind::Added, 0, 0),
            ("link".into(), None, ChangeKind::Added, 1, 0),
        ]
    );
}

#[test]
fn binary_files_have_no_hunks_and_no_line_counts() {
    let (_dir, objects) = store();
    let from = flat(&objects, &[("img.png", FILE, b"\x89PNG\0\x01")]);
    let to = flat(
        &objects,
        &[
            ("img.png", FILE, b"\x89PNG\0\x02"),
            ("blob.bin", FILE, &[0xff, 0xfe, b'\n']),
        ],
    );
    let (from, to) = (State::recorded(from), State::recorded(to));
    let result = compare(&objects, &from, &to).unwrap();
    assert_eq!(
        summary(&result.files),
        [
            ("blob.bin".into(), None, ChangeKind::Added, 0, 0),
            ("img.png".into(), None, ChangeKind::Modified, 0, 0),
        ]
    );
    assert!(result.files.iter().all(|file| file.binary));
    let diff = file_diff(&objects, &from, &to, &path("img.png")).unwrap();
    assert!(diff.binary);
    assert!(diff.hunks.is_empty());
    assert_eq!(diff.change, ChangeKind::Modified);
}

#[test]
fn a_side_above_the_inline_cap_gives_no_hunks() {
    let (_dir, objects) = store();
    let small = b"one\n".as_slice();
    let large = "line\n".repeat(MAX_INLINE_BYTES / 5 + 1);
    let from = flat(&objects, &[("big.txt", FILE, small)]);
    let to = flat(&objects, &[("big.txt", FILE, large.as_bytes())]);
    let (from, to) = (State::recorded(from), State::recorded(to));
    let diff = file_diff(&objects, &from, &to, &path("big.txt")).unwrap();
    assert!(diff.too_large && !diff.binary);
    assert!(diff.hunks.is_empty());
    assert_eq!(diff.change, ChangeKind::Modified);
}

#[test]
fn file_diff_returns_the_hunks_of_one_listed_file() {
    let (_dir, objects) = store();
    let from = flat(
        &objects,
        &[("src/lib.rs", FILE, b"fn a() {}\n"), ("keep", FILE, b"k\n")],
    );
    let to = flat(
        &objects,
        &[("src/lib.rs", FILE, b"fn b() {}\n"), ("keep", FILE, b"k\n")],
    );
    let (from, to) = (State::recorded(from), State::recorded(to));
    let diff = file_diff(&objects, &from, &to, &path("src/lib.rs")).unwrap();
    assert!(!diff.binary && !diff.too_large);
    assert_eq!(diff.hunks.len(), 1);
    let kinds: Vec<LineKind> = diff.hunks[0].lines.iter().map(|line| line.kind).collect();
    assert_eq!(kinds, [LineKind::Removed, LineKind::Added]);
    for absent in ["keep", "nowhere"] {
        let error = file_diff(&objects, &from, &to, &path(absent)).unwrap_err();
        assert_eq!(error.code(), "path_not_in_comparison");
        assert_eq!(error.details().unwrap()["path"], absent);
    }
}

#[test]
fn a_deleted_or_renamed_file_diffs_by_its_listed_path() {
    let (_dir, objects) = store();
    let from = flat(&objects, &[("a", FILE, b"x\ny\n"), ("old", FILE, b"m\n")]);
    let to = flat(&objects, &[("new", FILE, b"m\n")]);
    let (from, to) = (State::recorded(from), State::recorded(to));
    let deleted = file_diff(&objects, &from, &to, &path("a")).unwrap();
    assert_eq!(deleted.change, ChangeKind::Deleted);
    assert_eq!(deleted.hunks[0].old_lines, 2);
    assert_eq!(deleted.hunks[0].new_lines, 0);
    let renamed = file_diff(&objects, &from, &to, &path("new")).unwrap();
    assert_eq!(renamed.change, ChangeKind::Renamed);
    assert_eq!(renamed.old_path, Some(path("old")));
    assert!(renamed.hunks.is_empty());
    let error = file_diff(&objects, &from, &to, &path("old")).unwrap_err();
    assert_eq!(error.code(), "path_not_in_comparison");
}

/// A version recorded on `r1`, then synced onto a newer main and edited again: carried onto the new base, the earlier version differs from the later one only by the later edit, and a line-merged file is read from the state without being stored.
#[test]
fn a_carried_state_hides_what_the_new_base_brought() {
    let (_dir, objects) = store();
    let stored = |files: &[(&str, EntryKind, &[u8])]| {
        write_initial_tree(&objects, flat(&objects, files)).unwrap()
    };
    let base = stored(&[("lib", FILE, b"a\nb\nc\n"), ("same", FILE, b"1\n")]);
    let earlier = stored(&[("lib", FILE, b"A\nb\nc\n"), ("same", FILE, b"ours\n")]);
    let onto = stored(&[
        ("lib", FILE, b"a\nb\nC\n"),
        ("same", FILE, b"main\n"),
        ("main.md", FILE, b"from main\n"),
    ]);
    let later = flat(
        &objects,
        &[
            ("lib", FILE, b"A\nb\nC\nd\n"),
            ("same", FILE, b"resolved\n"),
            ("main.md", FILE, b"from main\n"),
        ],
    );
    let carried = carried_state(&objects, &base, &earlier, &onto).unwrap();
    assert_eq!(carried.conflicts, [path("same")]);
    let merged = hash_object(ObjectKind::Blob, b"A\nb\nC\n");
    assert!(!objects.contains(&merged).unwrap());
    let later = State::recorded(later);
    let comparison = compare(&objects, &carried.state, &later).unwrap();
    assert_eq!(
        summary(&comparison.files),
        [
            ("lib".to_owned(), None, ChangeKind::Modified, 1, 0),
            ("same".to_owned(), None, ChangeKind::Modified, 1, 1),
        ]
    );
    let diff = file_diff(&objects, &carried.state, &later, &path("lib")).unwrap();
    let kinds: Vec<LineKind> = diff.hunks[0].lines.iter().map(|line| line.kind).collect();
    assert_eq!(kinds.last(), Some(&LineKind::Added));
    assert!(!objects.contains(&merged).unwrap());
}

#[test]
fn a_stored_tree_flattens_to_its_files() {
    let (_dir, objects) = store();
    let root = sample(&objects);
    let entries = load_flat(&objects, &root).unwrap();
    let paths: Vec<String> = entries
        .keys()
        .map(|path| String::from_utf8(path.clone()).unwrap())
        .collect();
    assert_eq!(paths, ["empty", "link", "src/deep/run.sh", "src/lib.rs"]);
}

#[test]
fn a_working_side_reads_files_the_store_does_not_hold_from_disk() {
    let (dir, objects) = store();
    let root = dir.path().join("ws");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), b"fn a() {}\nfn b() {}\n").unwrap();
    std::os::unix::fs::symlink("src/lib.rs", root.join("link")).unwrap();
    let from = flat(&objects, &[("src/lib.rs", FILE, b"fn a() {}\n")]);
    // The working entries carry the hashes a scan would find; their blobs are not stored.
    let working: FlatTree = [
        ("src/lib.rs", FILE, b"fn a() {}\nfn b() {}\n" as &[u8]),
        ("link", EntryKind::Symlink, b"src/lib.rs"),
    ]
    .iter()
    .map(|(path, kind, content)| {
        let hash = hash_object(ObjectKind::Blob, content);
        (path.as_bytes().to_vec(), FlatEntry { kind: *kind, hash })
    })
    .collect();
    let to = State::working(root.clone(), working);
    assert!(to.is_working());
    let from = State::recorded(from);
    let result = compare(&objects, &from, &to).unwrap();
    assert_eq!(
        summary(&result.files),
        [
            ("link".into(), None, ChangeKind::Added, 1, 0),
            ("src/lib.rs".into(), None, ChangeKind::Modified, 1, 0),
        ]
    );
    let diff = file_diff(&objects, &from, &to, &path("link")).unwrap();
    assert_eq!(diff.hunks[0].lines[0].text, "src/lib.rs");
    assert!(diff.hunks[0].lines[0].no_newline);

    fs::write(
        root.join("src/lib.rs"),
        b"fn a() {}\nfn b() {}\nfn c() {}\n",
    )
    .unwrap();
    // The working side names the hash the scan found, so content that no longer has it gives no counts.
    let edited = compare(&objects, &from, &to).unwrap_err();
    assert_eq!(edited.code(), "workspace_io");
    assert_eq!(edited.details().unwrap()["path"], "src/lib.rs");

    fs::remove_file(root.join("src/lib.rs")).unwrap();
    let error = compare(&objects, &from, &to).unwrap_err();
    assert_eq!(error.code(), "workspace_io");
    assert_eq!(error.details().unwrap()["path"], "src/lib.rs");
}

#[test]
fn change_kinds_have_their_wire_names() {
    let names: Vec<&str> = [
        ChangeKind::Added,
        ChangeKind::Modified,
        ChangeKind::Deleted,
        ChangeKind::Renamed,
        ChangeKind::ModeChanged,
    ]
    .iter()
    .map(|kind| kind.as_str())
    .collect();
    assert_eq!(
        names,
        ["added", "modified", "deleted", "renamed", "mode_changed"]
    );
}
