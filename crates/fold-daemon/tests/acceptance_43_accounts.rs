#![cfg(test)]

use axum::http::StatusCode;
use fold_core::{ManualClock, WorkerState};
use serde_json::json;

use crate::accounts::{TOKEN_A, TOKEN_B, accounts_with_two, choose, ran_with, until_started};
use crate::project_pause::{LEAD, lead_running};
use crate::worker_resume::{MAIN, until_state, worker};
use crate::workers_support::FakeClaude;

/// A person with two Claude subscriptions gives the project the first one, and every worker fold starts runs on it. When that one runs low, the person chooses the second in the project's Settings: the area lead continues in its own session with the second token, without a pause and without losing its context, and the task worker the lead starts afterwards runs on the second one too.
#[tokio::test]
async fn acceptance_43_a_person_switches_the_projects_subscription_without_a_pause() {
    let clock = ManualClock::new();
    let (_accounts, dir) = accounts_with_two(&clock);
    let mut fake = FakeClaude::new("result");
    fake.launcher.accounts = Some(dir);
    let mut harness = lead_running(&fake, WorkerState::Idle).await;
    let (status, body) = choose(&harness, Some("work-a")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let runs = until_started(&fake, LEAD, 2).await;
    assert!(ran_with(&runs[1], Some(TOKEN_A)), "the lead runs on work-a");
    let session = worker(&harness, LEAD).await.session_id;
    until_state(&harness, LEAD, WorkerState::Idle).await;

    let (status, body) = choose(&harness, Some("work-b")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let runs = until_started(&fake, LEAD, 3).await;
    assert_eq!(
        runs[2].flag("--resume"),
        Some(session.as_str()),
        "the same session"
    );
    assert!(ran_with(&runs[2], Some(TOKEN_B)), "the lead runs on work-b");
    until_state(&harness, LEAD, WorkerState::Idle).await;
    let paused = harness
        .events()
        .await
        .iter()
        .any(|event| event.kind == "project.paused" || event.kind == "worker.stopped");
    assert!(!paused, "nothing paused or stopped");

    harness.connect(LEAD).await;
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Draw the page", "labels": ["ui"]}),
        )
        .await;
    harness
        .call(
            LEAD,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    let worker_runs = eventually_one_run(&fake, &harness).await;
    assert!(
        ran_with(&worker_runs, Some(TOKEN_B)),
        "a new worker starts on work-b"
    );
}

/// The first run of the task worker that the lead started.
async fn eventually_one_run(
    fake: &FakeClaude,
    harness: &crate::common::acceptance::Harness,
) -> crate::workers_support::Run {
    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    let name = listed["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["role"] == "task_worker")
        .and_then(|entry| entry["name"].as_str())
        .unwrap()
        .to_owned();
    until_started(fake, &name, 1).await.remove(0)
}
