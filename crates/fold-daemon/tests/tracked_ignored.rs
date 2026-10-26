#![cfg(test)]

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use fold_daemon::services::projects::{
    FolderSync, ProjectAttention, RecordOutcome, RemoteContext, sync_folder,
};
use fold_vcs::git::Git;
use serde_json::{Value, json};

use crate::common::acceptance::Harness;
use crate::folder_stash::wait_for;
use crate::human_changes::{file_of, integrate, record_as_ana};

fn root(h: &Harness) -> PathBuf {
    PathBuf::from(h.root())
}

fn write(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

async fn status(h: &Harness) -> Value {
    let (status, body) = h
        .socket(Method::GET, "/api/v1/projects/demo/folder", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn sync(h: &Harness) -> FolderSync {
    sync_folder(&RemoteContext::of(&h.handle), &Git::default())
        .await
        .unwrap()
}

/// Main holds `app.env` and `build/version.txt`, then an agent's change adds the rules that match both and edits `app.env` in the same proposal; the folder follows main.
async fn tracked_then_ignored(h: &Harness) -> String {
    integrate(
        h,
        "frontend",
        &[("app.env", "KEY=1\n"), ("build/version.txt", "1.0\n")],
    )
    .await;
    let revision = integrate(
        h,
        "frontend",
        &[(".foldignore", "*.env\nbuild/\n"), ("app.env", "KEY=2\n")],
    )
    .await;
    wait_for(&root(h), ".foldignore", "*.env\nbuild/\n").await;
    wait_for(&root(h), "app.env", "KEY=2\n").await;
    revision
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_checkpoint_of_an_agent_keeps_a_tracked_file_that_a_new_rule_matches() {
    let h = Harness::start_plain().await;
    let revision = tracked_then_ignored(&h).await;
    // The proposal changed the tracked file and did not delete it, although its own new rule matches it.
    assert_eq!(
        file_of(&h, &revision, "app.env").await.as_deref(),
        Some("KEY=2\n")
    );
    assert_eq!(
        file_of(&h, &revision, "build/version.txt").await.as_deref(),
        Some("1.0\n")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn record_status_and_needs_you_agree_on_a_tracked_file_the_rules_match() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    tracked_then_ignored(&h).await;
    write(&dir, "build/out.bin", "generated\n");
    write(&dir, "local.env", "SECRET=1\n");
    // Untracked files the rules match count nowhere; the tracked ones stay as main has them.
    assert!(!sync(&h).await.followed.unrecorded);
    assert_eq!(status(&h).await["changes"], json!([]));
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::UpToDate { .. }
    ));

    write(&dir, "app.env", "KEY=3\n");
    write(&dir, "build/version.txt", "2.0\n");
    let synced = sync(&h).await;
    assert!(synced.followed.unrecorded);
    assert_eq!(synced.attention(), [ProjectAttention::UncommittedChanges]);
    assert_eq!(
        status(&h).await["changes"],
        json!([
            {"path": "app.env", "kind": "modified", "entry": "file", "held": false},
            {"path": "build/version.txt", "kind": "modified", "entry": "file", "held": false},
        ])
    );
    let RecordOutcome::Recorded {
        revision, files, ..
    } = record_as_ana(&h).await.unwrap()
    else {
        panic!("a record");
    };
    assert_eq!(files, ["app.env", "build/version.txt"]);
    let revision = revision.to_string();
    assert_eq!(
        file_of(&h, &revision, "app.env").await.as_deref(),
        Some("KEY=3\n")
    );
    assert_eq!(file_of(&h, &revision, "local.env").await, None);
    assert_eq!(file_of(&h, &revision, "build/out.bin").await, None);

    // A tracked file deleted from the folder is a deletion, whatever the rules say.
    std::fs::remove_file(dir.join("app.env")).unwrap();
    assert!(sync(&h).await.followed.unrecorded);
    assert_eq!(
        status(&h).await["changes"],
        json!([{"path": "app.env", "kind": "deleted", "entry": "file", "held": false}])
    );
    let RecordOutcome::Recorded { revision, .. } = record_as_ana(&h).await.unwrap() else {
        panic!("a record");
    };
    assert_eq!(file_of(&h, &revision.to_string(), "app.env").await, None);
    // Once main no longer tracks it, the rules leave the file out again.
    write(&dir, "app.env", "KEY=4\n");
    assert!(!sync(&h).await.followed.unrecorded);
    assert_eq!(status(&h).await["changes"], json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tracked_file_over_the_size_limit_keeps_its_recorded_version() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    let config = dir.join(".fold/config.toml");
    let kept = std::fs::read_to_string(&config).unwrap_or_default();
    std::fs::write(&config, format!("max_file_bytes = 32\n{kept}")).unwrap();
    let large = "x".repeat(100);
    write(&dir, "README.md", &large);
    assert!(!sync(&h).await.followed.unrecorded);
    let body = status(&h).await;
    assert_eq!(body["changes"], json!([]));
    assert_eq!(
        body["skipped"],
        json!([{"path": "README.md", "reason": "too_large"}])
    );
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::UpToDate { .. }
    ));
    // Another change records alone, and main keeps the version of the file it had.
    write(&dir, "notes.md", "notes\n");
    let RecordOutcome::Recorded {
        revision,
        files,
        skipped,
        ..
    } = record_as_ana(&h).await.unwrap()
    else {
        panic!("a record");
    };
    assert_eq!(files, ["notes.md"]);
    assert_eq!(skipped.len(), 1);
    assert_eq!(
        file_of(&h, &revision.to_string(), "README.md")
            .await
            .as_deref(),
        Some("# demo\n")
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("README.md")).unwrap(),
        large
    );
}
