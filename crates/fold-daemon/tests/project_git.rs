#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use crate::git_support::{approved, commit_of, start_job, start_watched_job, wait_for_rows};
use crate::reviews_support::{Fixture, p};
use fold_core::events::types;
use fold_core::revision::Revision;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{Actor, EventFilter, HumanName, ManualClock, RevisionId, Timestamp};
use fold_daemon::projects::ProjectHandle;
use fold_daemon::services::projects::{
    ProjectAttention, PullOutcome, commit_main, init_project, pull,
};
use fold_store::projects::ProjectRow;
use fold_store::testing::TestProject;
use fold_store::workspaces::{self, advance_main, insert_revision};
use fold_vcs::git::Git;
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use tempfile::TempDir;

/// `git` in `dir` as a person would run it there, without the user's configuration.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Ana")
        .env("GIT_AUTHOR_EMAIL", "ana@example.com")
        .env("GIT_COMMITTER_NAME", "Ana")
        .env("GIT_COMMITTER_EMAIL", "ana@example.com")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn branch(root: &Path) -> String {
    git(root, &["rev-parse", "refs/heads/main"])
        .trim()
        .to_owned()
}

/// Waits until `done` holds, polling, for at most 30 seconds.
async fn until(mut done: impl AsyncFnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while !done().await {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

/// The types of the Git attention events, oldest first.
async fn attention_events(fx: &Fixture) -> Vec<String> {
    let filter = EventFilter {
        types: vec![
            types::GIT_OUTSIDE_COMMITS,
            types::GIT_UNCOMMITTED_CHANGES,
            types::GIT_ATTENTION_CLEARED,
        ],
        ..EventFilter::default()
    };
    fold_store::events::read_after(fx.project.store().read(), 0, &filter, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|event| event.event_type.as_str().to_owned())
        .collect()
}

/// `P-1` integrated as `r2`: it changes `src/lib.rs`, adds `src/new.rs` and the symlink `docs/readme`.
async fn integrate(fx: &Fixture) {
    let review = approved(fx).await;
    fx.reviews
        .integrate_proposal(&review, p(1), Some(1))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_integration_moves_the_branch_and_the_folder_and_leaves_it_clean() {
    let fx = Fixture::new().await;
    let _job = start_job(&fx, Git::default());
    wait_for_rows(&fx, 1).await;
    let root = fx.project.paths().root().to_owned();
    assert_eq!(branch(&root), commit_of(&fx, 1).await);
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
    assert!(
        std::fs::read_to_string(root.join(".git/info/exclude"))
            .unwrap()
            .lines()
            .any(|line| line == "/.fold/")
    );

    integrate(&fx).await;
    wait_for_rows(&fx, 2).await;
    let second = commit_of(&fx, 2).await;
    until(async || branch(&root) == second).await;
    assert!(root.join("src/new.rs").is_file());
    assert_eq!(
        std::fs::read_link(root.join("docs/readme")).unwrap(),
        Path::new("../README.md")
    );
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_folder_with_changes_waits_with_the_attention_item_and_catches_up_once_clean() {
    let fx = Fixture::new().await;
    let (job, context) = start_watched_job(&fx, Git::default());
    wait_for_rows(&fx, 1).await;
    let root = fx.project.paths().root().to_owned();
    let first = commit_of(&fx, 1).await;
    std::fs::write(root.join("README.md"), "# mine\n").unwrap();

    integrate(&fx).await;
    wait_for_rows(&fx, 2).await;
    until(async || context.attention() == [ProjectAttention::UncommittedChanges]).await;
    assert_eq!(branch(&root), first);
    assert!(!root.join("src/new.rs").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        "# mine\n"
    );
    // The event is stored after the attention changes in memory.
    until(async || attention_events(&fx).await == ["git.uncommitted_changes"]).await;
    drop(job);

    // The attention item is derived from the folder, so a restart shows it again.
    let (job, context) = start_watched_job(&fx, Git::default());
    until(async || context.attention() == [ProjectAttention::UncommittedChanges]).await;
    until(async || attention_events(&fx).await.len() == 2).await;
    assert_eq!(branch(&root), first);
    drop(job);

    // The human puts the change away; the next start brings the folder to main.
    git(&root, &["checkout", "--quiet", "--", "README.md"]);
    let (_job, context) = start_watched_job(&fx, Git::default());
    let second = commit_of(&fx, 2).await;
    until(async || branch(&root) == second).await;
    assert!(root.join("src/new.rs").is_file());
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
    until(async || context.attention().is_empty()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_outside_commit_stops_the_branch_and_raises_the_attention_item() {
    let fx = Fixture::new().await;
    let (_job, context) = start_watched_job(&fx, Git::default());
    wait_for_rows(&fx, 1).await;
    let root = fx.project.paths().root().to_owned();
    std::fs::write(root.join("README.md"), "# mine\n").unwrap();
    git(&root, &["commit", "--quiet", "-am", "mine"]);
    let outside = branch(&root);

    integrate(&fx).await;
    wait_for_rows(&fx, 2).await;
    until(async || context.attention() == [ProjectAttention::OutsideCommits]).await;
    assert_eq!(branch(&root), outside);
    until(async || attention_events(&fx).await == ["git.outside_commits"]).await;
    let filter = EventFilter {
        types: vec![types::GIT_OUTSIDE_COMMITS],
        ..EventFilter::default()
    };
    let events = fold_store::events::read_after(fx.project.store().read(), 0, &filter, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].actor.as_str(), "fold");
    assert_eq!(
        events[0].data,
        serde_json::json!({"branch": "main", "commit": outside})
    );
}

#[tokio::test]
async fn workspaces_are_made_under_the_fold_folder() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    assert_eq!(
        fx.text("SELECT path FROM workspaces").await.as_deref(),
        Some(".fold/ws/t-1")
    );
    assert!(fx.project.paths().ws_dir().join("t-1/src/new.rs").is_file());
    assert_eq!(
        fx.project.paths().ws_dir(),
        fx.project.paths().root().join(".fold/ws")
    );
}

#[tokio::test]
async fn a_project_with_the_old_layout_is_refused_with_a_hint() {
    let project = TestProject::new().await;
    std::fs::write(project.paths().fold_dir().join("main-materialised"), "r1\n").unwrap();
    let row = ProjectRow {
        slug: "old".to_owned(),
        name: "old".to_owned(),
        root: project.paths().root().to_string_lossy().into_owned(),
    };
    let error = ProjectHandle::open(&row, Arc::new(project.clock().clone()))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "project_unavailable");
    assert!(error.hint().unwrap().contains("fld init"));
    assert_eq!(error.details().unwrap()["reason"], "old_layout");
}

/// A project made by `fld init` in a folder holding `a.txt`, open with its Git job.
struct Opened {
    _dir: TempDir,
    root: PathBuf,
    handle: Arc<ProjectHandle>,
}

impl Opened {
    async fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let host = fold_core::HostPaths::new(dir.path().join("data").to_str(), None).unwrap();
        let clock = ManualClock::new();
        let folder = dir.path().join("app");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("a.txt"), "one\n").unwrap();
        let made = init_project(&host, &folder, None, &clock).await.unwrap();
        let row = ProjectRow {
            slug: made.slug.clone(),
            name: made.name.clone(),
            root: made.root.to_string_lossy().into_owned(),
        };
        let handle = Arc::new(ProjectHandle::open(&row, Arc::new(clock)).await.unwrap());
        Self {
            _dir: dir,
            root: made.root,
            handle,
        }
    }

    /// Records a revision on top of `main` holding `files`, as an integration would, and writes its commit as the Git job does after one.
    async fn add_revision(&self, files: &[(&str, &[u8])]) {
        let store = &self.handle.store;
        let head = workspaces::head(store.read()).await.unwrap().unwrap();
        let objects = ObjectStore::open(&self.handle.paths.fold_dir()).unwrap();
        let mut flat = FlatTree::new();
        for (path, bytes) in files {
            let hash = objects
                .write(fold_core::object::ObjectKind::Blob, bytes)
                .unwrap();
            flat.insert(
                path.as_bytes().to_vec(),
                FlatEntry {
                    kind: EntryKind::File,
                    hash,
                },
            );
        }
        let number = RevisionId::from_number(head.number.number() + 1).unwrap();
        let revision = Revision {
            tree: write_initial_tree(&objects, flat).unwrap(),
            parent: Some(head.hash),
            number,
            integration: None,
            created_at: Timestamp::from_millis(1_700_000_000_000).unwrap(),
        };
        let stored = write_revision(&objects, &revision).unwrap();
        store
            .write(async |tx| {
                insert_revision(tx, &stored).await?;
                advance_main(tx, head.number, number).await
            })
            .await
            .unwrap();
        self.commit().await;
    }

    async fn commit(&self) -> fold_daemon::services::projects::Committed {
        commit_main(
            &self.handle.store,
            &self.handle.paths,
            &Git::default(),
            self.handle.clock.as_ref(),
        )
        .await
        .unwrap()
    }

    /// `fld pull` with no remote, by the human `ana`.
    async fn pull(&self) -> PullOutcome {
        let ana = Actor::human(HumanName::parse("ana").unwrap());
        pull(&self.handle, &ana, &Git::default(), None)
            .await
            .unwrap()
    }

    /// A commit the human makes in the project folder, outside fold.
    fn commit_outside(&self, path: &str, text: &str) -> String {
        std::fs::write(self.root.join(path), text).unwrap();
        git(&self.root, &["add", path]);
        git(&self.root, &["commit", "--quiet", "-m", "mine"]);
        branch(&self.root)
    }

    async fn committed_revisions(&self) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM revision_commits")
            .fetch_one(self.handle.store.read())
            .await
            .unwrap()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fold_pull_brings_an_outside_commit_in_as_a_pull_by_the_human() {
    let opened = Opened::new().await;
    until(async || opened.committed_revisions().await == 1).await;
    let outside = opened.commit_outside("b.txt", "mine\n");
    opened
        .add_revision(&[("a.txt", b"one\n"), ("c.txt", b"fold\n")])
        .await;
    // The commit of r2 is written, and the branch stays on the human's commit.
    assert_eq!(
        opened.commit().await.outside.map(|c| c.to_string()),
        Some(outside.clone())
    );
    assert_eq!(branch(&opened.root), outside);
    assert!(!opened.root.join("c.txt").exists());

    // A pull that names a remote would not read the commits made here, so it is refused until they are in.
    let ana = Actor::human(HumanName::parse("ana").unwrap());
    let named = pull(&opened.handle, &ana, &Git::default(), Some("origin"))
        .await
        .unwrap_err();
    assert_eq!(named.code(), Some("invalid_request"));
    let fold_daemon::services::projects::ProjectError::Fold(named) = named else {
        panic!("a fold error");
    };
    assert_eq!(named.details().unwrap()["reason"], "outside_commits");
    assert!(named.hint().unwrap().contains("then fld pull origin"));
    assert_eq!(branch(&opened.root), outside);

    let pulled = opened.pull().await;
    let PullOutcome::Integrated {
        remote,
        branch: pulled_branch,
        remote_commit,
        revision,
    } = pulled
    else {
        panic!("a clean pull: {pulled:?}");
    };
    assert_eq!((remote.as_str(), pulled_branch.as_str()), (".git", "main"));
    assert_eq!(remote_commit.as_str(), outside);
    assert_eq!(revision.to_string(), "r3");
    let (kind, actor): (String, String) =
        sqlx::query_as("SELECT kind, actor FROM integrations ORDER BY number DESC LIMIT 1")
            .fetch_one(opened.handle.store.read())
            .await
            .unwrap();
    assert_eq!((kind.as_str(), actor.as_str()), ("pull", "human:ana"));

    let committed = opened.commit().await;
    assert_eq!(committed.outside, None);
    until(async || opened.committed_revisions().await == 3).await;
    let merge = branch(&opened.root);
    let parents = git(&opened.root, &["log", "-1", "--format=%P", &merge]);
    let second = git(&opened.root, &["rev-parse", &format!("{merge}^1")]);
    assert_eq!(parents.trim(), format!("{} {outside}", second.trim()));
    assert_eq!(
        git(&opened.root, &["log", "-1", "--format=%B", second.trim()])
            .lines()
            .next(),
        Some("fold r2")
    );
    assert_eq!(
        std::fs::read_to_string(opened.root.join("c.txt")).unwrap(),
        "fold\n"
    );
    assert_eq!(
        std::fs::read_to_string(opened.root.join("b.txt")).unwrap(),
        "mine\n"
    );
    assert_eq!(git(&opened.root, &["status", "--porcelain"]), "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflicting_outside_commit_becomes_a_sync_task_and_nothing_is_forced() {
    let opened = Opened::new().await;
    until(async || opened.committed_revisions().await == 1).await;
    let outside = opened.commit_outside("a.txt", "mine\n");
    opened.add_revision(&[("a.txt", b"fold\n")]).await;

    let pulled = opened.pull().await;
    let PullOutcome::Conflicted {
        remote,
        remote_commit,
        created,
        ..
    } = pulled
    else {
        panic!("a conflict: {pulled:?}");
    };
    assert_eq!(remote, ".git");
    assert_eq!(remote_commit.as_str(), outside);
    assert!(created);
    opened.commit().await;
    assert_eq!(branch(&opened.root), outside);
    assert_eq!(
        std::fs::read_to_string(opened.root.join("a.txt")).unwrap(),
        "mine\n"
    );
}
