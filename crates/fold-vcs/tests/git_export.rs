#![cfg(test)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use fold_core::object::ObjectHash;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree, Tree, flatten};
use fold_core::{FoldError, RevisionId, Timestamp, codes};
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_export::{
    ExportBase, ExportRevision, ExportTarget, Signature, TargetState, check_target, create_target,
    export,
};
use fold_vcs::git_sync_map::{SYNC_MAP_FILE, SyncMap};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::write_initial_tree;

use crate::common::*;

const MAIN: &str = "refs/heads/main";

struct Fixture {
    dir: tempfile::TempDir,
    store: ObjectStore,
    git: Git,
}

impl Fixture {
    fn new() -> Self {
        let (dir, store) = store();
        Self {
            dir,
            store,
            git: Git::default(),
        }
    }

    fn fold_dir(&self) -> std::path::PathBuf {
        self.dir.path().join(".fold")
    }

    fn map(&self) -> SyncMap {
        SyncMap::load(&self.fold_dir()).unwrap()
    }

    fn tree(&self, files: &[(&str, EntryKind, &[u8])]) -> ObjectHash {
        let mut flat = FlatTree::new();
        for (path, kind, bytes) in files {
            let hash = blob(&self.store, bytes);
            flat.insert(path.as_bytes().to_vec(), FlatEntry { kind: *kind, hash });
        }
        write_initial_tree(&self.store, flat).unwrap()
    }

    fn export(
        &self,
        repo: &Path,
        base: Option<&ExportBase>,
        revisions: &[ExportRevision],
    ) -> Result<Vec<(RevisionId, CommitId)>, FoldError> {
        let target = ExportTarget {
            repo,
            refname: MAIN,
        };
        let mut map = self.map();
        export(
            &self.store,
            &self.git,
            &target,
            base,
            revisions,
            &mut map,
            &self.fold_dir(),
        )
    }

    /// Path to mode and content, read from a fold tree.
    fn fold_files(&self, tree: &ObjectHash) -> BTreeMap<Vec<u8>, (String, Vec<u8>)> {
        let load = |hash: &ObjectHash| -> Result<Tree, FoldError> {
            Ok(Tree::decode(&self.store.read(hash)?.payload)?)
        };
        flatten(tree, &load)
            .unwrap()
            .into_iter()
            .map(|(path, entry)| {
                let mode = match entry.kind {
                    EntryKind::File => "100644",
                    EntryKind::Executable => "100755",
                    EntryKind::Symlink => "120000",
                    EntryKind::Directory => unreachable!(),
                };
                let bytes = self.store.read(&entry.hash).unwrap().payload;
                (path, (mode.to_owned(), bytes))
            })
            .collect()
    }
}

fn revision(number: u32, tree: ObjectHash, message: &str) -> ExportRevision {
    let fold = Signature {
        name: "fold".to_owned(),
        email: "fold@localhost".to_owned(),
    };
    ExportRevision {
        number: RevisionId::from_number(number).unwrap(),
        tree,
        created_at: Timestamp::from_millis(1_700_000_000_000 + i64::from(number) * 1000).unwrap(),
        message: message.to_owned(),
        author: fold.clone(),
        committer: fold,
        merge: None,
    }
}

/// Path to mode and content, read from a Git commit with `git ls-tree -r`.
fn git_files(repo: &Path, commit: &str) -> BTreeMap<Vec<u8>, (String, Vec<u8>)> {
    let listing = git_command(repo)
        .args(["ls-tree", "-r", "-z", commit])
        .output()
        .unwrap();
    assert!(listing.status.success());
    listing
        .stdout
        .split(|&byte| byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let tab = record.iter().position(|&byte| byte == b'\t').unwrap();
            let head = std::str::from_utf8(&record[..tab]).unwrap();
            let fields: Vec<&str> = head.split(' ').collect();
            let content = git_command(repo)
                .args(["cat-file", "blob", fields[2]])
                .output()
                .unwrap()
                .stdout;
            (record[tab + 1..].to_vec(), (fields[0].to_owned(), content))
        })
        .collect()
}

fn first_parent_log(repo: &Path) -> Vec<String> {
    git(repo, &["log", "--first-parent", "--format=%H", "main"])
        .lines()
        .map(str::to_owned)
        .collect()
}

fn message(repo: &Path, commit: &str) -> String {
    git(repo, &["log", "-1", "--format=%B", commit])
}

/// Three revisions that add, change and delete files and swap a file for a directory and back.
fn three(fx: &Fixture) -> Vec<ExportRevision> {
    let t1 = fx.tree(&[
        ("README.md", EntryKind::File, b"one\n"),
        ("run.sh", EntryKind::Executable, b"#!/bin/sh\n"),
        ("link", EntryKind::Symlink, b"README.md"),
        ("odd\n\"name", EntryKind::File, b"quoted\n"),
        ("docs", EntryKind::File, b"a file first\n"),
    ]);
    let t2 = fx.tree(&[
        ("README.md", EntryKind::File, b"two\n"),
        ("run.sh", EntryKind::Executable, b"#!/bin/sh\n"),
        ("link", EntryKind::Symlink, b"run.sh"),
        ("docs/guide.md", EntryKind::File, b"now a directory\n"),
        ("empty", EntryKind::File, b""),
    ]);
    let t3 = fx.tree(&[
        ("README.md", EntryKind::Executable, b"two\n"),
        ("docs", EntryKind::File, b"a file again\n"),
        ("empty", EntryKind::File, b""),
    ]);
    vec![
        revision(1, t1, "fold r1\n\nRevision: r1\n"),
        revision(
            2,
            t2,
            "P-4: Database schema for reports\n\nRevision: r2\nTask: T-7\nIntegrated by: claude.w1\n",
        ),
        revision(3, t3, "fold r3\n\nRevision: r3\n"),
    ]
}

#[test]
fn three_revisions_become_three_commits_with_the_same_trees() {
    let fx = Fixture::new();
    let repo = fx.dir.path().join("export");
    let target = ExportTarget {
        repo: &repo,
        refname: MAIN,
    };
    assert_eq!(
        check_target(&fx.git, &target, None).unwrap(),
        TargetState::Missing
    );
    create_target(&fx.git, &target).unwrap();
    let revisions = three(&fx);
    let added = fx.export(&repo, None, &revisions).unwrap();
    assert_eq!(added.len(), 3);

    let log = first_parent_log(&repo);
    assert_eq!(log.len(), 3);
    let oldest_first: Vec<&String> = log.iter().rev().collect();
    for ((revision, (number, commit)), git_commit) in revisions.iter().zip(&added).zip(oldest_first)
    {
        assert_eq!(*number, revision.number);
        assert_eq!(commit.as_str(), git_commit);
        assert_eq!(git_files(&repo, git_commit), fx.fold_files(&revision.tree));
        assert_eq!(
            message(&repo, git_commit),
            format!("{}\n", revision.message)
        );
    }
    let modes: Vec<String> = git(&repo, &["ls-tree", "--format=%(objectmode)", &log[2]])
        .lines()
        .map(str::to_owned)
        .collect();
    for mode in ["100644", "100755", "120000"] {
        assert!(modes.iter().any(|found| found == mode), "{mode}");
    }
    let author = git(&repo, &["log", "-1", "--format=%an <%ae> %at %cn", &log[0]]);
    assert_eq!(author, "fold <fold@localhost> 1700000003 fold\n");

    let map = fx.map();
    assert_eq!(map.entries(), added.as_slice());
    let text = fs::read_to_string(fx.fold_dir().join(SYNC_MAP_FILE)).unwrap();
    assert_eq!(text.lines().count(), 3);
    assert!(text.starts_with(&format!("r1 {}\n", added[0].1)));
    assert!(!fs::read_dir(fx.fold_dir()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".marks")
    }));
}

#[test]
fn a_later_export_continues_the_chain_with_only_the_new_revisions() {
    let fx = Fixture::new();
    let repo = fx.dir.path().join("export");
    let target = ExportTarget {
        repo: &repo,
        refname: MAIN,
    };
    create_target(&fx.git, &target).unwrap();
    let revisions = three(&fx);
    let first = fx.export(&repo, None, &revisions).unwrap();
    let last = first.last().unwrap().clone();
    assert_eq!(
        check_target(&fx.git, &target, Some(&last)).unwrap(),
        TargetState::Ready
    );
    assert_eq!(fx.export(&repo, None, &[]).unwrap(), []);

    let t4 = fx.tree(&[("README.md", EntryKind::File, b"four\n")]);
    let t5 = fx.tree(&[
        ("README.md", EntryKind::File, b"four\n"),
        ("new.txt", EntryKind::File, b"five\n"),
    ]);
    let base = ExportBase {
        commit: last.1.clone(),
        tree: revisions[2].tree,
    };
    let more = [revision(4, t4, "fold r4\n"), revision(5, t5, "fold r5\n")];
    let added = fx.export(&repo, Some(&base), &more).unwrap();
    let log = first_parent_log(&repo);
    assert_eq!(log.len(), 5);
    assert_eq!(log[0], added[1].1.as_str());
    assert_eq!(log[1], added[0].1.as_str());
    assert_eq!(log[2], last.1.as_str());
    let parents = git(&repo, &["log", "-1", "--format=%P", &log[1]]);
    assert_eq!(parents.trim(), last.1.as_str());
    assert_eq!(git_files(&repo, &log[0]), fx.fold_files(&t5));
    let changed = git(&repo, &["show", "--name-status", "--format=", &log[0]]);
    assert_eq!(changed, "A\tnew.txt\n");
    assert_eq!(fx.map().entries().len(), 5);
}

#[test]
fn a_moved_branch_or_another_repository_is_diverged() {
    let fx = Fixture::new();
    let repo = fx.dir.path().join("export");
    let target = ExportTarget {
        repo: &repo,
        refname: MAIN,
    };
    create_target(&fx.git, &target).unwrap();
    let added = fx.export(&repo, None, &three(&fx)).unwrap();
    let last = added.last().unwrap().clone();

    // A missing or empty path holds no commit to continue.
    for path in [fx.dir.path().join("absent"), fx.dir.path().join("empty")] {
        if path.ends_with("empty") {
            fs::create_dir(&path).unwrap();
        }
        let elsewhere = ExportTarget {
            repo: &path,
            refname: MAIN,
        };
        let error = check_target(&fx.git, &elsewhere, Some(&last)).unwrap_err();
        assert_eq!(error.code(), codes::EXPORT_DIVERGED);
        assert_eq!(error.details().unwrap()["revision"], "r3");
    }
    assert!(!fx.dir.path().join("absent").exists());

    // Another repository that lacks the commit.
    let other = fx.dir.path().join("other");
    fs::create_dir(&other).unwrap();
    git(&other, &["init", "--quiet", "--initial-branch=main"]);
    fs::write(other.join("x"), "x").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "--quiet", "-m", "x"]);
    let error = check_target(
        &fx.git,
        &ExportTarget {
            repo: &other,
            refname: MAIN,
        },
        Some(&last),
    )
    .unwrap_err();
    assert_eq!(error.code(), codes::EXPORT_DIVERGED);
    // A repository whose branch exists although nothing was exported yet.
    let error = check_target(
        &fx.git,
        &ExportTarget {
            repo: &other,
            refname: MAIN,
        },
        None,
    )
    .unwrap_err();
    assert_eq!(error.code(), codes::EXPORT_DIVERGED);

    // A commit made in the target after the export.
    let outside = git(
        &repo,
        &[
            "commit-tree",
            "-p",
            "main",
            "-m",
            "outside fold",
            "main^{tree}",
        ],
    );
    git(&repo, &["update-ref", "refs/heads/main", outside.trim()]);
    let error = check_target(&fx.git, &target, Some(&last)).unwrap_err();
    assert_eq!(error.code(), codes::EXPORT_DIVERGED);
    assert_eq!(error.details().unwrap()["commit"], last.1.as_str());
}

#[test]
fn a_non_empty_directory_that_is_not_a_repository_is_refused() {
    let fx = Fixture::new();
    let plain = fx.dir.path().join("plain");
    fs::create_dir(&plain).unwrap();
    fs::write(plain.join("file"), "x").unwrap();
    let target = ExportTarget {
        repo: &plain,
        refname: MAIN,
    };
    let error = check_target(&fx.git, &target, None).unwrap_err();
    assert_eq!(error.code(), codes::INVALID_REQUEST);

    // A directory inside a repository's working tree is not that repository's top.
    let repo = fx.dir.path().join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "--quiet"]);
    fs::create_dir(repo.join("sub")).unwrap();
    fs::write(repo.join("sub/file"), "x").unwrap();
    let nested = repo.join("sub");
    let target = ExportTarget {
        repo: &nested,
        refname: MAIN,
    };
    assert_eq!(
        check_target(&fx.git, &target, None).unwrap_err().code(),
        codes::INVALID_REQUEST
    );

    let file = plain.join("file");
    let target = ExportTarget {
        repo: &file,
        refname: MAIN,
    };
    assert_eq!(
        check_target(&fx.git, &target, None).unwrap_err().code(),
        codes::INVALID_REQUEST
    );
    let tag = ExportTarget {
        repo: &fx.dir.path().join("new"),
        refname: "refs/tags/v1",
    };
    assert_eq!(
        create_target(&fx.git, &tag).unwrap_err().code(),
        codes::INVALID_REQUEST
    );
}

#[test]
fn a_new_target_is_bare_and_a_checked_out_branch_is_refused() {
    let fx = Fixture::new();
    let repo = fx.dir.path().join("export");
    let target = ExportTarget {
        repo: &repo,
        refname: MAIN,
    };
    create_target(&fx.git, &target).unwrap();
    assert_eq!(git(&repo, &["rev-parse", "--is-bare-repository"]), "true\n");
    assert_eq!(git(&repo, &["rev-parse", "--show-object-format"]), "sha1\n");

    let work = fx.dir.path().join("work");
    fs::create_dir(&work).unwrap();
    git(&work, &["init", "--quiet", "--initial-branch=main"]);
    let error = fx.export(&work, None, &three(&fx)).unwrap_err();
    assert_eq!(error.code(), codes::INVALID_REQUEST);
    assert_eq!(error.details().unwrap()["ref"], MAIN);
    assert!(fx.map().entries().is_empty());
    git(&work, &["checkout", "--quiet", "-b", "elsewhere"]);
    let added = fx.export(&work, None, &three(&fx)).unwrap();
    assert_eq!(added.len(), 3);
    assert_eq!(first_parent_log(&work).len(), 3);
}

#[test]
fn a_sha256_repository_is_refused() {
    let fx = Fixture::new();
    let repo = fx.dir.path().join("sha256");
    fs::create_dir(&repo).unwrap();
    git(
        &repo,
        &["init", "--quiet", "--bare", "--object-format=sha256"],
    );
    let target = ExportTarget {
        repo: &repo,
        refname: MAIN,
    };
    let error = check_target(&fx.git, &target, None).unwrap_err();
    assert_eq!(error.code(), codes::INVALID_REQUEST);
    assert_eq!(error.details().unwrap()["object_format"], "sha256");
}

#[test]
fn a_bare_repository_is_a_target() {
    let fx = Fixture::new();
    let bare = fx.dir.path().join("bare.git");
    fs::create_dir(&bare).unwrap();
    git(&bare, &["init", "--quiet", "--bare"]);
    let target = ExportTarget {
        repo: &bare,
        refname: MAIN,
    };
    assert_eq!(
        check_target(&fx.git, &target, None).unwrap(),
        TargetState::Ready
    );
    fx.export(&bare, None, &three(&fx)[..1]).unwrap();
    assert_eq!(first_parent_log(&bare).len(), 1);
}

#[test]
fn a_failed_fast_import_leaves_the_map_and_the_branch_unchanged() {
    let fx = Fixture::new();
    let repo = fx.dir.path().join("export");
    create_target(
        &fx.git,
        &ExportTarget {
            repo: &repo,
            refname: MAIN,
        },
    )
    .unwrap();
    let revisions = three(&fx);
    // A held ref lock makes fast-import fail when it moves the branch, after it wrote the commits.
    let lock = repo.join("refs/heads/main.lock");
    fs::write(&lock, "").unwrap();
    let error = fx.export(&repo, None, &revisions).unwrap_err();
    assert_eq!(error.code(), codes::GIT_FAILED);
    assert_eq!(error.details().unwrap()["command"], "fast-import");
    assert!(fx.map().entries().is_empty());
    assert!(!fx.fold_dir().join(SYNC_MAP_FILE).exists());
    fs::remove_file(&lock).unwrap();
    assert!(
        !git_command(&repo)
            .args(["rev-parse", "--verify", "--quiet", "main"])
            .status()
            .unwrap()
            .success()
    );

    // A blob missing from the object store stops the stream before fast-import sees its end.
    let gone = blob(&fx.store, b"about to vanish\n");
    let tree = fx.tree(&[("gone.txt", EntryKind::File, b"about to vanish\n")]);
    fs::remove_file(fx.store.path(&gone)).unwrap();
    let error = fx
        .export(&repo, None, &[revision(1, tree, "fold r1\n")])
        .unwrap_err();
    assert_eq!(error.code(), codes::OBJECT_MISSING);
    assert!(fx.map().entries().is_empty());

    // The retry exports the same revisions.
    let added = fx.export(&repo, None, &revisions).unwrap();
    assert_eq!(added.len(), 3);
    assert_eq!(first_parent_log(&repo).len(), 3);
}

#[test]
fn a_missing_git_program_is_git_not_found() {
    let fx = Fixture::new();
    let missing = Git::new(fx.dir.path().join("bin/git"));
    let repo = fx.dir.path().join("export");
    let target = ExportTarget {
        repo: &repo,
        refname: MAIN,
    };
    let mut map = fx.map();
    let error = export(
        &fx.store,
        &missing,
        &target,
        None,
        &three(&fx),
        &mut map,
        &fx.fold_dir(),
    )
    .unwrap_err();
    assert_eq!(error.code(), codes::GIT_NOT_FOUND);
    assert_eq!(
        create_target(&missing, &target).unwrap_err().code(),
        codes::GIT_NOT_FOUND
    );
}
