#![cfg(test)]

pub mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use fold_core::object::ObjectHash;
use fold_core::tree::{EntryKind, flatten};
use fold_core::{FoldError, codes};
use fold_vcs::git::Git;
use fold_vcs::git_import::{GitImportOptions, archive_tree};
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::RepoPath;
use fold_vcs::scan::{DEFAULT_MAX_FILE_BYTES, SkipReason, Skipped};

use common::*;

const OPTS: GitImportOptions = GitImportOptions {
    max_file_bytes: DEFAULT_MAX_FILE_BYTES,
};

fn write(root: &Path, path: &str, bytes: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// Every path of `tree` with its kind and content.
fn contents(store: &ObjectStore, tree: &ObjectHash) -> BTreeMap<String, (EntryKind, Vec<u8>)> {
    let load = |hash: &ObjectHash| -> Result<_, FoldError> {
        let object = store.read(hash)?;
        Ok(fold_core::tree::Tree::decode(&object.payload)?)
    };
    flatten(tree, &load)
        .unwrap()
        .into_iter()
        .map(|(path, entry)| {
            let bytes = store.read(&entry.hash).unwrap().payload;
            (String::from_utf8(path).unwrap(), (entry.kind, bytes))
        })
        .collect()
}

/// A repository with two commits; the second holds a file, an executable, a symlink, a submodule, a committed `.fold/` and a committed file that `.gitignore` names, next to untracked files. Returns the first commit.
fn sample_repo(repo: &Path) -> String {
    fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "--quiet", "--initial-branch=main"]);
    write(repo, "README.md", b"first\n");
    git(repo, &["add", "."]);
    git(repo, &["commit", "--quiet", "-m", "first"]);
    let first = git(repo, &["rev-parse", "HEAD"]).trim().to_owned();

    write(repo, "README.md", b"# app\n");
    write(repo, ".gitignore", b"*.log\nbuild/\n");
    write(repo, "src/lib.rs", b"pub fn x() {}\n");
    write(repo, "run.sh", b"#!/bin/sh\n");
    fs::set_permissions(repo.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("src/lib.rs", repo.join("link")).unwrap();
    write(repo, "kept.log", b"committed although ignored\n");
    write(repo, ".fold/state", b"never imported\n");
    write(repo, "docs/.fold", b"never imported\n");
    git(repo, &["add", "."]);
    git(
        repo,
        &["add", "--force", "kept.log", ".fold/state", "docs/.fold"],
    );
    git(
        repo,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            "160000,1111111111111111111111111111111111111111,vendor/sub",
        ],
    );
    git(repo, &["commit", "--quiet", "-m", "second"]);
    write(repo, "build/out.bin", b"untracked and ignored\n");
    write(repo, "notes.txt", b"untracked\n");
    write(repo, "README.md", b"changed in the working tree\n");
    first
}

#[test]
fn head_imports_exactly_the_committed_files() {
    let (dir, store) = store();
    let repo = dir.path().join("repo");
    sample_repo(&repo);
    let result = archive_tree(&store, &Git::default(), &repo, "HEAD", &OPTS).unwrap();
    let found = contents(&store, &result.tree);
    let expected: BTreeMap<String, (EntryKind, Vec<u8>)> = [
        (".gitignore", EntryKind::File, &b"*.log\nbuild/\n"[..]),
        ("README.md", EntryKind::File, b"# app\n"),
        ("kept.log", EntryKind::File, b"committed although ignored\n"),
        ("link", EntryKind::Symlink, b"src/lib.rs"),
        ("run.sh", EntryKind::Executable, b"#!/bin/sh\n"),
        ("src/lib.rs", EntryKind::File, b"pub fn x() {}\n"),
    ]
    .into_iter()
    .map(|(path, kind, bytes)| (path.to_owned(), (kind, bytes.to_vec())))
    .collect();
    assert_eq!(found, expected);
    assert_eq!(result.files, 6);
    let bytes: usize = expected.values().map(|(_, bytes)| bytes.len()).sum();
    assert_eq!(result.bytes, bytes as u64);
    assert!(result.skipped.is_empty());
    // The working tree is only read by git.
    assert_eq!(
        fs::read(repo.join("README.md")).unwrap(),
        b"changed in the working tree\n"
    );
}

#[test]
fn another_commit_gives_that_commits_tree() {
    let (dir, store) = store();
    let repo = dir.path().join("repo");
    let first = sample_repo(&repo);
    let git_program = Git::default();
    let result = archive_tree(&store, &git_program, &repo, &first, &OPTS).unwrap();
    let found = contents(&store, &result.tree);
    assert_eq!(found.len(), 1);
    assert_eq!(found["README.md"], (EntryKind::File, b"first\n".to_vec()));
    assert_eq!(
        git_program
            .resolve_commit(&repo, "HEAD~1")
            .unwrap()
            .as_str(),
        first
    );
}

#[test]
fn files_over_the_limit_are_skipped_and_listed() {
    let (dir, store) = store();
    let repo = dir.path().join("repo");
    sample_repo(&repo);
    let opts = GitImportOptions { max_file_bytes: 10 };
    let result = archive_tree(&store, &Git::default(), &repo, "HEAD", &opts).unwrap();
    let found = contents(&store, &result.tree);
    assert_eq!(
        found.keys().collect::<Vec<_>>(),
        ["README.md", "link", "run.sh"]
    );
    let skipped = |path: &str| Skipped {
        path: RepoPath::parse(path.as_bytes()).unwrap(),
        reason: SkipReason::TooLarge,
    };
    assert_eq!(
        result.skipped,
        [
            skipped(".gitignore"),
            skipped("kept.log"),
            skipped("src/lib.rs")
        ]
    );
    let tiny = GitImportOptions { max_file_bytes: 2 };
    let result = archive_tree(&store, &Git::default(), &repo, "HEAD", &tiny).unwrap();
    assert!(result.skipped.contains(&skipped("link")));
}

#[test]
fn a_missing_git_program_is_git_not_found() {
    let (dir, store) = store();
    let missing = Git::new(dir.path().join("bin/git"));
    let error = archive_tree(&store, &missing, dir.path(), "HEAD", &OPTS).unwrap_err();
    assert_eq!(error.code(), codes::GIT_NOT_FOUND);
}

#[test]
fn a_failing_listing_is_git_failed_with_its_status_only() {
    let (dir, store) = store();
    let error = archive_tree(&store, &Git::default(), dir.path(), "HEAD", &OPTS).unwrap_err();
    assert_eq!(error.code(), codes::GIT_FAILED);
    let details = error.details().unwrap();
    assert_eq!(details["command"], "ls-tree");
    assert_eq!(details["status"], 128);
    assert!(!error.message().contains("fatal"));
    let refused = archive_tree(&store, &Git::default(), dir.path(), "--output=x", &OPTS);
    assert_eq!(refused.unwrap_err().code(), codes::INVALID_REQUEST);
}

/// A commit whose tree is built by `git mktree` from `lines`, so it can hold names that `git add` would refuse.
fn crafted_commit(repo: &Path, lines: &str) -> String {
    let mut command = git_command(repo);
    command
        .args(["mktree", "--missing"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    let mut child = command.spawn().unwrap();
    std::io::Write::write_all(&mut child.stdin.take().unwrap(), lines.as_bytes()).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let tree = String::from_utf8(output.stdout).unwrap();
    git(repo, &["commit-tree", "-m", "crafted", tree.trim()])
        .trim()
        .to_owned()
}

#[test]
fn a_committed_path_that_leaves_the_tree_or_repeats_fails_with_invalid_path() {
    let (dir, store) = store();
    let repo = dir.path().join("repo");
    sample_repo(&repo);
    let blob = git(&repo, &["rev-parse", "HEAD:README.md"])
        .trim()
        .to_owned();
    let escape = crafted_commit(&repo, &format!("100644 blob {blob}\t..\n"));
    let error = archive_tree(&store, &Git::default(), &repo, &escape, &OPTS).unwrap_err();
    assert_eq!(error.code(), codes::INVALID_PATH);
    let twice = crafted_commit(
        &repo,
        &format!("100644 blob {blob}\ta\n100644 blob {blob}\ta\n"),
    );
    let error = archive_tree(&store, &Git::default(), &repo, &twice, &OPTS).unwrap_err();
    assert_eq!(error.code(), codes::INVALID_PATH);
    assert_eq!(error.details().unwrap()["path"], "a");
}

#[test]
fn a_blob_the_repository_lacks_is_git_failed() {
    let (dir, store) = store();
    let repo = dir.path().join("repo");
    sample_repo(&repo);
    let lost = crafted_commit(
        &repo,
        "100644 blob 1111111111111111111111111111111111111111\tlost.txt\n",
    );
    let error = archive_tree(&store, &Git::default(), &repo, &lost, &OPTS).unwrap_err();
    assert_eq!(error.code(), codes::GIT_FAILED);
    assert_eq!(error.details().unwrap()["command"], "ls-tree");
}

#[test]
fn export_attributes_do_not_change_what_imports() {
    let (dir, store) = store();
    let repo = dir.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    write(
        &repo,
        ".gitattributes",
        b"secret export-ignore\nstamp export-subst\n",
    );
    write(&repo, "secret", b"left out of archives\n");
    write(&repo, "stamp", b"$Format:%H$\n");
    write(&repo, "src/lib.rs", b"pub fn x() {}\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "attributes"]);
    // From a subdirectory too, the whole committed tree imports.
    let result = archive_tree(&store, &Git::default(), &repo.join("src"), "HEAD", &OPTS).unwrap();
    let found = contents(&store, &result.tree);
    assert_eq!(
        found.keys().collect::<Vec<_>>(),
        [".gitattributes", "secret", "src/lib.rs", "stamp"]
    );
    assert_eq!(found["stamp"].1, b"$Format:%H$\n");
    assert_eq!(found["secret"].1, b"left out of archives\n");
}

/// A stand-in for `git` that lists one five-byte file and answers `cat-file` with `reply`, a shell command.
fn fake_git(dir: &Path, name: &str, reply: &str) -> Git {
    let script = dir.join(name);
    let listing = "100644 blob 1111111111111111111111111111111111111111       5\\ta.txt\\0";
    fs::write(
        &script,
        format!("#!/bin/sh\nfor arg; do\ncase \"$arg\" in\nls-tree) printf '{listing}'; exit ;;\ncat-file) read oid; {reply}; exit ;;\nesac\ndone\n"),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    Git::new(script)
}

#[test]
fn a_broken_blob_stream_is_git_failed() {
    let (dir, store) = store();
    for (name, reply, status) in [
        ("short", r#"printf '%s blob 5\nab' "$oid""#, None),
        ("missing", r#"printf '%s missing\n' "$oid""#, None),
        ("unended", r#"printf '%s blob 5\nabcde' "$oid""#, None),
        ("dies", "exit 3", Some(3)),
    ] {
        let git = fake_git(dir.path(), name, reply);
        let error = archive_tree(&store, &git, dir.path(), "HEAD", &OPTS).unwrap_err();
        assert_eq!(error.code(), codes::GIT_FAILED, "{name}");
        let details = error.details().unwrap();
        assert_eq!(details["command"], "cat-file", "{name}");
        if let Some(status) = status {
            assert_eq!(details["status"], status);
        }
    }
}
