#![cfg(test)]

pub mod common;

use std::fs;
use std::os::fd::AsRawFd;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use fold_core::object::{ObjectKind, hash_object};
use fold_core::tree::{EntryKind, FlatEntry};
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::index::StatIndex;
use fold_vcs::materialise::{Batch, remove_entry, write_entry};
use fold_vcs::paths::RepoPath;
use fold_vcs::scan::{DEFAULT_MAX_FILE_BYTES, ScanOptions, rescan_paths, scan};
use fold_vcs::snapshot::snapshot;
use rustix::fs::{CWD, RenameFlags, renameat_with};

use common::*;

const WRITES: usize = 400;
const SCANS: usize = 150;

/// Exchanges `ws/d`, a real directory, with `ws/swap`, a symlink to `outside`, as fast as it can until stopped, so `d` is a directory one moment and an outside symlink the next.
struct Swapper {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<usize>,
}

impl Swapper {
    fn start(root: &Path) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (d, swap) = (root.join("d"), root.join("swap"));
        let flag = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let mut swaps = 0;
            while !flag.load(Ordering::Relaxed) {
                renameat_with(CWD, &d, CWD, &swap, RenameFlags::EXCHANGE).unwrap();
                swaps += 1;
            }
            swaps
        });
        Self { stop, thread }
    }

    fn stop(self) -> usize {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.join().unwrap()
    }
}

/// `ws/d` holds `keep` so pruning never removes it, `ws/swap` links to the empty `outside`.
fn layout(dir: &Path) -> (PathBuf, PathBuf) {
    let root = dir.join("ws");
    let outside = dir.join("outside");
    fs::create_dir_all(root.join("d")).unwrap();
    fs::write(root.join("d/keep"), b"keep").unwrap();
    fs::create_dir(&outside).unwrap();
    symlink(&outside, root.join("swap")).unwrap();
    (root, outside)
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn refused(code: &str) -> bool {
    matches!(code, "invalid_path" | "workspace_io")
}

#[test]
fn a_parent_swapped_for_an_outside_symlink_never_takes_a_write_or_a_removal_outside() {
    let (dir, store) = store();
    let (root, outside) = layout(dir.path());
    let entry = FlatEntry {
        kind: EntryKind::File,
        hash: blob(&store, b"written"),
    };
    let link = FlatEntry {
        kind: EntryKind::Symlink,
        hash: blob(&store, b"target"),
    };
    fs::create_dir(outside.join("sub")).unwrap();
    let cleared = RepoPath::parse(b"d/sub").unwrap();
    let swapper = Swapper::start(&root);
    let mut landed = 0;
    let mut batch = Batch::open(&root).unwrap();
    for number in 0..WRITES {
        let path = RepoPath::parse(format!("d/sub/f{number}").as_bytes()).unwrap();
        let kind = if number % 2 == 0 { &entry } else { &link };
        match batch.write_entry(&store, &path, kind, &opts(HOST, false)) {
            Ok(_) => landed += 1,
            Err(error) => assert!(refused(error.code()), "{error}"),
        }
        if let Err(error) = batch.remove_entry(&path) {
            assert!(refused(error.code()), "{error}");
        }
        if let Err(error) = batch.remove_empty_dirs(&cleared) {
            assert!(refused(error.code()), "{error}");
        }
        let single = RepoPath::parse(format!("d/g{number}").as_bytes()).unwrap();
        if let Err(error) = write_entry(&store, &root, &single, &entry, &opts(HOST, false)) {
            assert!(refused(error.code()), "{error}");
        }
        if let Err(error) = remove_entry(&root, &single) {
            assert!(refused(error.code()), "{error}");
        }
    }
    batch.finish().unwrap();
    let swaps = swapper.stop();
    assert!(swaps > 0 && landed > 0, "{swaps} swaps, {landed} writes");
    assert_eq!(names(&outside), ["sub"]);
    assert!(names(&root.join(".fold/tmp")).is_empty());
    assert_eq!(names(dir.path()), [".fold", "outside", "ws"]);
}

#[test]
fn a_scanned_directory_swapped_for_an_outside_symlink_is_never_recorded_or_stored() {
    let (dir, store) = store();
    let (root, outside) = layout(dir.path());
    let secret = b"outside secret".to_vec();
    fs::write(outside.join("inner.txt"), &secret).unwrap();
    fs::write(outside.join("only-outside.txt"), &secret).unwrap();
    let outside_hash = hash_object(ObjectKind::Blob, &secret);
    // The real directory by its descriptor, whichever name holds it at the moment.
    let held = fs::File::open(root.join("d")).unwrap();
    let real = PathBuf::from(format!("/proc/self/fd/{}", held.as_raw_fd()));
    let rules = IgnoreRules::new(&root);
    let opts = ScanOptions {
        rules: &rules,
        max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        caps: HOST,
        recorded: None,
    };
    let index_path = dir.path().join(".fold/ws-index");
    let inner = RepoPath::parse(b"d/inner.txt").unwrap();
    let mut index = StatIndex::empty(HOST);
    let swapper = Swapper::start(&root);
    for number in 0..SCANS {
        fs::write(real.join("inner.txt"), format!("inside {number}")).unwrap();
        let scanned = scan(&root, &mut index, &opts).unwrap();
        let rescanned =
            rescan_paths(&root, &mut index, std::slice::from_ref(&inner), &opts).unwrap();
        let snapped = snapshot(&store, &root, &mut index, &index_path, None, &opts).unwrap();
        for entries in [&scanned.entries, &rescanned.entries, &snapped.entries] {
            for (path, found) in entries {
                let path = String::from_utf8_lossy(path);
                assert_ne!(found.hash, outside_hash, "{path}");
                assert!(!path.contains("only-outside"), "{path}");
            }
        }
    }
    let swaps = swapper.stop();
    assert!(swaps > 0);
    assert!(!store.contains(&outside_hash).unwrap());
    let scanned = scan(&root, &mut index, &opts).unwrap();
    assert!(
        scanned
            .entries
            .keys()
            .any(|path| path.ends_with(b"/inner.txt")),
        "the real directory is still scanned"
    );
}
