#![cfg(test)]

use std::fs::{self, File};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

use fold_core::object::{ObjectKind, hash_object};
use fold_core::tree::{ChangeCounts, EntryKind, FlatEntry, FlatTree};
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::index::StatIndex;
use fold_vcs::materialise::materialise;
use fold_vcs::paths::RepoPath;
use fold_vcs::platform::Capabilities;
use fold_vcs::scan::{
    ChangeKind, DEFAULT_MAX_FILE_BYTES, PathChange, ScanOptions, ScanResult, SkipReason, changes,
    count_changes, rescan_paths, rescan_scope, scan,
};

use crate::common::*;

fn options<'a>(rules: &'a IgnoreRules, caps: Capabilities, max: u64) -> ScanOptions<'a> {
    ScanOptions {
        rules,
        max_file_bytes: max,
        caps,
        recorded: None,
    }
}

fn run(root: &Path, index: &mut StatIndex) -> ScanResult {
    let rules = IgnoreRules::new(root);
    scan(root, index, &options(&rules, HOST, DEFAULT_MAX_FILE_BYTES)).unwrap()
}

fn tracked(root: &Path) -> Vec<String> {
    let mut index = StatIndex::empty(HOST);
    flat_paths(&run(root, &mut index).entries)
}

fn rp(path: &str) -> RepoPath {
    RepoPath::parse(path.as_bytes()).unwrap()
}

fn workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    fs::create_dir(&root).unwrap();
    (dir, root)
}

#[test]
fn a_second_scan_hashes_nothing_and_a_touched_file_hashes_alone() {
    let (_dir, root) = workspace();
    write(&root, "a.txt", b"one");
    write(&root, "d/b.txt", b"two");
    write(&root, "d/e/c.txt", b"three");
    let mut index = StatIndex::empty(HOST);
    let first = run(&root, &mut index);
    assert_eq!(first.hashed, 3);
    assert_eq!(
        first.entries[b"a.txt".as_slice()].hash,
        hash_object(ObjectKind::Blob, b"one")
    );
    assert_eq!(
        run(&root, &mut index).hashed,
        3,
        "nothing is fresh before a save"
    );
    settle(&mut index, &root.with_extension("idx"));
    let second = run(&root, &mut index);
    assert_eq!(second.hashed, 0);
    assert_eq!(second.entries, first.entries);

    let file = fs::OpenOptions::new()
        .write(true)
        .open(root.join("d/b.txt"))
        .unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    let third = run(&root, &mut index);
    assert_eq!(third.hashed, 1);
    assert_eq!(third.entries, first.entries);
}

#[test]
fn an_edit_and_a_removal_show_in_the_next_scan() {
    let (_dir, root) = workspace();
    write(&root, "a.txt", b"one");
    write(&root, "d/b.txt", b"two");
    let mut index = StatIndex::empty(HOST);
    run(&root, &mut index);
    write(&root, "a.txt", b"three");
    fs::remove_dir_all(root.join("d")).unwrap();
    let next = run(&root, &mut index);
    assert_eq!(flat_paths(&next.entries), ["a.txt"]);
    assert_eq!(next.removed, [rp("d/b.txt")]);
    assert_eq!(index.len(), 1);
}

#[test]
fn symlinks_are_recorded_and_never_followed() {
    let (_dir, root) = workspace();
    write(&root, "real/inner.txt", b"x");
    symlink("real", root.join("dirlink")).unwrap();
    symlink("missing", root.join("dangling")).unwrap();
    let mut index = StatIndex::empty(HOST);
    let found = run(&root, &mut index);
    assert_eq!(
        flat_paths(&found.entries),
        ["dangling", "dirlink", "real/inner.txt"]
    );
    let link = found.entries[b"dirlink".as_slice()];
    assert_eq!(link.kind, EntryKind::Symlink);
    assert_eq!(link.hash, hash_object(ObjectKind::Blob, b"real"));
    settle(&mut index, &root.with_extension("idx"));
    let again = run(&root, &mut index);
    assert_eq!(again.hashed, 0);
    assert_eq!(again.entries, found.entries);
}

#[test]
fn executables_sockets_and_fifos() {
    let (_dir, root) = workspace();
    write(&root, "run.sh", b"#!/bin/sh\n");
    fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    write(&root, "plain", b"x");
    let _socket = std::os::unix::net::UnixListener::bind(root.join("sock")).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(root.join("pipe"))
            .status()
            .unwrap()
            .success()
    );
    let mut index = StatIndex::empty(HOST);
    let found = run(&root, &mut index);
    assert_eq!(flat_paths(&found.entries), ["plain", "run.sh"]);
    assert_eq!(
        found.entries[b"run.sh".as_slice()].kind,
        EntryKind::Executable
    );
    assert_eq!(found.entries[b"plain".as_slice()].kind, EntryKind::File);
    let mut skipped: Vec<_> = found
        .skipped
        .iter()
        .map(|skipped| (skipped.path.to_string(), skipped.reason))
        .collect();
    skipped.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        skipped,
        [
            ("pipe".to_string(), SkipReason::Special),
            ("sock".to_string(), SkipReason::Special)
        ]
    );
    assert!(index.lookup(&rp("pipe")).is_none());
}

#[test]
fn a_large_file_is_listed_as_too_large() {
    let (_dir, root) = workspace();
    write(&root, "small", b"123");
    write(&root, "big", b"1234");
    let rules = IgnoreRules::new(&root);
    let mut index = StatIndex::empty(HOST);
    let found = scan(&root, &mut index, &options(&rules, HOST, 3)).unwrap();
    assert_eq!(flat_paths(&found.entries), ["small"]);
    assert_eq!(found.skipped.len(), 1);
    assert_eq!(found.skipped[0].path, rp("big"));
    assert_eq!(found.skipped[0].reason, SkipReason::TooLarge);
}

#[test]
fn an_unreadable_file_is_skipped() {
    let (_dir, root) = workspace();
    write(&root, "secret", b"x");
    write(&root, "open", b"y");
    fs::set_permissions(root.join("secret"), fs::Permissions::from_mode(0o000)).unwrap();
    if File::open(root.join("secret")).is_ok() {
        return;
    }
    let mut index = StatIndex::empty(HOST);
    let found = run(&root, &mut index);
    assert_eq!(flat_paths(&found.entries), ["open"]);
    assert_eq!(found.skipped[0].reason, SkipReason::Unreadable);
}

fn ignore_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let (dir, root) = workspace();
    for path in [
        "keep.txt",
        "skip.log",
        "src/main.rs",
        "src/skip.log",
        "src/sub/deep.log",
        "src/sub/keep.txt",
        "build/out.bin",
        "src/build/inner.bin",
    ] {
        write(&root, path, b"x");
    }
    (dir, root)
}

#[test]
fn an_ignore_file_starting_with_a_byte_order_mark_still_applies_and_a_linked_one_is_not_read() {
    let (dir, root) = workspace();
    write(&root, ".gitignore", "\u{feff}*.log\n".as_bytes());
    write(&root, "a.log", b"x");
    write(&root, "src/b.tmp", b"x");
    write(dir.path(), "outside", b"*.tmp\n");
    symlink(dir.path().join("outside"), root.join("src/.gitignore")).unwrap();
    assert_eq!(
        tracked(&root),
        [".gitignore", "src/.gitignore", "src/b.tmp"]
    );
}

#[test]
fn a_root_gitignore_applies_everywhere_and_a_nested_one_only_below_its_folder() {
    let (_dir, root) = ignore_fixture();
    write(&root, ".gitignore", b"*.log\nbuild/\n");
    write(&root, "src/.gitignore", b"main.rs\n");
    assert_eq!(
        tracked(&root),
        [
            ".gitignore",
            "keep.txt",
            "src/.gitignore",
            "src/sub/keep.txt"
        ]
    );
    write(&root, "main.rs", b"top level");
    assert!(tracked(&root).contains(&"main.rs".to_string()));
}

#[test]
fn a_foldignore_adds_patterns_and_can_bring_a_file_back() {
    let (_dir, root) = ignore_fixture();
    write(&root, ".gitignore", b"*.log\n");
    write(&root, ".foldignore", b"/keep.txt\n!src/skip.log\n");
    let found = tracked(&root);
    assert!(!found.contains(&"keep.txt".to_string()));
    assert!(found.contains(&"src/skip.log".to_string()));
    assert!(!found.contains(&"skip.log".to_string()));
    assert!(found.contains(&"src/sub/keep.txt".to_string()));
}

#[test]
fn a_nested_foldignore_applies_only_below_its_folder() {
    let (_dir, root) = ignore_fixture();
    write(&root, "src/sub/.foldignore", b"keep.txt\n");
    let found = tracked(&root);
    assert!(!found.contains(&"src/sub/keep.txt".to_string()));
    assert!(found.contains(&"keep.txt".to_string()));
}

#[test]
fn a_deeper_file_wins_over_a_shallower_one() {
    let (_dir, root) = ignore_fixture();
    write(&root, ".gitignore", b"*.log\n");
    write(&root, "src/sub/.gitignore", b"!*.log\n");
    let found = tracked(&root);
    assert!(found.contains(&"src/sub/deep.log".to_string()));
    assert!(!found.contains(&"src/skip.log".to_string()));
}

#[test]
fn a_file_cannot_come_back_inside_an_ignored_directory() {
    let (_dir, root) = ignore_fixture();
    write(&root, ".gitignore", b"build/\n!build/out.bin\n");
    assert!(!tracked(&root).contains(&"build/out.bin".to_string()));
    let rules = IgnoreRules::new(&root);
    assert!(rules.is_ignored(&rp("build/out.bin"), false));
    assert!(rules.is_ignored(&rp("build"), true));
    assert!(!rules.is_ignored(&rp("build"), false));
}

#[test]
fn only_the_workspace_files_count() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project/ws");
    fs::create_dir_all(&root).unwrap();
    write(&root, "a.log", b"x");
    write(&root, "keep.txt", b"x");
    write(&root, ".hidden", b"x");
    write(&root, ".ignore", b"*.log\n");
    write(&root, ".git/config", b"x");
    write(&root, ".git/info/exclude", b"keep.txt\n");
    write(&root, ".fold/state", b"x");
    write(&root, ".fold/tmp/staged", b"x");
    write(&root, "sub/.git/config", b"x");
    write(&root, "sub/.fold/state", b"x");
    write(&root, "sub/.fold-tmp-12-3", b"an ordinary name");
    write(&root, "sub/.fold-tmp-notes", b"real file");
    write(dir.path(), "project/.gitignore", b"*\n");
    write(dir.path(), "project/.foldignore", b"*\n");
    write(dir.path(), ".gitignore", b"*\n");
    assert_eq!(
        tracked(&root),
        [
            ".hidden",
            ".ignore",
            "a.log",
            "keep.txt",
            "sub/.fold-tmp-12-3",
            "sub/.fold-tmp-notes"
        ]
    );
}

#[test]
fn an_ignored_directory_is_never_entered() {
    let (_dir, root) = workspace();
    write(&root, ".gitignore", b"private/\n");
    write(&root, "private/inner/file", b"x");
    write(&root, "public", b"x");
    fs::set_permissions(root.join("private"), fs::Permissions::from_mode(0o000)).unwrap();
    let mut index = StatIndex::empty(HOST);
    let found = run(&root, &mut index);
    fs::set_permissions(root.join("private"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(found.skipped.is_empty());
    assert_eq!(flat_paths(&found.entries), [".gitignore", "public"]);
}

#[test]
fn an_unreadable_directory_is_skipped() {
    let (_dir, root) = workspace();
    write(&root, "locked/file", b"x");
    write(&root, "open", b"x");
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(root.join("locked")).is_ok();
    let mut index = StatIndex::empty(HOST);
    let found = run(&root, &mut index);
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    if !readable {
        assert_eq!(found.skipped.len(), 1);
        assert_eq!(found.skipped[0].path, rp("locked"));
        assert_eq!(found.skipped[0].reason, SkipReason::Unreadable);
    }
}

#[test]
fn a_partial_rescan_below_an_unsearchable_directory_fails_instead_of_dropping_its_files() {
    let (_dir, root) = workspace();
    write(&root, "locked/file", b"x");
    write(&root, "open", b"x");
    let mut index = StatIndex::empty(HOST);
    run(&root, &mut index);
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(root.join("locked")).is_ok();
    let rules = IgnoreRules::new(&root);
    let opts = options(&rules, HOST, DEFAULT_MAX_FILE_BYTES);
    let found = rescan_paths(&root, &mut index, &[rp("locked/file")], &opts);
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return;
    }
    assert!(found.is_err());
    assert!(index.lookup(&rp("locked/file")).is_some());
}

#[test]
fn global_git_excludes_have_no_effect() {
    if let Ok(root) = std::env::var("FOLD_TEST_GLOBAL_ROOT") {
        let found = tracked(Path::new(&root));
        assert_eq!(found, ["a.log", "keep.txt"]);
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    write(&home, ".config/git/ignore", b"*.log\n");
    write(
        &home,
        "gitconfig",
        b"[core]\n\texcludesFile = ~/.config/git/ignore\n",
    );
    let root = dir.path().join("ws");
    write(&root, "a.log", b"x");
    write(&root, "keep.txt", b"x");
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "scan::global_git_excludes_have_no_effect",
            "--nocapture",
        ])
        .env("FOLD_TEST_GLOBAL_ROOT", &root)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("GIT_CONFIG_GLOBAL", home.join("gitconfig"))
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn rules_follow_a_changed_ignore_file_on_the_next_scan() {
    let (_dir, root) = ignore_fixture();
    let rules = IgnoreRules::new(&root);
    let mut index = StatIndex::empty(HOST);
    let opts = options(&rules, HOST, DEFAULT_MAX_FILE_BYTES);
    let before = scan(&root, &mut index, &opts).unwrap();
    assert!(before.entries.contains_key(b"skip.log".as_slice()));
    write(&root, ".gitignore", b"*.log\n");
    let after = scan(&root, &mut index, &opts).unwrap();
    assert!(!after.entries.contains_key(b"skip.log".as_slice()));
    assert!(after.removed.contains(&rp("skip.log")));
    assert!(index.lookup(&rp("skip.log")).is_none());
}

#[test]
fn a_symlinked_ignore_file_is_not_read() {
    let (_dir, root) = workspace();
    write(&root, "elsewhere", b"*.log\n");
    symlink("elsewhere", root.join(".gitignore")).unwrap();
    write(&root, "a.log", b"x");
    assert!(tracked(&root).contains(&"a.log".to_string()));
}

#[test]
fn without_an_exec_bit_the_recorded_kind_stays() {
    let (_store_dir, store) = store();
    let script = blob(&store, b"#!/bin/sh\n");
    let plain = blob(&store, b"text");
    let tree = tree(
        &store,
        &[
            ("plain", EntryKind::File, plain),
            ("run.sh", EntryKind::Executable, script),
        ],
    );
    let dir = tempfile::tempdir().unwrap();
    let root2 = dir.path().join("ws");
    let done = materialise(&store, &tree, &root2, &opts(BARE, false)).unwrap();
    let mut index = done.index;
    // The bare platform would show both files with the same mode bits.
    fs::set_permissions(root2.join("run.sh"), fs::Permissions::from_mode(0o644)).unwrap();
    let rules = IgnoreRules::new(&root2);
    let opts = options(&rules, BARE, DEFAULT_MAX_FILE_BYTES);
    let found = scan(&root2, &mut index, &opts).unwrap();
    assert_eq!(
        found.entries[b"run.sh".as_slice()].kind,
        EntryKind::Executable
    );
    assert_eq!(found.entries[b"plain".as_slice()].kind, EntryKind::File);

    let mut fresh = StatIndex::empty(BARE);
    let recorded: FlatTree = [(
        b"run.sh".to_vec(),
        FlatEntry {
            kind: EntryKind::Executable,
            hash: script,
        },
    )]
    .into();
    let with_base = ScanOptions {
        recorded: Some(&recorded),
        ..opts
    };
    let found = scan(&root2, &mut fresh, &with_base).unwrap();
    assert_eq!(
        found.entries[b"run.sh".as_slice()].kind,
        EntryKind::Executable
    );
    assert_eq!(found.entries[b"plain".as_slice()].kind, EntryKind::File);
}

#[test]
fn an_emulated_symlink_reports_no_change() {
    let dir = tempfile::tempdir().unwrap();
    let (_store_dir, store) = store();
    let target = blob(&store, b"real/target");
    let tree = tree(&store, &[("link", EntryKind::Symlink, target)]);
    let root = dir.path().join("emulated");
    let done = materialise(&store, &tree, &root, &opts(BARE, false)).unwrap();
    assert_eq!(done.degraded.len(), 1);
    let mut index = done.index;
    settle(&mut index, &root.with_extension("idx"));
    let rules = IgnoreRules::new(&root);
    let found = scan(
        &root,
        &mut index,
        &options(&rules, BARE, DEFAULT_MAX_FILE_BYTES),
    )
    .unwrap();
    assert_eq!(found.hashed, 0);
    let entry = found.entries[b"link".as_slice()];
    assert_eq!(entry.kind, EntryKind::Symlink);
    assert_eq!(entry.hash, target);

    write(&root, "link", b"edited target");
    fs::File::options()
        .write(true)
        .open(root.join("link"))
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(9))
        .unwrap();
    let found = scan(
        &root,
        &mut index,
        &options(&rules, BARE, DEFAULT_MAX_FILE_BYTES),
    )
    .unwrap();
    assert_eq!(found.hashed, 1);
    assert_eq!(found.entries[b"link".as_slice()].kind, EntryKind::File);
}

#[test]
fn rescan_looks_at_the_touched_paths_only() {
    let (_dir, root) = workspace();
    write(&root, "a.txt", b"one");
    write(&root, "d/b.txt", b"two");
    write(&root, "d/e/c.txt", b"three");
    write(&root, "other.txt", b"four");
    write(&root, ".gitignore", b"*.tmp\n");
    let mut index = StatIndex::empty(HOST);
    run(&root, &mut index);
    let rules = IgnoreRules::new(&root);
    let opts = options(&rules, HOST, DEFAULT_MAX_FILE_BYTES);

    write(&root, "a.txt", b"changed!");
    write(&root, "other.txt", b"not looked at");
    write(&root, "d/new.txt", b"new");
    write(&root, "d/skip.tmp", b"ignored");
    fs::remove_file(root.join("d/b.txt")).unwrap();
    fs::remove_dir_all(root.join("d/e")).unwrap();
    let touched = [rp("a.txt"), rp("d"), rp("d/skip.tmp"), rp("nothing/here")];
    let found = rescan_paths(&root, &mut index, &touched, &opts).unwrap();
    assert_eq!(flat_paths(&found.entries), ["a.txt", "d/new.txt"]);
    assert_eq!(found.removed, [rp("d/b.txt"), rp("d/e/c.txt")]);
    assert_eq!(
        index.lookup(&rp("other.txt")).unwrap().hash,
        hash_object(ObjectKind::Blob, b"four")
    );
    assert!(index.lookup(&rp("d/new.txt")).is_some());

    // A directory that became a file, and a file that became a directory.
    fs::remove_dir_all(root.join("d")).unwrap();
    write(&root, "d", b"now a file");
    fs::remove_file(root.join("a.txt")).unwrap();
    write(&root, "a.txt/inner", b"x");
    let found = rescan_paths(&root, &mut index, &[rp("d"), rp("a.txt")], &opts).unwrap();
    assert_eq!(flat_paths(&found.entries), ["a.txt/inner", "d"]);
    assert!(index.lookup(&rp("a.txt")).is_none());
    assert!(index.lookup(&rp("d/new.txt")).is_none());

    // Paths under a symlinked directory are not tracked through it.
    symlink("a.txt", root.join("via")).unwrap();
    let found = rescan_paths(&root, &mut index, &[rp("via/inner")], &opts).unwrap();
    assert!(found.entries.is_empty());
}

#[test]
fn a_new_directory_touched_at_every_level_is_hashed_once() {
    let (_dir, root) = workspace();
    write(&root, "old.txt", b"old");
    let mut index = StatIndex::empty(HOST);
    run(&root, &mut index);
    let rules = IgnoreRules::new(&root);
    let opts = options(&rules, HOST, DEFAULT_MAX_FILE_BYTES);
    write(&root, "new/a/b/f.txt", b"deep");
    let touched = [rp("new/a/b/f.txt"), rp("new/a/b"), rp("new/a"), rp("new")];
    let found = rescan_paths(&root, &mut index, &touched, &opts).unwrap();
    assert_eq!(flat_paths(&found.entries), ["new/a/b/f.txt"]);
    assert_eq!(found.hashed, 1);
}

#[test]
fn the_rescan_scope_keeps_only_targets_no_other_target_covers() {
    let touched = [
        rp("a/b"),
        rp("a-b/c"),
        rp("a"),
        rp("x/y/z"),
        rp("x/.gitignore"),
        rp("a/b"),
    ];
    assert_eq!(
        rescan_scope(&touched).unwrap(),
        [rp("a"), rp("a-b/c"), rp("x")]
    );
    assert_eq!(rescan_scope(&[rp("a"), rp(".foldignore")]), None);
}

#[test]
fn changes_compare_by_hash_and_kind() {
    let hash = |text: &[u8]| hash_object(ObjectKind::Blob, text);
    let entry = |kind, text: &[u8]| FlatEntry {
        kind,
        hash: hash(text),
    };
    let base: FlatTree = [
        (b"same".to_vec(), entry(EntryKind::File, b"1")),
        (b"edited".to_vec(), entry(EntryKind::File, b"2")),
        (b"mode".to_vec(), entry(EntryKind::File, b"3")),
        (b"gone".to_vec(), entry(EntryKind::File, b"4")),
    ]
    .into();
    let current: FlatTree = [
        (b"same".to_vec(), entry(EntryKind::File, b"1")),
        (b"edited".to_vec(), entry(EntryKind::File, b"2b")),
        (b"mode".to_vec(), entry(EntryKind::Executable, b"3")),
        (b"new".to_vec(), entry(EntryKind::Symlink, b"5")),
    ]
    .into();
    let kinds = |path: &str, kind| PathChange {
        path: path.as_bytes().to_vec(),
        kind,
    };
    assert_eq!(
        changes(&base, &current),
        [
            kinds("edited", ChangeKind::Modified),
            kinds("gone", ChangeKind::Deleted),
            kinds("mode", ChangeKind::Modified),
            kinds("new", ChangeKind::Added),
        ]
    );
    assert!(changes(&base, &base).is_empty());
    assert!(changes(&FlatTree::new(), &FlatTree::new()).is_empty());
}

#[test]
fn change_counts_count_each_kind_of_changed_path() {
    let hash = |text: &[u8]| hash_object(ObjectKind::Blob, text);
    let file = |text: &[u8]| FlatEntry {
        kind: EntryKind::File,
        hash: hash(text),
    };
    let base: FlatTree = [
        (b"README.md".to_vec(), file(b"readme")),
        (b"old.txt".to_vec(), file(b"old")),
        (b"src/lib.rs".to_vec(), file(b"lib")),
        (b"src/main.rs".to_vec(), file(b"main")),
        (b"same.txt".to_vec(), file(b"same")),
    ]
    .into();
    let current: FlatTree = [
        (b"README.md".to_vec(), file(b"readme, edited")),
        (b"docs/a.md".to_vec(), file(b"a")),
        (b"docs/b.md".to_vec(), file(b"b")),
        (b"src/lib.rs".to_vec(), file(b"lib, edited")),
        (b"src/main.rs".to_vec(), file(b"main, edited")),
        (b"src/new.rs".to_vec(), file(b"new")),
        (b"same.txt".to_vec(), file(b"same")),
    ]
    .into();
    assert_eq!(
        count_changes(&changes(&base, &current)),
        ChangeCounts {
            files_added: 3,
            files_modified: 3,
            files_deleted: 1,
        }
    );
    assert_eq!(count_changes(&[]), ChangeCounts::default());
}

#[test]
fn a_touched_ignore_file_rescans_its_folder() {
    let (_dir, root) = ignore_fixture();
    let mut index = StatIndex::empty(HOST);
    run(&root, &mut index);
    let rules = IgnoreRules::new(&root);
    let opts = options(&rules, HOST, DEFAULT_MAX_FILE_BYTES);

    write(&root, "src/.gitignore", b"*.log\n");
    let found = rescan_paths(&root, &mut index, &[rp("src/.gitignore")], &opts).unwrap();
    assert_eq!(
        flat_paths(&found.entries),
        [
            "src/.gitignore",
            "src/build/inner.bin",
            "src/main.rs",
            "src/sub/keep.txt"
        ]
    );
    assert_eq!(found.removed, [rp("src/skip.log"), rp("src/sub/deep.log")]);
    assert!(index.lookup(&rp("skip.log")).is_some());

    write(&root, ".foldignore", b"build/\n");
    let found = rescan_paths(&root, &mut index, &[rp(".foldignore")], &opts).unwrap();
    assert!(found.entries.contains_key(b"keep.txt".as_slice()));
    assert!(found.removed.contains(&rp("build/out.bin")));
    assert!(found.removed.contains(&rp("src/build/inner.bin")));
    assert!(index.lookup(&rp("build/out.bin")).is_none());
}

#[test]
fn an_emulated_symlink_stays_a_link_when_the_index_is_lost() {
    let dir = tempfile::tempdir().unwrap();
    let (_store_dir, store) = store();
    let target = blob(&store, b"real/target");
    let tree = tree(&store, &[("link", EntryKind::Symlink, target)]);
    let root = dir.path().join("emulated");
    materialise(&store, &tree, &root, &opts(BARE, false)).unwrap();
    let recorded: FlatTree = [(
        b"link".to_vec(),
        FlatEntry {
            kind: EntryKind::Symlink,
            hash: target,
        },
    )]
    .into();
    let rules = IgnoreRules::new(&root);
    let with_base = ScanOptions {
        recorded: Some(&recorded),
        ..options(&rules, BARE, DEFAULT_MAX_FILE_BYTES)
    };
    let mut lost = StatIndex::empty(BARE);
    let found = scan(&root, &mut lost, &with_base).unwrap();
    assert!(changes(&recorded, &found.entries).is_empty());
    assert_eq!(lost.lookup(&rp("link")).unwrap().kind, EntryKind::Symlink);

    let mut on_host = StatIndex::empty(HOST);
    let host_opts = ScanOptions {
        caps: HOST,
        ..with_base
    };
    let found = scan(&root, &mut on_host, &host_opts).unwrap();
    assert_eq!(found.entries[b"link".as_slice()].kind, EntryKind::File);
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    fn flat_tree() -> impl Strategy<Value = FlatTree> {
        let kind = prop_oneof![
            Just(EntryKind::File),
            Just(EntryKind::Executable),
            Just(EntryKind::Symlink)
        ];
        prop::collection::btree_map(
            prop::sample::select(vec!["a", "a/b", "b", "c/d", "c/e", "f"]),
            (kind, 0u8..3),
            0..6,
        )
        .prop_map(|entries| {
            entries
                .into_iter()
                .map(|(path, (kind, content))| {
                    let hash = hash_object(ObjectKind::Blob, &[content]);
                    (path.as_bytes().to_vec(), FlatEntry { kind, hash })
                })
                .collect()
        })
    }

    proptest! {
        /// Applying the reported changes to the base gives the current tree, and each change has the kind its two sides imply.
        #[test]
        fn changes_turn_the_base_into_the_current_tree(base in flat_tree(), current in flat_tree()) {
            let mut applied = base.clone();
            for change in changes(&base, &current) {
                let expected = match (base.get(&change.path), current.get(&change.path)) {
                    (None, Some(_)) => ChangeKind::Added,
                    (Some(_), None) => ChangeKind::Deleted,
                    (Some(old), Some(new)) => {
                        prop_assert_ne!(old, new);
                        ChangeKind::Modified
                    }
                    (None, None) => unreachable!(),
                };
                prop_assert_eq!(change.kind, expected);
                match current.get(&change.path) {
                    Some(entry) => applied.insert(change.path.clone(), *entry),
                    None => applied.remove(&change.path),
                };
            }
            prop_assert_eq!(applied, current);
        }
    }
}

#[test]
fn a_same_size_rewrite_in_the_recorded_tick_is_hashed_again_until_a_save() {
    let (_dir, root) = workspace();
    write(&root, "f", b"aaaa");
    let mut index = StatIndex::empty(HOST);
    run(&root, &mut index);
    let recorded = fs::metadata(root.join("f")).unwrap().modified().unwrap();
    write(&root, "f", b"bbbb");
    File::options()
        .write(true)
        .open(root.join("f"))
        .unwrap()
        .set_modified(recorded)
        .unwrap();
    let found = run(&root, &mut index);
    assert_eq!(found.hashed, 1);
    assert_eq!(
        found.entries[b"f".as_slice()].hash,
        hash_object(ObjectKind::Blob, b"bbbb")
    );

    settle(&mut index, &root.with_extension("idx"));
    assert_eq!(run(&root, &mut index).hashed, 0);
}

#[test]
fn a_foldignore_rule_wins_over_a_deeper_gitignore() {
    let (_dir, root) = workspace();
    write(&root, ".foldignore", b"!sub/x.log\n");
    write(&root, "sub/.gitignore", b"x.log\ny.log\n");
    write(&root, "sub/x.log", b"x");
    write(&root, "sub/y.log", b"y");
    write(&root, "sub/deep/.foldignore", b"z.txt\n");
    write(&root, ".gitignore", b"!sub/deep/z.txt\n");
    write(&root, "sub/deep/z.txt", b"z");
    assert_eq!(
        tracked(&root),
        [
            ".foldignore",
            ".gitignore",
            "sub/.gitignore",
            "sub/deep/.foldignore",
            "sub/x.log"
        ]
    );
    let rules = IgnoreRules::new(&root);
    assert!(!rules.is_ignored(&rp("sub/x.log"), false));
    assert!(rules.is_ignored(&rp("sub/y.log"), false));
    assert!(rules.is_ignored(&rp("sub/deep/z.txt"), false));

    let mut index = StatIndex::empty(HOST);
    let opts = options(&rules, HOST, DEFAULT_MAX_FILE_BYTES);
    let touched = [rp("sub/x.log"), rp("sub/y.log"), rp("sub/deep/z.txt")];
    let found = rescan_paths(&root, &mut index, &touched, &opts).unwrap();
    assert_eq!(flat_paths(&found.entries), ["sub/x.log"]);
}

mod walker_agreement {
    use super::*;
    use proptest::prelude::*;

    const FOLDERS: [&str; 3] = ["", "sub", "sub/deep"];
    const FILES: [&str; 7] = [
        "x.log",
        "a.txt",
        "sub/x.log",
        "sub/a.txt",
        "sub/deep/x.log",
        "sub/deep/a.txt",
        "deep/x.log",
    ];
    const LINES: [&str; 10] = [
        "*.log",
        "!*.log",
        "x.log",
        "!x.log",
        "deep/",
        "!deep/",
        "/a.txt",
        "!sub/x.log",
        "sub/deep/",
        "!a.txt",
    ];

    fn ignore_files() -> impl Strategy<Value = Vec<(usize, bool, Vec<usize>)>> {
        prop::collection::vec(
            (
                0..FOLDERS.len(),
                any::<bool>(),
                prop::collection::vec(0..LINES.len(), 1..4),
            ),
            0..5,
        )
    }

    /// What the `ignore` crate's walker tracks with the settings a workspace needs.
    fn walked(root: &Path) -> Vec<String> {
        let mut found: Vec<String> = ::ignore::WalkBuilder::new(root)
            .git_ignore(true)
            .require_git(false)
            .git_global(false)
            .git_exclude(false)
            .ignore(false)
            .parents(false)
            .hidden(false)
            .add_custom_ignore_filename(".foldignore")
            .build()
            .map(Result::unwrap)
            .filter(|entry| !entry.file_type().unwrap().is_dir())
            .map(|entry| {
                let relative = entry.path().strip_prefix(root).unwrap();
                relative.to_str().unwrap().to_string()
            })
            .collect();
        found.sort();
        found
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// The scan, `is_ignored` and the crate's walker agree on every file.
        #[test]
        fn the_scan_tracks_what_the_walker_tracks(files in ignore_files()) {
            let (_dir, root) = workspace();
            for file in FILES {
                write(&root, file, b"x");
            }
            for (folder, fold, lines) in &files {
                let name = if *fold { ".foldignore" } else { ".gitignore" };
                let path = match FOLDERS[*folder] {
                    "" => name.to_string(),
                    folder => format!("{folder}/{name}"),
                };
                let mut text = String::new();
                for line in lines {
                    text.push_str(LINES[*line]);
                    text.push('\n');
                }
                write(&root, &path, text.as_bytes());
            }
            let ours = tracked(&root);
            prop_assert_eq!(&ours, &walked(&root));
            let rules = IgnoreRules::new(&root);
            for file in FILES {
                let ignored = rules.is_ignored(&rp(file), false);
                prop_assert_eq!(ignored, !ours.contains(&file.to_string()), "{}", file);
            }
        }
    }
}
