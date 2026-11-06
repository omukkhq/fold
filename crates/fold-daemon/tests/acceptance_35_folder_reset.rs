#![cfg(test)]

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, WAIT};
use crate::folder_stash::wait_for;
use crate::human_changes::{branch_born, integrate, notice_with};

async fn fld_ok(h: &Harness, method: Method, action: &str) -> Value {
    let (status, body) = h
        .socket(method, &format!("/api/v1/projects/demo/{action}"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{action}: {body}");
    body
}

async fn web_ok(h: &Harness, method: Method, action: &str) -> Value {
    let (status, body) = h
        .http(
            method,
            &format!("/api/v1/projects/demo/{action}"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{action}: {body}");
    body
}

async fn needs_you_kinds(h: &Harness) -> Vec<String> {
    web_ok(h, Method::GET, "needs-you").await["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["kind"].as_str().unwrap().to_owned())
        .collect()
}

fn write(root: &Path, path: &str, text: &str) {
    std::fs::write(root.join(path), text).unwrap();
}

/// Acceptance 35, recovering the project folder without the main agent: a command that went wrong truncated `plan.md` and left empty files in the project folder, the human recorded them by mistake, and the record conflicts with main, so fold holds the folder for the main agent, which opens its main workspace for the merge. From the web app the human resets the folder: fold stashes the junk as `stash@{0}`, the folder matches main again, the waiting change is dropped with its merge workspace abandoned, and the main agent hears that fold did it. The human then drops the junk entry with `fld stash drop`, and nothing waits in Needs you.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_35_the_human_resets_the_folder_and_drops_the_held_change() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged("main", &["main"]).await;
    let mut inbox = h.register_inbox("main").await;
    let root = PathBuf::from(h.root());
    integrate(&h, "frontend", &[("plan.md", "the plan\n")]).await;
    wait_for(&root, "plan.md", "the plan\n").await;

    // The junk, recorded by mistake while main changed the plan.
    write(&root, "plan.md", "");
    for name in ["x1", "x2"] {
        write(&root, name, "");
    }
    let head = integrate(&h, "frontend", &[("plan.md", "the plan, version 2\n")]).await;
    let recorded = fld_ok(&h, Method::POST, "record").await;
    assert_eq!(recorded["outcome"], "conflicted", "{recorded}");
    let opened = h.call("main", "open_main_workspace", json!({})).await;
    let ws = opened["workspace"].as_str().unwrap().to_owned();
    eventually(WAIT, async || {
        let kinds = needs_you_kinds(&h).await;
        if kinds.contains(&"human_change".to_owned()) {
            Ok(())
        } else {
            Err(kinds)
        }
    })
    .await;

    // The web app resets the folder.
    let reset = web_ok(&h, Method::POST, "folder/reset").await;
    assert_eq!(reset["stashed"], true);
    assert_eq!(reset["at_head"], true);
    assert_eq!(reset["revision"], head.as_str());
    assert_eq!(reset["files"], json!(["plan.md", "x1", "x2"]));
    assert_eq!(reset["dropped_change"]["workspace"], ws.as_str());
    assert_eq!(
        std::fs::read_to_string(root.join("plan.md")).unwrap(),
        "the plan, version 2\n"
    );
    assert!(!root.join("x1").exists() && !root.join("x2").exists());
    let notice = notice_with(&mut inbox, "took back").await;
    assert!(notice.contains(&format!("fold abandoned {ws}")), "{notice}");
    let state: String = sqlx::query_scalar("SELECT state FROM workspaces WHERE kind = 'main'")
        .fetch_one(h.handle.store.read())
        .await
        .unwrap();
    assert_eq!(state, "abandoned");

    // The junk goes for good.
    let listed = fld_ok(&h, Method::GET, "stash").await;
    assert_eq!(listed["entries"].as_array().unwrap().len(), 1);
    let dropped = fld_ok(&h, Method::DELETE, "stash/0").await;
    assert_eq!(dropped["entries"], 0);
    assert!(needs_you_kinds(&h).await.is_empty());
    crate::common::acceptance::assert_in_order(
        &h.events().await,
        &[
            "human_change.conflicted",
            "stash.pushed",
            "human_change.withdrawn",
            "workspace.abandoned",
            "stash.discarded",
        ],
    );
}
