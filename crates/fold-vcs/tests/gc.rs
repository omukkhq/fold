#![cfg(test)]

use crate::common;

use std::collections::HashSet;
use std::fs::{self, File};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::revision::Revision;
use fold_core::tree::EntryKind;
use fold_core::{IntegrationId, RevisionId, Timestamp};
use fold_vcs::gc::{GRACE, locate_workspace_dir, mark, plan_sweep, remove_workspace_dir, sweep};
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::index::StatIndex;
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::write_revision;
use fold_vcs::scan::{DEFAULT_MAX_FILE_BYTES, ScanOptions};
use fold_vcs::snapshot::snapshot;

use crate::common::{HOST, blob, store, tree};

const OLD: Duration = Duration::from_secs(2 * 60 * 60);
const YOUNG: Duration = Duration::from_secs(10 * 60);

fn age_file(path: &Path, by: Duration) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - by)
        .unwrap();
}

fn age_all(objects: &ObjectStore, by: Duration) {
    for entry in objects.entries().unwrap() {
        age_file(&objects.path(&entry.hash), by);
    }
}

/// A tree holding one file with `content`, and that file's blob.
fn single(objects: &ObjectStore, content: &str) -> (ObjectHash, ObjectHash) {
    let file = blob(objects, content.as_bytes());
    (tree(objects, &[("file", EntryKind::File, file)]), file)
}

fn first_revision(tree: ObjectHash) -> Revision {
    Revision {
        tree,
        parent: None,
        number: RevisionId::from_number(1).unwrap(),
        integration: None,
        created_at: Timestamp::EPOCH,
    }
}

#[test]
fn young_orphans_stay_and_old_orphans_go() {
    let (_dir, objects) = store();
    let (kept_tree, kept_blob) = single(&objects, "kept");
    let old = blob(&objects, b"old orphan");
    age_all(&objects, OLD);
    let young = blob(&objects, b"young orphan");
    age_file(&objects.path(&young), YOUNG);
    let freed = std::fs::metadata(objects.path(&old)).unwrap().len();
    let marked = mark(&objects, [kept_tree]).unwrap();
    let report = sweep(&objects, &marked, SystemTime::now(), GRACE).unwrap();
    assert_eq!(
        (report.examined, report.deleted, report.bytes_freed),
        (4, 1, freed)
    );
    for hash in [kept_tree, kept_blob, young] {
        assert!(objects.contains(&hash).unwrap());
    }
    assert!(!objects.contains(&old).unwrap());
}

#[test]
fn a_blob_a_snapshot_reuses_during_the_run_survives() {
    let (dir, objects) = store();
    let (base_tree, _) = single(&objects, "base");
    let reused = blob(&objects, b"written long ago, then again");
    age_all(&objects, OLD);
    let marked = mark(&objects, [base_tree]).unwrap();
    assert!(!marked.contains(&reused));

    let root = dir.path().join("ws").join("t-1");
    common::write(&root, "notes.txt", b"written long ago, then again");
    let rules = IgnoreRules::new(&root);
    let opts = ScanOptions {
        rules: &rules,
        max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        caps: HOST,
        recorded: None,
    };
    let mut index = StatIndex::empty(HOST);
    let index_path = dir.path().join(".fold").join("ws-1.index");
    snapshot(&objects, &root, &mut index, &index_path, None, &opts).unwrap();

    let report = sweep(&objects, &marked, SystemTime::now(), GRACE).unwrap();
    assert!(objects.contains(&reused).unwrap());
    // The snapshot's new tree is young and kept too; nothing else is unreferenced.
    assert_eq!(report.deleted, 0);
}

#[test]
fn scratch_files_go_after_the_grace_period() {
    let (_dir, objects) = store();
    let scratch = objects.scratch_dir().to_owned();
    fs::write(scratch.join("old"), b"x").unwrap();
    fs::write(scratch.join("young"), b"x").unwrap();
    fs::create_dir(scratch.join("dir")).unwrap();
    age_file(&scratch.join("old"), OLD);
    age_file(&scratch.join("young"), YOUNG);

    let planned = plan_sweep(&objects, &HashSet::new(), SystemTime::now(), GRACE).unwrap();
    assert_eq!(planned.tmp_removed, 1);
    assert!(scratch.join("old").exists());
    let report = sweep(&objects, &HashSet::new(), SystemTime::now(), GRACE).unwrap();
    assert_eq!(report.tmp_removed, 1);
    assert!(!scratch.join("old").exists());
    assert!(scratch.join("young").exists() && scratch.join("dir").is_dir());
}

#[test]
fn a_plan_counts_what_a_sweep_deletes_and_deletes_nothing() {
    let (_dir, objects) = store();
    let kept = blob(&objects, b"kept");
    let orphan = blob(&objects, b"orphan");
    age_all(&objects, OLD);
    let marked: HashSet<_> = [kept].into();
    let planned = plan_sweep(&objects, &marked, SystemTime::now(), GRACE).unwrap();
    assert!(objects.contains(&orphan).unwrap());
    let swept = sweep(&objects, &marked, SystemTime::now(), GRACE).unwrap();
    assert_eq!(planned, swept);
    assert_eq!(swept.deleted, 1);
    assert!(!objects.contains(&orphan).unwrap());
}

#[test]
fn mark_walks_revision_parents_and_reports_missing_objects() {
    let (_dir, objects) = store();
    let (first_tree, first_blob) = single(&objects, "first");
    let (second_tree, second_blob) = single(&objects, "second");
    let lost_blob = blob(&objects, b"lost");
    let sub = tree(&objects, &[("lost", EntryKind::File, lost_blob)]);
    let with_dir = tree(
        &objects,
        &[
            ("file", EntryKind::File, second_blob),
            ("sub", EntryKind::Directory, sub),
        ],
    );
    fs::remove_file(objects.path(&lost_blob)).unwrap();
    let r1 = first_revision(first_tree);
    write_revision(&objects, &r1).unwrap();
    let r2 = Revision {
        tree: with_dir,
        parent: Some(r1.hash()),
        number: RevisionId::from_number(2).unwrap(),
        integration: IntegrationId::from_number(1),
        created_at: Timestamp::EPOCH,
    };
    write_revision(&objects, &r2).unwrap();
    let absent_root = fold_core::object::hash_object(ObjectKind::Tree, b"never stored");
    let orphan = blob(&objects, b"orphan");
    age_all(&objects, OLD);

    let marked = mark(&objects, [r2.hash(), absent_root]).unwrap();
    for hash in [
        r1.hash(),
        first_tree,
        first_blob,
        with_dir,
        second_blob,
        sub,
    ] {
        assert!(marked.contains(&hash));
    }
    assert!(!marked.contains(&second_tree) && !marked.contains(&orphan));
    let report = sweep(&objects, &marked, SystemTime::now(), GRACE).unwrap();
    let mut missing = vec![absent_root, lost_blob];
    missing.sort();
    assert_eq!(report.missing, missing);
    assert!(!objects.contains(&orphan).unwrap() && !objects.contains(&second_tree).unwrap());
    assert_eq!(report.deleted, 2);
}

#[test]
fn a_damaged_tree_stops_the_walk() {
    let (_dir, objects) = store();
    let (damaged, _) = single(&objects, "content");
    let path = objects.path(&damaged);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(&path, b"\x01\x02\x00\x05trash").unwrap();
    let error = mark(&objects, [damaged]).unwrap_err();
    assert_eq!(error.code(), "object_corrupt");
}

struct Layout {
    _dir: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
}

fn layout() -> Layout {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    fs::create_dir_all(root.join("ws")).unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("keep.txt"), b"keep").unwrap();
    Layout {
        _dir: dir,
        root,
        outside,
    }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn a_read_only_checkout_is_removed_without_following_its_links() {
    let layout = layout();
    let checkout = layout.root.join("ws").join("review-p-1");
    common::write(&checkout, "src/lib.rs", b"code");
    common::write(&checkout, "README", b"text");
    symlink(&layout.outside, checkout.join("src").join("out")).unwrap();
    fs::set_permissions(
        layout.outside.join("keep.txt"),
        fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    for file in ["src/lib.rs", "README"] {
        fs::set_permissions(checkout.join(file), fs::Permissions::from_mode(0o444)).unwrap();
    }
    fs::set_permissions(checkout.join("src"), fs::Permissions::from_mode(0o555)).unwrap();
    let outside_mode = mode(&layout.outside);

    let relative = Path::new("ws/review-p-1");
    assert_eq!(
        locate_workspace_dir(&layout.root, relative).unwrap(),
        Some(checkout.canonicalize().unwrap())
    );
    assert!(remove_workspace_dir(&layout.root, relative).unwrap());
    assert!(!checkout.exists());
    assert!(layout.outside.join("keep.txt").exists());
    assert_eq!(mode(&layout.outside.join("keep.txt")), 0o444);
    assert_eq!(mode(&layout.outside), outside_mode);

    assert!(!remove_workspace_dir(&layout.root, relative).unwrap());
    assert_eq!(locate_workspace_dir(&layout.root, relative).unwrap(), None);
}

#[test]
fn a_symlink_to_a_read_only_directory_outside_ws_leaves_it_alone() {
    let layout = layout();
    let locked = layout.outside.join("locked");
    fs::create_dir_all(locked.join("inner")).unwrap();
    fs::write(locked.join("inner").join("file"), b"keep").unwrap();
    let checkout = layout.root.join("ws").join("t-1");
    common::write(&checkout, "deep/lib.rs", b"code");
    symlink(&locked, checkout.join("locked")).unwrap();
    symlink(locked.join("inner"), checkout.join("deep").join("inner")).unwrap();
    for dir in [locked.join("inner"), locked.clone()] {
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    }
    fs::set_permissions(checkout.join("deep"), fs::Permissions::from_mode(0o500)).unwrap();

    assert!(remove_workspace_dir(&layout.root, Path::new("ws/t-1")).unwrap());
    assert!(!checkout.exists());
    assert_eq!(mode(&locked), 0o555);
    assert_eq!(mode(&locked.join("inner")), 0o555);
    assert!(locked.join("inner").join("file").is_file());
    for dir in [locked.clone(), locked.join("inner")] {
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn an_absolute_path_inside_ws_is_accepted() {
    let layout = layout();
    let dir = layout.root.join("ws").join("t-2");
    fs::create_dir(&dir).unwrap();
    assert!(remove_workspace_dir(&layout.root, &dir).unwrap());
    assert!(!dir.exists());
}

#[test]
fn paths_outside_ws_are_invalid() {
    let layout = layout();
    fs::create_dir_all(layout.root.join("ws").join("t-1").join("deep")).unwrap();
    fs::write(layout.root.join("ws").join("file"), b"x").unwrap();
    symlink(&layout.outside, layout.root.join("ws").join("escape")).unwrap();
    symlink(
        layout.root.join("ws").join("t-1"),
        layout.root.join("ws").join("alias"),
    )
    .unwrap();
    let refused = [
        PathBuf::from("ws/../.."),
        PathBuf::from("ws/t-1/.."),
        PathBuf::from("ws/../ws/t-1"),
        PathBuf::from(""),
        layout.root.clone(),
        PathBuf::from("ws"),
        PathBuf::from("ws/t-1/deep"),
        PathBuf::from("ws/file"),
        PathBuf::from("ws/escape"),
        PathBuf::from("ws/alias"),
        PathBuf::from("elsewhere/t-1"),
        layout.outside.clone(),
    ];
    for path in refused {
        let error = remove_workspace_dir(&layout.root, &path).unwrap_err();
        assert_eq!(error.code(), "invalid_path", "{}", path.display());
    }
    assert!(layout.outside.join("keep.txt").exists());
    assert!(layout.root.join("ws").join("t-1").join("deep").is_dir());
}

#[test]
fn without_ws_a_workspace_directory_is_already_gone() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    fs::create_dir(&root).unwrap();
    assert!(!remove_workspace_dir(&root, Path::new("ws/t-1")).unwrap());
    let error = remove_workspace_dir(&root, Path::new("other/t-1")).unwrap_err();
    assert_eq!(error.code(), "invalid_path");
}
