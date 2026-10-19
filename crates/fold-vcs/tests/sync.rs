#![cfg(test)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use fold_core::merge::{ConflictKind, merge_trees};
use fold_core::object::{ObjectHash, ObjectKind, hash_object};
use fold_core::tree::{EntryKind, FlatEntry, Tree, apply_changes, flatten};
use fold_core::{FoldError, RevisionId, codes};
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::index::StatIndex;
use fold_vcs::materialise::materialise;
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::RepoPath;
use fold_vcs::restore::{RestoreOptions, restore_tree};
use fold_vcs::scan::{DEFAULT_MAX_FILE_BYTES, ScanOptions};
use fold_vcs::snapshot::snapshot;
use fold_vcs::sync::{
    ConflictedPath, MarkerLabels, SyncInput, SyncOutcome, sidecar, sync, unresolved, write_merge,
};

use crate::common::*;

struct Ws {
    _dir: tempfile::TempDir,
    store: ObjectStore,
    root: PathBuf,
    index_path: PathBuf,
    index: StatIndex,
    base: ObjectHash,
}

fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

fn path(text: &str) -> RepoPath {
    RepoPath::parse(text.as_bytes()).unwrap()
}

fn tree_of(store: &ObjectStore, entries: &[(&str, EntryKind, &[u8])]) -> ObjectHash {
    let changes: BTreeMap<Vec<u8>, Option<FlatEntry>> = entries
        .iter()
        .map(|(path, kind, bytes)| {
            let hash = store.write(ObjectKind::Blob, bytes).unwrap();
            (
                path.as_bytes().to_vec(),
                Some(FlatEntry { kind: *kind, hash }),
            )
        })
        .collect();
    let load = |hash: &ObjectHash| {
        let object = store.read_expecting(hash, ObjectKind::Tree)?;
        Ok(Tree::decode(&object.payload)?)
    };
    let built = apply_changes(None, &changes, &load).unwrap();
    for (_, payload) in &built.new_trees {
        store.write(ObjectKind::Tree, payload).unwrap();
    }
    built.root
}

fn files(store: &ObjectStore, files: &[(&str, &[u8])]) -> ObjectHash {
    let entries: Vec<_> = files
        .iter()
        .map(|(path, bytes)| (*path, EntryKind::File, *bytes))
        .collect();
    tree_of(store, &entries)
}

impl Ws {
    fn new(base: &[(&str, &[u8])]) -> Self {
        let (dir, store) = store();
        let root = dir.path().join("ws");
        let base = files(&store, base);
        let done = materialise(&store, &base, &root, &opts(HOST, false)).unwrap();
        Self {
            index_path: dir.path().join(".fold/ws-index"),
            _dir: dir,
            store,
            root,
            index: done.index,
            base,
        }
    }

    /// The automatic checkpoint a sync command takes first.
    fn checkpoint(&mut self) -> ObjectHash {
        let rules = IgnoreRules::new(&self.root);
        let scan = ScanOptions {
            rules: &rules,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            caps: HOST,
            recorded: None,
        };
        snapshot(
            &self.store,
            &self.root,
            &mut self.index,
            &self.index_path,
            Some(self.base),
            &scan,
        )
        .unwrap()
        .tree
    }

    fn sync(&mut self, ours: ObjectHash, main: ObjectHash) -> Result<SyncOutcome, FoldError> {
        self.sync_numbered(ours, main, r(3), r(5))
    }

    fn sync_numbered(
        &mut self,
        ours: ObjectHash,
        main: ObjectHash,
        base_number: RevisionId,
        main_number: RevisionId,
    ) -> Result<SyncOutcome, FoldError> {
        sync(
            &self.store,
            &self.root,
            &mut self.index,
            SyncInput {
                base_tree: self.base,
                ours_tree: ours,
                main_tree: main,
                base_number,
                main_number,
            },
        )
    }

    fn main(&self, entries: &[(&str, &[u8])]) -> ObjectHash {
        files(&self.store, entries)
    }

    fn read(&self, path: &str) -> Vec<u8> {
        fs::read(self.root.join(path)).unwrap()
    }

    fn text(&self, path: &str) -> String {
        String::from_utf8(self.read(path)).unwrap()
    }

    fn exists(&self, path: &str) -> bool {
        fs::symlink_metadata(self.root.join(path)).is_ok()
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        write(&self.root, path, bytes);
    }

    /// Every file under the root with its bytes, fold's own directory left out.
    fn listing(&self) -> BTreeMap<String, Vec<u8>> {
        let mut found = BTreeMap::new();
        collect(&self.root, &self.root, &mut found);
        found
    }
}

fn collect(root: &Path, dir: &Path, found: &mut BTreeMap<String, Vec<u8>>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".fold" {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if entry.file_type().unwrap().is_dir() {
            found.insert(format!("{relative}/"), Vec::new());
            collect(root, &entry.path(), found);
        } else {
            found.insert(relative, fs::read(entry.path()).unwrap());
        }
    }
}

fn kinds(outcome: &SyncOutcome) -> Vec<(String, ConflictKind)> {
    outcome
        .conflicted
        .iter()
        .map(|c| (c.path.to_string(), c.kind))
        .collect()
}

const LINES: &[u8] = b"1\n2\n3\n4\n5\n6\n7\n";

#[test]
fn main_only_changes_are_written_and_main_deletes_are_removed() {
    let mut ws = Ws::new(&[("keep", b"keep\n"), ("gone", b"gone\n"), ("edit", b"old\n")]);
    let ours = ws.checkpoint();
    let main = ws.main(&[
        ("keep", b"keep\n"),
        ("edit", b"new\n"),
        ("dir/added", b"added\n"),
    ]);
    let outcome = ws.sync(ours, main).unwrap();
    assert_eq!(outcome.new_base, r(5));
    assert!(!outcome.already_current);
    assert!(outcome.conflicted.is_empty());
    assert_eq!(outcome.written, 3);
    assert_eq!(ws.text("edit"), "new\n");
    assert_eq!(ws.text("dir/added"), "added\n");
    assert!(!ws.exists("gone"));
    assert_eq!(ws.text("keep"), "keep\n");

    let added = ws.index.lookup(&path("dir/added")).unwrap();
    assert_eq!(added.hash, hash_object(ObjectKind::Blob, b"added\n"));
    assert_eq!(added.kind, EntryKind::File);
    assert!(ws.index.lookup(&path("gone")).is_none());
    assert_eq!(
        ws.index.lookup(&path("edit")).unwrap().hash,
        hash_object(ObjectKind::Blob, b"new\n")
    );
}

#[test]
fn edits_in_different_places_merge_without_markers() {
    let mut ws = Ws::new(&[("f", LINES)]);
    ws.write("f", b"one\n2\n3\n4\n5\n6\n7\n");
    let ours = ws.checkpoint();
    let main = ws.main(&[("f", b"1\n2\n3\n4\n5\n6\nseven\n")]);
    let outcome = ws.sync(ours, main).unwrap();
    assert!(outcome.conflicted.is_empty());
    assert_eq!(ws.text("f"), "one\n2\n3\n4\n5\n6\nseven\n");
    assert!(unresolved(&ws.root, &[]).unwrap().is_empty());
}

#[test]
fn edits_on_the_same_lines_get_markers_with_the_exact_labels() {
    let mut ws = Ws::new(&[("f", b"a\nold\nb\n")]);
    ws.write("f", b"a\nmine\nb\n");
    let ours = ws.checkpoint();
    let main = ws.main(&[("f", b"a\nmain\nb\n")]);
    let outcome = ws.sync_numbered(ours, main, r(12), r(13)).unwrap();
    assert_eq!(
        kinds(&outcome),
        [("f".to_string(), ConflictKind::BothModifiedText)]
    );
    let expected =
        "a\n<<<<<<< workspace\nmine\n||||||| base r12\nold\n=======\nmain\n>>>>>>> main r13\nb\n";
    assert_eq!(ws.text("f"), expected);
    assert!(!ws.exists("f.fold-main"));
    assert_eq!(
        ws.index.lookup(&path("f")).unwrap().hash,
        hash_object(ObjectKind::Blob, expected.as_bytes())
    );
    assert_eq!(outcome.new_base, r(13));
}

#[test]
fn a_binary_changed_on_both_sides_keeps_the_workspace_bytes_and_gains_a_sidecar() {
    let mut ws = Ws::new(&[("b.bin", b"\0one")]);
    ws.write("b.bin", b"\0two");
    let ours = ws.checkpoint();
    let main = ws.main(&[("b.bin", b"\0three")]);
    let outcome = ws.sync(ours, main).unwrap();
    assert_eq!(
        kinds(&outcome),
        [("b.bin".to_string(), ConflictKind::BothModifiedBinary)]
    );
    assert_eq!(ws.read("b.bin"), b"\0two");
    assert_eq!(ws.read("b.bin.fold-main"), b"\0three");
    assert_eq!(outcome.written, 1);
    assert!(ws.index.lookup(&path("b.bin.fold-main")).is_some());
}

#[test]
fn modify_delete_writes_a_sidecar_only_when_main_kept_the_file() {
    let mut ws = Ws::new(&[("edited", b"1\n"), ("deleted", b"1\n")]);
    ws.write("edited", b"mine\n");
    fs::remove_file(ws.root.join("deleted")).unwrap();
    let ours = ws.checkpoint();
    let main = ws.main(&[("deleted", b"main\n")]);
    let outcome = ws.sync(ours, main).unwrap();
    assert_eq!(
        kinds(&outcome),
        [
            ("deleted".to_string(), ConflictKind::ModifyDelete),
            ("edited".to_string(), ConflictKind::ModifyDelete),
        ]
    );
    assert_eq!(ws.text("edited"), "mine\n");
    assert!(!ws.exists("edited.fold-main"));
    assert!(!ws.exists("deleted"));
    assert_eq!(ws.text("deleted.fold-main"), "main\n");
}

#[test]
fn a_stale_sidecar_is_replaced_whatever_its_shape() {
    let mut ws = Ws::new(&[("a.bin", b"\0one"), ("b.bin", b"\0one")]);
    ws.write("a.bin", b"\0two");
    ws.write("b.bin", b"\0two");
    ws.write("a.bin.fold-main", b"stale");
    ws.write("b.bin.fold-main/old", b"stale");
    let ours = ws.checkpoint();
    let main = ws.main(&[("a.bin", b"\0three"), ("b.bin", b"\0four")]);
    ws.sync(ours, main).unwrap();
    assert_eq!(ws.read("a.bin.fold-main"), b"\0three");
    assert_eq!(ws.read("b.bin.fold-main"), b"\0four");
    assert!(ws.index.lookup(&path("b.bin.fold-main/old")).is_none());
}

#[test]
fn a_nested_conflict_writes_main_s_directory_beside_the_workspace_file() {
    let mut ws = Ws::new(&[("a/b", b"1\n")]);
    fs::remove_dir_all(ws.root.join("a")).unwrap();
    ws.write("a", b"file\n");
    ws.write("a.fold-main", b"stale from an earlier sync\n");
    let ours = ws.checkpoint();
    let main = ws.main(&[("a/b", b"2\n"), ("a/c/d", b"d\n")]);
    let outcome = ws.sync(ours, main).unwrap();
    assert_eq!(
        kinds(&outcome),
        [("a".to_string(), ConflictKind::TypeChange)]
    );
    assert_eq!(ws.text("a"), "file\n");
    assert_eq!(ws.text("a.fold-main/b"), "2\n");
    assert_eq!(ws.text("a.fold-main/c/d"), "d\n");
    assert_eq!(outcome.written, 2);

    let open = unresolved(&ws.root, &outcome.conflicted).unwrap();
    assert_eq!(open, [path("a")]);
    fs::remove_dir_all(ws.root.join("a.fold-main")).unwrap();
    assert!(
        unresolved(&ws.root, &outcome.conflicted)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_workspace_directory_against_a_main_file_keeps_the_directory() {
    let mut ws = Ws::new(&[("d/x", b"x\n")]);
    ws.write("d/extra", b"mine\n");
    let ours = ws.checkpoint();
    let main = ws.main(&[("d", b"file\n")]);
    let outcome = ws.sync(ours, main).unwrap();
    assert_eq!(
        kinds(&outcome),
        [("d".to_string(), ConflictKind::TypeChange)]
    );
    assert_eq!(ws.text("d/x"), "x\n");
    assert_eq!(ws.text("d/extra"), "mine\n");
    assert_eq!(ws.text("d.fold-main"), "file\n");
    assert_eq!(
        unresolved(&ws.root, &outcome.conflicted).unwrap(),
        [path("d")]
    );
}

#[test]
fn main_turning_a_file_into_a_directory_and_back_is_written() {
    let mut ws = Ws::new(&[("a", b"file\n"), ("d/x", b"x\n"), ("d/y/z", b"z\n")]);
    let ours = ws.checkpoint();
    let main = ws.main(&[("a/b", b"nested\n"), ("d", b"now a file\n")]);
    let outcome = ws.sync(ours, main).unwrap();
    assert!(outcome.conflicted.is_empty());
    assert_eq!(ws.text("a/b"), "nested\n");
    assert_eq!(ws.text("d"), "now a file\n");
}

#[test]
fn modes_and_symlinks_from_main_are_written() {
    let mut ws = Ws::new(&[("keep", b"k\n")]);
    let ours = ws.checkpoint();
    let main = tree_of(
        &ws.store,
        &[
            ("keep", EntryKind::File, b"k\n"),
            ("link", EntryKind::Symlink, b"keep"),
            ("run.sh", EntryKind::Executable, b"#!/bin/sh\n"),
        ],
    );
    ws.sync(ours, main).unwrap();
    assert_eq!(
        fs::read_link(ws.root.join("link")).unwrap(),
        PathBuf::from("keep")
    );
    let mode = fs::metadata(ws.root.join("run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0);
    assert_eq!(
        ws.index.lookup(&path("link")).unwrap().kind,
        EntryKind::Symlink
    );
}

#[test]
fn a_second_sync_at_the_same_head_is_already_current_and_writes_nothing() {
    let mut ws = Ws::new(&[("f", b"1\n")]);
    let ours = ws.checkpoint();
    let main = ws.main(&[("f", b"2\n"), ("g", b"g\n")]);
    let first = ws.sync_numbered(ours, main, r(1), r(2)).unwrap();
    assert_eq!(first.new_base, r(2));
    fs::remove_file(ws.root.join("g")).unwrap();
    let before = ws.listing();
    let ours = ws.checkpoint();
    let second = ws.sync_numbered(ours, main, r(2), r(2)).unwrap();
    assert_eq!(
        second,
        SyncOutcome {
            new_base: r(2),
            conflicted: Vec::new(),
            written: 0,
            already_current: true,
        }
    );
    assert_eq!(ws.listing(), before);
}

#[test]
fn an_unrecorded_file_main_did_not_touch_is_unchanged() {
    let mut ws = Ws::new(&[("keep", b"keep\n"), ("f", b"1\n")]);
    let ours = ws.checkpoint();
    ws.write("notes.txt", b"unrecorded\n");
    ws.write("keep", b"edited after the checkpoint\n");
    let main = ws.main(&[("keep", b"keep\n"), ("f", b"2\n")]);
    ws.sync(ours, main).unwrap();
    assert_eq!(ws.text("notes.txt"), "unrecorded\n");
    assert_eq!(ws.text("keep"), "edited after the checkpoint\n");
    assert_eq!(ws.text("f"), "2\n");
}

fn assert_refused(ws: &mut Ws, ours: ObjectHash, main: ObjectHash, paths: &[&str]) {
    let before = ws.listing();
    let error = ws.sync(ours, main).unwrap_err();
    assert_eq!(error.code(), codes::WORKSPACE_CHANGED_DURING_SYNC);
    assert_eq!(error.details().unwrap()["paths"], serde_json::json!(paths));
    assert_eq!(ws.listing(), before);
}

#[test]
fn a_file_edited_between_the_checkpoint_and_the_sync_refuses_the_sync() {
    let mut ws = Ws::new(&[("f", b"1\n"), ("gone", b"g\n")]);
    let ours = ws.checkpoint();
    ws.write("f", b"edited after the checkpoint\n");
    let main = ws.main(&[("f", b"2\n"), ("new", b"n\n")]);
    assert_refused(&mut ws, ours, main, &["f"]);
    assert!(!ws.exists("new"));
    assert!(ws.exists("gone"));
}

#[test]
fn a_changed_removal_or_marker_target_refuses_the_sync() {
    let mut ws = Ws::new(&[("gone", b"g\n"), ("m", b"a\nold\nb\n")]);
    ws.write("m", b"a\nmine\nb\n");
    let ours = ws.checkpoint();
    ws.write("gone", b"edited\n");
    fs::remove_file(ws.root.join("m")).unwrap();
    let main = ws.main(&[("m", b"a\nmain\nb\n")]);
    assert_refused(&mut ws, ours, main, &["gone", "m"]);
}

#[test]
fn a_mode_change_or_a_link_in_place_of_a_file_refuses_the_sync() {
    let mut ws = Ws::new(&[("f", b"1\n"), ("g", b"1\n")]);
    let ours = ws.checkpoint();
    fs::set_permissions(ws.root.join("f"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_file(ws.root.join("g")).unwrap();
    symlink("f", ws.root.join("g")).unwrap();
    let main = ws.main(&[("f", b"2\n"), ("g", b"2\n")]);
    assert_refused(&mut ws, ours, main, &["f", "g"]);
}

#[test]
fn an_unrecorded_file_where_main_writes_refuses_the_sync() {
    let mut ws = Ws::new(&[("d/x", b"x\n"), ("e", b"e\n")]);
    let ours = ws.checkpoint();
    ws.write("new", b"unrecorded\n");
    ws.write("d/extra", b"unrecorded\n");
    fs::remove_file(ws.root.join("e")).unwrap();
    ws.write("e", b"e\n");
    let main = ws.main(&[
        ("new", b"main\n"),
        ("d", b"file\n"),
        ("e/f", b"below a file main removes\n"),
    ]);
    let before = ws.listing();
    let error = ws.sync(ours, main).unwrap_err();
    assert_eq!(error.code(), codes::WORKSPACE_CHANGED_DURING_SYNC);
    let paths = &error.details().unwrap()["paths"];
    assert_eq!(*paths, serde_json::json!(["d", "new"]));
    assert_eq!(ws.listing(), before);
}

#[test]
fn a_main_file_replaces_empty_directories() {
    let mut ws = Ws::new(&[("d/x", b"x\n")]);
    let ours = ws.checkpoint();
    fs::create_dir_all(ws.root.join("empty/inner")).unwrap();
    fs::create_dir_all(ws.root.join("d/left/over")).unwrap();
    let main = ws.main(&[("empty", b"file\n"), ("d", b"file\n")]);
    let outcome = ws.sync(ours, main).unwrap();
    assert!(outcome.conflicted.is_empty());
    assert_eq!(ws.text("empty"), "file\n");
    assert_eq!(ws.text("d"), "file\n");
    assert!(ws.index.lookup(&path("d/x")).is_none());
}

#[test]
fn unrecorded_content_where_a_sidecar_goes_refuses_the_sync() {
    let mut ws = Ws::new(&[
        ("a.bin", b"\0one"),
        ("b.bin", b"\0one"),
        ("c.bin", b"\0one"),
    ]);
    for name in ["a.bin", "b.bin", "c.bin"] {
        ws.write(name, b"\0two");
    }
    ws.write("c.bin.fold-main", b"recorded");
    let ours = ws.checkpoint();
    ws.write("a.bin.fold-main", b"written after the checkpoint");
    ws.write("b.bin.fold-main/notes", b"written after the checkpoint");
    fs::create_dir(ws.root.join("b.bin.fold-main/empty")).unwrap();
    ws.write("c.bin.fold-main", b"edited after the checkpoint");
    let main = ws.main(&[
        ("a.bin", b"\0three"),
        ("b.bin", b"\0three"),
        ("c.bin", b"\0three"),
    ]);
    assert_refused(
        &mut ws,
        ours,
        main,
        &[
            "a.bin.fold-main",
            "b.bin.fold-main/notes",
            "c.bin.fold-main",
        ],
    );
}

#[test]
fn a_sidecar_takes_a_path_main_tracks() {
    let mut ws = Ws::new(&[("a.bin", b"\0one"), ("b.bin", b"\0one")]);
    ws.write("a.bin", b"\0two");
    ws.write("b.bin", b"\0two");
    let ours = ws.checkpoint();
    let main = ws.main(&[
        ("a.bin", b"\0three"),
        ("a.bin.fold-main/tracked", b"main's file\n"),
        ("b.bin", b"\0three"),
        ("b.bin.fold-main", b"main's file\n"),
    ]);
    let outcome = ws.sync(ours, main).unwrap();
    assert_eq!(ws.read("a.bin.fold-main"), b"\0three");
    assert_eq!(ws.read("b.bin.fold-main"), b"\0three");
    assert_eq!(outcome.written, 2);
}

#[test]
fn a_marker_file_takes_the_mode_main_set() {
    let mut ws = Ws::new(&[("f", b"a\nold\nb\n")]);
    ws.write("f", b"a\nmine\nb\n");
    let ours = ws.checkpoint();
    let main = tree_of(&ws.store, &[("f", EntryKind::Executable, b"a\nmain\nb\n")]);
    let outcome = ws.sync(ours, main).unwrap();
    assert_eq!(
        kinds(&outcome),
        [("f".to_string(), ConflictKind::BothModifiedText)]
    );
    let mode = fs::metadata(ws.root.join("f"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0);
    assert_eq!(
        ws.index.lookup(&path("f")).unwrap().kind,
        EntryKind::Executable
    );
}

/// Every file a sync writes holds an object, so restoring the checkpoint taken before it undoes the sync, also one stopped part-way.
#[test]
fn restoring_the_checkpoint_taken_before_a_sync_undoes_it() {
    let mut ws = Ws::new(&[
        ("text", b"a\nold\nb\n"),
        ("bin", b"\0one"),
        ("gone", b"g\n"),
        ("edit", b"1\n"),
    ]);
    ws.write("text", b"a\nmine\nb\n");
    ws.write("bin", b"\0two");
    let ours = ws.checkpoint();
    let before = ws.listing();
    let main = ws.main(&[
        ("text", b"a\nmain\nb\n"),
        ("bin", b"\0three"),
        ("edit", b"2\n"),
        ("added", b"added\n"),
    ]);
    ws.sync(ours, main).unwrap();
    assert_ne!(ws.listing(), before);
    ws.index.save(&ws.index_path).unwrap();

    let rules = IgnoreRules::new(&ws.root);
    let opts = RestoreOptions {
        scan: ScanOptions {
            rules: &rules,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            caps: HOST,
            recorded: None,
        },
        write: opts(HOST, false),
    };
    restore_tree(
        &ws.store,
        &ws.root,
        &mut ws.index,
        &ws.index_path,
        &ours,
        &opts,
    )
    .unwrap();
    assert_eq!(ws.listing(), before);
}

#[test]
fn a_file_in_place_of_a_parent_directory_refuses_the_sync() {
    let mut ws = Ws::new(&[("keep", b"k\n")]);
    let ours = ws.checkpoint();
    ws.write("dir", b"unrecorded file where main has a directory\n");
    let main = ws.main(&[("keep", b"k\n"), ("dir/a", b"a\n")]);
    assert_refused(&mut ws, ours, main, &["dir/a"]);
}

#[test]
fn unresolved_lists_markers_and_sidecars_until_both_are_gone() {
    let mut ws = Ws::new(&[("text", b"a\nold\nb\n"), ("bin", b"\0one")]);
    ws.write("text", b"a\nmine\nb\n");
    ws.write("bin", b"\0two");
    let ours = ws.checkpoint();
    let main = ws.main(&[("text", b"a\nmain\nb\n"), ("bin", b"\0three")]);
    let outcome = ws.sync(ours, main).unwrap();
    let conflicted = &outcome.conflicted;
    assert_eq!(
        unresolved(&ws.root, conflicted).unwrap(),
        [path("bin"), path("text")]
    );
    ws.write("text", b"a\nresolved\nb\n");
    assert_eq!(unresolved(&ws.root, conflicted).unwrap(), [path("bin")]);
    fs::remove_file(ws.root.join("bin.fold-main")).unwrap();
    assert!(unresolved(&ws.root, conflicted).unwrap().is_empty());
    fs::remove_file(ws.root.join("text")).unwrap();
    assert!(unresolved(&ws.root, conflicted).unwrap().is_empty());
}

#[test]
fn write_merge_uses_the_caller_s_labels() {
    let mut ws = Ws::new(&[("f", b"a\nold\nb\n")]);
    ws.write("f", b"a\nmine\nb\n");
    let ours_tree = ws.checkpoint();
    let main_tree = ws.main(&[("f", b"a\nmain\nb\n")]);
    let load = |hash: &ObjectHash| {
        let object = ws.store.read_expecting(hash, ObjectKind::Tree)?;
        Ok(Tree::decode(&object.payload)?)
    };
    let base = flatten(&ws.base, &load).unwrap();
    let ours = flatten(&ours_tree, &load).unwrap();
    let main = flatten(&main_tree, &load).unwrap();
    let blob = |hash: &ObjectHash| Ok(ws.store.read_expecting(hash, ObjectKind::Blob)?.payload);
    let merge = merge_trees(&base, &ours, &main, &blob).unwrap();
    let labels = MarkerLabels {
        ours: "sync workspace".into(),
        base: Some("export".into()),
        theirs: "remote".into(),
    };
    let conflicted = write_merge(
        &ws.store,
        &ws.root,
        &mut ws.index,
        &merge,
        &ours_tree,
        &labels,
    )
    .unwrap();
    assert_eq!(
        conflicted,
        [ConflictedPath {
            path: path("f"),
            kind: ConflictKind::BothModifiedText
        }]
    );
    assert_eq!(
        ws.text("f"),
        "a\n<<<<<<< sync workspace\nmine\n||||||| export\nold\n=======\nmain\n>>>>>>> remote\nb\n"
    );
}

#[test]
fn sidecar_names_append_the_suffix() {
    assert_eq!(sidecar(&path("a/b.rs")).unwrap(), path("a/b.rs.fold-main"));
    assert_eq!(
        MarkerLabels::sync(r(4), r(9)),
        MarkerLabels {
            ours: "workspace".into(),
            base: Some("base r4".into()),
            theirs: "main r9".into(),
        }
    );
}
