#![cfg(test)]

use std::time::{Duration, Instant};

use crate::common::acceptance::Harness;
use fold_store::testing::eventually;
use serde_json::{Value, json};

/// Long enough for a shell to start and a workspace to be scanned on a slow machine, and much shorter than the check, so only a stopped run ends in it.
const STOP_WAIT: Duration = Duration::from_secs(30);

/// `agent` waits for the end of a check run of `task` and returns that `checks.finished` event. fold's message with the result also ends a wait, so the agent waits again until the event arrives.
async fn finished(harness: &Harness, agent: &str, task: &str) -> Value {
    loop {
        let woke = harness
            .call(
                agent,
                "wait_for_events",
                json!({"types": ["checks.finished"], "task": task, "timeout_s": 60}),
            )
            .await;
        if let Some(event) = woke["events"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|event| event["type"] == "checks.finished")
        {
            return event.clone();
        }
    }
}

/// Acceptance 54: backend calls `run_checks`, whose answer says not to edit until `checks.finished`, then edits a tracked file while the check still runs. fold stops the run at the scan that finds the edit instead of at the check's end: the run fails with `files_changed`, `checks.finished` names the edited path, and fold's message names it too.
#[tokio::test]
async fn acceptance_54_an_edit_during_a_run_stops_it_at_once() {
    let harness = Harness::start().await;
    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let work = harness.start_work("backend", &task).await;
    let marks = tempfile::TempDir::new().unwrap();
    let started_mark = marks.path().join("started");
    let checks = format!(
        "[[check]]\nname = \"slow\"\nrun = \"touch '{}' && sleep 600\"\n",
        started_mark.display()
    );
    work.write(&[
        ("fold-checks.toml", &checks),
        ("src/login.txt", "GET /login\n"),
    ]);

    let started = harness
        .call("backend", "run_checks", json!({"task": task}))
        .await;
    assert_eq!(started["checks"], "running", "{started}");
    assert!(
        started["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("until checks.finished")),
        "{started}"
    );
    eventually(STOP_WAIT, async || {
        if started_mark.exists() {
            Ok(())
        } else {
            Err("the check has not started")
        }
    })
    .await;

    let edited_at = Instant::now();
    work.write(&[("src/login.txt", "POST /login\n")]);
    let event = tokio::time::timeout(STOP_WAIT, finished(&harness, "backend", &task))
        .await
        .expect("the edit did not stop the run");
    assert!(edited_at.elapsed() < STOP_WAIT);
    assert_eq!(event["data"]["state"], "failed", "{event}");
    assert_eq!(event["data"]["reason"], "files_changed", "{event}");
    assert_eq!(event["data"]["path"], "src/login.txt", "{event}");

    let run = harness
        .call("backend", "get_check_run", json!({"run": started["run"]}))
        .await;
    assert_eq!(run["state"], "failed", "{run}");
    assert_eq!(run["reason"], "files_changed", "{run}");
    let message = last_from_fold(&harness).await;
    assert!(
        message.contains("src/login.txt changed during the run"),
        "{message}"
    );
}

/// Acceptance 54, a check that writes a tracked file: fold stops the run at the scan that finds the file instead of at the check's end, and its message tells the worker to ignore what a check writes.
#[tokio::test]
async fn acceptance_54_a_check_that_writes_a_tracked_file_stops_the_run_at_once() {
    let harness = Harness::start().await;
    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let work = harness.start_work("backend", &task).await;
    work.write(&[
        (
            "fold-checks.toml",
            "[[check]]\nname = \"gen\"\nrun = \"echo out > generated.txt && sleep 600\"\n",
        ),
        ("src/login.txt", "GET /login\n"),
    ]);

    let started = harness
        .call("backend", "run_checks", json!({"task": task}))
        .await;
    assert_eq!(started["checks"], "running", "{started}");
    let event = tokio::time::timeout(STOP_WAIT, finished(&harness, "backend", &task))
        .await
        .expect("the written file did not stop the run");
    assert_eq!(event["data"]["reason"], "files_changed", "{event}");
    assert_eq!(event["data"]["path"], "generated.txt", "{event}");
    let message = last_from_fold(&harness).await;
    assert!(
        message.contains("generated.txt changed during the run"),
        "{message}"
    );
    assert!(message.contains(".gitignore"), "{message}");
}

/// Acceptance 54, a sync during a run: the files a sync writes are no edit, so the run ends as `workspace_changed` without a path, never as `files_changed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_54_a_sync_during_a_run_is_not_an_edit() {
    let mut harness = Harness::start().await;
    harness.connect_tagged("main", &["main"]).await;
    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let work = harness.start_work("backend", &task).await;
    let marks = tempfile::TempDir::new().unwrap();
    let started_mark = marks.path().join("started");
    let done_mark = marks.path().join("done");
    let checks = format!(
        "[[check]]\nname = \"slow\"\nrun = \"touch '{}' && while [ ! -e '{}' ]; do sleep 0.1; done\"\n",
        started_mark.display(),
        done_mark.display()
    );
    work.write(&[
        ("fold-checks.toml", &checks),
        ("src/login.txt", "GET /login\n"),
    ]);
    let started = harness
        .call("backend", "run_checks", json!({"task": task}))
        .await;
    assert_eq!(started["checks"], "running", "{started}");
    eventually(STOP_WAIT, async || {
        if started_mark.exists() {
            Ok(())
        } else {
            Err("the check has not started")
        }
    })
    .await;

    let opened = harness.call("main", "open_main_workspace", json!({})).await;
    let main_dir = std::path::Path::new(harness.root()).join(opened["path"].as_str().unwrap());
    std::fs::write(main_dir.join("NOTES.md"), "notes\n").unwrap();
    let recorded = harness
        .call(
            "main",
            "record_changes",
            json!({"ws": opened["workspace"], "message": "Add notes"}),
        )
        .await;
    let submitted = harness
        .call(
            "main",
            "submit_main_change",
            json!({"changeset": recorded["changeset"], "message": "Add notes"}),
        )
        .await;
    assert_eq!(submitted["result"], "succeeded", "{submitted}");
    harness
        .call("backend", "sync_workspace", json!({"ws": work.workspace}))
        .await;
    assert!(work.dir.join("NOTES.md").exists());

    std::fs::write(&done_mark, "").unwrap();
    let event = tokio::time::timeout(STOP_WAIT, finished(&harness, "backend", &task))
        .await
        .expect("the run did not end");
    assert_eq!(event["data"]["state"], "failed", "{event}");
    assert_eq!(event["data"]["reason"], "workspace_changed", "{event}");
    assert!(event["data"].get("path").is_none(), "{event}");
}

/// The newest message from fold to backend.
async fn last_from_fold(harness: &Harness) -> String {
    sqlx::query_scalar(
        "SELECT m.body FROM messages m
         JOIN message_recipients r ON r.message_id = m.id
         JOIN workers w ON w.id = r.worker_id
         WHERE m.sender_kind = 'fold' AND w.name = 'backend'
         ORDER BY m.number DESC LIMIT 1",
    )
    .fetch_one(harness.handle.store.read())
    .await
    .unwrap()
}
