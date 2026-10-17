#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::Command;

use axum::http::{Method, StatusCode};
use fold_core::codes;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, WAIT, assert_in_order};

const MAIN: &str = "main";

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8(output.stdout).unwrap()
}

/// How many revisions after `r1` have their Git commit.
async fn committed(h: &Harness) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM revision_commits c JOIN revisions r ON r.id = c.revision_id WHERE r.number > 1",
    )
    .fetch_one(h.handle.store.read())
    .await
    .unwrap()
}

/// The main agent opens its main workspace and gets its id and directory.
async fn open(h: &Harness, reused: bool) -> (String, PathBuf) {
    let opened = h.call(MAIN, "open_main_workspace", json!({})).await;
    assert_eq!(opened["reused"], reused, "{opened}");
    let dir = Path::new(h.root()).join(opened["path"].as_str().unwrap());
    (opened["workspace"].as_str().unwrap().to_owned(), dir)
}

async fn record(h: &Harness, ws: &str, message: &str) -> String {
    let recorded = h
        .call(
            MAIN,
            "record_changes",
            json!({"ws": ws, "message": message}),
        )
        .await;
    recorded["changeset"].as_str().unwrap().to_owned()
}

fn paths(compared: &Value) -> Vec<&str> {
    compared["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect()
}

/// Acceptance 12, a direct change: the user asks the main agent to add the agents' rules without a task. The main agent opens the main workspace, records the files and shows the diff; only it may open the workspace and submit. After the user's OK the change reaches main as one revision and one Git commit with the main agent's message, the activity names it a direct change and the history shows it with origin `direct`. A second change that conflicts with an integrated proposal writes nothing; after a sync it goes in too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_12_the_main_agent_changes_main_directly_after_the_users_ok() {
    let mut h = Harness::start().await;
    h.connect_tagged(MAIN, &["main"]).await;

    // Nobody but the main agent opens the main workspace.
    let refused = h
        .refused("frontend", "open_main_workspace", json!({}))
        .await;
    assert_eq!(refused["code"], codes::ORCHESTRATOR_REQUIRED);
    let refused = h.refused("review", "open_main_workspace", json!({})).await;
    assert_eq!(refused["code"], codes::MAIN_AGENT_REQUIRED);

    let (ws, dir) = open(&h, false).await;
    assert!(dir.ends_with(".fold/ws/main"), "{}", dir.display());
    let (again, _) = open(&h, true).await;
    assert_eq!(again, ws);
    let read = h.call(MAIN, "get_workspace", json!({"ws": ws})).await;
    assert_eq!(read["task"], Value::Null);
    assert_eq!(read["state"], "active");

    std::fs::write(dir.join("AGENTS.md"), "# Rules\n\nRun the tests.\n").unwrap();
    let changeset = record(&h, &ws, "Add the agent rules").await;
    // The diff the main agent shows the user.
    let compared = h
        .call(
            MAIN,
            "compare_revisions",
            json!({"from": "r1", "to": changeset}),
        )
        .await;
    assert_eq!(paths(&compared), ["AGENTS.md"]);
    // The web lists the main workspace with the task workspaces, with its working changes and its diff.
    let (status, listed) = h
        .http(Method::GET, "/api/v1/projects/demo/workspaces", None, HUMAN)
        .await;
    assert_eq!(status, StatusCode::OK);
    let entry = listed["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == ws.as_str())
        .unwrap();
    assert_eq!(entry["kind"], "main");
    assert_eq!(entry["task"], Value::Null);
    assert_eq!(entry["task_title"], Value::Null);
    assert_eq!(entry["owner"], MAIN);
    assert_eq!(entry["live"]["task"], Value::Null);
    let (status, changes) = h
        .http(
            Method::GET,
            &format!("/api/v1/projects/demo/workspaces/{ws}/changes"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changes}");
    let changed: Vec<&str> = changes["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert_eq!(changed, ["AGENTS.md"]);
    let (status, diff) = h
        .http(
            Method::GET,
            &format!("/api/v1/projects/demo/diff?from=r1&to={ws}&path=AGENTS.md"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{diff}");

    for (agent, code) in [
        ("frontend", codes::ORCHESTRATOR_REQUIRED),
        ("review", codes::MAIN_AGENT_REQUIRED),
    ] {
        let refused = h
            .refused(
                agent,
                "submit_main_change",
                json!({"changeset": changeset, "message": "Add the agent rules"}),
            )
            .await;
        assert_eq!(refused["code"], code, "{agent}");
    }

    // The user said OK.
    let message = "Add the agent rules\n\nThe user asked for the rules of the parent folder.";
    let submitted = h
        .call(
            MAIN,
            "submit_main_change",
            json!({"changeset": changeset, "message": message}),
        )
        .await;
    assert_eq!(submitted["result"], "succeeded");
    assert_eq!(submitted["revision"], "r2");
    assert_eq!(h.head().await.number.to_string(), "r2");
    let revision = h.call(MAIN, "get_revision", json!({"r": "r2"})).await;
    assert_eq!(paths(&revision), ["AGENTS.md"]);
    assert_eq!(revision["task"], Value::Null);
    let read = h.call(MAIN, "get_workspace", json!({"ws": ws})).await;
    assert_eq!(read["state"], "archived");

    eventually(WAIT, async || match committed(&h).await {
        1 => Ok(()),
        count => Err(count),
    })
    .await;
    let git_dir = h.project.paths().git_dir();
    let body = git(&git_dir, &["log", "-1", "--format=%B", "refs/fold/head"]);
    assert!(body.starts_with(&format!("{message}\n\n")), "{body}");
    assert!(
        body.contains(&format!("Fold-Changeset: {changeset}\n")),
        "{body}"
    );
    assert!(body.contains("Integrated-by: main\n"), "{body}");
    assert_eq!(
        git(&git_dir, &["log", "-1", "--format=%an", "refs/fold/head"]),
        "main\n"
    );

    let events = h.events().await;
    let direct: Vec<&Value> = events
        .iter()
        .filter(|event| event.kind == "integration.succeeded")
        .map(|event| &event.data)
        .collect();
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0]["kind"], "direct");
    assert_eq!(direct[0]["changeset"], changeset.as_str());
    assert_eq!(direct[0]["approval"], "user_in_conversation");
    assert_in_order(
        &events,
        &[
            "workspace.created",
            "changeset.recorded",
            "integration.succeeded",
            "main.advanced",
            "workspace.archived",
        ],
    );
    let (status, history) = h
        .http(Method::GET, "/api/v1/projects/demo/revisions", None, HUMAN)
        .await;
    assert_eq!(status, StatusCode::OK);
    let newest = &history["revisions"][0];
    assert_eq!(newest["id"], "r2");
    assert_eq!(newest["origin"], "direct");
    assert_eq!(newest["title"], "Add the agent rules");
    assert_eq!(newest["author"], "main");

    // A second change meets a proposal that changed the same file first.
    let (ws, dir) = open(&h, false).await;
    assert!(dir.ends_with(".fold/ws/main-2"), "{}", dir.display());
    std::fs::write(dir.join("README.md"), "# demo by main\n").unwrap();
    let first = record(&h, &ws, "Name the project").await;
    let task = h.create_task("frontend", "Readme").await;
    let work = h.start_work("frontend", &task).await;
    work.write(&[("README.md", "# demo by frontend\n")]);
    let proposal = h.propose("frontend", &work, "Describe the demo").await;
    h.approve(&proposal).await;
    h.call(
        "review",
        "integrate_proposal",
        json!({"id": proposal, "expected_version": 1}),
    )
    .await;
    assert_eq!(h.head().await.number.to_string(), "r3");

    let conflicted = h
        .refused(
            MAIN,
            "submit_main_change",
            json!({"changeset": first, "message": "Name the project"}),
        )
        .await;
    assert_eq!(conflicted["code"], codes::INTEGRATION_CONFLICTED);
    assert_eq!(conflicted["details"]["files"][0]["path"], "README.md");
    assert_eq!(h.head().await.number.to_string(), "r3");
    let read = h.call(MAIN, "get_workspace", json!({"ws": ws})).await;
    assert_eq!(read["state"], "active");

    let synced = h.call(MAIN, "sync_workspace", json!({"ws": ws})).await;
    assert_eq!(synced["conflicted"], json!(["README.md"]));
    std::fs::write(dir.join("README.md"), "# demo by main and frontend\n").unwrap();
    let resolved = record(&h, &ws, "Name the project").await;
    let submitted = h
        .call(
            MAIN,
            "submit_main_change",
            json!({"changeset": resolved, "message": "Name the project"}),
        )
        .await;
    assert_eq!(submitted["revision"], "r4");
}
