#![cfg(test)]

use std::time::Duration;

use crate::common::acceptance::{HUMAN, Harness, SLUG, assert_in_order};
use axum::http::{Method, StatusCode};
use fold_store::testing::eventually;
use serde_json::{Value, json};

/// Long enough for a shell to start and a workspace to be scanned twice on a slow machine.
const CHECKS_WAIT: Duration = Duration::from_secs(30);

const CHECKS: &str = r#"
[[check]]
name  = "endpoint"
run   = "grep -q 'POST /login' src/login.txt"
files = "^src/"

[[check]]
name  = "e2e"
run   = "test -f src/login.txt && test -f README.md"
stage = "integration"
"#;

/// `agent` waits for the end of a check run of `task` and returns that `checks.finished` event.
async fn finished(harness: &Harness, agent: &str, task: &str) -> Value {
    let woke = harness
        .call(
            agent,
            "wait_for_events",
            json!({"types": ["checks.finished"], "task": task, "timeout_s": 60}),
        )
        .await;
    woke["events"].as_array().unwrap().last().unwrap().clone()
}

/// The newest message from fold to `agent`.
async fn last_from_fold(harness: &Harness, agent: &str) -> String {
    sqlx::query_scalar(
        "SELECT m.body FROM messages m
         JOIN message_recipients r ON r.message_id = m.id
         JOIN workers w ON w.id = r.worker_id
         WHERE m.sender_kind = 'fold' AND w.name = ?
         ORDER BY m.number DESC LIMIT 1",
    )
    .bind(agent)
    .fetch_one(harness.handle.store.read())
    .await
    .unwrap()
}

/// Acceptance 18: backend adds a project's checks with its work. `run_checks` fails on the first try and its message names the check and the log; the fixed work's `create_proposal` waits for green proposal checks and fold opens the proposal. Because the proposal adds `fold-checks.toml`, it gets the human review mark, so the review lead's approval alone does not integrate it. Review reads the stored results instead of running the checks, the human integrates, the integration waits in the queue for the integration checks on the merge with main, and main moves only after they pass.
#[tokio::test]
async fn acceptance_18_checks_gate_the_proposal_and_the_integration() {
    let harness = Harness::start().await;
    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let work = harness.start_work("backend", &task).await;
    work.write(&[
        ("fold-checks.toml", CHECKS),
        ("src/login.txt", "GET /login\n"),
    ]);

    let started = harness
        .call("backend", "run_checks", json!({"task": task}))
        .await;
    assert_eq!(started["checks"], "running", "{started}");
    let event = finished(&harness, "backend", &task).await;
    assert_eq!(event["data"]["state"], "failed", "{event}");
    let failure = last_from_fold(&harness, "backend").await;
    assert!(failure.contains("endpoint failed (exit 1)"), "{failure}");
    let run = harness
        .call("backend", "get_check_run", json!({"run": started["run"]}))
        .await;
    assert_eq!(run["failed"], json!(["endpoint"]));

    work.write(&[("src/login.txt", "POST /login\n")]);
    let created = harness
        .call(
            "backend",
            "create_proposal",
            json!({"task": task, "title": "Add the login endpoint", "evidence": "fold ran the checks"}),
        )
        .await;
    assert_eq!(created["checks"], "running", "{created}");
    let event = finished(&harness, "backend", &task).await;
    assert_eq!(event["data"]["state"], "passed", "{event}");
    let listed = harness
        .call("review", "list_proposals", json!({"state": "open"}))
        .await;
    let proposal = listed["proposals"][0]["id"].as_str().unwrap().to_owned();
    assert!(
        last_from_fold(&harness, "backend")
            .await
            .starts_with(&format!("fold made {proposal} v1"))
    );

    let read = harness
        .call("review", "get_proposal", json!({"id": proposal}))
        .await;
    assert_eq!(read["human_review"]["set_by"], "fold");
    assert_eq!(read["checks"][0]["state"], "passed");
    assert_eq!(read["checks"][0]["stage"], "proposal");
    harness.approve(&proposal).await;
    let refused = harness
        .refused(
            "review",
            "integrate_proposal",
            json!({"id": proposal, "expected_version": 1}),
        )
        .await;
    assert_eq!(refused["code"], "human_approval_missing");

    let (code, answer) = harness
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/proposals/{proposal}/integrate"),
            Some(&json!({"expected_version": 1})),
            HUMAN,
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{answer}");
    assert_eq!(answer["result"], "queued");
    eventually(CHECKS_WAIT, async || {
        let head = harness.head().await.number.to_string();
        if head == "r2" { Ok(()) } else { Err(head) }
    })
    .await;
    let integrated = harness
        .call("review", "get_proposal", json!({"id": proposal}))
        .await;
    assert_eq!(integrated["state"], "integrated");
    let stages: Vec<&str> = integrated["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["stage"].as_str().unwrap())
        .collect();
    assert_eq!(stages, ["integration", "proposal"], "{integrated}");
    assert_in_order(
        &harness.events().await,
        &[
            "checks.started",
            "checks.finished",
            "checks.started",
            "checks.finished",
            "proposal.opened",
            "task.human_review_set",
            "review.submitted",
            "checks.started",
            "checks.finished",
            "integration.succeeded",
            "main.advanced",
            "proposal.integrated",
        ],
    );
}
