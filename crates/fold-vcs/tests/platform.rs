#![cfg(test)]

pub mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use fold_core::tree::EntryKind;
use fold_vcs::materialise::materialise;
use fold_vcs::paths::RepoPath;
use fold_vcs::platform::{
    Capabilities, SymlinkOutcome, create_symlink, file_identity, is_executable, set_mode,
};

use common::*;

#[test]
fn a_bare_platform_still_materialises() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    let done = materialise(&store, &sample(&store), &root, &opts(BARE, false)).unwrap();

    let link = fs::symlink_metadata(root.join("link")).unwrap();
    assert!(link.is_file());
    assert_eq!(fs::read(root.join("link")).unwrap(), b"src/lib.rs");
    assert_eq!(done.degraded, [RepoPath::parse(b"link").unwrap()]);
    let recorded = done
        .index
        .lookup(&RepoPath::parse(b"link").unwrap())
        .unwrap();
    assert_eq!(recorded.kind, EntryKind::Symlink);

    let script = RepoPath::parse(b"src/deep/run.sh").unwrap();
    let entry = done.index.lookup(&script).unwrap();
    assert_eq!(entry.kind, EntryKind::Executable);
    assert_eq!(entry.inode, 0);
    let meta = fs::symlink_metadata(root.join("src/deep/run.sh")).unwrap();
    assert!(is_executable(&meta, entry.kind, &BARE));
    let lib = RepoPath::parse(b"src/lib.rs").unwrap();
    let lib_meta = fs::symlink_metadata(root.join("src/lib.rs")).unwrap();
    assert!(!is_executable(
        &lib_meta,
        done.index.lookup(&lib).unwrap().kind,
        &BARE
    ));
}

#[test]
fn freshness_ignores_inodes_when_the_platform_has_none() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    let done = materialise(&store, &sample(&store), &root, &opts(BARE, false)).unwrap();
    let lib = RepoPath::parse(b"src/lib.rs").unwrap();
    let path = root.join("src/lib.rs");
    let before = fs::metadata(&path).unwrap();
    assert!(done.index.is_fresh(&lib, &before).is_some());

    let mtime = before.modified().unwrap();
    fs::remove_file(&path).unwrap();
    fs::write(&path, b"pub fn x() {}\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    let after = fs::metadata(&path).unwrap();
    assert!(done.index.is_fresh(&lib, &after).is_some());

    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let changed = fs::metadata(&path).unwrap();
    assert!(done.index.is_fresh(&lib, &changed).is_none());
}

#[test]
fn read_only_without_a_mode_bit_uses_the_read_only_attribute() {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    materialise(&store, &sample(&store), &root, &opts(BARE, true)).unwrap();
    for file in ["empty", "src/lib.rs", "link"] {
        let meta = fs::symlink_metadata(root.join(file)).unwrap();
        assert!(meta.permissions().readonly(), "{file}");
    }
}

#[test]
fn set_mode_cases() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f");
    fs::write(&file, b"x").unwrap();
    let mode = || fs::metadata(&file).unwrap().permissions().mode() & 0o7777;
    set_mode(&file, true, false, &HOST).unwrap();
    assert_eq!(mode(), 0o755);
    set_mode(&file, false, true, &HOST).unwrap();
    assert_eq!(mode(), 0o444);
    set_mode(&file, true, true, &HOST).unwrap();
    assert_eq!(mode(), 0o555);
    let keep = Capabilities {
        exec_bit: false,
        ..HOST
    };
    set_mode(&file, true, false, &keep).unwrap();
    assert_eq!(mode(), 0o555, "no executable bit leaves the mode alone");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o666)).unwrap();
    set_mode(&file, false, true, &keep).unwrap();
    assert_eq!(mode(), 0o444);
}

#[test]
fn symlink_creation_and_identity() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    let emulated = dir.path().join("emulated");
    assert_eq!(
        create_symlink(b"t", &real, &HOST).unwrap(),
        SymlinkOutcome::Created
    );
    assert_eq!(
        create_symlink(b"t", &emulated, &BARE).unwrap(),
        SymlinkOutcome::Emulated
    );
    assert!(create_symlink(b"t", &emulated, &BARE).is_err());
    let meta = fs::symlink_metadata(&emulated).unwrap();
    assert_ne!(file_identity(&meta, &HOST), 0);
    assert_eq!(file_identity(&meta, &BARE), 0);
}

#[test]
fn probe_agrees_with_a_direct_symlink_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let fold = dir.path().join(".fold");
    let caps = Capabilities::probe(&fold).unwrap();
    let direct = std::os::unix::fs::symlink("x", dir.path().join("direct")).is_ok();
    assert_eq!(caps.symlinks, direct);
    assert!(caps.exec_bit && caps.inodes && caps.read_only_bit);
    assert_eq!(fs::read_dir(&fold).unwrap().count(), 0, "probe cleans up");
}
