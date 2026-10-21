#![cfg(test)]

use crate::workspaces_support;

use std::sync::Arc;

use crate::workspaces_support::three_revisions;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use fold_core::object::ObjectKind;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{Clock, ManualClock, Timestamp};
use fold_daemon::http::router;
use fold_daemon::{Daemon, DaemonConfig, RunningDaemon};
use fold_store::projects::{self, ProjectRow};
use fold_store::testing::TestProject;
use fold_store::{Store, StoreError, workspaces};
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

struct Host {
    _dir: TempDir,
    daemon: RunningDaemon,
}

/// A daemon whose registry lists `project` as `alpha`.
async fn host(project: &TestProject) -> Host {
    let dir = TempDir::new().unwrap();
    let config = DaemonConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        socket: dir.path().join("run").join("fold.sock"),
        data_dir: dir.path().join("data"),
        runtime_dir: dir.path().join("run"),
        web_dir: None,
        session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
    };
    let clock = ManualClock::new();
    let registry = Store::open_registry(&config.registry_path()).await.unwrap();
    let row = ProjectRow {
        slug: "alpha".to_owned(),
        name: "Alpha".to_owned(),
        root: project.paths().root().to_string_lossy().into_owned(),
    };
    let now = clock.now();
    registry
        .write(async |tx| {
            projects::insert_project(tx, &row, now).await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    registry.close().await.unwrap();
    let daemon = Daemon::start(config, Arc::new(clock)).await.unwrap();
    Host { _dir: dir, daemon }
}

impl Host {
    async fn get(&self, path: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .uri(format!("/api/v1/projects/alpha{path}"))
            .body(Body::empty())
            .unwrap();
        let response = router(self.daemon.state().clone())
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
}

fn code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap()
}

#[tokio::test]
async fn the_five_repository_routes_answer_for_a_known_revision() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    let host = host(&project).await;

    let (status, state) = host.get("/repository").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state["head"], "r3");
    assert_eq!(
        state["main_path"],
        project.paths().root().to_string_lossy().as_ref()
    );
    let ids: Vec<&str> = state["recent"]
        .as_array()
        .unwrap()
        .iter()
        .map(|revision| revision["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["r3", "r2", "r1"]);
    let r2 = &state["recent"][1];
    assert_eq!(r2["origin"], "proposal");
    assert_eq!(r2["author"], "backend");
    assert_eq!(r2["task"], "T-3");
    assert_eq!(r2["proposal"], "P-5");
    assert_eq!(r2["title"], "Add the guide");
    assert_eq!(r2["parent"], "r1");
    assert_eq!(r2["created_at"], 1002);
    assert_eq!(state["recent"][0]["pull"], "abc123");
    assert_eq!(state["recent"][2]["origin"], "initial");

    let (status, page) = host.get("/revisions?limit=2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["revisions"].as_array().unwrap().len(), 2);
    assert_eq!(page["before"], "r2");
    let (_, page) = host.get("/revisions?limit=2&before=r2").await;
    assert_eq!(page["revisions"][0]["id"], "r1");
    assert_eq!(page["before"], Value::Null);

    let (status, detail) = host.get("/revisions/r3").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["revision"]["id"], "r3");
    assert_eq!(detail["files"][0]["path"], "README.md");
    assert_eq!(detail["files"][0]["change"], "deleted");
    assert_eq!(detail["files"][0]["previous_path"], Value::Null);
    assert_eq!(detail["files"][0]["new_hash"], Value::Null);
    assert!(detail["files"][0]["old_hash"].is_string());
    assert_eq!(
        detail["totals"],
        json!({"files": 2, "added_lines": 1, "removed_lines": 1})
    );

    let (status, tree) = host.get("/revisions/r3/tree?path=src").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tree["path"], "src");
    assert_eq!(tree["entries"][1]["name"], "main.rs");
    assert_eq!(tree["entries"][1]["path"], "src/main.rs");
    assert_eq!(tree["entries"][1]["kind"], "file");
    assert_eq!(tree["entries"][1]["size"], 13);
    let (_, root) = host.get("/revisions/r3/tree").await;
    assert_eq!(root["entries"][0]["name"], "docs");
    assert_eq!(root["entries"][0]["kind"], "directory");
    assert_eq!(root["entries"][0]["size"], Value::Null);

    let (status, file) = host.get("/revisions/r1/file?path=README.md").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(file["path"], "README.md");
    assert_eq!(file["text"], "hello\n");
    assert_eq!(file["binary"], false);
    assert_eq!(file["too_large"], false);
    assert_eq!(file["size"], 6);
    assert_eq!(file["kind"], "file");
    let (_, binary) = host.get("/revisions/r1/file?path=logo.png").await;
    assert_eq!(binary["binary"], true);
    assert!(binary.get("text").is_none());
    assert_eq!(binary["hash"].as_str().unwrap().len(), 64);
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn the_tree_route_sends_a_symlink_target_and_no_target_for_a_file() {
    let project = TestProject::new().await;
    let objects = workspaces_support::objects(&project);
    let mut flat = FlatTree::new();
    for (path, kind, content) in [
        ("README.md", EntryKind::File, &b"hello\n"[..]),
        ("config.ts", EntryKind::Symlink, b"../shared/config.ts"),
    ] {
        let hash = objects.write(ObjectKind::Blob, content).unwrap();
        flat.insert(path.as_bytes().to_vec(), FlatEntry { kind, hash });
    }
    let tree = write_initial_tree(&objects, flat).unwrap();
    let r1 = workspaces::initial_revision(tree, Timestamp::from_millis(1_000).unwrap()).unwrap();
    let stored = write_revision(&objects, &r1).unwrap();
    workspaces::create_initial_revision(project.store(), &stored, None)
        .await
        .unwrap();
    let host = host(&project).await;

    let (status, tree) = host.get("/revisions/r1/tree").await;
    assert_eq!(status, StatusCode::OK, "{tree}");
    assert_eq!(tree["entries"][0]["name"], "README.md");
    assert!(tree["entries"][0].get("target").is_none());
    assert_eq!(tree["entries"][1]["kind"], "symlink");
    assert_eq!(tree["entries"][1]["target"], "../shared/config.ts");
    assert_eq!(tree["entries"][1]["size"], 19);
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn unknown_revisions_and_bad_input_use_the_envelope() {
    let project = TestProject::new().await;
    three_revisions(&project).await;
    let host = host(&project).await;
    for path in [
        "/revisions/r9",
        "/revisions/r9/tree",
        "/revisions/r9/file?path=README.md",
    ] {
        let (status, body) = host.get(path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(code(&body), "revision_not_found", "{path}");
    }
    for (path, status, expected) in [
        ("/revisions/T-1", StatusCode::BAD_REQUEST, "invalid_id"),
        (
            "/revisions?before=nine",
            StatusCode::BAD_REQUEST,
            "invalid_id",
        ),
        (
            "/revisions?limit=many",
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            "/revisions/r1/tree?path=../x",
            StatusCode::BAD_REQUEST,
            "invalid_path",
        ),
        (
            "/revisions/r1/file",
            StatusCode::BAD_REQUEST,
            "invalid_path",
        ),
        (
            "/revisions/r1/file?path=nowhere",
            StatusCode::NOT_FOUND,
            "not_found",
        ),
        (
            "/revisions/r1/tree?path=README.md",
            StatusCode::NOT_FOUND,
            "not_found",
        ),
    ] {
        let (got, body) = host.get(path).await;
        assert_eq!((got, code(&body)), (status, expected), "{path}");
    }
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn an_empty_project_has_a_null_head() {
    let project = TestProject::new().await;
    let host = host(&project).await;
    let (status, state) = host.get("/repository").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state["head"], Value::Null);
    assert_eq!(state["recent"], json!([]));
    host.daemon.shutdown().await;
}

/// A project made by `fld init` starts from the genesis `r0`, whose parent is none, so the revision pages end there and not at `r1`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_revision_pages_reach_the_genesis_r0_and_end_there() {
    use crate::common::acceptance::{HUMAN, Harness, SLUG};
    use axum::http::Method;

    let source = TempDir::new().unwrap();
    std::fs::write(source.path().join("README.md"), "# demo\n").unwrap();
    let (h, _) = Harness::start_initialized(source.path()).await;
    std::fs::write(
        std::path::Path::new(h.root()).join("notes.md"),
        "the plan\n",
    )
    .unwrap();
    let human = fold_core::Actor::human(fold_core::HumanName::parse(HUMAN).unwrap());
    let recorded =
        fold_daemon::services::projects::record(&h.handle, &human, &fold_vcs::git::Git::default())
            .await
            .unwrap();
    assert!(
        matches!(
            &recorded,
            fold_daemon::services::projects::RecordOutcome::Recorded { revision, .. }
                if revision.to_string() == "r2"
        ),
        "{recorded:?}"
    );

    let mut pages = Vec::new();
    let mut before: Option<String> = None;
    loop {
        let path = match &before {
            Some(before) => format!("/api/v1/projects/{SLUG}/revisions?limit=1&before={before}"),
            None => format!("/api/v1/projects/{SLUG}/revisions?limit=1"),
        };
        let (status, page) = h.http(Method::GET, &path, None, HUMAN).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        let ids: Vec<&str> = page["revisions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|revision| revision["id"].as_str().unwrap())
            .collect();
        pages.push((ids.join(","), page["before"].clone()));
        match page["before"].as_str() {
            Some(next) if pages.len() < 5 => before = Some(next.to_owned()),
            _ => break,
        }
    }
    assert_eq!(
        pages,
        [
            ("r2".to_owned(), json!("r2")),
            ("r1".to_owned(), json!("r1")),
            ("r0".to_owned(), Value::Null),
        ]
    );
}
