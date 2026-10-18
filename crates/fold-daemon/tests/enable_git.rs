#![cfg(test)]

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use fold_core::events::types;
use fold_core::{Actor, EventFilter, HostPaths, HumanName, ManualClock};
use fold_daemon::projects::ProjectHandle;
use fold_daemon::services::projects::{GitIdentity, ProjectError, enable_git, init_project};
use fold_store::projects::ProjectRow;
use fold_vcs::git::Git;
use tempfile::TempDir;

pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A plain project `app` made by `fld init` in a folder that holds `README.md`, and its open handle.
pub struct Plain {
    _dir: TempDir,
    pub handle: Arc<ProjectHandle>,
}

impl Plain {
    pub async fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let host = HostPaths::new(dir.path().join("data").to_str(), None).unwrap();
        let clock = ManualClock::new();
        let root = dir.path().join("app");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("README.md"), "# app\n").unwrap();
        let made = init_project(&host, &root, None, &crate::common::ana(), &clock)
            .await
            .unwrap();
        let row = ProjectRow {
            slug: made.slug.clone(),
            name: made.name.clone(),
            root: made.root.to_string_lossy().into_owned(),
        };
        let handle = Arc::new(ProjectHandle::open(&row, Arc::new(clock)).await.unwrap());
        Self { _dir: dir, handle }
    }

    pub fn root(&self) -> &Path {
        self.handle.paths.root()
    }

    pub async fn enable(
        &self,
        identity: GitIdentity,
    ) -> Result<fold_daemon::services::projects::GitEnabled, ProjectError> {
        enable_git(&self.handle, &ana(), &Git::default(), identity).await
    }
}

pub fn ana() -> Actor {
    Actor::human(HumanName::parse("ana").unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn git_turns_on_in_a_plain_project_with_the_gitignore_as_the_humans_revision() {
    let project = Plain::new().await;
    let root = project.root().to_owned();
    std::fs::write(root.join("notes.md"), "mine\n").unwrap();
    let identity = GitIdentity {
        name: Some("Ana Lima".to_owned()),
        email: Some("ana@example.com".to_owned()),
    };
    let enabled = project.enable(identity).await.unwrap();
    assert_eq!(enabled.head.to_string(), "r2");
    assert_eq!(
        enabled.gitignore.map(|r| r.to_string()).as_deref(),
        Some("r2")
    );
    assert!(enabled.remotes.is_empty());
    let commit = enabled.commit.unwrap();
    assert_eq!(
        git(&root, &["rev-parse", "refs/heads/main"]).trim(),
        commit.as_str()
    );
    assert_eq!(git(&root, &["rev-list", "--count", "main"]).trim(), "2");
    assert_eq!(git(&root, &["config", "user.name"]).trim(), "Ana Lima");
    assert_eq!(
        std::fs::read_to_string(root.join(".gitignore")).unwrap(),
        ".fold/\n"
    );
    // r2 holds only the `.gitignore`: the human's other change stays theirs.
    assert_eq!(
        git(&root, &["show", "--name-only", "--format=", "main"]).trim(),
        ".gitignore"
    );
    assert_eq!(git(&root, &["status", "--porcelain"]), "?? notes.md\n");
    let (kind, actor): (String, String) =
        sqlx::query_as("SELECT kind, actor FROM integrations ORDER BY number DESC LIMIT 1")
            .fetch_one(project.handle.store.read())
            .await
            .unwrap();
    assert_eq!((kind.as_str(), actor.as_str()), ("record", "human:ana"));
    let filter = EventFilter {
        types: vec![types::GIT_SYNC_ENABLED],
        ..EventFilter::default()
    };
    let events = fold_store::events::read_after(project.handle.store.read(), 0, &filter, 10)
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].actor.as_str(), "human:ana");

    let again = project.enable(GitIdentity::default()).await.unwrap_err();
    let ProjectError::Fold(again) = again else {
        panic!("a fold error");
    };
    assert_eq!(again.details().unwrap()["reason"], "git_sync_on");
    assert!(again.hint().unwrap().contains("fld remote add"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_git_repository_with_history_is_refused_and_an_empty_one_is_taken() {
    let project = Plain::new().await;
    let root = project.root().to_owned();
    git(&root, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(root.join("x.txt"), "x\n").unwrap();
    git(&root, &["add", "x.txt"]);
    git(
        &root,
        &[
            "-c",
            "user.name=A",
            "-c",
            "user.email=a@b",
            "commit",
            "--quiet",
            "-m",
            "x",
        ],
    );
    let refused = project.enable(GitIdentity::default()).await.unwrap_err();
    let ProjectError::Fold(refused) = refused else {
        panic!("a fold error");
    };
    assert_eq!(refused.details().unwrap()["reason"], "git_history");
    assert!(
        fold_store::workspaces::git_sync(project.handle.store.read())
            .await
            .unwrap()
            .is_none()
    );

    std::fs::remove_dir_all(root.join(".git")).unwrap();
    std::fs::remove_file(root.join("x.txt")).unwrap();
    git(&root, &["init", "--quiet", "--initial-branch=main"]);
    let enabled = project.enable(GitIdentity::default()).await.unwrap();
    assert!(enabled.commit.is_some());
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn only_a_human_turns_git_on_and_the_identity_must_fit_an_author_line() {
    let project = Plain::new().await;
    let refused = enable_git(
        &project.handle,
        &Actor::System,
        &Git::default(),
        GitIdentity::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code(), Some(fold_core::codes::HUMAN_REQUIRED));
    let bad = GitIdentity {
        name: Some("Ana <x>".to_owned()),
        email: None,
    };
    assert_eq!(
        project.enable(bad).await.unwrap_err().code(),
        Some(fold_core::codes::INVALID_REQUEST)
    );
    assert!(!project.root().join(".git").exists());
}

/// A stop after `.git` exists and before Git sync is on leaves a repository without a commit, which the next run takes as it is.
#[cfg(feature = "failpoints")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_after_the_repository_is_finished_by_the_next_run() {
    let scenario = fail::FailScenario::setup();
    let project = Plain::new().await;
    fail::cfg("enable_git_after_repository", "return").unwrap();
    let stopped = project.enable(GitIdentity::default()).await.unwrap_err();
    fail::remove("enable_git_after_repository");
    assert_eq!(stopped.code(), Some("failpoint"));
    assert!(project.root().join(".git").is_dir());
    assert!(
        fold_store::workspaces::git_sync(project.handle.store.read())
            .await
            .unwrap()
            .is_none()
    );
    let enabled = project.enable(GitIdentity::default()).await.unwrap();
    assert_eq!(enabled.head.to_string(), "r2");
    assert_eq!(
        git(project.root(), &["rev-list", "--count", "main"]).trim(),
        "2"
    );
    scenario.teardown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_repository_on_another_branch_is_taken_on_the_configured_branch() {
    let project = Plain::new().await;
    let root = project.root().to_owned();
    git(&root, &["init", "--quiet", "--initial-branch=master"]);
    project.enable(GitIdentity::default()).await.unwrap();
    assert_eq!(
        git(&root, &["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/main"
    );
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_git_route_is_socket_only_and_turns_git_on_once() {
    use axum::http::{Method, StatusCode};
    use serde_json::json;
    let h = crate::common::acceptance::Harness::start_plain().await;
    let path = "/api/v1/projects/demo/git";
    let (status, _) = h.http(Method::POST, path, Some(&json!({})), "ana").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = h
        .socket(Method::POST, path, Some(&json!({"git_email": "a<b"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_request");
    assert!(!h.handle.paths.root().join(".git").exists());
    let (status, body) = h
        .socket(Method::POST, path, Some(&json!({"git_name": "Ana Lima"})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["revision"], body["gitignore"]);
    assert!(body["commit"].is_string(), "{body}");
    assert_eq!(body["remotes"], json!([]));
    let (status, body) = h.socket(Method::POST, path, Some(&json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["details"]["reason"], "git_sync_on");
}
