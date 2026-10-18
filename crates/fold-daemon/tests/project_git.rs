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
use fold_daemon::services::projects::{ProjectAttention, commit_main, pull};
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

#[tokio::test]
async fn a_commit_run_appends_git_committed_with_each_row_it_records() {
    let fx = Fixture::new().await;
    integrate(&fx).await;
    let committed = commit_main(
        fx.project.store(),
        fx.project.paths(),
        &Git::default(),
        None,
        fx.project.clock(),
    )
    .await
    .unwrap();
    assert_eq!(fx.count("SELECT COUNT(*) FROM revision_commits").await, 2);
    assert_eq!(
        fx.count(
            r#"SELECT COUNT(*) FROM events e JOIN revision_commits rc ON rc.revision_id = e.revision_id AND rc."commit" = json_extract(e.data, '$.commit') WHERE e.type = 'git.committed'"#
        )
        .await,
        2
    );
    let published: Vec<_> = committed
        .events
        .events()
        .iter()
        .map(|event| (event.event_type, event.revision))
        .collect();
    assert_eq!(
        published,
        [
            (
                types::GIT_COMMITTED,
                Some(RevisionId::try_from_db(1).unwrap())
            ),
            (
                types::GIT_COMMITTED,
                Some(RevisionId::try_from_db(2).unwrap())
            ),
        ]
    );
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

    let filter = EventFilter {
        types: vec![types::GIT_COMMITTED],
        ..EventFilter::default()
    };
    until(async || {
        fold_store::events::read_after(fx.project.store().read(), 0, &filter, 10)
            .await
            .unwrap()
            .len()
            == 2
    })
    .await;
    let events = fold_store::events::read_after(fx.project.store().read(), 0, &filter, 10)
        .await
        .unwrap();
    assert_eq!(
        events[1].revision,
        Some(RevisionId::try_from_db(2).unwrap())
    );
    assert_eq!(events[1].data, serde_json::json!({"commit": second}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_main_does_not_touch_stays_and_the_folder_follows_around_it() {
    let fx = Fixture::new().await;
    let (_job, context) = start_watched_job(&fx, Git::default());
    wait_for_rows(&fx, 1).await;
    let root = fx.project.paths().root().to_owned();
    std::fs::write(root.join("README.md"), "# mine\n").unwrap();

    integrate(&fx).await;
    wait_for_rows(&fx, 2).await;
    let second = commit_of(&fx, 2).await;
    until(async || branch(&root) == second).await;
    assert!(root.join("src/new.rs").is_file());
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        "# mine\n"
    );
    // The index follows main for the paths fold wrote, so Git shows only the human's change.
    assert_eq!(git(&root, &["status", "--porcelain"]), " M README.md\n");
    until(async || context.attention() == [ProjectAttention::UncommittedChanges]).await;
    until(async || attention_events(&fx).await == ["git.uncommitted_changes"]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_on_a_path_main_changed_holds_the_folder_and_the_branch_until_it_is_undone() {
    let fx = Fixture::new().await;
    let (job, context) = start_watched_job(&fx, Git::default());
    wait_for_rows(&fx, 1).await;
    let root = fx.project.paths().root().to_owned();
    let first = commit_of(&fx, 1).await;
    std::fs::write(root.join("src/lib.rs"), "pub fn mine() {}\n").unwrap();

    integrate(&fx).await;
    wait_for_rows(&fx, 2).await;
    until(async || context.attention() == [ProjectAttention::UncommittedChanges]).await;
    assert_eq!(branch(&root), first);
    // The folder and the branch move together or not at all, so nothing of r2 is written.
    assert!(!root.join("src/new.rs").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
        "pub fn mine() {}\n"
    );
    until(async || attention_events(&fx).await == ["git.uncommitted_changes"]).await;
    drop(job);

    // The attention item is derived from the folder, so a restart shows it again.
    let (job, context) = start_watched_job(&fx, Git::default());
    until(async || context.attention() == [ProjectAttention::UncommittedChanges]).await;
    until(async || attention_events(&fx).await.len() == 2).await;
    assert_eq!(branch(&root), first);
    drop(job);

    // The human puts the change away; the next start brings the folder to main.
    git(&root, &["checkout", "--quiet", "--", "src/lib.rs"]);
    let (_job, context) = start_watched_job(&fx, Git::default());
    let second = commit_of(&fx, 2).await;
    until(async || branch(&root) == second).await;
    assert!(root.join("src/new.rs").is_file());
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
    until(async || context.attention().is_empty()).await;
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
        let made = crate::common::init_with_git(&host, &folder, &clock)
            .await
            .unwrap();
        let row = ProjectRow {
            slug: made.slug.clone(),
            name: made.name.clone(),
            root: made.root.to_string_lossy().into_owned(),
        };
        let handle = ProjectHandle::open(&row, Arc::new(clock))
            .await
            .unwrap()
            .share();
        Self {
            _dir: dir,
            root: made.root,
            handle,
        }
    }

    /// Records a revision on top of `main` holding `files`, as an integration would, and runs the Git job as it does after one.
    async fn add_revision(&self, files: &[(&str, &[u8])]) {
        self.add_revision_only(files).await;
        self.commit().await;
    }

    /// [`Opened::add_revision`] without the Git job's run.
    async fn add_revision_only(&self, files: &[(&str, &[u8])]) {
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
    }

    async fn commit(&self) -> fold_daemon::services::projects::Committed {
        let context = fold_daemon::services::projects::RemoteContext::of(&self.handle);
        fold_daemon::services::projects::sync_folder(&context, &Git::default())
            .await
            .unwrap()
            .committed
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
async fn a_commit_of_the_human_is_recorded_at_once_and_merged_with_main() {
    let opened = Opened::new().await;
    until(async || opened.committed_revisions().await == 1).await;
    let outside = opened.commit_outside("b.txt", "mine\n");
    // An agent's revision lands meanwhile; the next run of the Git job records the human's commit on top of it.
    opened
        .add_revision(&[("a.txt", b"one\n"), ("c.txt", b"fold\n")])
        .await;
    let (kind, actor, remote): (String, String, Option<String>) = sqlx::query_as(
        "SELECT kind, actor, remote_commit FROM integrations ORDER BY number DESC LIMIT 1",
    )
    .fetch_one(opened.handle.store.read())
    .await
    .unwrap();
    assert_eq!(
        (kind.as_str(), actor.as_str(), remote.as_deref()),
        ("record", "human:ana", Some(outside.as_str()))
    );
    assert_eq!(
        fold_store::workspaces::head(opened.handle.store.read())
            .await
            .unwrap()
            .unwrap()
            .number
            .to_string(),
        "r3"
    );
    until(async || opened.committed_revisions().await == 3).await;
    // The revision's commit merges the human's commit, and the branch and the folder follow it.
    let merge = branch(&opened.root);
    let parents = git(&opened.root, &["log", "-1", "--format=%P", &merge]);
    let second = git(&opened.root, &["rev-parse", &format!("{merge}^1")]);
    assert_eq!(parents.trim(), format!("{} {outside}", second.trim()));
    assert_eq!(
        std::fs::read_to_string(opened.root.join("c.txt")).unwrap(),
        "fold\n"
    );
    assert_eq!(
        std::fs::read_to_string(opened.root.join("b.txt")).unwrap(),
        "mine\n"
    );
    assert_eq!(git(&opened.root, &["status", "--porcelain"]), "");

    // fld pull only talks to a remote, and this project has none.
    let ana = Actor::human(HumanName::parse("ana").unwrap());
    let refused = pull(&opened.handle, &ana, &Git::default(), None)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some("remote_not_set"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflicting_commit_of_the_human_waits_for_the_main_agent_and_nothing_is_forced() {
    let opened = Opened::new().await;
    until(async || opened.committed_revisions().await == 1).await;
    // Main moves first, so the human's commit, on top of r1, meets main's change of the same file.
    opened.add_revision_only(&[("a.txt", b"fold\n")]).await;
    let outside = opened.commit_outside("a.txt", "mine\n");
    opened.commit().await;
    let waiting: (String, String) =
        sqlx::query_as("SELECT state, commit_id FROM human_changes ORDER BY id DESC LIMIT 1")
            .fetch_one(opened.handle.store.read())
            .await
            .unwrap();
    assert_eq!(waiting, ("conflicted".to_owned(), outside.clone()));
    // Another run records nothing twice, and the branch and the folder stay on the human's commit.
    opened.commit().await;
    let changes: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT state, commit_id FROM human_changes ORDER BY id")
            .fetch_all(opened.handle.store.read())
            .await
            .unwrap();
    // The first is init's: the folder's files as r1.
    assert_eq!(
        changes,
        [
            ("recorded".to_owned(), None),
            ("conflicted".to_owned(), Some(outside.clone()))
        ]
    );
    assert_eq!(branch(&opened.root), outside);
    assert_eq!(
        std::fs::read_to_string(opened.root.join("a.txt")).unwrap(),
        "mine\n"
    );
}
