#![cfg(test)]

pub mod common;

use std::fs;

use fold_core::tree::{EntryKind, FlatEntry};
use fold_vcs::materialise::{remove_entry, write_entry};
use fold_vcs::paths::{RepoPath, WorkspaceError, resolve};

use common::*;

#[test]
fn bad_paths_are_refused() {
    let cases: [&[u8]; 15] = [
        b"../x",
        b"/abs",
        b"a//b",
        b"a/./b",
        b"a/../b",
        b"a\0b",
        b"",
        b"a/",
        b".",
        b".fold",
        b".fold/tmp/1-0",
        b"a/.fold",
        b"a/.fold/state",
        b".git",
        b"src/.git/config",
    ];
    for case in cases {
        let error = RepoPath::parse(case).unwrap_err();
        assert_eq!(error.code(), "invalid_path", "{case:?}");
    }
}

#[test]
fn good_paths_parse() {
    assert!(RepoPath::parse(b".folder/a/.foldignore").is_ok());
    assert!(RepoPath::parse(b".github/.gitignore/.fold-tmp-1-0").is_ok());
    let path = RepoPath::parse(b"a/b.c/.hidden").unwrap();
    assert_eq!(path.components().count(), 3);
    assert_eq!(path.to_string(), "a/b.c/.hidden");
}

#[test]
fn a_symlinked_parent_blocks_a_write_and_nothing_escapes() {
    let (dir, store) = store();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let root = dir.path().join("ws");
    fs::create_dir(&root).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("dir")).unwrap();
    let hash = blob(&store, b"x");
    let entry = FlatEntry {
        kind: EntryKind::File,
        hash,
    };
    let path = RepoPath::parse(b"dir/file").unwrap();
    let error = write_entry(&store, &root, &path, &entry, &opts(HOST, false)).unwrap_err();
    assert!(matches!(error, WorkspaceError::InvalidPath(_)));
    assert_eq!(error.code(), "invalid_path");
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    let nested = RepoPath::parse(b"dir/a/b").unwrap();
    assert!(resolve(&root, &nested).is_err());
    assert!(remove_entry(&root, &path).is_err());
}

#[test]
fn a_file_in_the_parent_chain_is_refused() {
    let (dir, _store) = store();
    fs::write(dir.path().join("f"), b"x").unwrap();
    let path = RepoPath::parse(b"f/g").unwrap();
    assert!(resolve(dir.path(), &path).is_err());
}

#[test]
fn the_last_component_may_be_a_symlink() {
    let (dir, _store) = store();
    std::os::unix::fs::symlink("/nowhere", dir.path().join("l")).unwrap();
    let path = RepoPath::parse(b"l").unwrap();
    assert_eq!(resolve(dir.path(), &path).unwrap(), dir.path().join("l"));
}

#[test]
fn missing_parents_resolve() {
    let (dir, _store) = store();
    let path = RepoPath::parse(b"a/b/c").unwrap();
    assert_eq!(
        resolve(dir.path(), &path).unwrap(),
        dir.path().join("a/b/c")
    );
}

#[test]
fn remove_entry_unlinks_without_following_and_prunes_empty_parents() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    let outside = dir.path().join("keep");
    fs::write(&outside, b"keep").unwrap();
    fs::create_dir_all(root.join("a/b")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("a/b/l")).unwrap();
    fs::write(root.join("a/other"), b"x").unwrap();
    remove_entry(&root, &RepoPath::parse(b"a/b/l").unwrap()).unwrap();
    assert!(!root.join("a/b").exists());
    assert!(root.join("a").exists());
    assert_eq!(fs::read(&outside).unwrap(), b"keep");
    remove_entry(&root, &RepoPath::parse(b"a/other").unwrap()).unwrap();
    assert!(!root.join("a").exists());
    assert!(root.exists());
    remove_entry(&root, &RepoPath::parse(b"a/other").unwrap()).unwrap();
    fs::create_dir(root.join("d")).unwrap();
    assert!(remove_entry(&root, &RepoPath::parse(b"d").unwrap()).is_err());
    let _ = store;
}

#[test]
fn a_directory_entry_cannot_be_written() {
    let (dir, store) = store();
    let entry = FlatEntry {
        kind: EntryKind::Directory,
        hash: blob(&store, b""),
    };
    let path = RepoPath::parse(b"d").unwrap();
    let error = write_entry(&store, dir.path(), &path, &entry, &opts(HOST, false)).unwrap_err();
    assert_eq!(error.code(), "tree_invalid");
}
