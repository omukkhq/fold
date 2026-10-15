#![cfg(test)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::tree::{EntryKind, Tree, flatten};
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::index::StatIndex;
use fold_vcs::objects::ObjectStore;
use fold_vcs::scan::{DEFAULT_MAX_FILE_BYTES, ScanOptions, SkipReason, scan};
use fold_vcs::snapshot::{Snapshot, WarningReason, record, snapshot};

use crate::common::*;

struct Fixture {
    _dir: tempfile::TempDir,
    store: ObjectStore,
    root: PathBuf,
    index_path: PathBuf,
    index: StatIndex,
}

fn fixture() -> Fixture {
    let (dir, store) = store();
    let root = dir.path().join("ws");
    fs::create_dir(&root).unwrap();
    let index_path = dir.path().join(".fold/ws-index");
    Fixture {
        _dir: dir,
        store,
        root,
        index_path,
        index: StatIndex::empty(HOST),
    }
}

impl Fixture {
    fn snap(&mut self, base: Option<ObjectHash>) -> Snapshot {
        self.snap_max(base, DEFAULT_MAX_FILE_BYTES)
    }

    fn snap_max(&mut self, base: Option<ObjectHash>, max: u64) -> Snapshot {
        let rules = IgnoreRules::new(&self.root);
        let opts = ScanOptions {
            rules: &rules,
            max_file_bytes: max,
            caps: HOST,
            recorded: None,
        };
        snapshot(
            &self.store,
            &self.root,
            &mut self.index,
            &self.index_path,
            base,
            &opts,
        )
        .unwrap()
    }

    fn objects(&self) -> usize {
        self.store.entries().unwrap().len()
    }

    fn tree_of(&self, hash: &ObjectHash) -> Tree {
        let object = self.store.read_expecting(hash, ObjectKind::Tree).unwrap();
        Tree::decode(&object.payload).unwrap()
    }
}

fn deep_workspace(root: &Path) {
    write(root, "top.txt", b"top");
    write(root, "a/b/c/leaf.txt", b"leaf");
    write(root, "a/b/c/other.txt", b"other");
    write(root, "a/b/sibling.txt", b"sibling");
    write(root, "d/e.txt", b"e");
}

fn hash_of(tree: &Tree, name: &str) -> ObjectHash {
    tree.entries
        .iter()
        .find(|entry| entry.name == name.as_bytes())
        .unwrap()
        .hash
}

#[test]
fn a_snapshot_records_every_file_and_link() {
    let mut fx = fixture();
    deep_workspace(&fx.root);
    symlink("top.txt", fx.root.join("link")).unwrap();
    write(&fx.root, "run.sh", b"#!/bin/sh\n");
    fs::set_permissions(fx.root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    let snap = fx.snap(None);
    assert!(snap.warnings.is_empty());
    let loaded = flatten(&snap.tree, &|hash| {
        let object = fx.store.read_expecting(hash, ObjectKind::Tree)?;
        Ok(Tree::decode(&object.payload)?)
    })
    .unwrap();
    assert_eq!(loaded, snap.entries);
    assert_eq!(snap.entries.len(), 7);
    assert_eq!(
        snap.entries[b"run.sh".as_slice()].kind,
        EntryKind::Executable
    );
    let link = snap.entries[b"link".as_slice()];
    assert_eq!(link.kind, EntryKind::Symlink);
    assert_eq!(fx.store.read(&link.hash).unwrap().payload, b"top.txt");
    let leaf = snap.entries[b"a/b/c/leaf.txt".as_slice()];
    assert_eq!(fx.store.read(&leaf.hash).unwrap().payload, b"leaf");
    assert!(StatIndex::load(&fx.index_path, HOST).len() == 7);
}

#[test]
fn an_unchanged_workspace_returns_the_base_tree() {
    let mut fx = fixture();
    deep_workspace(&fx.root);
    let first = fx.snap(None);
    let objects = fx.objects();
    let again = fx.snap(Some(first.tree));
    assert_eq!(again.tree, first.tree);
    assert_eq!(again.entries, first.entries);
    assert_eq!(fx.objects(), objects);
}

#[test]
fn one_edited_deep_file_makes_new_trees_for_its_ancestors_only() {
    let mut fx = fixture();
    deep_workspace(&fx.root);
    let first = fx.snap(None);
    let before = fx.objects();
    write(&fx.root, "a/b/c/leaf.txt", b"edited leaf");
    let second = fx.snap(Some(first.tree));
    assert_ne!(second.tree, first.tree);
    assert_eq!(fx.objects(), before + 5, "one blob and four trees");

    let (old_root, new_root) = (fx.tree_of(&first.tree), fx.tree_of(&second.tree));
    assert_eq!(hash_of(&old_root, "d"), hash_of(&new_root, "d"));
    assert_ne!(hash_of(&old_root, "a"), hash_of(&new_root, "a"));
    let (old_a, new_a) = (
        fx.tree_of(&hash_of(&old_root, "a")),
        fx.tree_of(&hash_of(&new_root, "a")),
    );
    let (old_b, new_b) = (
        fx.tree_of(&hash_of(&old_a, "b")),
        fx.tree_of(&hash_of(&new_a, "b")),
    );
    assert_eq!(
        hash_of(&old_b, "sibling.txt"),
        hash_of(&new_b, "sibling.txt")
    );
    assert_ne!(hash_of(&old_b, "c"), hash_of(&new_b, "c"));
}

#[test]
fn the_same_content_always_gives_the_same_root() {
    let mut one = fixture();
    deep_workspace(&one.root);
    let from_scratch = one.snap(None);

    let mut other = fixture();
    write(&other.root, "d/e.txt", b"e");
    write(&other.root, "a/b/sibling.txt", b"sibling");
    write(&other.root, "stray/file", b"gone later");
    write(&other.root, "a/b/c/leaf.txt", b"old");
    let early = other.snap(None);
    write(&other.root, "top.txt", b"top");
    write(&other.root, "a/b/c/other.txt", b"other");
    write(&other.root, "a/b/c/leaf.txt", b"leaf");
    fs::remove_dir_all(other.root.join("stray")).unwrap();
    let later = other.snap(Some(early.tree));
    assert_eq!(later.tree, from_scratch.tree);
    assert_eq!(later.entries, from_scratch.entries);
}

#[test]
fn removals_and_swaps_match_a_fresh_snapshot() {
    let mut fx = fixture();
    deep_workspace(&fx.root);
    write(&fx.root, "swap/inner.txt", b"x");
    write(&fx.root, "flip", b"file");
    let first = fx.snap(None);

    fs::remove_dir_all(fx.root.join("swap")).unwrap();
    write(&fx.root, "swap", b"now a file");
    fs::remove_file(fx.root.join("flip")).unwrap();
    write(&fx.root, "flip/now/dir.txt", b"y");
    fs::remove_dir_all(fx.root.join("a")).unwrap();
    let next = fx.snap(Some(first.tree));

    let mut fresh = fixture();
    write(&fresh.root, "top.txt", b"top");
    write(&fresh.root, "d/e.txt", b"e");
    write(&fresh.root, "swap", b"now a file");
    write(&fresh.root, "flip/now/dir.txt", b"y");
    assert_eq!(next.tree, fresh.snap(None).tree);
}

#[test]
fn an_emptied_workspace_is_the_empty_tree() {
    let mut fx = fixture();
    let empty = fx.snap(None);
    assert!(empty.entries.is_empty());
    assert!(fx.tree_of(&empty.tree).entries.is_empty());
    write(&fx.root, "only", b"x");
    let one = fx.snap(Some(empty.tree));
    fs::remove_file(fx.root.join("only")).unwrap();
    assert_eq!(fx.snap(Some(one.tree)).tree, empty.tree);
}

#[test]
fn a_skipped_file_keeps_its_base_entry_and_adds_a_warning() {
    let mut fx = fixture();
    write(&fx.root, "big", b"ok");
    write(&fx.root, "fine", b"ok");
    let first = fx.snap(None);
    write(&fx.root, "big", b"now too large");
    write(&fx.root, "new-big", b"never recorded");
    let next = fx.snap_max(Some(first.tree), 8);
    assert_eq!(next.tree, first.tree, "no deletion for a skipped file");
    let mut warnings: Vec<_> = next
        .warnings
        .iter()
        .map(|warning| (warning.path.to_string(), warning.reason))
        .collect();
    warnings.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        warnings,
        [
            (
                "big".to_string(),
                WarningReason::Skipped(SkipReason::TooLarge)
            ),
            (
                "new-big".to_string(),
                WarningReason::Skipped(SkipReason::TooLarge)
            ),
        ]
    );
    assert!(next.entries.contains_key(b"big".as_slice()));
    assert!(!next.entries.contains_key(b"new-big".as_slice()));
}

#[test]
fn a_file_changed_after_the_scan_is_not_recorded() {
    let mut fx = fixture();
    write(&fx.root, "file", b"before");
    write(&fx.root, "added", b"v1");
    let first = fx.snap(None);
    let base = flatten(&first.tree, &|hash| {
        let object = fx.store.read_expecting(hash, ObjectKind::Tree)?;
        Ok(Tree::decode(&object.payload)?)
    })
    .unwrap();

    write(&fx.root, "file", b"scanned");
    write(&fx.root, "new", b"scanned too");
    let rules = IgnoreRules::new(&fx.root);
    let opts = ScanOptions {
        rules: &rules,
        max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        caps: HOST,
        recorded: Some(&base),
    };
    let scanned = scan(&fx.root, &mut fx.index, &opts).unwrap();
    write(&fx.root, "file", b"rewritten after");
    fs::remove_file(fx.root.join("new")).unwrap();
    let next = record(
        &fx.store,
        &fx.root,
        &mut fx.index,
        &fx.index_path,
        Some(first.tree),
        &base,
        scanned,
    )
    .unwrap();
    assert_eq!(next.tree, first.tree);
    assert_eq!(next.warnings.len(), 2);
    assert!(
        next.warnings
            .iter()
            .all(|warning| warning.reason == WarningReason::ChangedDuringSnapshot)
    );
    assert!(fx.index.lookup(&"file".parse_path()).is_none());
}

trait ParsePath {
    fn parse_path(&self) -> fold_vcs::paths::RepoPath;
}

impl ParsePath for str {
    fn parse_path(&self) -> fold_vcs::paths::RepoPath {
        fold_vcs::paths::RepoPath::parse(self.as_bytes()).unwrap()
    }
}

#[test]
fn a_special_file_is_warned_about_and_never_deleted() {
    let mut fx = fixture();
    write(&fx.root, "keep", b"x");
    let first = fx.snap(None);
    let _socket = std::os::unix::net::UnixListener::bind(fx.root.join("sock")).unwrap();
    let next = fx.snap(Some(first.tree));
    assert_eq!(next.tree, first.tree);
    assert_eq!(next.warnings.len(), 1);
    assert_eq!(
        next.warnings[0].reason,
        WarningReason::Skipped(SkipReason::Special)
    );
}

#[test]
fn a_snapshot_writes_nothing_into_the_workspace() {
    let mut fx = fixture();
    deep_workspace(&fx.root);
    let listing = |root: &Path| {
        let mut names = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let meta = fs::symlink_metadata(entry.path()).unwrap();
                names.push((entry.path(), meta.len(), meta.modified().unwrap()));
                if meta.is_dir() {
                    stack.push(entry.path());
                }
            }
        }
        names.sort();
        names
    };
    let before = listing(&fx.root);
    fx.snap(None);
    assert_eq!(listing(&fx.root), before);
}

fn load_flat(store: &ObjectStore, tree: &ObjectHash) -> fold_core::tree::FlatTree {
    flatten(tree, &|hash| {
        let object = store.read_expecting(hash, ObjectKind::Tree)?;
        Ok(Tree::decode(&object.payload)?)
    })
    .unwrap()
}

#[test]
fn an_unreadable_directory_keeps_every_base_file() {
    let mut fx = fixture();
    write(&fx.root, "locked/file", b"x");
    write(&fx.root, "locked/deep/other", b"y");
    write(&fx.root, "open", b"x");
    let first = fx.snap(None);
    fs::set_permissions(fx.root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(fx.root.join("locked")).is_ok();
    let next = fx.snap(Some(first.tree));
    fs::set_permissions(fx.root.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return;
    }
    assert_eq!(next.tree, first.tree);
    assert_eq!(next.entries, first.entries);
    assert_eq!(next.warnings.len(), 1);
    assert_eq!(next.warnings[0].path.to_string(), "locked");
    assert_eq!(
        next.warnings[0].reason,
        WarningReason::Skipped(SkipReason::Unreadable)
    );
}

fn scan_then_record(
    fx: &mut Fixture,
    base_tree: ObjectHash,
    between: impl FnOnce(&Path),
) -> Snapshot {
    let base = load_flat(&fx.store, &base_tree);
    let rules = IgnoreRules::new(&fx.root);
    let opts = ScanOptions {
        rules: &rules,
        max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        caps: HOST,
        recorded: Some(&base),
    };
    let scanned = scan(&fx.root, &mut fx.index, &opts).unwrap();
    between(&fx.root);
    record(
        &fx.store,
        &fx.root,
        &mut fx.index,
        &fx.index_path,
        Some(base_tree),
        &base,
        scanned,
    )
    .unwrap()
}

#[test]
fn a_file_swapped_for_a_symlink_after_the_scan_is_never_read_through() {
    let mut fx = fixture();
    write(&fx.root, "keep", b"k");
    let first = fx.snap(None);
    let outside = fx._dir.path().join("outside");
    fs::write(&outside, b"outside content").unwrap();
    write(&fx.root, "file", b"scanned");
    let next = scan_then_record(&mut fx, first.tree, |root| {
        fs::remove_file(root.join("file")).unwrap();
        symlink(&outside, root.join("file")).unwrap();
    });
    assert_eq!(next.tree, first.tree);
    assert_eq!(next.warnings.len(), 1);
    assert_eq!(
        next.warnings[0].reason,
        WarningReason::ChangedDuringSnapshot
    );
    let leaked = fold_core::object::hash_object(ObjectKind::Blob, b"outside content");
    assert!(!fx.store.contains(&leaked).unwrap());
}

#[test]
fn a_file_swapped_for_a_fifo_after_the_scan_does_not_block() {
    let mut fx = fixture();
    write(&fx.root, "keep", b"k");
    let first = fx.snap(None);
    write(&fx.root, "file", b"scanned");
    let next = scan_then_record(&mut fx, first.tree, |root| {
        fs::remove_file(root.join("file")).unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(root.join("file"))
            .status()
            .unwrap();
        assert!(made.success());
    });
    assert_eq!(next.tree, first.tree);
    assert_eq!(
        next.warnings[0].reason,
        WarningReason::ChangedDuringSnapshot
    );
}

#[test]
fn a_file_over_a_base_directory_that_changes_after_the_scan_keeps_the_directory() {
    let mut fx = fixture();
    write(&fx.root, "x/a", b"a");
    write(&fx.root, "x/b", b"b");
    write(&fx.root, "keep", b"k");
    let first = fx.snap(None);
    fs::remove_dir_all(fx.root.join("x")).unwrap();
    write(&fx.root, "x", b"now a file");
    let next = scan_then_record(&mut fx, first.tree, |root| {
        write(root, "x", b"rewritten after the scan");
    });
    assert_eq!(next.tree, first.tree);
    assert_eq!(next.entries, first.entries);
    assert_eq!(load_flat(&fx.store, &next.tree), next.entries);
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    const PATHS: [&str; 6] = ["a", "a/b", "a/b/c", "a/d", "e", "e/f"];

    #[derive(Debug, Clone)]
    enum Step {
        Write(usize, u8),
        Remove(usize),
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            (0..PATHS.len(), 0u8..4).prop_map(|(path, content)| Step::Write(path, content)),
            (0..PATHS.len()).prop_map(Step::Remove),
        ]
    }

    fn clear(target: &Path) {
        match fs::symlink_metadata(target) {
            Ok(meta) if meta.is_dir() => fs::remove_dir_all(target).unwrap(),
            Ok(_) => fs::remove_file(target).unwrap(),
            Err(_) => {}
        }
    }

    fn apply(root: &Path, step: &Step) {
        match *step {
            Step::Write(path, content) => {
                let path = PATHS[path];
                let mut ancestor = root.to_path_buf();
                for name in Path::new(path).parent().into_iter().flat_map(Path::iter) {
                    ancestor.push(name);
                    if fs::symlink_metadata(&ancestor).is_ok_and(|meta| !meta.is_dir()) {
                        fs::remove_file(&ancestor).unwrap();
                    }
                }
                clear(&root.join(path));
                write(root, path, &[b'0' + content; 3]);
            }
            Step::Remove(path) => clear(&root.join(PATHS[path])),
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        /// Snapshots taken after each step, each on top of the one before, end at the same tree as one snapshot of the final workspace taken from nothing.
        #[test]
        fn incremental_snapshots_match_a_fresh_one(steps in prop::collection::vec(step(), 1..8)) {
            let mut fx = fixture();
            let mut tree = fx.snap(None).tree;
            for step in &steps {
                apply(&fx.root, step);
                let next = fx.snap(Some(tree));
                prop_assert!(next.warnings.is_empty());
                prop_assert_eq!(&load_flat(&fx.store, &next.tree), &next.entries);
                tree = next.tree;
            }
            let mut fresh = fixture();
            for step in &steps {
                apply(&fresh.root, step);
            }
            prop_assert_eq!(fresh.snap(None).tree, tree);
        }
    }
}
