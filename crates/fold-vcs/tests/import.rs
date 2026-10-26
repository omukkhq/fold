#![cfg(test)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use fold_core::object::{ObjectHash, ObjectKind, hash_object};
use fold_core::tree::{EntryKind, FlatEntry, FlatTree, Tree, flatten};
use fold_core::{FoldError, codes};
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::import::{ImportOptions, ImportResult, import_dir};
use fold_vcs::materialise::materialise;
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::RepoPath;
use fold_vcs::platform::Capabilities;
use fold_vcs::scan::{DEFAULT_MAX_FILE_BYTES, SkipReason, Skipped};

use crate::common::*;

fn write(root: &Path, path: &str, bytes: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn import_opts(caps: Capabilities) -> ImportOptions {
    ImportOptions {
        max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        caps,
    }
}

fn import_with(store: &ObjectStore, source: &Path, opts: &ImportOptions) -> ImportResult {
    import_dir(store, source, &IgnoreRules::new(source), None, opts).unwrap()
}

fn import(store: &ObjectStore, source: &Path) -> ImportResult {
    import_with(store, source, &import_opts(HOST))
}

/// Nested folders, an executable script, a symlink, an empty directory, `.git` and `.fold` directories, and ignore files at two levels: the root `.gitignore` leaves out `build/` and `src/.gitignore` leaves out `*.log` below `src/`.
fn fixture(root: &Path) {
    write(root, "README.md", b"# app\n");
    write(root, "src/lib.rs", b"pub fn x() {}\n");
    write(root, "src/deep/mod.rs", b"mod a;\n");
    write(root, "src/deep/debug.log", b"noise\n");
    write(root, "top.log", b"top\n");
    write(root, "src/.gitignore", b"*.log\n");
    write(root, "run.sh", b"#!/bin/sh\necho hi\n");
    fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("src/lib.rs", root.join("link")).unwrap();
    fs::create_dir_all(root.join("empty/inner")).unwrap();
    write(root, ".git/HEAD", b"ref: refs/heads/main\n");
    write(root, ".fold/fold.db", b"db");
    write(root, "vendor/.git/config", b"[core]\n");
    write(root, "build/out.bin", b"\x00\x01");
    write(root, ".gitignore", b"build/\n");
}

fn flat(store: &ObjectStore, tree: &ObjectHash) -> FlatTree {
    let load = |hash: &ObjectHash| -> Result<Tree, FoldError> {
        let object = store.read_expecting(hash, ObjectKind::Tree)?;
        Ok(Tree::decode(&object.payload).unwrap())
    };
    flatten(tree, &load).unwrap()
}

fn paths(flat: &FlatTree) -> Vec<String> {
    flat.keys()
        .map(|path| String::from_utf8(path.clone()).unwrap())
        .collect()
}

fn source() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("source");
    fs::create_dir(&root).unwrap();
    (dir, root)
}

#[test]
fn the_imported_tree_lists_exactly_the_tracked_files() {
    let (_dir, root) = source();
    fixture(&root);
    let (_store_dir, store) = store();
    let result = import(&store, &root);
    let flat = flat(&store, &result.tree);
    assert_eq!(
        paths(&flat),
        [
            ".gitignore",
            "README.md",
            "link",
            "run.sh",
            "src/.gitignore",
            "src/deep/mod.rs",
            "src/lib.rs",
            "top.log",
        ]
    );
    assert_eq!(flat[b"run.sh".as_slice()].kind, EntryKind::Executable);
    assert_eq!(flat[b"src/lib.rs".as_slice()].kind, EntryKind::File);
    let link = flat[b"link".as_slice()];
    assert_eq!(link.kind, EntryKind::Symlink);
    let target = store.read_expecting(&link.hash, ObjectKind::Blob).unwrap();
    assert_eq!(target.payload, b"src/lib.rs");
    assert_eq!(result.files, 8);
    assert_eq!(result.bytes, 72);
    assert!(result.skipped.is_empty());
}

#[test]
fn a_source_without_ignore_files_imports_everything_but_git_and_fold() {
    let (_dir, root) = source();
    write(&root, "a.txt", b"a");
    write(&root, "build/out.bin", b"b");
    write(&root, "nested/x.log", b"c");
    write(&root, ".git/HEAD", b"h");
    write(&root, ".fold/config.toml", b"f");
    write(&root, "nested/.fold/x", b"f");
    let (_store_dir, store) = store();
    let result = import(&store, &root);
    assert_eq!(
        paths(&flat(&store, &result.tree)),
        ["a.txt", "build/out.bin", "nested/x.log"]
    );
}

#[test]
fn a_foldignore_adds_patterns_and_brings_back_what_a_gitignore_leaves_out() {
    let (_dir, root) = source();
    write(&root, ".gitignore", b"*.log\nbuild/\n");
    write(&root, ".foldignore", b"secret.txt\n!keep.log\n");
    write(&root, "main.rs", b"fn main() {}\n");
    write(&root, "debug.log", b"d");
    write(&root, "keep.log", b"k");
    write(&root, "secret.txt", b"s");
    write(&root, "nested/secret.txt", b"s");
    write(&root, "nested/keep.log", b"k");
    write(&root, "build/out.bin", b"b");
    let (_store_dir, store) = store();
    let result = import(&store, &root);
    assert_eq!(
        paths(&flat(&store, &result.tree)),
        [
            ".foldignore",
            ".gitignore",
            "keep.log",
            "main.rs",
            "nested/keep.log",
        ]
    );
    assert!(result.skipped.is_empty());
}

#[test]
fn skip_reasons_print_with_stable_names() {
    assert_eq!(SkipReason::TooLarge.as_str(), "too_large");
    assert_eq!(SkipReason::Special.as_str(), "special");
    assert_eq!(SkipReason::Unreadable.as_str(), "unreadable");
}

#[test]
fn a_file_over_the_limit_is_skipped_as_too_large() {
    let (_dir, root) = source();
    write(&root, "small", b"1234");
    write(&root, "big", b"12345");
    write(&root, "dir/big", b"12345");
    let (_store_dir, store) = store();
    let opts = ImportOptions {
        max_file_bytes: 4,
        caps: HOST,
    };
    let result = import_with(&store, &root, &opts);
    assert_eq!(paths(&flat(&store, &result.tree)), ["small"]);
    let skipped = |path: &str| Skipped {
        path: RepoPath::parse(path.as_bytes()).unwrap(),
        reason: SkipReason::TooLarge,
    };
    assert_eq!(result.skipped, [skipped("big"), skipped("dir/big")]);
    assert_eq!(result.bytes, 4);
}

/// A process that can read any file, such as root, cannot see the unreadable case.
fn can_read(path: &Path) -> bool {
    fs::File::open(path).is_ok()
}

#[test]
fn an_unreadable_file_or_folder_is_skipped_and_the_import_continues() {
    let (_dir, root) = source();
    write(&root, "ok.txt", b"ok");
    write(&root, "secret.txt", b"s");
    write(&root, "locked/inside.txt", b"i");
    fs::set_permissions(root.join("secret.txt"), fs::Permissions::from_mode(0o000)).unwrap();
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let readable = can_read(&root.join("secret.txt"));
    let (_store_dir, store) = store();
    let result = import(&store, &root);
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return;
    }
    assert_eq!(paths(&flat(&store, &result.tree)), ["ok.txt"]);
    let reasons: Vec<(String, SkipReason)> = result
        .skipped
        .iter()
        .map(|skipped| (skipped.path.to_string(), skipped.reason))
        .collect();
    assert_eq!(
        reasons,
        [
            ("locked".to_owned(), SkipReason::Unreadable),
            ("secret.txt".to_owned(), SkipReason::Unreadable),
        ]
    );
}

#[test]
fn a_fifo_is_skipped_as_special() {
    let (_dir, root) = source();
    write(&root, "a.txt", b"a");
    let made = std::process::Command::new("mkfifo")
        .arg(root.join("pipe"))
        .status()
        .unwrap();
    assert!(made.success());
    let (_store_dir, store) = store();
    let result = import(&store, &root);
    assert_eq!(paths(&flat(&store, &result.tree)), ["a.txt"]);
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].path.to_string(), "pipe");
    assert_eq!(result.skipped[0].reason, SkipReason::Special);
}

#[test]
fn a_source_without_an_executable_bit_imports_regular_files() {
    let (_dir, root) = source();
    write(&root, "run.sh", b"#!/bin/sh\n");
    fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    let (_store_dir, store) = store();
    let result = import_with(&store, &root, &import_opts(BARE));
    assert_eq!(
        flat(&store, &result.tree)[b"run.sh".as_slice()].kind,
        EntryKind::File
    );
}

#[test]
fn importing_twice_gives_the_same_tree_and_materialise_reproduces_the_source() {
    let (_dir, root) = source();
    fixture(&root);
    let (_store_dir, store) = store();
    let first = import(&store, &root);
    let second = import(&store, &root);
    assert_eq!(first, second);
    let (_other_dir, other) = self::store();
    assert_eq!(import(&other, &root).tree, first.tree);

    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("main");
    materialise(&store, &first.tree, &target, &opts(HOST, true)).unwrap();
    for path in paths(&flat(&store, &first.tree)) {
        let (from, to) = (root.join(&path), target.join(&path));
        let meta = fs::symlink_metadata(&from).unwrap();
        if meta.file_type().is_symlink() {
            assert_eq!(fs::read_link(&from).unwrap(), fs::read_link(&to).unwrap());
        } else {
            assert_eq!(fs::read(&from).unwrap(), fs::read(&to).unwrap(), "{path}");
            let exec = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o111;
            assert_eq!(exec(&from) != 0, exec(&to) != 0, "{path}");
        }
    }
    assert!(!target.join("empty").exists());
    assert!(!target.join("build").exists());
}

#[test]
fn an_empty_source_gives_the_empty_tree() {
    let (_dir, root) = source();
    fs::create_dir_all(root.join("only/empty/dirs")).unwrap();
    let (_store_dir, store) = store();
    let result = import(&store, &root);
    assert_eq!(result.files, 0);
    assert_eq!(result.bytes, 0);
    assert_eq!(
        result.tree,
        fold_vcs::revision_store::write_initial_tree(&store, FlatTree::new()).unwrap()
    );
}

#[test]
fn a_missing_source_stops_the_import() {
    let (dir, _root) = source();
    let (_store_dir, store) = store();
    let absent = dir.path().join("absent");
    let error = import_dir(
        &store,
        &absent,
        &IgnoreRules::new(&absent),
        None,
        &import_opts(HOST),
    )
    .unwrap_err();
    assert_eq!(error.code(), codes::WORKSPACE_IO);
    assert!(error.hint().is_some());
}

#[test]
fn a_failing_object_store_stops_the_import() {
    let (_dir, root) = source();
    write(&root, "a.txt", b"a");
    let (store_dir, store) = store();
    let scratch = store_dir.path().join(".fold/objects/tmp");
    fs::remove_dir_all(&scratch).unwrap();
    fs::write(&scratch, b"not a directory").unwrap();
    let error = import_dir(
        &store,
        &root,
        &IgnoreRules::new(&root),
        None,
        &import_opts(HOST),
    )
    .unwrap_err();
    assert_eq!(error.code(), codes::OBJECT_IO);
}

#[test]
fn an_import_keeps_tracked_files_the_ignore_rules_match_and_their_recorded_version_when_too_large()
{
    let dir = tempfile::tempdir().unwrap();
    let (_store_dir, store) = store();
    let root = dir.path();
    write(root, ".foldignore", b"secret.env\nbuild/\n");
    write(root, "secret.env", b"KEY=1");
    write(root, "build/version.txt", b"1.0");
    write(root, "build/out.bin", b"generated");
    write(root, "big.bin", b"grown past the limit");
    let entry = |content: &[u8]| FlatEntry {
        kind: EntryKind::File,
        hash: hash_object(ObjectKind::Blob, content),
    };
    let tracked: FlatTree = [
        (b"secret.env".to_vec(), entry(b"KEY=0")),
        (b"build/version.txt".to_vec(), entry(b"1.0")),
        (b"big.bin".to_vec(), entry(b"small")),
    ]
    .into();
    let opts = ImportOptions {
        max_file_bytes: 19,
        ..import_opts(HOST)
    };
    let imported =
        import_dir(&store, root, &IgnoreRules::new(root), Some(&tracked), &opts).unwrap();
    let flat = fold_vcs::compare::load_flat(&store, &imported.tree).unwrap();
    assert_eq!(
        flat_paths(&flat),
        [".foldignore", "big.bin", "build/version.txt", "secret.env"]
    );
    assert_eq!(flat[b"secret.env".as_slice()], entry(b"KEY=1"));
    assert_eq!(flat[b"big.bin".as_slice()], entry(b"small"));
    assert_eq!(imported.skipped.len(), 1);
    let untracked = import_dir(&store, root, &IgnoreRules::new(root), None, &opts).unwrap();
    assert_eq!(
        flat_paths(&fold_vcs::compare::load_flat(&store, &untracked.tree).unwrap()),
        [".foldignore"]
    );
}
