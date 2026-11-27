#![cfg(test)]

use std::path::Path;
use std::time::Duration;

use crate::common::acceptance::Harness;
use fold_store::testing::eventually;
use serde_json::json;

/// Long enough for a shell to start on a slow machine.
const SETUP_WAIT: Duration = Duration::from_secs(30);

const SETUP: &str =
    "[setup]\nrun = \"mkdir -p node_modules && echo ready > node_modules/marker\"\n";

/// Every message from fold to `agent`, oldest first.
async fn from_fold(harness: &Harness, agent: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT m.body FROM messages m
         JOIN message_recipients r ON r.message_id = m.id
         JOIN workers w ON w.id = r.worker_id
         WHERE m.sender_kind = 'fold' AND w.name = ?
         ORDER BY m.number",
    )
    .bind(agent)
    .fetch_all(harness.handle.store.read())
    .await
    .unwrap()
}

/// fold's message that the setup passed in `workspace`, once it came.
async fn setup_passed(harness: &Harness, agent: &str, workspace: &str) {
    let wanted = format!("The setup of fold-checks.toml passed in {workspace}.");
    eventually(SETUP_WAIT, async || {
        let messages = from_fold(harness, agent).await;
        if messages.contains(&wanted) {
            Ok(())
        } else {
            Err(messages)
        }
    })
    .await;
}

/// Acceptance 56: main's `fold-checks.toml` has a `[setup]` that installs into the ignored `node_modules/`. A new task workspace and a new review checkout each answer `setup: running`, and after fold's message that the setup passed, the setup's marker is in the folder. A tree without `[setup]` answers `setup: none` and runs nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_56_fold_runs_the_setup_in_each_new_folder() {
    let mut harness = Harness::start().await;
    harness.connect_tagged("main", &["main"]).await;

    let plain = harness.create_task("backend", "Before the setup").await;
    let created = harness
        .call("backend", "create_workspace", json!({"task": plain}))
        .await;
    assert_eq!(created["setup"], "none", "{created}");

    let opened = harness.call("main", "open_main_workspace", json!({})).await;
    let main_dir = Path::new(harness.root()).join(opened["path"].as_str().unwrap());
    std::fs::write(main_dir.join("fold-checks.toml"), SETUP).unwrap();
    std::fs::write(main_dir.join(".gitignore"), "node_modules/\n").unwrap();
    let recorded = harness
        .call(
            "main",
            "record_changes",
            json!({"ws": opened["workspace"], "message": "Add the setup"}),
        )
        .await;
    let submitted = harness
        .call(
            "main",
            "submit_main_change",
            json!({"changeset": recorded["changeset"], "message": "Add the setup"}),
        )
        .await;
    assert_eq!(submitted["result"], "succeeded", "{submitted}");

    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let created = harness
        .call("backend", "create_workspace", json!({"task": task}))
        .await;
    assert_eq!(created["setup"], "running", "{created}");
    let workspace = created["workspace"].as_str().unwrap().to_owned();
    let dir = Path::new(harness.root()).join(created["path"].as_str().unwrap());
    setup_passed(&harness, "backend", &workspace).await;
    assert_eq!(
        std::fs::read_to_string(dir.join("node_modules/marker")).unwrap(),
        "ready\n"
    );

    std::fs::write(dir.join("src.txt"), "login\n").unwrap();
    let recorded = harness
        .call(
            "backend",
            "record_changes",
            json!({"ws": workspace, "message": "Add the endpoint"}),
        )
        .await;
    assert!(recorded["changeset"].as_str().is_some(), "{recorded}");
    let proposed = harness
        .call(
            "backend",
            "create_proposal",
            json!({"intent": "Make the change the task asks for.", "task": task, "title": "Add the login endpoint"}),
        )
        .await;
    let proposal = proposed["id"].as_str().unwrap().to_owned();
    harness.give_review("review", &proposal).await;
    let checkout = harness
        .call("review", "create_review_checkout", json!({"id": proposal}))
        .await;
    assert_eq!(checkout["setup"], "running", "{checkout}");
    let checkout_id = checkout["workspace"].as_str().unwrap().to_owned();
    setup_passed(&harness, "review", &checkout_id).await;
    let checkout_dir = Path::new(harness.root()).join(checkout["path"].as_str().unwrap());
    assert!(checkout_dir.join("node_modules/marker").exists());

    let again = harness
        .call("review", "create_review_checkout", json!({"id": proposal}))
        .await;
    assert_eq!(
        again["setup"], "none",
        "a checkout fold already made runs no setup again"
    );
}

/// Acceptance 56, a failed setup: fold's message names the workspace, the exit code and the end of the log.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_56_a_failed_setup_is_told_with_its_log() {
    let mut harness = Harness::start().await;
    harness.connect_tagged("main", &["main"]).await;
    let opened = harness.call("main", "open_main_workspace", json!({})).await;
    let main_dir = Path::new(harness.root()).join(opened["path"].as_str().unwrap());
    std::fs::write(
        main_dir.join("fold-checks.toml"),
        "[setup]\nrun = \"echo no lockfile >&2; exit 3\"\n",
    )
    .unwrap();
    let recorded = harness
        .call(
            "main",
            "record_changes",
            json!({"ws": opened["workspace"], "message": "Add the setup"}),
        )
        .await;
    let submitted = harness
        .call(
            "main",
            "submit_main_change",
            json!({"changeset": recorded["changeset"], "message": "Add the setup"}),
        )
        .await;
    assert_eq!(submitted["result"], "succeeded", "{submitted}");

    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let created = harness
        .call("backend", "create_workspace", json!({"task": task}))
        .await;
    assert_eq!(created["setup"], "running", "{created}");
    let workspace = created["workspace"].as_str().unwrap().to_owned();
    let wanted = format!("The setup of fold-checks.toml failed in {workspace} (exit 3).");
    let message = eventually(SETUP_WAIT, async || {
        let messages = from_fold(&harness, "backend").await;
        messages
            .into_iter()
            .find(|message| message.starts_with(&wanted))
            .ok_or("no setup message yet")
    })
    .await;
    assert!(message.ends_with("no lockfile"), "{message}");
}
