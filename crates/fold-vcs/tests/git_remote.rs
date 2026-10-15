#![cfg(test)]

pub mod common;

use std::path::{Path, PathBuf};

use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{RevisionId, Timestamp, codes};
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_export::{ExportBase, ExportRevision, ExportTarget, Signature, write_commits};
use fold_vcs::git_remote::{
    MIRROR_DIR, fetch_branch, fold_ref, is_ancestor, prepare_mirror, push_branch, remote_ref,
    set_ref,
};
use fold_vcs::git_sync_map::SYNC_MAP_FILE;
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::write_initial_tree;

use common::*;

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

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn fold_dir(&self) -> PathBuf {
        self.path(".fold")
    }

    fn mirror(&self) -> PathBuf {
        self.fold_dir().join(MIRROR_DIR)
    }

    /// An empty bare repository standing in for the remote.
    fn remote(&self, name: &str) -> PathBuf {
        let remote = self.path(name);
        std::fs::create_dir(&remote).unwrap();
        git(
            &remote,
            &["init", "--quiet", "--bare", "--initial-branch=main"],
        );
        remote
    }

    fn revision(&self, number: u32, text: &[u8]) -> ExportRevision {
        let mut flat = FlatTree::new();
        flat.insert(
            b"a.txt".to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash: blob(&self.store, text),
            },
        );
        let fold = Signature {
            name: "fold".to_owned(),
            email: "fold@localhost".to_owned(),
        };
        ExportRevision {
            number: RevisionId::from_number(number).unwrap(),
            tree: write_initial_tree(&self.store, flat).unwrap(),
            created_at: Timestamp::from_millis(1_700_000_000_000 + i64::from(number) * 1000)
                .unwrap(),
            message: format!("fold r{number}\n"),
            author: fold.clone(),
            committer: fold,
        }
    }

    fn write(
        &self,
        base: Option<&ExportBase>,
        revisions: &[ExportRevision],
    ) -> Vec<(RevisionId, CommitId)> {
        let mirror = self.mirror();
        let refname = fold_ref("main");
        let target = ExportTarget {
            repo: &mirror,
            refname: &refname,
        };
        write_commits(
            &self.store,
            &self.git,
            &target,
            base,
            revisions,
            &self.fold_dir(),
        )
        .unwrap()
    }
}

fn head(repo: &Path) -> String {
    git(repo, &["rev-parse", "refs/heads/main"])
        .trim()
        .to_owned()
}

#[test]
fn the_mirror_is_bare_and_follows_the_last_remote_it_was_given() {
    let fixture = Fixture::new();
    let mirror = fixture.mirror();
    prepare_mirror(&fixture.git, &mirror, "/srv/one.git").unwrap();
    assert_eq!(
        git(&mirror, &["rev-parse", "--is-bare-repository"]),
        "true\n"
    );
    assert_eq!(
        git(&mirror, &["remote", "get-url", "origin"]),
        "/srv/one.git\n"
    );
    prepare_mirror(&fixture.git, &mirror, "/srv/two.git").unwrap();
    assert_eq!(
        git(&mirror, &["remote", "get-url", "origin"]),
        "/srv/two.git\n"
    );
    assert_eq!(git(&mirror, &["remote"]), "origin\n");
    let error = prepare_mirror(&fixture.git, &mirror, "--upload-pack=x").unwrap_err();
    assert_eq!(error.code(), codes::INVALID_REQUEST);
    assert!(!error.message().contains("upload-pack"));
}

#[test]
fn commits_go_to_an_empty_remote_and_later_ones_on_top() {
    let fixture = Fixture::new();
    let remote = fixture.remote("remote.git");
    let mirror = fixture.mirror();
    prepare_mirror(&fixture.git, &mirror, remote.to_str().unwrap()).unwrap();
    assert_eq!(fetch_branch(&fixture.git, &mirror, "main").unwrap(), None);

    let first = [fixture.revision(1, b"one\n"), fixture.revision(2, b"two\n")];
    let written = fixture.write(None, &first);
    assert_eq!(written.len(), 2);
    // Writing the same revisions again gives the same commits.
    assert_eq!(fixture.write(None, &first), written);
    assert!(!fixture.fold_dir().join(SYNC_MAP_FILE).exists());
    push_branch(&fixture.git, &mirror, "main").unwrap();
    assert_eq!(head(&remote), written[1].1.as_str());

    let tip = fetch_branch(&fixture.git, &mirror, "main")
        .unwrap()
        .unwrap();
    assert_eq!(tip, written[1].1);
    assert_eq!(
        git(&mirror, &["rev-parse", &remote_ref("main")]).trim(),
        tip.as_str()
    );
    let base = ExportBase {
        commit: tip.clone(),
        tree: first[1].tree,
    };
    let later = fixture.write(Some(&base), &[fixture.revision(3, b"three\n")]);
    assert!(is_ancestor(&fixture.git, &mirror, &tip, &later[0].1).unwrap());
    assert!(!is_ancestor(&fixture.git, &mirror, &later[0].1, &tip).unwrap());
    push_branch(&fixture.git, &mirror, "main").unwrap();
    assert_eq!(head(&remote), later[0].1.as_str());
    let log = git(&remote, &["log", "--format=%s", "main"]);
    assert_eq!(log, "fold r3\nfold r2\nfold r1\n");
}

#[test]
fn a_remote_that_moved_rejects_the_push_as_remote_ahead() {
    let fixture = Fixture::new();
    let remote = fixture.remote("remote.git");
    let mirror = fixture.mirror();
    prepare_mirror(&fixture.git, &mirror, remote.to_str().unwrap()).unwrap();
    let first = fixture.write(None, &[fixture.revision(1, b"one\n")]);
    push_branch(&fixture.git, &mirror, "main").unwrap();

    let clone = fixture.path("clone");
    git(
        fixture.dir.path(),
        &["clone", "--quiet", remote.to_str().unwrap(), "clone"],
    );
    write(&clone, "b.txt", b"other\n");
    git(&clone, &["add", "b.txt"]);
    git(&clone, &["commit", "--quiet", "-m", "other"]);
    git(&clone, &["push", "--quiet", "origin", "main"]);
    let theirs = head(&remote);

    let base = ExportBase {
        commit: first[0].1.clone(),
        tree: fixture.revision(1, b"one\n").tree,
    };
    fixture.write(Some(&base), &[fixture.revision(2, b"two\n")]);
    let error = push_branch(&fixture.git, &mirror, "main").unwrap_err();
    assert_eq!(error.code(), codes::REMOTE_AHEAD);
    assert_eq!(error.details().unwrap()["branch"], "main");
    assert_eq!(head(&remote), theirs);

    let tip = fetch_branch(&fixture.git, &mirror, "main")
        .unwrap()
        .unwrap();
    assert_eq!(tip.as_str(), theirs);
    set_ref(&fixture.git, &mirror, &fold_ref("main"), Some(&first[0].1)).unwrap();
    assert_eq!(
        git(&mirror, &["rev-parse", &fold_ref("main")]).trim(),
        first[0].1.as_str()
    );
    set_ref(&fixture.git, &mirror, &fold_ref("main"), None).unwrap();
    assert!(
        git_command(&mirror)
            .args(["rev-parse", "--verify", "--quiet", &fold_ref("main")])
            .output()
            .unwrap()
            .stdout
            .is_empty()
    );
}

#[test]
fn a_deleted_remote_branch_counts_as_empty_and_clears_the_tracking_ref() {
    let fixture = Fixture::new();
    let remote = fixture.remote("remote.git");
    let mirror = fixture.mirror();
    prepare_mirror(&fixture.git, &mirror, remote.to_str().unwrap()).unwrap();
    fixture.write(None, &[fixture.revision(1, b"one\n")]);
    push_branch(&fixture.git, &mirror, "main").unwrap();
    assert!(
        fetch_branch(&fixture.git, &mirror, "main")
            .unwrap()
            .is_some()
    );
    git(&remote, &["update-ref", "-d", "refs/heads/main"]);
    assert_eq!(fetch_branch(&fixture.git, &mirror, "main").unwrap(), None);
    assert_eq!(
        fixture
            .git
            .ref_commit(&mirror, &remote_ref("main"))
            .unwrap(),
        None
    );
}

#[test]
fn an_unreachable_remote_is_git_failed_and_a_missing_git_is_git_not_found() {
    let fixture = Fixture::new();
    let mirror = fixture.mirror();
    let absent = fixture.path("absent.git");
    prepare_mirror(&fixture.git, &mirror, absent.to_str().unwrap()).unwrap();
    let error = fetch_branch(&fixture.git, &mirror, "main").unwrap_err();
    assert_eq!(error.code(), codes::GIT_FAILED);
    assert_eq!(error.details().unwrap()["command"], "ls-remote");
    fixture.write(None, &[fixture.revision(1, b"one\n")]);
    let error = push_branch(&fixture.git, &mirror, "main").unwrap_err();
    assert_eq!(error.code(), codes::GIT_FAILED);
    assert_eq!(error.details().unwrap()["command"], "push");
    assert_eq!(
        fetch_branch(&fixture.git, &mirror, "a..b")
            .unwrap_err()
            .code(),
        codes::INVALID_REQUEST
    );
    assert_eq!(
        push_branch(&fixture.git, &mirror, "-x").unwrap_err().code(),
        codes::INVALID_REQUEST
    );

    let missing = Git::new(fixture.path("bin/git"));
    let error = prepare_mirror(&missing, &fixture.path("other"), "/srv/x.git").unwrap_err();
    assert_eq!(error.code(), codes::GIT_NOT_FOUND);
    for error in [
        fetch_branch(&missing, &mirror, "main").unwrap_err(),
        push_branch(&missing, &mirror, "main").unwrap_err(),
    ] {
        assert_eq!(error.code(), codes::GIT_NOT_FOUND);
    }
    let commit: CommitId = "0".repeat(40).parse().unwrap();
    assert_eq!(
        is_ancestor(&fixture.git, &mirror, &commit, &commit)
            .unwrap_err()
            .code(),
        codes::GIT_FAILED
    );
}

#[test]
fn a_remote_rewritten_to_the_ext_transport_runs_no_command() {
    let fixture = Fixture::new();
    let mirror = fixture.mirror();
    let alias = fixture.path("alias.git");
    assert_eq!(
        prepare_mirror(&fixture.git, &mirror, "ext::sh -c true")
            .unwrap_err()
            .code(),
        codes::INVALID_REQUEST
    );
    prepare_mirror(&fixture.git, &mirror, alias.to_str().unwrap()).unwrap();
    // A config file that allows `ext` and rewrites the remote to it, as a hostile or careless edit would.
    let marker = fixture.path("ran");
    let rewrite = format!(
        "url.ext::sh -c touch% {}.insteadOf",
        marker.to_str().unwrap()
    );
    git(&mirror, &["config", "protocol.ext.allow", "always"]);
    git(&mirror, &["config", &rewrite, alias.to_str().unwrap()]);
    fixture.write(None, &[fixture.revision(1, b"one\n")]);
    for error in [
        fetch_branch(&fixture.git, &mirror, "main").unwrap_err(),
        push_branch(&fixture.git, &mirror, "main").unwrap_err(),
    ] {
        assert_eq!(error.code(), codes::GIT_FAILED);
    }
    assert!(!marker.exists());
}
