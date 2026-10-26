#![cfg(test)]

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, WAIT};
use crate::folder_stash::{stash_as_ana, unstash_as_ana, wait_for};
use crate::human_changes::{branch_born, git, integrate, record_as_ana};

const ROUTE: &str = "/api/v1/projects/demo/folder";

fn root(h: &Harness) -> PathBuf {
    PathBuf::from(h.root())
}

fn write(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

async fn status(h: &Harness) -> Value {
    let (status, body) = h.socket(Method::GET, ROUTE, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// The checkout record row, which only a follow, a record, a stash or an unstash writes.
async fn written(h: &Harness) -> Option<fold_store::workspaces::Checkout> {
    fold_store::workspaces::checkout(h.handle.store.read())
        .await
        .unwrap()
}

fn kinds(body: &Value) -> Vec<&str> {
    body["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|blocker| blocker["kind"].as_str().unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_clean_folder_is_at_main_with_nothing_to_record() {
    let h = Harness::start_plain().await;
    assert_eq!(
        status(&h).await,
        json!({
            "head": "r1", "folder": "r1", "at_main": true, "behind_because": null, "branch": null,
            "changes": [], "skipped": [], "ignored_tracked": [], "blockers": [], "stash": [],
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_changes_are_what_a_record_takes_and_a_path_main_changed_holds_the_folder() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    integrate(&h, "frontend", &[("docs.md", "a\n"), ("old.md", "old\n")]).await;
    wait_for(&dir, "old.md", "old\n").await;
    write(&dir, "docs.md", "mine\n");
    write(&dir, "notes/new.md", "new\n");
    std::fs::remove_file(dir.join("old.md")).unwrap();
    let revision = integrate(&h, "frontend", &[("docs.md", "main\n")]).await;
    // The Git job's follow after the integration is done once it noted the held path.
    eventually(WAIT, async || match h.handle.git_attention.folder_held() {
        Some((_, held)) if held == ["docs.md"] => Ok(()),
        other => Err(other),
    })
    .await;
    let before = written(&h).await;

    let body = status(&h).await;
    assert_eq!(body["head"], revision.as_str());
    assert_eq!(body["at_main"], false);
    assert_eq!(body["behind_because"], "folder_held");
    assert_eq!(
        body["changes"],
        json!([
            {"path": "docs.md", "kind": "modified", "entry": "file", "held": true},
            {"path": "notes/new.md", "kind": "added", "entry": "file", "held": false},
            {"path": "old.md", "kind": "deleted", "entry": "file", "held": false},
        ])
    );
    assert_eq!(
        body["blockers"],
        json!([{"kind": "folder_held", "paths": ["docs.md"], "next": "run `fld record`, or take the changes back"}])
    );
    // A read records nothing, follows nothing and leaves the files as the human left them.
    assert_eq!(written(&h).await, before);
    assert_eq!(
        std::fs::read_to_string(dir.join("docs.md")).unwrap(),
        "mine\n"
    );

    // The record conflicts and the change waits for the main agent, which stands for the held folder.
    record_as_ana(&h).await.unwrap();
    let body = status(&h).await;
    assert_eq!(kinds(&body), ["human_change"]);
    assert_eq!(body["blockers"][0]["paths"], json!(["docs.md"]));
    assert!(
        body["blockers"][0]["next"]
            .as_str()
            .unwrap()
            .contains("main agent")
    );
    assert_eq!(body["changes"][0]["held"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stash_is_listed_newest_first_and_a_conflicted_unstash_holds_the_folder() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    write(&dir, "plan.md", "plan\n");
    stash_as_ana(&h).await.unwrap();
    write(&dir, "README.md", "# mine\n");
    stash_as_ana(&h).await.unwrap();
    let body = status(&h).await;
    assert_eq!(body["at_main"], true);
    assert_eq!(body["changes"], json!([]));
    let stash = body["stash"].as_array().unwrap();
    assert_eq!(stash.len(), 2);
    assert_eq!(stash[0]["stash"], "stash@{0}");
    assert_eq!(stash[0]["state"], "stashed");
    assert_eq!(stash[0]["files"], 1);
    assert_eq!(stash[0]["next"], true);
    assert_eq!(stash[0]["actor"], "human:ana");
    assert_eq!(stash[1]["stash"], "stash@{1}");
    assert_eq!(stash[1]["next"], false);

    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    wait_for(&dir, "README.md", "# fold\n").await;
    unstash_as_ana(&h).await.unwrap();
    let body = status(&h).await;
    assert_eq!(kinds(&body), ["unstash_conflicted"]);
    assert_eq!(body["blockers"][0]["paths"], json!(["README.md"]));
    assert_eq!(
        body["blockers"][0]["next"],
        "fix the conflict markers in these files, then run `fld record`"
    );
    assert_eq!(body["stash"][0]["state"], "conflicted");
    assert_eq!(body["stash"][0]["next"], false);
    assert_eq!(
        body["changes"],
        json!([{"path": "README.md", "kind": "modified", "entry": "file", "held": false}])
    );
    // While the markers wait, main's next change does not reach the folder.
    integrate(&h, "frontend", &[("src/new.rs", "fn new() {}\n")]).await;
    let body = status(&h).await;
    assert_eq!(body["at_main"], false);
    assert_eq!(body["behind_because"], "unstash_conflicted");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_git_sync_a_branch_fold_did_not_move_names_the_command_that_brings_it_back() {
    let h = Harness::start().await;
    branch_born(&h).await;
    let dir = root(&h);
    let body = status(&h).await;
    assert_eq!(body["branch"], "main");
    assert_eq!(body["blockers"], json!([]));
    git(&dir, &["checkout", "--quiet", "-b", "mine"]);
    let body = status(&h).await;
    assert_eq!(kinds(&body), ["head_moved"]);
    assert_eq!(
        body["blockers"][0]["next"],
        "run `git switch main` in the project folder"
    );
    git(&dir, &["switch", "--quiet", "main"]);
    eventually(WAIT, async || {
        let body = status(&h).await;
        if body["blockers"] == json!([]) {
            Ok(())
        } else {
            Err(body)
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_folder_route_is_socket_only() {
    let h = Harness::start_plain().await;
    let (status, _) = h.http(Method::GET, ROUTE, None, "ana").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
