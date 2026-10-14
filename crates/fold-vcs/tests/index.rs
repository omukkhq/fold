#![cfg(test)]

pub mod common;

use std::fs::{self, File};
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, SystemTime};

use fold_core::object::{ObjectKind, hash_object};
use fold_core::tree::EntryKind;
use fold_vcs::index::{StatEntry, StatIndex};
use fold_vcs::paths::RepoPath;

use common::*;

fn entry_for(path: &std::path::Path) -> StatEntry {
    let meta = fs::symlink_metadata(path).unwrap();
    StatEntry::from_metadata(
        &meta,
        EntryKind::File,
        hash_object(ObjectKind::Blob, b"content"),
        &HOST,
    )
}

fn rp(text: &str) -> RepoPath {
    RepoPath::parse(text.as_bytes()).unwrap()
}

fn saved_index(dir: &std::path::Path) -> (StatIndex, std::path::PathBuf) {
    let file = dir.join("f");
    fs::write(&file, b"hello").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(60))
        .unwrap();
    let mut index = StatIndex::empty(HOST);
    index.upsert(rp("f"), entry_for(&file));
    index.upsert(rp("a/b"), entry_for(&file));
    let idx = dir.join("idx/ws-9.idx");
    index.save(&idx).unwrap();
    (index, idx)
}

#[test]
fn save_and_load_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let (saved, idx) = saved_index(dir.path());
    let loaded = StatIndex::load(&idx, HOST);
    assert!(!loaded.rebuilt());
    assert!(!saved.rebuilt());
    assert_eq!(loaded.len(), 2);
    assert!(!loaded.is_empty());
    assert_eq!(loaded.lookup(&rp("f")), saved.lookup(&rp("f")));
    assert_eq!(loaded.saved_at_ns(), saved.saved_at_ns());
    assert!(loaded.saved_at_ns().is_some());
    let paths: Vec<_> = loaded.iter().map(|(path, _)| path.to_string()).collect();
    assert_eq!(paths, ["a/b", "f"]);
    assert_eq!(loaded.capabilities(), &HOST);
    let leftovers = fs::read_dir(idx.parent().unwrap()).unwrap().count();
    assert_eq!(leftovers, 1);
}

#[test]
fn a_missing_file_loads_empty_and_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    let index = StatIndex::load(&dir.path().join("none.idx"), HOST);
    assert!(index.rebuilt());
    assert!(index.is_empty());
}

#[test]
fn damaged_files_load_empty_and_rebuilt() {
    let dir = tempfile::tempdir().unwrap();
    let (_, idx) = saved_index(dir.path());
    let good = fs::read(&idx).unwrap();
    let mut variants: Vec<Vec<u8>> = Vec::new();
    for position in [0, 4, 6, good.len() / 2, good.len() - 1] {
        let mut flipped = good.clone();
        flipped[position] ^= 0x01;
        variants.push(flipped);
    }
    variants.push(good[..good.len() - 1].to_vec());
    variants.push(Vec::new());
    variants.push(vec![0; 10]);
    // A well-checksummed file of another version, and one with a bad magic.
    for (offset, value) in [(4usize, 2u8), (0, b'X')] {
        let mut body = good[..good.len() - 32].to_vec();
        body[offset] = value;
        let sum = blake3::hash(&body);
        body.extend_from_slice(sum.as_bytes());
        variants.push(body);
    }
    for bytes in variants {
        fs::write(&idx, bytes).unwrap();
        let index = StatIndex::load(&idx, HOST);
        assert!(index.rebuilt());
        assert!(index.is_empty());
    }
}

fn resum(mut body: Vec<u8>) -> Vec<u8> {
    let sum = blake3::hash(&body);
    body.extend_from_slice(sum.as_bytes());
    body
}

#[test]
fn well_checksummed_garbage_is_still_refused() {
    let dir = tempfile::tempdir().unwrap();
    let idx = dir.path().join("x.idx");
    let mut header = b"FIDX\x01".to_vec();
    // Claims one entry but holds none.
    header.push(1);
    let mut trailing = header.clone();
    trailing.push(0);
    let mut bad_path = b"FIDX\x01\x01\x02..".to_vec();
    bad_path.extend_from_slice(&[0; 28 + 32]);
    let mut empty_ok = b"FIDX\x01\x00".to_vec();
    empty_ok.push(9); // trailing byte after the last entry
    for body in [header, trailing, bad_path, empty_ok] {
        fs::write(&idx, resum(body)).unwrap();
        assert!(StatIndex::load(&idx, HOST).rebuilt());
    }
    fs::write(&idx, resum(b"FIDX\x01\x00".to_vec())).unwrap();
    let index = StatIndex::load(&idx, HOST);
    assert!(!index.rebuilt());
    assert!(index.is_empty());
}

#[test]
fn an_unknown_entry_kind_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (_, idx) = saved_index(dir.path());
    let good = fs::read(&idx).unwrap();
    let mut body = good[..good.len() - 32].to_vec();
    // First entry "a/b": path len 1 byte + 3 path bytes, then 8+8+8 stat bytes, then the packed mode.
    let kind_offset = 4 + 1 + 1 + 1 + 3 + 24 + 2;
    body[kind_offset] = 9;
    fs::write(&idx, resum(body)).unwrap();
    assert!(StatIndex::load(&idx, HOST).rebuilt());
}

#[test]
fn freshness_follows_size_mtime_and_mode() {
    let dir = tempfile::tempdir().unwrap();
    let (index, _) = saved_index(dir.path());
    let file = dir.path().join("f");
    let hash = hash_object(ObjectKind::Blob, b"content");
    let fresh = |index: &StatIndex| index.is_fresh(&rp("f"), &fs::metadata(&file).unwrap());
    assert_eq!(fresh(&index), Some(hash));
    assert_eq!(
        index.is_fresh(&rp("unknown"), &fs::metadata(&file).unwrap()),
        None
    );

    let mtime = fs::metadata(&file).unwrap().modified().unwrap();
    File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(mtime - Duration::from_secs(5))
        .unwrap();
    assert_eq!(fresh(&index), None, "mtime");
    File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    assert_eq!(fresh(&index), Some(hash));

    let mut perms = fs::metadata(&file).unwrap().permissions();
    PermissionsExt::set_mode(&mut perms, 0o600);
    fs::set_permissions(&file, perms).unwrap();
    assert_eq!(fresh(&index), None, "mode");
    let mut perms = fs::metadata(&file).unwrap().permissions();
    PermissionsExt::set_mode(&mut perms, 0o644);
    fs::set_permissions(&file, perms).unwrap();
    assert_eq!(fresh(&index), Some(hash));

    fs::write(&file, b"hello world").unwrap();
    File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    assert_eq!(fresh(&index), None, "size");
}

#[test]
fn a_changed_inode_is_stale_unless_a_side_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let (index, _) = saved_index(dir.path());
    let file = dir.path().join("f");
    let mtime = fs::metadata(&file).unwrap().modified().unwrap();
    let replacement = dir.path().join("replacement");
    fs::write(&replacement, b"hello").unwrap();
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o644)).unwrap();
    fs::rename(&replacement, &file).unwrap();
    File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    let meta = fs::metadata(&file).unwrap();
    assert_eq!(index.is_fresh(&rp("f"), &meta), None);

    let mut blind = StatIndex::empty(BARE);
    let mut stored = *index.lookup(&rp("f")).unwrap();
    blind.upsert(rp("f"), stored);
    assert!(blind.is_fresh(&rp("f"), &meta).is_some());
    stored.inode = 0;
    let mut none_stored = StatIndex::empty(HOST);
    none_stored.upsert(rp("f"), stored);
    assert!(none_stored.is_fresh(&rp("f"), &meta).is_some());
}

#[test]
fn an_entry_newer_than_the_save_is_not_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let (mut index, idx) = saved_index(dir.path());
    let file = dir.path().join("f");
    let meta = fs::metadata(&file).unwrap();
    assert!(index.is_fresh(&rp("f"), &meta).is_some());

    File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(3600))
        .unwrap();
    let meta = fs::metadata(&file).unwrap();
    index.upsert(rp("f"), entry_for(&file));
    assert_eq!(index.is_fresh(&rp("f"), &meta), None);
    let reloaded = StatIndex::load(&idx, HOST);
    assert!(reloaded.lookup(&rp("f")).is_some());

    let mut equal = StatIndex::load(&idx, HOST);
    let mut same_tick = *equal.lookup(&rp("f")).unwrap();
    same_tick.mtime_ns = equal.saved_at_ns().unwrap();
    equal.upsert(rp("f"), same_tick);
    assert_eq!(equal.is_fresh(&rp("f"), &meta), None);
}

#[test]
fn upsert_remove_and_resave() {
    let dir = tempfile::tempdir().unwrap();
    let (mut index, idx) = saved_index(dir.path());
    assert!(index.remove(&rp("a/b")).is_some());
    assert!(index.remove(&rp("a/b")).is_none());
    index.save(&idx).unwrap();
    assert_eq!(StatIndex::load(&idx, HOST).len(), 1);
}

#[test]
fn a_failed_save_leaves_no_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut index = StatIndex::empty(HOST);
    let target = dir.path().join("taken");
    fs::create_dir(&target).unwrap();
    assert!(index.save(&target).is_err());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn the_file_type_must_fit_the_recorded_kind() {
    let dir = tempfile::tempdir().unwrap();
    let (index, _) = saved_index(dir.path());
    let meta = fs::metadata(dir.path().join("f")).unwrap();
    let mut as_link = *index.lookup(&rp("f")).unwrap();
    as_link.kind = EntryKind::Symlink;
    let mut host = StatIndex::empty(HOST);
    host.upsert(rp("f"), as_link);
    assert_eq!(host.is_fresh(&rp("f"), &meta), None);
    let mut bare = StatIndex::empty(BARE);
    bare.upsert(rp("f"), as_link);
    assert!(bare.is_fresh(&rp("f"), &meta).is_some(), "an emulated link");

    let sub = dir.path().join("d");
    fs::create_dir(&sub).unwrap();
    let dir_meta = fs::symlink_metadata(&sub).unwrap();
    let mut index = StatIndex::empty(HOST);
    index.upsert(rp("d"), entry_for(&sub));
    assert_eq!(index.is_fresh(&rp("d"), &dir_meta), None);
}
