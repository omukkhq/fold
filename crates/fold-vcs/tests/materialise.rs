#![cfg(test)]

pub mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use fold_core::tree::{EntryKind, FlatEntry};
use fold_vcs::index::StatIndex;
use fold_vcs::materialise::{Batch, Synced, materialise, remove_entry, write_entry};
use fold_vcs::paths::{RepoPath, resolve};

use common::*;

fn mode(path: &std::path::Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn file(store: &fold_vcs::objects::ObjectStore, text: &[u8]) -> FlatEntry {
    FlatEntry {
        kind: EntryKind::File,
        hash: blob(store, text),
    }
}

#[test]
fn a_tree_is_written_with_content_modes_and_links() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    let tree = sample(&store);
    let done = materialise(&store, &tree, &root, &opts(HOST, false)).unwrap();
    assert!(done.degraded.is_empty());
    assert_eq!(
        fs::read(root.join("src/lib.rs")).unwrap(),
        b"pub fn x() {}\n"
    );
    assert_eq!(fs::read(root.join("empty")).unwrap(), b"");
    assert_eq!(mode(&root.join("src/lib.rs")), 0o644);
    assert_eq!(mode(&root.join("src/deep/run.sh")), 0o755);
    assert_eq!(
        fs::read_link(root.join("link")).unwrap().to_str().unwrap(),
        "src/lib.rs"
    );
    assert_eq!(done.index.len(), 4);
    assert_eq!(names(&root), [".fold", "empty", "link", "src"]);
    assert!(
        names(&root.join(".fold/tmp")).is_empty(),
        "no temp files remain"
    );
}

#[test]
fn modes_ignore_the_umask() {
    let (dir, store) = store();
    let hash = blob(&store, b"x");
    let root = dir.path().join("ws");
    for (name, kind, expected) in [
        ("f", EntryKind::File, 0o644),
        ("x", EntryKind::Executable, 0o755),
    ] {
        let entry = FlatEntry { kind, hash };
        let path = RepoPath::parse(name.as_bytes()).unwrap();
        write_entry(&store, &root, &path, &entry, &opts(HOST, false)).unwrap();
        assert_eq!(mode(&root.join(name)), expected);
    }
}

#[test]
fn a_non_empty_target_is_refused_and_untouched() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("keep"), b"mine").unwrap();
    let error = materialise(&store, &sample(&store), &root, &opts(HOST, false)).unwrap_err();
    assert_eq!(error.code(), "target_not_empty");
    assert_eq!(fs::read(root.join("keep")).unwrap(), b"mine");
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
}

#[test]
fn an_empty_existing_target_is_accepted() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    fs::create_dir(&root).unwrap();
    materialise(&store, &sample(&store), &root, &opts(HOST, false)).unwrap();
    assert!(root.join("empty").exists());
}

#[test]
fn a_target_that_is_a_file_is_an_io_error() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    fs::write(&root, b"x").unwrap();
    let error = materialise(&store, &sample(&store), &root, &opts(HOST, false)).unwrap_err();
    assert_eq!(error.code(), "workspace_io");
}

#[test]
fn writing_over_a_symlink_replaces_it_and_spares_the_target() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    fs::create_dir(&root).unwrap();
    let outside = dir.path().join("outside");
    fs::write(&outside, b"precious").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("f")).unwrap();
    let entry = FlatEntry {
        kind: EntryKind::File,
        hash: blob(&store, b"new"),
    };
    let path = RepoPath::parse(b"f").unwrap();
    write_entry(&store, &root, &path, &entry, &opts(HOST, false)).unwrap();
    assert_eq!(fs::read(&outside).unwrap(), b"precious");
    assert!(!fs::symlink_metadata(root.join("f")).unwrap().is_symlink());
    assert_eq!(fs::read(root.join("f")).unwrap(), b"new");
}

#[test]
fn a_symlink_target_is_not_checked() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    let entry = FlatEntry {
        kind: EntryKind::Symlink,
        hash: blob(&store, b"/etc/../../absolute"),
    };
    let path = RepoPath::parse(b"l").unwrap();
    write_entry(&store, &root, &path, &entry, &opts(HOST, false)).unwrap();
    assert_eq!(
        fs::read_link(root.join("l")).unwrap().to_str().unwrap(),
        "/etc/../../absolute"
    );
}

#[test]
fn read_only_leaves_no_write_bit() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    materialise(&store, &sample(&store), &root, &opts(HOST, true)).unwrap();
    for file in ["empty", "src/lib.rs", "src/deep/run.sh"] {
        assert_eq!(mode(&root.join(file)) & 0o222, 0, "{file}");
    }
    assert_eq!(mode(&root.join("src/deep/run.sh")), 0o555);
}

#[test]
fn every_written_path_is_fresh() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    let done = materialise(&store, &sample(&store), &root, &opts(HOST, true)).unwrap();
    for (path, entry) in done.index.iter() {
        let meta = fs::symlink_metadata(resolve(&root, path).unwrap()).unwrap();
        assert_eq!(done.index.is_fresh(path, &meta), Some(entry.hash), "{path}");
    }
}

#[test]
fn a_missing_blob_fails_part_way_and_keeps_the_directory() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    let ghost = fold_core::object::hash_object(fold_core::object::ObjectKind::Blob, b"ghost");
    let tree = tree(
        &store,
        &[
            ("a", EntryKind::File, blob(&store, b"a")),
            ("b", EntryKind::File, ghost),
        ],
    );
    let error = materialise(&store, &tree, &root, &opts(HOST, false)).unwrap_err();
    assert_eq!(error.code(), "object_missing");
    assert!(root.join("a").exists());
    assert!(!root.join("b").exists());
    assert_eq!(names(&root), [".fold", "a"]);
    assert!(
        names(&root.join(".fold/tmp")).is_empty(),
        "failed temp file is removed"
    );
}

#[test]
fn a_blob_is_not_a_tree() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    let error = materialise(&store, &blob(&store, b"x"), &root, &opts(HOST, false)).unwrap_err();
    assert_eq!(error.code(), "object_corrupt");
}

#[test]
fn empty_directories_in_a_tree_are_created() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    let empty = tree(&store, &[]);
    let top = tree(&store, &[("d", EntryKind::Directory, empty)]);
    materialise(&store, &top, &root, &opts(HOST, false)).unwrap();
    assert!(root.join("d").is_dir());
}

#[test]
fn file_directory_swaps_work_when_the_delete_comes_first() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    let file = |text: &[u8]| FlatEntry {
        kind: EntryKind::File,
        hash: blob(&store, text),
    };
    let put = |name: &str, text: &[u8]| {
        let path = RepoPath::parse(name.as_bytes()).unwrap();
        write_entry(&store, &root, &path, &file(text), &opts(HOST, false))
    };
    let del = |name: &str| {
        fold_vcs::materialise::remove_entry(&root, &RepoPath::parse(name.as_bytes()).unwrap())
    };

    put("a", b"file").unwrap();
    assert!(put("a/b", b"x").is_err(), "a file parent is refused");
    del("a").unwrap();
    put("a/b", b"x").unwrap();
    put("a/c", b"y").unwrap();
    assert!(
        put("a", b"file").is_err(),
        "a directory is never overwritten"
    );
    del("a/b").unwrap();
    del("a/c").unwrap();
    put("a", b"file").unwrap();
    assert_eq!(fs::read(root.join("a")).unwrap(), b"file");
}

#[test]
fn materialise_flushes_every_file_and_directory_before_it_returns() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    let done = materialise(&store, &sample(&store), &root, &opts(HOST, false)).unwrap();
    // A real symlink has no data to flush; the directories are the parent of the new root, the root, src and src/deep.
    assert_eq!(done.synced, Synced { files: 3, dirs: 4 });
    let bare = dir.path().join("bare");
    let done = materialise(&store, &sample(&store), &bare, &opts(BARE, false)).unwrap();
    assert_eq!(done.synced, Synced { files: 4, dirs: 4 });
}

#[test]
fn a_batch_flushes_its_writes_and_removals_once_at_the_end() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    fs::create_dir_all(root.join("old")).unwrap();
    fs::write(root.join("old/z"), b"z").unwrap();
    let mut batch = Batch::open(&root).unwrap();
    for name in ["a/x", "a/y"] {
        let path = RepoPath::parse(name.as_bytes()).unwrap();
        batch
            .write_entry(
                &store,
                &path,
                &file(&store, name.as_bytes()),
                &opts(HOST, false),
            )
            .unwrap();
    }
    batch
        .remove_entry(&RepoPath::parse(b"old/z").unwrap())
        .unwrap();
    // `old` was pruned, so only the root and `a` are left to flush.
    assert_eq!(batch.finish().unwrap(), Synced { files: 2, dirs: 2 });
    assert_eq!(names(&root), [".fold", "a"]);
}

#[test]
fn a_large_batch_flushes_every_file() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    let entry = file(&store, b"x");
    // The batch creates the root, so the root's parent gains an entry too.
    let mut batch = Batch::open(&root).unwrap();
    for number in 0..300 {
        let path = RepoPath::parse(format!("f{number}").as_bytes()).unwrap();
        batch
            .write_entry(&store, &path, &entry, &opts(HOST, false))
            .unwrap();
    }
    assert_eq!(
        batch.finish().unwrap(),
        Synced {
            files: 300,
            dirs: 2
        }
    );
}

#[test]
fn leftovers_in_the_staging_directory_are_removed_by_the_next_batch() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    let staging = root.join(".fold/tmp");
    fs::create_dir_all(staging.join("half/deep")).unwrap();
    fs::write(staging.join("4242-7"), b"interrupted").unwrap();
    fs::write(staging.join("half/deep/x"), b"x").unwrap();
    Batch::open(&root).unwrap().finish().unwrap();
    assert!(names(&staging).is_empty());

    fs::write(staging.join("4242-8"), b"interrupted").unwrap();
    let path = RepoPath::parse(b"f").unwrap();
    write_entry(
        &store,
        &root,
        &path,
        &file(&store, b"f"),
        &opts(HOST, false),
    )
    .unwrap();
    assert!(names(&staging).is_empty());
}

#[test]
fn a_leftover_index_save_is_removed_on_the_next_create() {
    let (dir, store) = store();
    let index_dir = dir.path().join(".fold/index");
    fs::create_dir_all(&index_dir).unwrap();
    fs::write(index_dir.join(".save-ws-9.idx-4242-0.tmp"), b"half").unwrap();
    fs::write(index_dir.join(".save-ws-8.idx-4242-0.tmp"), b"other").unwrap();
    let mut done = materialise(
        &store,
        &sample(&store),
        &dir.path().join("ws"),
        &opts(HOST, false),
    )
    .unwrap()
    .index;
    done.save(&index_dir.join("ws-9.idx")).unwrap();
    assert_eq!(names(&index_dir), [".save-ws-8.idx-4242-0.tmp", "ws-9.idx"]);
    assert_eq!(StatIndex::load(&index_dir.join("ws-9.idx"), HOST).len(), 4);
}

#[test]
fn user_files_named_like_temp_files_write_normally_and_stay() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    for name in [
        ".fold-tmp-1-0",
        "4242-0",
        "x.tmp",
        "tmp/1-0",
        "a/.fold/tmp/1-0",
    ] {
        let path = RepoPath::parse(name.as_bytes()).unwrap();
        write_entry(
            &store,
            &root,
            &path,
            &file(&store, name.as_bytes()),
            &opts(HOST, false),
        )
        .unwrap();
    }
    Batch::open(&root).unwrap().finish().unwrap();
    assert_eq!(
        fs::read(root.join(".fold-tmp-1-0")).unwrap(),
        b".fold-tmp-1-0"
    );
    assert_eq!(fs::read(root.join("x.tmp")).unwrap(), b"x.tmp");
    assert_eq!(fs::read(root.join("tmp/1-0")).unwrap(), b"tmp/1-0");
    assert_eq!(
        fs::read(root.join("a/.fold/tmp/1-0")).unwrap(),
        b"a/.fold/tmp/1-0"
    );
}

#[test]
fn a_planted_staging_link_is_refused_and_nothing_outside_is_touched() {
    let (dir, store) = store();
    let outside = dir.path().join("outside");
    fs::create_dir_all(outside.join("tmp")).unwrap();
    fs::write(outside.join("tmp/precious"), b"keep").unwrap();
    for planted in [".fold", ".fold/tmp"] {
        let root = dir.path().join(format!("ws-{}", planted.len()));
        fs::create_dir_all(root.join(planted).parent().unwrap()).unwrap();
        let target = if planted == ".fold" {
            outside.clone()
        } else {
            outside.join("tmp")
        };
        std::os::unix::fs::symlink(&target, root.join(planted)).unwrap();
        assert_eq!(Batch::open(&root).unwrap_err().code(), "invalid_path");
        let path = RepoPath::parse(b"f").unwrap();
        let error = write_entry(
            &store,
            &root,
            &path,
            &file(&store, b"f"),
            &opts(HOST, false),
        );
        assert_eq!(error.unwrap_err().code(), "invalid_path");
    }
    let file_root = dir.path().join("ws-file");
    fs::create_dir_all(&file_root).unwrap();
    fs::write(file_root.join(".fold"), b"not a directory").unwrap();
    assert_eq!(Batch::open(&file_root).unwrap_err().code(), "invalid_path");
    assert_eq!(fs::read(outside.join("tmp/precious")).unwrap(), b"keep");
    assert_eq!(names(&outside.join("tmp")), ["precious"]);
}

#[test]
fn removing_an_entry_creates_no_staging_directory() {
    let (dir, _store) = store();
    let root = dir.path().join("ws");
    fs::create_dir_all(root.join("d")).unwrap();
    fs::write(root.join("d/f"), b"x").unwrap();
    fs::write(root.join("keep"), b"x").unwrap();
    remove_entry(&root, &RepoPath::parse(b"d/f").unwrap()).unwrap();
    assert_eq!(names(&root), ["keep"]);
}
