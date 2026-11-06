#![cfg(test)]

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness};
use crate::folder_stash::wait_for;
use crate::human_changes::{branch_born, integrate};

async fn fld(h: &Harness, method: Method, action: &str) -> (StatusCode, Value) {
    h.socket(method, &format!("/api/v1/projects/demo/{action}"), None)
        .await
}

async fn fld_ok(h: &Harness, method: Method, action: &str) -> Value {
    let (status, body) = fld(h, method, action).await;
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

fn write(root: &Path, path: &str, text: &str) {
    std::fs::write(root.join(path), text).unwrap();
}

fn stash_names(body: &Value) -> Vec<(String, Vec<String>)> {
    body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["stash"].as_str().unwrap().to_owned(),
                entry["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|file| file.as_str().unwrap().to_owned())
                    .collect(),
            )
        })
        .collect()
}

/// Acceptance 37, dropping stash entries: a command that went wrong left empty files and a truncated plan in the project folder, and the human stashed them to get the folder back to main; later the human stashes real work on top. The human drops the junk entry from the web app, the real work comes back with `fld unstash`, and `fld stash clear` empties the stash. No file of the folder changes on a drop, the stash list hides dropped entries, and each drop is a `stash.discarded` event.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_37_the_human_drops_a_junk_stash_entry_and_clears_the_stash() {
    let h = Harness::start().await;
    branch_born(&h).await;
    let root = PathBuf::from(h.root());
    integrate(&h, "frontend", &[("plan.md", "the plan\n")]).await;
    wait_for(&root, "plan.md", "the plan\n").await;

    // The junk: an emptied plan and empty files.
    write(&root, "plan.md", "");
    for name in ["a", "b", "c"] {
        write(&root, name, "");
    }
    let stashed = fld_ok(&h, Method::POST, "stash").await;
    assert_eq!(stashed["files"], json!(["a", "b", "c", "plan.md"]));
    assert_eq!(
        std::fs::read_to_string(root.join("plan.md")).unwrap(),
        "the plan\n"
    );

    // Real work goes on top.
    write(&root, "notes.md", "my notes\n");
    fld_ok(&h, Method::POST, "stash").await;
    let listed = web_ok(&h, Method::GET, "stash").await;
    assert_eq!(
        stash_names(&listed),
        [
            ("stash@{0}".to_owned(), vec!["notes.md".to_owned()]),
            (
                "stash@{1}".to_owned(),
                ["a", "b", "c", "plan.md"].map(str::to_owned).to_vec()
            ),
        ]
    );

    // The web app drops the junk; the folder does not change.
    write(&root, "draft.md", "unfinished\n");
    let dropped = web_ok(&h, Method::DELETE, "stash/1").await;
    assert_eq!(dropped["entries"], 1);
    assert_eq!(dropped["dropped"][0]["stash"], "stash@{1}");
    assert_eq!(
        std::fs::read_to_string(root.join("draft.md")).unwrap(),
        "unfinished\n"
    );
    assert!(!root.join("a").exists());
    assert_eq!(
        stash_names(&fld_ok(&h, Method::GET, "stash").await),
        [("stash@{0}".to_owned(), vec!["notes.md".to_owned()])]
    );

    // The real work comes back, and the junk never does.
    std::fs::remove_file(root.join("draft.md")).unwrap();
    let unstashed = fld_ok(&h, Method::POST, "unstash").await;
    assert_eq!(unstashed["outcome"], "applied");
    assert_eq!(
        std::fs::read_to_string(root.join("notes.md")).unwrap(),
        "my notes\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("plan.md")).unwrap(),
        "the plan\n"
    );
    let (status, body) = fld(&h, Method::POST, "unstash").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["details"]["reason"], "stash_empty");

    // fld stash clear drops what is left in one step.
    fld_ok(&h, Method::POST, "stash").await;
    write(&root, "notes.md", "second notes\n");
    fld_ok(&h, Method::POST, "stash").await;
    let cleared = fld_ok(&h, Method::DELETE, "stash").await;
    assert_eq!(cleared["entries"], 0);
    assert_eq!(cleared["dropped"].as_array().unwrap().len(), 2);
    assert!(stash_names(&fld_ok(&h, Method::GET, "stash").await).is_empty());
    let discarded: Vec<Value> = h
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "stash.discarded")
        .map(|event| event.data)
        .collect();
    assert_eq!(discarded.len(), 2);
    assert_eq!(discarded[0]["stash"], "stash@{1}");
    assert_eq!(discarded[1], json!({"dropped": 2, "entries": 0}));
}
