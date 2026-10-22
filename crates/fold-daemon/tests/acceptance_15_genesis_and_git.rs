#![cfg(test)]

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, WAIT};
use crate::human_changes::{git, integrate, notice_with};

const MAIN: &str = "main";

fn write(root: &Path, path: &str, text: &str) {
    std::fs::write(root.join(path), text).unwrap();
}

fn read(root: &Path, path: &str) -> String {
    std::fs::read_to_string(root.join(path)).unwrap()
}

async fn post(h: &Harness, route: &str, body: Option<&Value>) -> Value {
    let (status, body) = h
        .socket(
            Method::POST,
            &format!("/api/v1/projects/demo/{route}"),
            body,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// Acceptance 15, a plain folder that turns Git on: `fld init` in a folder that is no Git repository starts the history from the empty genesis `r0` and records the folder's files as `r1`, the human's change, with no Git at all. The agents' work reaches the folder through fold's own checkout, and `fld record` records the human's edits. `fld init --with-git` then turns Git on: the `.gitignore` becomes a revision of the human, every revision after the genesis gets its commit, and from then on a commit the human makes in the folder is recorded at once as the human's change, of which the main agent hears.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_15_a_plain_folder_starts_from_the_genesis_and_turns_git_on() {
    let source = tempfile::TempDir::new().unwrap();
    std::fs::write(source.path().join("README.md"), "# demo\n").unwrap();
    let (mut h, initialized) = Harness::start_initialized(source.path()).await;
    crate::common::roles::give_role(
        &h.handle.store,
        "review",
        fold_core::roles::Role::Reviewer,
        None,
    )
    .await;
    h.lead_the_work_area().await;
    let root = PathBuf::from(h.root());
    h.connect_tagged(MAIN, &["main"]).await;
    let mut inbox = h.register_inbox(MAIN).await;

    // 1. The genesis: r0 is empty, r1 holds the folder's files as the human's change, and there is no Git.
    assert_eq!(initialized.revision.to_string(), "r1");
    assert!(!initialized.git_sync);
    assert!(!root.join(".git").exists());
    let r0 = h.call("review", "get_revision", json!({"r": "r0"})).await;
    assert_eq!(r0["files"], json!([]), "{r0}");
    let history = h.call("review", "get_repository_state", json!({})).await;
    assert_eq!(history["head"], "r1");

    // 2. An agent's work reaches the folder through fold's checkout, and the human's edit stays the human's.
    write(&root, "notes.md", "mine\n");
    let revision = integrate(&h, "frontend", &[("src.txt", "agent\n")]).await;
    eventually(WAIT, async || {
        if root.join("src.txt").is_file() {
            Ok(())
        } else {
            Err("the folder follows main")
        }
    })
    .await;
    assert_eq!(revision, "r2");
    assert_eq!(read(&root, "notes.md"), "mine\n");
    let recorded = post(&h, "record", None).await;
    assert_eq!(recorded["outcome"], "recorded", "{recorded}");
    assert_eq!(recorded["revision"], "r3");
    assert_eq!(recorded["commit"], Value::Null);
    notice_with(&mut inbox, "recorded r3").await;

    // 3. Git turns on: the .gitignore is the human's r4, and every revision after the genesis gets its commit.
    let enabled = post(
        &h,
        "git",
        Some(&json!({"git_name": "Ana Lima", "git_email": "ana@example.com"})),
    )
    .await;
    assert_eq!(enabled["revision"], "r4", "{enabled}");
    assert_eq!(enabled["gitignore"], "r4");
    assert_eq!(enabled["remotes"], json!([]));
    assert_eq!(read(&root, ".gitignore"), ".fold/\n");
    assert_eq!(git(&root, &["rev-list", "--count", "main"]).trim(), "4");
    assert_eq!(git(&root, &["status", "--porcelain"]), "");

    // 4. A commit the human makes is recorded at once as the human's change, and the main agent hears of it.
    write(&root, "plan.md", "the plan\n");
    git(&root, &["add", "plan.md"]);
    git(
        &root,
        &[
            "-c",
            "user.name=Ana Lima",
            "-c",
            "user.email=ana@example.com",
            "commit",
            "--quiet",
            "-m",
            "Add the plan",
        ],
    );
    let commit = git(&root, &["rev-parse", "HEAD"]).trim().to_owned();
    let notice = notice_with(&mut inbox, "recorded r5").await;
    assert!(
        notice.contains("from the project folder: plan.md"),
        "{notice}"
    );
    assert_eq!(h.head().await.number.to_string(), "r5");
    // Main did not move since the human's commit, so r5's Git commit is that commit itself.
    eventually(WAIT, async || {
        let r5 = h.call("review", "get_revision", json!({"r": "r5"})).await;
        if r5["commit"] == commit.as_str() {
            Ok(())
        } else {
            Err(r5)
        }
    })
    .await;
    assert_eq!(git(&root, &["rev-parse", "main"]).trim(), commit);
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
}
