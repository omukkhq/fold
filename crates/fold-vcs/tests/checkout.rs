#![cfg(test)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use fold_core::object::{ObjectKind, hash_object};
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_vcs::checkout::{
    FolderEntry, folder_changes, folder_stamp, follow, held, overlay, read_folder_entry,
};
use fold_vcs::paths::RepoPath;

use crate::common::*;

fn entry(kind: EntryKind, bytes: &[u8]) -> FlatEntry {
    FlatEntry {
        kind,
        hash: hash_object(ObjectKind::Blob, bytes),
    }
}

fn file(bytes: &[u8]) -> FlatEntry {
    entry(EntryKind::File, bytes)
}

fn flat(entries: &[(&str, FlatEntry)]) -> FlatTree {
    entries
        .iter()
        .map(|(path, entry)| (path.as_bytes().to_vec(), *entry))
        .collect()
}

fn path(text: &str) -> RepoPath {
    RepoPath::parse(text.as_bytes()).unwrap()
}

#[test]
fn a_folder_stamp_changes_with_what_the_paths_hold_and_with_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", b"one\n");
    write(root, "other.txt", b"one\n");
    let paths = [path("a.txt"), path("gone/b.txt"), path("a.txt/c")];
    let first = folder_stamp(root, &paths);
    assert_eq!(folder_stamp(root, &paths), first);
    // A path the stamp does not name changes nothing.
    write(root, "other.txt", b"two\n");
    assert_eq!(folder_stamp(root, &paths), first);
    write(root, "a.txt", b"two\n");
    let written = folder_stamp(root, &paths);
    assert_ne!(written, first);
    write(root, "gone/b.txt", b"new\n");
    assert_ne!(folder_stamp(root, &paths), written);
    fs::remove_file(root.join("a.txt")).unwrap();
    symlink("other.txt", root.join("a.txt")).unwrap();
    let linked = folder_stamp(root, &paths);
    assert_eq!(folder_stamp(root, &paths), linked);
    assert_ne!(folder_stamp(&root.join("missing"), &paths), linked);
}

#[test]
fn an_entry_is_read_without_following_a_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.txt", b"one\n");
    write(root, "run.sh", b"#!/bin/sh\n");
    fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("a.txt", root.join("link")).unwrap();
    fs::create_dir(root.join("dir")).unwrap();
    let read = |name: &str| read_folder_entry(root, &path(name), 1024, &HOST, None).unwrap();
    assert_eq!(read("a.txt"), FolderEntry::Entry(file(b"one\n")));
    assert_eq!(
        read("run.sh"),
        FolderEntry::Entry(entry(EntryKind::Executable, b"#!/bin/sh\n"))
    );
    assert_eq!(
        read("link"),
        FolderEntry::Entry(entry(EntryKind::Symlink, b"a.txt"))
    );
    assert_eq!(read("dir"), FolderEntry::Other);
    assert_eq!(read("missing"), FolderEntry::Missing);
    assert_eq!(read("missing/below"), FolderEntry::Missing);
    // A parent that is a file or a symlink stands in the way and is never followed.
    assert_eq!(read("a.txt/below"), FolderEntry::Other);
    assert_eq!(read("link/below"), FolderEntry::Other);
    // A file over the limit is something a tree entry does not record.
    assert_eq!(
        read_folder_entry(root, &path("a.txt"), 2, &HOST, None).unwrap(),
        FolderEntry::Other
    );
    // Without an executable bit, the kind fold knows decides.
    assert_eq!(
        read_folder_entry(
            root,
            &path("a.txt"),
            1024,
            &BARE,
            Some(EntryKind::Executable)
        )
        .unwrap(),
        FolderEntry::Entry(entry(EntryKind::Executable, b"one\n"))
    );
}

#[test]
fn a_follow_waits_while_a_path_main_changed_holds_a_human_change_then_writes_everything() {
    let (dir, store) = store();
    let root = dir.path().join("folder");
    fs::create_dir(&root).unwrap();
    for text in [
        &b"one\n"[..],
        b"two\n",
        b"mine\n",
        b"kept\n",
        b"new\n",
        b"#!/bin/sh\n",
        b"a.txt",
    ] {
        blob(&store, text);
    }
    write(&root, "a.txt", b"one\n");
    write(&root, "b.txt", b"mine\n");
    write(&root, "gone/c.txt", b"kept\n");
    write(&root, "same.txt", b"two\n");
    write(&root, "own.txt", b"mine\n");
    let record = flat(&[
        ("a.txt", file(b"one\n")),
        ("b.txt", file(b"one\n")),
        ("gone/c.txt", file(b"kept\n")),
        ("same.txt", file(b"one\n")),
        ("own.txt", file(b"one\n")),
    ]);
    let main = flat(&[
        ("a.txt", file(b"two\n")),
        ("b.txt", file(b"two\n")),
        ("same.txt", file(b"two\n")),
        ("own.txt", file(b"one\n")),
        ("new/d.txt", file(b"new\n")),
        ("run.sh", entry(EntryKind::Executable, b"#!/bin/sh\n")),
        ("link", entry(EntryKind::Symlink, b"a.txt")),
    ]);
    let changes = folder_changes(&root, &record, &main, 1024, &HOST).unwrap();
    // `own.txt` is the human's too, but main did not change it, so it is not even read.
    assert!(
        changes
            .iter()
            .all(|(changed, _)| *changed != path("own.txt"))
    );
    assert_eq!(held(&record, &main, &changes), [path("b.txt")]);
    let followed = follow(&store, &root, &record, &main, &changes, HOST).unwrap();
    // `b.txt` holds the human's own text, so nothing of main's is written; only the path that holds main's entry already moves the record.
    assert_eq!(followed.held, [path("b.txt")]);
    assert!(followed.written.is_empty());
    assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"one\n");
    assert_eq!(fs::read(root.join("b.txt")).unwrap(), b"mine\n");
    assert!(root.join("gone/c.txt").is_file());
    assert!(!root.join("new").exists());
    let mut settled = record.clone();
    settled.insert(b"same.txt".to_vec(), file(b"two\n"));
    assert_eq!(followed.record, settled);

    // The human takes main's text: the next follow writes, removes or notes every path main changed.
    write(&root, "b.txt", b"two\n");
    let again = folder_changes(&root, &followed.record, &main, 1024, &HOST).unwrap();
    let second = follow(&store, &root, &followed.record, &main, &again, HOST).unwrap();
    assert!(second.held.is_empty());
    assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"two\n");
    assert!(!root.join("gone").exists());
    assert_eq!(fs::read(root.join("new/d.txt")).unwrap(), b"new\n");
    assert_ne!(
        fs::metadata(root.join("run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
    assert_eq!(
        fs::read_link(root.join("link")).unwrap().to_str(),
        Some("a.txt")
    );
    assert_eq!(fs::read(root.join("own.txt")).unwrap(), b"mine\n");
    assert_eq!(second.record, main);
    assert_eq!(
        second.written,
        [
            path("a.txt"),
            path("gone/c.txt"),
            path("link"),
            path("new/d.txt"),
            path("run.sh")
        ]
    );
}

#[test]
fn a_follow_cut_off_after_its_writes_takes_them_as_written_the_next_time() {
    let (dir, store) = store();
    let root = dir.path().join("folder");
    blob(&store, b"two\n");
    write(&root, "a.txt", b"two\n");
    let record = flat(&[("a.txt", file(b"one\n"))]);
    let main = flat(&[("a.txt", file(b"two\n"))]);
    let changes = folder_changes(&root, &record, &main, 1024, &HOST).unwrap();
    let followed = follow(&store, &root, &record, &main, &changes, HOST).unwrap();
    assert!(followed.held.is_empty() && followed.written.is_empty());
    assert_eq!(followed.record, main);
}

#[test]
fn a_directory_main_puts_where_a_file_was_finds_its_place_free() {
    let (dir, store) = store();
    let root = dir.path().join("folder");
    blob(&store, b"below\n");
    write(&root, "a", b"file\n");
    let record = flat(&[("a", file(b"file\n"))]);
    let main = flat(&[("a/b", file(b"below\n"))]);
    let changes = folder_changes(&root, &record, &main, 1024, &HOST).unwrap();
    let followed = follow(&store, &root, &record, &main, &changes, HOST).unwrap();
    assert!(followed.held.is_empty());
    assert_eq!(fs::read(root.join("a/b")).unwrap(), b"below\n");
    assert_eq!(followed.record, main);
}

#[test]
fn a_file_main_puts_where_a_directory_of_its_files_was_takes_the_emptied_place() {
    let (dir, store) = store();
    let root = dir.path().join("folder");
    blob(&store, b"file\n");
    write(&root, "a/b", b"below\n");
    write(&root, "c/d", b"below\n");
    write(&root, "c/own.txt", b"the human's\n");
    let record = flat(&[("a/b", file(b"below\n")), ("c/d", file(b"below\n"))]);
    let main = flat(&[("a", file(b"file\n")), ("c", file(b"file\n"))]);
    let changes = folder_changes(&root, &record, &main, 1024, &HOST).unwrap();
    let followed = follow(&store, &root, &record, &main, &changes, HOST).unwrap();
    // A directory that still holds a file of the human's stands where main's file goes, so the folder waits.
    assert_eq!(followed.held, [path("c")]);
    assert!(followed.written.is_empty());
    assert_eq!(fs::read(root.join("a/b")).unwrap(), b"below\n");
    assert_eq!(fs::read(root.join("c/own.txt")).unwrap(), b"the human's\n");
    assert_eq!(followed.record, record);

    // Once the human's file is gone, both directories of fold's files give their place to main's files.
    fs::remove_file(root.join("c/own.txt")).unwrap();
    let again = folder_changes(&root, &record, &main, 1024, &HOST).unwrap();
    let second = follow(&store, &root, &record, &main, &again, HOST).unwrap();
    assert!(second.held.is_empty());
    assert_eq!(fs::read(root.join("a")).unwrap(), b"file\n");
    assert_eq!(fs::read(root.join("c")).unwrap(), b"file\n");
    assert_eq!(
        second.written,
        [path("a"), path("a/b"), path("c"), path("c/d")]
    );
    assert_eq!(second.record, main);
}

#[test]
fn a_recorded_change_lays_its_entries_over_the_record() {
    let record = flat(&[("a", file(b"1")), ("b", file(b"1")), ("c", file(b"1"))]);
    let base = flat(&[("a", file(b"0")), ("b", file(b"1"))]);
    let tree = flat(&[("a", file(b"2")), ("d", file(b"2"))]);
    // `a` and `d` changed, `b` went; `c` the change left alone.
    assert_eq!(
        overlay(&record, &base, &tree),
        flat(&[("a", file(b"2")), ("c", file(b"1")), ("d", file(b"2"))])
    );
}
