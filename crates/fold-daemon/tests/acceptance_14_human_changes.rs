#![cfg(test)]

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, WAIT};
use crate::human_changes::{branch_born, git, integrate, notice_with};

const MAIN: &str = "main";

async fn fld_record(h: &Harness) -> Value {
    let (status, body) = h
        .socket(Method::POST, "/api/v1/projects/demo/record", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn needs_you(h: &Harness) -> Vec<Value> {
    let (status, body) = h
        .http(Method::GET, "/api/v1/projects/demo/needs-you", None, HUMAN)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == "human_change")
        .cloned()
        .collect()
}

fn write(root: &Path, path: &str, text: &str) {
    std::fs::write(root.join(path), text).unwrap();
}

fn clean(root: &Path) -> bool {
    // Without Git's optional index lock, so a poll never makes the commit writer's update of the folder fail.
    git(
        root,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--untracked-files=no",
        ],
    )
    .is_empty()
}

/// Acceptance 14, the human's own changes: the human edits files in the project folder while the agents work. Nothing reaches main until the human runs `fld record`; then the change is one revision of the human with no review, the main agent hears of it, and the folder follows main. A change that conflicts with an agent's integration leaves main as it is and waits in Needs you; the main agent prepares the merge in its main workspace and integrates it after the user's OK, and the folder follows main again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_14_the_human_records_changes_of_the_project_folder() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged(MAIN, &["main"]).await;
    let mut inbox = h.register_inbox(MAIN).await;
    let root = PathBuf::from(h.root());

    // The human copies the plan into the folder by hand; no worker sees it before the record.
    write(&root, "PLAN.md", "# Plan\n");
    let task = h.create_task("frontend", "Read the plan").await;
    let work = h.start_work("frontend", &task).await;
    assert!(!work.dir.join("PLAN.md").exists());
    assert_eq!(h.head().await.number.to_string(), "r1");

    let recorded = fld_record(&h).await;
    assert_eq!(recorded["outcome"], "recorded", "{recorded}");
    assert_eq!(recorded["revision"], "r2");
    assert_eq!(recorded["files"], json!(["PLAN.md"]));
    let notice = notice_with(&mut inbox, "recorded r2").await;
    assert!(
        notice.contains("human:ana recorded r2 from the project folder: PLAN.md; touches no open task; tell the user. Call compare_revisions."),
        "{notice}"
    );
    let compared = h
        .call(MAIN, "compare_revisions", json!({"from": "r1", "to": "r2"}))
        .await;
    assert_eq!(compared["files"][0]["path"], "PLAN.md", "{compared}");
    assert!(clean(&root));
    // The open workspace gets the plan through its normal sync.
    let synced = h
        .call("frontend", "sync_workspace", json!({"ws": work.workspace}))
        .await;
    assert_eq!(synced["base"], "r2", "{synced}");
    assert!(work.dir.join("PLAN.md").is_file());

    // The human and an agent change the same line.
    write(&root, "README.md", "# demo by ana\n");
    let agent = integrate(&h, "backend", &[("README.md", "# demo by fold\n")]).await;
    let conflicted = fld_record(&h).await;
    assert_eq!(conflicted["outcome"], "conflicted", "{conflicted}");
    assert_eq!(conflicted["revision"], agent);
    assert_eq!(conflicted["conflicted"][0]["path"], "README.md");
    assert_eq!(h.head().await.number.to_string(), agent);
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        "# demo by ana\n"
    );
    let waiting = needs_you(&h).await;
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    assert_eq!(waiting[0]["paths"], json!(["README.md"]));
    assert_eq!(waiting[0]["screen"], "repository");
    assert_eq!(waiting[0]["commit"], conflicted["commit"]);
    let notice = notice_with(&mut inbox, "conflicts with main").await;
    assert!(notice.contains("Call open_main_workspace."), "{notice}");

    // The main agent explains the conflict, prepares the merge and integrates it after the user's OK.
    let opened = h.call(MAIN, "open_main_workspace", json!({})).await;
    assert_eq!(opened["human_change"]["held"], true, "{opened}");
    let ws = opened["workspace"].as_str().unwrap().to_owned();
    let main_dir = root.join(opened["path"].as_str().unwrap());
    let marked = std::fs::read_to_string(main_dir.join("README.md")).unwrap();
    assert!(marked.contains("<<<<<<< workspace"), "{marked}");
    write(&main_dir, "README.md", "# demo by ana and fold\n");
    let changeset = h
        .call(
            MAIN,
            "record_changes",
            json!({"ws": ws, "message": "merge"}),
        )
        .await["changeset"]
        .as_str()
        .unwrap()
        .to_owned();
    let diff = h
        .call(
            MAIN,
            "compare_revisions",
            json!({"from": agent, "to": changeset}),
        )
        .await;
    assert_eq!(diff["files"][0]["path"], "README.md", "{diff}");
    let submitted = h
        .call(
            MAIN,
            "submit_main_change",
            json!({"changeset": changeset, "message": "Merge Ana's readme"}),
        )
        .await;
    assert_eq!(submitted["result"], "succeeded", "{submitted}");
    assert!(needs_you(&h).await.is_empty());
    eventually(WAIT, async || {
        let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
        if readme == "# demo by ana and fold\n" && clean(&root) {
            Ok(())
        } else {
            Err(readme)
        }
    })
    .await;
    let up_to_date = fld_record(&h).await;
    assert_eq!(up_to_date["outcome"], "up_to_date", "{up_to_date}");
}
