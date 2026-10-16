#![cfg(test)]

use std::path::{Path, PathBuf};

use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{RevisionId, Timestamp, codes};
use fold_vcs::git::{CommitId, Git};
use fold_vcs::git_export::{ExportBase, ExportRevision, ExportTarget, Signature, write_commits};
use fold_vcs::git_remote::{
    branch_ref, checked_out_branch, create_local_remote, default_branch, exclude_fold_dir,
    fetch_branch, is_ancestor, merge_base, prepare_mirror, push_branch, remote_ref, remove_remote,
    set_ref,
};
use fold_vcs::git_sync_map::SYNC_MAP_FILE;
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::write_initial_tree;

use crate::common::*;

struct Fixture {
    dir: tempfile::TempDir,
    store: ObjectStore,
    git: Git,
}

impl Fixture {
    fn new() -> Self {
        let (dir, store) = store();
        let fixture = Self {
            dir,
            store,
            git: Git::default(),
        };
        let mirror = fixture.mirror();
        std::fs::create_dir_all(&mirror).unwrap();
        git(
            &mirror,
            &["init", "--quiet", "--bare", "--initial-branch=main"],
        );
        fixture
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn fold_dir(&self) -> PathBuf {
        self.path(".fold")
    }

    /// The project's repository; bare here, because these tests write `refs/heads/main` with nothing checked out.
    fn mirror(&self) -> PathBuf {
        self.path("project.git")
    }

    /// The commit `refs/heads/main` of the project's repository points to.
    fn main_commit(&self) -> CommitId {
        head(&self.mirror()).parse().unwrap()
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
            merge: None,
        }
    }

    fn write(
        &self,
        base: Option<&ExportBase>,
        revisions: &[ExportRevision],
    ) -> Vec<(RevisionId, CommitId)> {
        let mirror = self.mirror();
        let refname = branch_ref("main");
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
fn a_remote_follows_the_last_url_it_was_given() {
    let fixture = Fixture::new();
    let mirror = fixture.mirror();
    prepare_mirror(&fixture.git, &mirror, "origin", "/srv/one.git").unwrap();
    assert_eq!(
        git(&mirror, &["remote", "get-url", "origin"]),
        "/srv/one.git\n"
    );
    prepare_mirror(&fixture.git, &mirror, "origin", "/srv/two.git").unwrap();
    assert_eq!(
        git(&mirror, &["remote", "get-url", "origin"]),
        "/srv/two.git\n"
    );
    assert_eq!(git(&mirror, &["remote"]), "origin\n");
    let error = prepare_mirror(&fixture.git, &mirror, "origin", "--upload-pack=x").unwrap_err();
    assert_eq!(error.code(), codes::INVALID_REQUEST);
    assert!(!error.message().contains("upload-pack"));
}

#[test]
fn commits_go_to_an_empty_remote_and_later_ones_on_top() {
    let fixture = Fixture::new();
    let remote = fixture.remote("remote.git");
    let mirror = fixture.mirror();
    prepare_mirror(&fixture.git, &mirror, "origin", remote.to_str().unwrap()).unwrap();
    assert_eq!(
        fetch_branch(&fixture.git, &mirror, "origin", "main").unwrap(),
        None
    );

    let first = [fixture.revision(1, b"one\n"), fixture.revision(2, b"two\n")];
    let written = fixture.write(None, &first);
    assert_eq!(written.len(), 2);
    // Writing the same revisions again gives the same commits.
    assert_eq!(fixture.write(None, &first), written);
    assert!(!fixture.fold_dir().join(SYNC_MAP_FILE).exists());
    push_branch(
        &fixture.git,
        &mirror,
        "origin",
        &fixture.main_commit(),
        "main",
    )
    .unwrap();
    assert_eq!(head(&remote), written[1].1.as_str());

    let tip = fetch_branch(&fixture.git, &mirror, "origin", "main")
        .unwrap()
        .unwrap();
    assert_eq!(tip, written[1].1);
    assert_eq!(
        git(&mirror, &["rev-parse", &remote_ref("origin", "main")]).trim(),
        tip.as_str()
    );
    let base = ExportBase {
        commit: tip.clone(),
        tree: first[1].tree,
    };
    let later = fixture.write(Some(&base), &[fixture.revision(3, b"three\n")]);
    assert!(is_ancestor(&fixture.git, &mirror, &tip, &later[0].1).unwrap());
    assert!(!is_ancestor(&fixture.git, &mirror, &later[0].1, &tip).unwrap());
    push_branch(
        &fixture.git,
        &mirror,
        "origin",
        &fixture.main_commit(),
        "main",
    )
    .unwrap();
    assert_eq!(head(&remote), later[0].1.as_str());
    let log = git(&remote, &["log", "--format=%s", "main"]);
    assert_eq!(log, "fold r3\nfold r2\nfold r1\n");
}

#[test]
fn a_remote_that_moved_rejects_the_push_as_remote_ahead() {
    let fixture = Fixture::new();
    let remote = fixture.remote("remote.git");
    let mirror = fixture.mirror();
    prepare_mirror(&fixture.git, &mirror, "origin", remote.to_str().unwrap()).unwrap();
    let first = fixture.write(None, &[fixture.revision(1, b"one\n")]);
    push_branch(
        &fixture.git,
        &mirror,
        "origin",
        &fixture.main_commit(),
        "main",
    )
    .unwrap();

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
    let error = push_branch(
        &fixture.git,
        &mirror,
        "origin",
        &fixture.main_commit(),
        "main",
    )
    .unwrap_err();
    assert_eq!(error.code(), codes::REMOTE_AHEAD);
    assert_eq!(error.details().unwrap()["branch"], "main");
    assert_eq!(head(&remote), theirs);

    let tip = fetch_branch(&fixture.git, &mirror, "origin", "main")
        .unwrap()
        .unwrap();
    assert_eq!(tip.as_str(), theirs);
    set_ref(
        &fixture.git,
        &mirror,
        &branch_ref("main"),
        Some(&first[0].1),
    )
    .unwrap();
    assert_eq!(
        git(&mirror, &["rev-parse", &branch_ref("main")]).trim(),
        first[0].1.as_str()
    );
    set_ref(&fixture.git, &mirror, &branch_ref("main"), None).unwrap();
    assert!(
        git_command(&mirror)
            .args(["rev-parse", "--verify", "--quiet", &branch_ref("main")])
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
    prepare_mirror(&fixture.git, &mirror, "origin", remote.to_str().unwrap()).unwrap();
    fixture.write(None, &[fixture.revision(1, b"one\n")]);
    push_branch(
        &fixture.git,
        &mirror,
        "origin",
        &fixture.main_commit(),
        "main",
    )
    .unwrap();
    assert!(
        fetch_branch(&fixture.git, &mirror, "origin", "main")
            .unwrap()
            .is_some()
    );
    git(&remote, &["update-ref", "-d", "refs/heads/main"]);
    assert_eq!(
        fetch_branch(&fixture.git, &mirror, "origin", "main").unwrap(),
        None
    );
    assert_eq!(
        fixture
            .git
            .ref_commit(&mirror, &remote_ref("origin", "main"))
            .unwrap(),
        None
    );
}

#[test]
fn an_unreachable_remote_is_git_failed_and_a_missing_git_is_git_not_found() {
    let fixture = Fixture::new();
    let mirror = fixture.mirror();
    let absent = fixture.path("absent.git");
    prepare_mirror(&fixture.git, &mirror, "origin", absent.to_str().unwrap()).unwrap();
    let error = fetch_branch(&fixture.git, &mirror, "origin", "main").unwrap_err();
    assert_eq!(error.code(), codes::GIT_FAILED);
    assert_eq!(error.details().unwrap()["command"], "ls-remote");
    fixture.write(None, &[fixture.revision(1, b"one\n")]);
    let error = push_branch(
        &fixture.git,
        &mirror,
        "origin",
        &fixture.main_commit(),
        "main",
    )
    .unwrap_err();
    assert_eq!(error.code(), codes::GIT_FAILED);
    assert_eq!(error.details().unwrap()["command"], "push");
    assert_eq!(
        fetch_branch(&fixture.git, &mirror, "origin", "a..b")
            .unwrap_err()
            .code(),
        codes::INVALID_REQUEST
    );
    assert_eq!(
        push_branch(
            &fixture.git,
            &mirror,
            "origin",
            &fixture.main_commit(),
            "-x"
        )
        .unwrap_err()
        .code(),
        codes::INVALID_REQUEST
    );

    let missing = Git::new(fixture.path("bin/git"));
    let error =
        prepare_mirror(&missing, &fixture.path("other"), "origin", "/srv/x.git").unwrap_err();
    assert_eq!(error.code(), codes::GIT_NOT_FOUND);
    for error in [
        fetch_branch(&missing, &mirror, "origin", "main").unwrap_err(),
        push_branch(&missing, &mirror, "origin", &fixture.main_commit(), "main").unwrap_err(),
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
        prepare_mirror(&fixture.git, &mirror, "origin", "ext::sh -c true")
            .unwrap_err()
            .code(),
        codes::INVALID_REQUEST
    );
    prepare_mirror(&fixture.git, &mirror, "origin", alias.to_str().unwrap()).unwrap();
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
        fetch_branch(&fixture.git, &mirror, "origin", "main").unwrap_err(),
        push_branch(
            &fixture.git,
            &mirror,
            "origin",
            &fixture.main_commit(),
            "main",
        )
        .unwrap_err(),
    ] {
        assert_eq!(error.code(), codes::GIT_FAILED);
    }
    assert!(!marker.exists());
}

#[test]
fn a_pulled_commit_is_the_second_parent_and_the_remote_takes_the_merge_without_force() {
    let fixture = Fixture::new();
    let remote = fixture.remote("remote.git");
    let mirror = fixture.mirror();
    prepare_mirror(&fixture.git, &mirror, "origin", remote.to_str().unwrap()).unwrap();
    let first = fixture.write(None, &[fixture.revision(1, b"one\n")]);
    push_branch(
        &fixture.git,
        &mirror,
        "origin",
        &fixture.main_commit(),
        "main",
    )
    .unwrap();
    git(
        fixture.dir.path(),
        &["clone", "--quiet", remote.to_str().unwrap(), "clone"],
    );
    let clone = fixture.path("clone");
    write(&clone, "b.txt", b"theirs\n");
    git(&clone, &["add", "b.txt"]);
    git(&clone, &["commit", "--quiet", "-m", "theirs"]);
    git(&clone, &["push", "--quiet", "origin", "main"]);
    let theirs = fetch_branch(&fixture.git, &mirror, "origin", "main")
        .unwrap()
        .unwrap();

    let base = ExportBase {
        commit: first[0].1.clone(),
        tree: fixture.revision(1, b"one\n").tree,
    };
    let merged = ExportRevision {
        merge: Some(theirs.clone()),
        ..fixture.revision(2, b"two\n")
    };
    let written = fixture.write(Some(&base), &[merged]);
    push_branch(
        &fixture.git,
        &mirror,
        "origin",
        &fixture.main_commit(),
        "main",
    )
    .unwrap();
    assert_eq!(head(&remote), written[0].1.as_str());
    let parents = git(&remote, &["log", "-1", "--format=%P", "main"]);
    assert_eq!(
        parents.trim(),
        format!("{} {}", first[0].1.as_str(), theirs.as_str())
    );

    // The merge and the pulled commit share the pulled commit; a history with no commit in common shares none.
    assert_eq!(
        merge_base(&fixture.git, &mirror, &written[0].1, &theirs).unwrap(),
        Some(theirs.clone())
    );
    let unrelated = fixture.path("unrelated");
    std::fs::create_dir(&unrelated).unwrap();
    git(&unrelated, &["init", "--quiet", "--initial-branch=main"]);
    write(&unrelated, "c.txt", b"elsewhere\n");
    git(&unrelated, &["add", "c.txt"]);
    git(&unrelated, &["commit", "--quiet", "-m", "elsewhere"]);
    let elsewhere: CommitId = git(&unrelated, &["rev-parse", "HEAD"])
        .trim()
        .parse()
        .unwrap();
    git(
        &mirror,
        &[
            "fetch",
            "--quiet",
            unrelated.to_str().unwrap(),
            "+main:refs/test/unrelated",
        ],
    );
    assert_eq!(
        merge_base(&fixture.git, &mirror, &theirs, &elsewhere).unwrap(),
        None
    );
    let missing: CommitId = "0".repeat(40).parse().unwrap();
    let error = merge_base(&fixture.git, &mirror, &theirs, &missing).unwrap_err();
    assert_eq!(error.code(), codes::GIT_FAILED);
}

#[test]
fn remotes_are_named_and_removed_and_a_new_local_path_becomes_a_repository() {
    let fixture = Fixture::new();
    let mirror = fixture.mirror();
    let remote = fixture.path("remote.git");
    // A missing local path is made a bare repository on the branch; an existing one is left alone.
    create_local_remote(&fixture.git, remote.to_str().unwrap(), "trunk").unwrap();
    assert_eq!(
        git(&remote, &["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/trunk"
    );
    create_local_remote(&fixture.git, remote.to_str().unwrap(), "main").unwrap();
    assert_eq!(
        git(&remote, &["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/trunk"
    );
    create_local_remote(&fixture.git, "relative/path", "main").unwrap();
    create_local_remote(&fixture.git, "https://example.com/a.git", "main").unwrap();
    assert!(!std::path::Path::new("relative/path").exists());

    // An empty repository has no HEAD branch yet; one with a commit names it.
    assert_eq!(
        default_branch(&fixture.git, &mirror, remote.to_str().unwrap()).unwrap(),
        None
    );
    prepare_mirror(&fixture.git, &mirror, "backup", remote.to_str().unwrap()).unwrap();
    let written = fixture.write(None, &[fixture.revision(1, b"one\n")]);
    push_branch(
        &fixture.git,
        &mirror,
        "backup",
        &fixture.main_commit(),
        "trunk",
    )
    .unwrap();
    assert_eq!(
        git(&remote, &["rev-parse", "trunk"]).trim(),
        written[0].1.as_str()
    );
    assert_eq!(
        default_branch(&fixture.git, &mirror, remote.to_str().unwrap()).unwrap(),
        Some("trunk".to_owned())
    );
    assert_eq!(
        fetch_branch(&fixture.git, &mirror, "backup", "trunk").unwrap(),
        Some(written[0].1.clone())
    );
    assert_eq!(
        default_branch(
            &fixture.git,
            &mirror,
            &fixture.path("absent").to_string_lossy()
        )
        .unwrap_err()
        .code(),
        codes::GIT_FAILED
    );

    remove_remote(&fixture.git, &mirror, "backup").unwrap();
    assert!(!git(&mirror, &["remote"]).contains("backup"));
    assert!(!git(&mirror, &["for-each-ref"]).contains("refs/remotes/backup"));
    remove_remote(&fixture.git, &mirror, "backup").unwrap();
    remove_remote(&fixture.git, &fixture.path("no-mirror"), "backup").unwrap();
    assert_eq!(
        remove_remote(&fixture.git, &mirror, "-x")
            .unwrap_err()
            .code(),
        codes::INVALID_REQUEST
    );
}

#[test]
fn git_commands_that_talk_to_a_remote_stop_at_the_deadline() {
    let fixture = Fixture::new();
    let mirror = fixture.mirror();
    // The listener accepts and never answers.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "git://127.0.0.1:{}/x.git",
        listener.local_addr().unwrap().port()
    );
    let git = Git::default().with_network_timeout(std::time::Duration::from_millis(500));
    prepare_mirror(&git, &mirror, "origin", &url).unwrap();
    let started = std::time::Instant::now();
    let error = fetch_branch(&git, &mirror, "origin", "main").unwrap_err();
    assert_eq!(error.code(), codes::GIT_FAILED);
    assert_eq!(error.details().unwrap()["timed_out"], true);
    assert!(!error.to_string().contains("127.0.0.1"));
    let error = default_branch(&git, &mirror, &url).unwrap_err();
    assert_eq!(error.details().unwrap()["timed_out"], true);
    fixture.write(None, &[fixture.revision(1, b"one\n")]);
    let error = push_branch(&git, &mirror, "origin", &fixture.main_commit(), "main").unwrap_err();
    assert_eq!(error.details().unwrap()["timed_out"], true);
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
}

#[test]
fn a_failed_init_of_a_new_local_remote_leaves_no_directory_behind() {
    let fixture = Fixture::new();
    let remote = fixture.path("remote.git");
    // A directory cannot be executed, so `init` fails after the folder exists.
    let broken = Git::new(fixture.path(""));
    let error = create_local_remote(&broken, remote.to_str().unwrap(), "main").unwrap_err();
    assert_eq!(error.code(), codes::GIT_FAILED);
    assert!(!remote.exists());
    create_local_remote(&fixture.git, remote.to_str().unwrap(), "main").unwrap();
    assert!(remote.join("HEAD").is_file());
}

#[test]
fn only_exit_one_of_symbolic_ref_means_a_detached_head() {
    let fixture = Fixture::new();
    let mirror = fixture.mirror();
    assert_eq!(
        checked_out_branch(&fixture.git, &mirror).unwrap(),
        Some("main".to_owned())
    );
    std::fs::write(
        mirror.join("HEAD"),
        "0123456789abcdef0123456789abcdef01234567\n",
    )
    .unwrap();
    assert_eq!(checked_out_branch(&fixture.git, &mirror).unwrap(), None);
    let error = checked_out_branch(&fixture.git, &fixture.path("absent")).unwrap_err();
    assert_eq!(error.code(), codes::GIT_FAILED);
}

#[test]
fn the_fold_line_is_appended_to_info_exclude_once_and_keeps_what_was_there() {
    let fixture = Fixture::new();
    let repo = fixture.path("repo");
    let exclude = repo.join("info/exclude");
    exclude_fold_dir(&repo).unwrap();
    assert_eq!(std::fs::read_to_string(&exclude).unwrap(), "/.fold/\n");
    for (before, after) in [
        ("*.log\n", "*.log\n/.fold/\n"),
        ("*.log", "*.log\n/.fold/\n"),
    ] {
        std::fs::write(&exclude, before).unwrap();
        exclude_fold_dir(&repo).unwrap();
        exclude_fold_dir(&repo).unwrap();
        assert_eq!(std::fs::read_to_string(&exclude).unwrap(), after);
    }
}
