#![cfg(test)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::tree::{EntryKind, FlatTree, Tree, flatten};
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::index::StatIndex;
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::Capabilities;
use fold_vcs::restore::{
    RestoreOptions, Restored, differs_from_latest, record_checkpoint_tree, reset_tree, restore_tree,
};
use fold_vcs::scan::{DEFAULT_MAX_FILE_BYTES, ScanOptions, scan};

use crate::common::*;

struct Fixture {
    dir: tempfile::TempDir,
    store: ObjectStore,
    root: PathBuf,
    index_path: PathBuf,
    index: StatIndex,
    caps: Capabilities,
    max: u64,
}

fn fixture(caps: Capabilities) -> Fixture {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    fs::create_dir(&root).unwrap();
    let index_path = dir.path().join(".fold/ws-index");
    Fixture {
        dir,
        store,
        root,
        index_path,
        index: StatIndex::empty(caps),
        caps,
        max: DEFAULT_MAX_FILE_BYTES,
    }
}

impl Fixture {
    fn checkpoint(&mut self, base: Option<ObjectHash>) -> ObjectHash {
        let rules = IgnoreRules::new(&self.root);
        let opts = ScanOptions {
            rules: &rules,
            max_file_bytes: self.max,
            caps: self.caps,
            recorded: None,
        };
        let snap = record_checkpoint_tree(
            &self.store,
            &self.root,
            &mut self.index,
            &self.index_path,
            base,
            &opts,
        )
        .unwrap();
        assert!(snap.warnings.is_empty(), "{:?}", snap.warnings);
        assert!(self.store.contains(&snap.tree).unwrap());
        snap.tree
    }

    fn restore(&mut self, target: &ObjectHash) -> Result<Restored, fold_core::FoldError> {
        let rules = IgnoreRules::new(&self.root);
        let opts = RestoreOptions {
            scan: ScanOptions {
                rules: &rules,
                max_file_bytes: self.max,
                caps: self.caps,
                recorded: None,
            },
            write: opts(self.caps, false),
        };
        restore_tree(
            &self.store,
            &self.root,
            &mut self.index,
            &self.index_path,
            target,
            &opts,
        )
    }

    /// A scan with an empty stat index, so every file is read from disk.
    fn fresh_scan(&self) -> FlatTree {
        self.scan_with(&mut StatIndex::empty(self.caps))
    }

    fn scan_with(&self, index: &mut StatIndex) -> FlatTree {
        let rules = IgnoreRules::new(&self.root);
        let opts = ScanOptions {
            rules: &rules,
            max_file_bytes: self.max,
            caps: self.caps,
            recorded: None,
        };
        scan(&self.root, index, &opts).unwrap().entries
    }

    fn flat(&self, tree: &ObjectHash) -> FlatTree {
        flatten(tree, &|hash| {
            let object = self.store.read_expecting(hash, ObjectKind::Tree)?;
            Ok(Tree::decode(&object.payload)?)
        })
        .unwrap()
    }
}

fn names(paths: &[fold_vcs::paths::RepoPath]) -> Vec<String> {
    paths.iter().map(ToString::to_string).collect()
}

fn read(root: &Path, path: &str) -> String {
    fs::read_to_string(root.join(path)).unwrap()
}

#[test]
fn a_restore_returns_edits_additions_and_deletions_to_the_checkpoint() {
    let mut fx = fixture(HOST);
    write(&fx.root, "keep.txt", b"same");
    write(&fx.root, "edit.txt", b"before");
    write(&fx.root, "gone.txt", b"deleted later");
    write(&fx.root, "src/deep/mod.rs", b"mod deep;");
    write(&fx.root, "swap", b"a file that becomes a directory");
    write(
        &fx.root,
        "folder/inner.txt",
        b"a directory that becomes a file",
    );
    write(&fx.root, "run.sh", b"#!/bin/sh\n");
    fs::set_permissions(fx.root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("keep.txt", fx.root.join("link")).unwrap();
    let target = fx.checkpoint(None);

    write(&fx.root, "edit.txt", b"after");
    fs::remove_file(fx.root.join("gone.txt")).unwrap();
    write(&fx.root, "added.txt", b"new");
    write(&fx.root, "new/dir/file.txt", b"nested");
    fs::remove_file(fx.root.join("src/deep/mod.rs")).unwrap();
    fs::remove_file(fx.root.join("swap")).unwrap();
    write(&fx.root, "swap/child.txt", b"now a directory");
    fs::remove_dir_all(fx.root.join("folder")).unwrap();
    write(&fx.root, "folder", b"now a file");
    fs::set_permissions(fx.root.join("run.sh"), fs::Permissions::from_mode(0o644)).unwrap();
    fs::remove_file(fx.root.join("link")).unwrap();
    symlink("edit.txt", fx.root.join("link")).unwrap();
    let edited = fx.checkpoint(Some(target));
    assert_ne!(edited, target);

    let restored = fx.restore(&target).unwrap();
    assert_eq!(
        fx.fresh_scan(),
        fx.flat(&target),
        "a fresh scan equals the target"
    );
    assert_eq!(
        names(&restored.removed),
        vec!["added.txt", "folder", "new/dir/file.txt", "swap/child.txt"]
    );
    assert_eq!(
        names(&restored.written),
        vec![
            "edit.txt",
            "folder/inner.txt",
            "gone.txt",
            "link",
            "run.sh",
            "src/deep/mod.rs",
            "swap"
        ]
    );
    assert!(restored.degraded.is_empty());
    assert_eq!(read(&fx.root, "edit.txt"), "before");
    assert_eq!(
        fs::read_link(fx.root.join("link")).unwrap(),
        Path::new("keep.txt")
    );
    assert!(
        !fx.root.join("new").exists(),
        "emptied directories are removed"
    );
    let mode = fs::metadata(fx.root.join("run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111);

    let mut saved = StatIndex::load(&fx.index_path, HOST);
    assert_eq!(saved.len(), fx.flat(&target).len());
    assert_eq!(fx.scan_with(&mut saved), fx.flat(&target));
    assert_eq!(fx.checkpoint(Some(edited)), target);

    let again = fx.restore(&target).unwrap();
    assert!(again.written.is_empty() && again.removed.is_empty());
}

#[test]
fn ignored_and_skipped_files_stay() {
    let mut fx = fixture(HOST);
    fx.max = 64;
    write(&fx.root, ".gitignore", b"*.log\nbuild/\n");
    write(&fx.root, "src/main.rs", b"fn main() {}");
    write(&fx.root, "old.log", b"ignored before the checkpoint");
    let target = fx.checkpoint(None);

    write(&fx.root, "src/main.rs", b"fn main() { edit() }");
    write(&fx.root, "src/extra.rs", b"tracked");
    write(&fx.root, "new.log", b"ignored after the checkpoint");
    write(&fx.root, "build/out.bin", b"ignored directory");
    write(&fx.root, "src/debug.log", b"ignored below a folder");
    fx.checkpoint(Some(target));
    write(&fx.root, "big.bin", &[7u8; 100]);

    let restored = fx.restore(&target).unwrap();
    assert_eq!(names(&restored.removed), vec!["src/extra.rs"]);
    assert_eq!(names(&restored.written), vec!["src/main.rs"]);
    assert_eq!(fx.fresh_scan(), fx.flat(&target));
    assert_eq!(read(&fx.root, "old.log"), "ignored before the checkpoint");
    assert_eq!(read(&fx.root, "new.log"), "ignored after the checkpoint");
    assert_eq!(read(&fx.root, "build/out.bin"), "ignored directory");
    assert_eq!(read(&fx.root, "src/debug.log"), "ignored below a folder");
    assert_eq!(fs::read(fx.root.join("big.bin")).unwrap().len(), 100);
}

#[test]
fn a_restore_over_a_symlink_never_writes_through_it() {
    let mut fx = fixture(HOST);
    let outside = fx.dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.txt"), b"outside").unwrap();
    write(&fx.root, "secret.txt", b"inside file");
    write(&fx.root, "sub/file.txt", b"inside directory");
    let target = fx.checkpoint(None);

    fs::remove_file(fx.root.join("secret.txt")).unwrap();
    symlink(outside.join("secret.txt"), fx.root.join("secret.txt")).unwrap();
    fs::remove_dir_all(fx.root.join("sub")).unwrap();
    symlink(&outside, fx.root.join("sub")).unwrap();
    fx.checkpoint(Some(target));

    fx.restore(&target).unwrap();
    assert_eq!(fs::read(outside.join("secret.txt")).unwrap(), b"outside");
    assert!(!outside.join("file.txt").exists());
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    let file = fs::symlink_metadata(fx.root.join("secret.txt")).unwrap();
    assert!(file.file_type().is_file());
    let dir = fs::symlink_metadata(fx.root.join("sub")).unwrap();
    assert!(dir.file_type().is_dir());
    assert_eq!(read(&fx.root, "secret.txt"), "inside file");
    assert_eq!(read(&fx.root, "sub/file.txt"), "inside directory");
    assert_eq!(fx.fresh_scan(), fx.flat(&target));
}

#[test]
fn a_missing_object_stops_the_restore_before_any_change() {
    let mut fx = fixture(HOST);
    write(&fx.root, "a.txt", b"current");
    write(&fx.root, "b.txt", b"current");
    fx.checkpoint(None);
    let lost = ObjectHash::from_bytes([9; 32]);
    let present = blob(&fx.store, b"present");
    let target = tree(
        &fx.store,
        &[
            ("a.txt", EntryKind::File, present),
            ("lost.txt", EntryKind::File, lost),
        ],
    );
    let error = fx.restore(&target).unwrap_err();
    assert_eq!(error.code(), "object_missing");
    assert_eq!(read(&fx.root, "a.txt"), "current");
    assert_eq!(read(&fx.root, "b.txt"), "current");
}

#[test]
fn without_symlink_support_a_link_is_written_as_a_file() {
    let mut fx = fixture(BARE);
    write(&fx.root, "plain.txt", b"text");
    let first = fx.checkpoint(None);
    let link = blob(&fx.store, b"plain.txt");
    let plain = fx.flat(&first)[b"plain.txt".as_slice()].hash;
    let target = tree(
        &fx.store,
        &[
            ("link", EntryKind::Symlink, link),
            ("plain.txt", EntryKind::File, plain),
        ],
    );
    let restored = fx.restore(&target).unwrap();
    assert_eq!(names(&restored.degraded), vec!["link"]);
    assert_eq!(read(&fx.root, "link"), "plain.txt");
    let mut saved = StatIndex::load(&fx.index_path, BARE);
    assert_eq!(fx.scan_with(&mut saved), fx.flat(&target));
}

#[test]
fn differs_from_latest_compares_trees() {
    let mut fx = fixture(HOST);
    write(&fx.root, "a.txt", b"one");
    let latest = fx.checkpoint(None);
    let same = fx.checkpoint(Some(latest));
    assert!(!differs_from_latest(Some(&latest), &same));
    assert!(differs_from_latest(None, &same));
    write(&fx.root, "a.txt", b"two");
    let edited = fx.checkpoint(Some(latest));
    assert!(differs_from_latest(Some(&latest), &edited));
}

fn refused_paths(error: &fold_core::FoldError) -> Vec<String> {
    assert_eq!(error.code(), "unrecorded_changes");
    serde_json::from_value(error.details().unwrap()["paths"].clone()).unwrap()
}

#[test]
fn changes_no_checkpoint_holds_refuse_the_restore() {
    let mut fx = fixture(HOST);
    write(&fx.root, "a.txt", b"one");
    write(&fx.root, "b.txt", b"one");
    let target = fx.checkpoint(None);

    write(&fx.root, "a.txt", b"edited after the checkpoint");
    write(&fx.root, "new.txt", b"added after the checkpoint");
    let error = fx.restore(&target).unwrap_err();
    assert_eq!(refused_paths(&error), vec!["a.txt", "new.txt"]);
    assert_eq!(read(&fx.root, "a.txt"), "edited after the checkpoint");
    assert_eq!(read(&fx.root, "new.txt"), "added after the checkpoint");

    fx.checkpoint(Some(target));
    fx.restore(&target).unwrap();
    assert_eq!(fx.fresh_scan(), fx.flat(&target));
}

#[test]
fn untracked_files_where_the_target_writes_refuse_the_restore() {
    let mut fx = fixture(HOST);
    write(&fx.root, ".gitignore", b"");
    write(&fx.root, "notes.txt", b"tracked");
    write(&fx.root, "swap", b"a file");
    write(&fx.root, "out/report.txt", b"report");
    let target = fx.checkpoint(None);

    write(&fx.root, ".gitignore", b"*.log\nnotes.txt\nout\n");
    write(&fx.root, "notes.txt", b"private");
    fs::remove_file(fx.root.join("swap")).unwrap();
    write(&fx.root, "swap/tracked.txt", b"tracked");
    write(&fx.root, "swap/build.log", b"ignored");
    fs::remove_dir_all(fx.root.join("out")).unwrap();
    write(&fx.root, "out", b"ignored file over a target directory");
    let current = fx.checkpoint(Some(target));
    // The target tracks notes.txt, so the checkpoint records its new content although a rule now matches it, and the restore loses nothing there.
    let mut scanned = fx.fresh_scan();
    scanned.insert(
        b"notes.txt".to_vec(),
        fx.flat(&current)[b"notes.txt".as_slice()],
    );
    assert_eq!(scanned, fx.flat(&current));

    let error = fx.restore(&target).unwrap_err();
    assert_eq!(refused_paths(&error), vec!["out/report.txt", "swap"]);
    assert_eq!(read(&fx.root, ".gitignore"), "*.log\nnotes.txt\nout\n");
    assert_eq!(read(&fx.root, "notes.txt"), "private");
    assert_eq!(read(&fx.root, "swap/build.log"), "ignored");

    fs::remove_file(fx.root.join("swap/build.log")).unwrap();
    fs::remove_file(fx.root.join("out")).unwrap();
    fx.restore(&target).unwrap();
    assert_eq!(read(&fx.root, "notes.txt"), "tracked");
    assert_eq!(fx.fresh_scan(), fx.flat(&target));
}

#[test]
fn a_restore_stopped_part_way_leaves_a_true_index_and_can_run_again() {
    let mut fx = fixture(HOST);
    write(&fx.root, "a.txt", b"one");
    write(&fx.root, "locked/b.txt", b"one");
    let target = fx.checkpoint(None);
    write(&fx.root, "a.txt", b"two");
    write(&fx.root, "locked/b.txt", b"two");
    fx.checkpoint(Some(target));

    fs::set_permissions(fx.root.join("locked"), fs::Permissions::from_mode(0o555)).unwrap();
    let error = fx.restore(&target).unwrap_err();
    fs::set_permissions(fx.root.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(error.code(), "workspace_io");
    assert_eq!(read(&fx.root, "a.txt"), "one", "the first write landed");
    assert_eq!(read(&fx.root, "locked/b.txt"), "two");
    assert_eq!(
        fs::read_dir(fx.root.join(".fold/tmp")).unwrap().count(),
        0,
        "no staged file is left behind"
    );
    let mut saved = StatIndex::load(&fx.index_path, HOST);
    assert_eq!(fx.scan_with(&mut saved), fx.fresh_scan());

    fx.index = StatIndex::load(&fx.index_path, HOST);
    fx.restore(&target).unwrap();
    assert_eq!(fx.fresh_scan(), fx.flat(&target));
}

#[test]
fn empty_directories_where_the_target_has_files_are_cleared() {
    let mut fx = fixture(HOST);
    write(&fx.root, "flat", b"a file");
    write(&fx.root, "deep", b"another file");
    let target = fx.checkpoint(None);
    fs::remove_file(fx.root.join("flat")).unwrap();
    fs::remove_file(fx.root.join("deep")).unwrap();
    fs::create_dir(fx.root.join("flat")).unwrap();
    fs::create_dir_all(fx.root.join("deep/inner/most")).unwrap();
    fx.checkpoint(Some(target));

    let restored = fx.restore(&target).unwrap();
    assert_eq!(names(&restored.written), vec!["deep", "flat"]);
    assert_eq!(read(&fx.root, "flat"), "a file");
    assert_eq!(read(&fx.root, "deep"), "another file");
    assert_eq!(fx.fresh_scan(), fx.flat(&target));
}

#[test]
fn a_reset_writes_the_target_over_unrecorded_files_and_keeps_ignored_ones() {
    let mut fx = fixture(HOST);
    write(&fx.root, "keep.txt", b"same");
    write(&fx.root, "src/lib.rs", b"v1");
    write(&fx.root, ".gitignore", b"node_modules/\n");
    let target = fx.checkpoint(None);
    write(&fx.root, "src/lib.rs", b"never recorded");
    write(&fx.root, "generated.txt", b"a check wrote this");
    write(&fx.root, "node_modules/dep.js", b"installed");
    let refused = fx.restore(&target);
    assert!(refused.is_err(), "a restore keeps unrecorded work");

    let rules = IgnoreRules::new(&fx.root);
    let opts = RestoreOptions {
        scan: ScanOptions {
            rules: &rules,
            max_file_bytes: fx.max,
            caps: fx.caps,
            recorded: None,
        },
        write: opts(fx.caps, false),
    };
    reset_tree(
        &fx.store,
        &fx.root,
        &mut fx.index,
        &fx.index_path,
        &target,
        &opts,
    )
    .unwrap();
    assert_eq!(read(&fx.root, "src/lib.rs"), "v1");
    assert!(!fx.root.join("generated.txt").exists());
    assert_eq!(read(&fx.root, "node_modules/dep.js"), "installed");
    assert_eq!(fx.fresh_scan(), fx.flat(&target));

    let empty = fx.dir.path().join("checks/main");
    reset_tree(
        &fx.store,
        &empty,
        &mut StatIndex::empty(fx.caps),
        &fx.dir.path().join("checks/main.index"),
        &target,
        &opts,
    )
    .unwrap();
    assert_eq!(read(&empty, "keep.txt"), "same");
}
