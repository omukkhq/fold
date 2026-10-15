use fold_core::object::{ObjectKind, hash_object};
use fold_core::tree::EntryKind;
use fold_vcs::browse::{DirEntry, parse_path, read_dir, read_file};
use fold_vcs::diff::MAX_INLINE_BYTES;
use fold_vcs::paths::RepoPath;

use crate::common::{blob, sample, store, tree};

#[cfg(test)]
fn path(text: &str) -> RepoPath {
    RepoPath::parse(text.as_bytes()).unwrap()
}

#[cfg(test)]
fn names(entries: &[DirEntry]) -> Vec<(String, EntryKind, Option<u64>)> {
    entries
        .iter()
        .map(|entry| {
            (
                String::from_utf8(entry.name.clone()).unwrap(),
                entry.kind,
                entry.size,
            )
        })
        .collect()
}

#[test]
fn the_root_and_a_nested_directory_list() {
    let (_dir, objects) = store();
    let root = sample(&objects);
    assert_eq!(
        names(&read_dir(&objects, &root, None).unwrap()),
        [
            ("empty".into(), EntryKind::File, Some(0)),
            ("link".into(), EntryKind::Symlink, Some(10)),
            ("src".into(), EntryKind::Directory, None),
        ]
    );
    let deep = read_dir(&objects, &root, Some(&path("src/deep"))).unwrap();
    assert_eq!(
        names(&deep),
        [("run.sh".into(), EntryKind::Executable, Some(10))]
    );
    assert_eq!(deep[0].hash, hash_object(ObjectKind::Blob, b"#!/bin/sh\n"));
}

#[test]
fn a_listed_symlink_carries_its_target_and_other_entries_none() {
    let (_dir, objects) = store();
    let root = sample(&objects);
    let targets: Vec<_> = read_dir(&objects, &root, None)
        .unwrap()
        .into_iter()
        .map(|entry| entry.target)
        .collect();
    assert_eq!(targets, [None, Some(b"src/lib.rs".to_vec()), None]);
    let deep = read_dir(&objects, &root, Some(&path("src/deep"))).unwrap();
    assert_eq!(deep[0].target, None);
}

#[test]
fn a_file_reads_and_a_symlink_returns_its_target_text() {
    let (_dir, objects) = store();
    let root = sample(&objects);
    let file = read_file(&objects, &root, &path("src/lib.rs")).unwrap();
    assert_eq!(file.bytes, b"pub fn x() {}\n");
    assert_eq!(file.kind, EntryKind::File);
    assert_eq!(file.hash, hash_object(ObjectKind::Blob, b"pub fn x() {}\n"));
    let link = read_file(&objects, &root, &path("link")).unwrap();
    assert_eq!(link.kind, EntryKind::Symlink);
    assert_eq!(link.bytes, b"src/lib.rs");
    assert!(!file.binary && !link.binary);
}

#[test]
fn absent_paths_and_wrong_kinds_are_not_found() {
    let (_dir, objects) = store();
    let root = sample(&objects);
    for (absent, listing) in [
        ("nowhere", true),
        ("src/lib.rs", true),
        ("src/lib.rs/below", false),
        ("src", false),
        ("src/missing.rs", false),
    ] {
        let error = if listing {
            read_dir(&objects, &root, Some(&path(absent))).unwrap_err()
        } else {
            read_file(&objects, &root, &path(absent)).unwrap_err()
        };
        assert_eq!(error.code(), "not_found", "{absent}");
        assert_eq!(error.details().unwrap()["path"], absent);
    }
}

#[test]
fn a_path_with_dot_dot_is_refused_before_any_read() {
    for bad in [
        "../etc/passwd",
        "src/../link",
        "/abs",
        "a//b",
        ".git/config",
    ] {
        let error = parse_path(bad).unwrap_err();
        assert_eq!(error.code(), "invalid_path", "{bad}");
        assert_eq!(error.details().unwrap()["path"], bad);
    }
    assert_eq!(parse_path("").unwrap(), None);
    assert_eq!(parse_path("src/deep").unwrap(), Some(path("src/deep")));
}

#[test]
fn a_file_above_the_inline_cap_gives_its_size_and_hash_only() {
    let (_dir, objects) = store();
    let at_cap = vec![b'a'; MAX_INLINE_BYTES];
    let above = vec![b'a'; MAX_INLINE_BYTES + 1];
    let (at_hash, above_hash) = (blob(&objects, &at_cap), blob(&objects, &above));
    let root = tree(
        &objects,
        &[
            ("at", EntryKind::File, at_hash),
            ("above", EntryKind::File, above_hash),
        ],
    );
    let read = read_file(&objects, &root, &path("at")).unwrap();
    assert!(!read.too_large);
    assert_eq!(read.bytes.len(), MAX_INLINE_BYTES);
    let large = read_file(&objects, &root, &path("above")).unwrap();
    assert!(large.too_large && !large.binary);
    assert!(large.bytes.is_empty());
    assert_eq!(large.size, MAX_INLINE_BYTES as u64 + 1);
    assert_eq!(large.hash, above_hash);
}
