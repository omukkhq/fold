#![cfg(test)]

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness};
use crate::human_changes::{branch_born, file_of, git};

const SECRET: &str = "config/local.env";

/// A `fld` command as the human runs it: a request over the socket.
async fn fld(h: &Harness, method: Method, action: &str, body: Option<&Value>) -> Value {
    let (status, answer) = h
        .socket(method, &format!("/api/v1/projects/demo/{action}"), body)
        .await;
    assert_eq!(status, StatusCode::OK, "{action}: {answer}");
    answer
}

fn write(root: &Path, path: &str, text: &str) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

fn read(root: &Path, path: &str) -> Option<String> {
    std::fs::read_to_string(root.join(path)).ok()
}

/// Acceptance 28, `fld untrack`: a file the human recorded, here a local secret, becomes ignored by a new `.foldignore` line and stays tracked, so `fld record` records its next change. `fld untrack` removes it from main in one revision titled for what it does, which the Git commit, `fld log` and the web app show; the file stays in the project folder and the human's other change stays unrecorded. An agent workspace made while main tracked the file syncs to that revision: the sync removes the file, and when the agent's tools write it again, the ignore rule keeps it out of the workspace's changes and out of main.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_28_the_human_stops_tracking_a_recorded_file_that_is_now_ignored() {
    let h = Harness::start().await;
    branch_born(&h).await;
    let root = PathBuf::from(h.root());
    write(&root, SECRET, "KEY=1\n");
    let recorded = fld(&h, Method::POST, "record", None).await;
    assert_eq!(recorded["files"], json!([SECRET]));
    let task = h.create_task("frontend", "Agent work").await;
    let work = h.start_work("frontend", &task).await;
    assert_eq!(read(&work.dir, SECRET).as_deref(), Some("KEY=1\n"));

    // The new rule matches a tracked file, so the file stays tracked and its change is recorded.
    write(&root, ".foldignore", "*.env\n");
    write(&root, SECRET, "KEY=2\n");
    let status = fld(&h, Method::GET, "folder", None).await;
    assert_eq!(status["ignored_tracked"], json!([SECRET]));
    let recorded = fld(&h, Method::POST, "record", None).await;
    assert_eq!(recorded["files"], json!([".foldignore", SECRET]));
    let revision = recorded["revision"].as_str().unwrap().to_owned();
    assert_eq!(
        file_of(&h, &revision, SECRET).await.as_deref(),
        Some("KEY=2\n")
    );

    // fld untrack removes it from main and leaves it, and the other change, in the folder.
    write(&root, "notes.md", "my notes\n");
    let untracked = fld(
        &h,
        Method::POST,
        "untrack",
        Some(&json!({"paths": [SECRET]})),
    )
    .await;
    assert_eq!(untracked["files"], json!([SECRET]));
    assert_eq!(untracked["unignored"], json!([]));
    let revision = untracked["revision"].as_str().unwrap().to_owned();
    let commit = untracked["commit"].as_str().unwrap().to_owned();
    assert_eq!(file_of(&h, &revision, SECRET).await, None);
    assert_eq!(read(&root, SECRET).as_deref(), Some("KEY=2\n"));
    assert_eq!(read(&root, "notes.md").as_deref(), Some("my notes\n"));
    let title = format!("Stop tracking {SECRET}");
    assert_eq!(
        git(&root, &["log", "-1", "--format=%s", &commit]).trim(),
        title
    );
    assert_eq!(git(&root, &["ls-files", SECRET]), "");
    let (code, log) = h
        .http(Method::GET, "/api/v1/projects/demo/revisions", None, HUMAN)
        .await;
    assert_eq!(code, StatusCode::OK, "{log}");
    assert_eq!(log["revisions"][0]["id"], revision.as_str());
    assert_eq!(log["revisions"][0]["origin"], "record");
    assert_eq!(log["revisions"][0]["title"], title.as_str());
    let status = fld(&h, Method::GET, "folder", None).await;
    assert_eq!(status["at_main"], true);
    assert_eq!(
        status["changes"],
        json!([{"path": "notes.md", "kind": "added", "entry": "file", "held": false}])
    );
    assert_eq!(status["ignored_tracked"], json!([]));

    // The agent's workspace syncs to the revision: the file leaves it and no longer counts.
    h.call("frontend", "sync_workspace", json!({"ws": work.workspace}))
        .await;
    assert_eq!(read(&work.dir, SECRET), None);
    work.write(&[(SECRET, "KEY=agent\n"), ("src.txt", "agent\n")]);
    let changes = h
        .call(
            "frontend",
            "get_workspace_changes",
            json!({"ws": work.workspace}),
        )
        .await;
    let paths: Vec<&str> = changes["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["src.txt"], "{changes}");
    let proposal = h.propose("frontend", &work, "Agent work").await;
    h.approve(&proposal).await;
    let integrated = h
        .call(
            "review",
            "integrate_proposal",
            json!({"id": proposal, "expected_version": 1}),
        )
        .await;
    let revision = integrated["revision"].as_str().unwrap();
    assert_eq!(file_of(&h, revision, SECRET).await, None);
    assert_eq!(
        file_of(&h, revision, "src.txt").await.as_deref(),
        Some("agent\n")
    );
    assert_eq!(read(&root, SECRET).as_deref(), Some("KEY=2\n"));
}
