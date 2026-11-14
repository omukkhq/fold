#![cfg(test)]

use std::time::Duration;

use fold_core::WorkerState;
use fold_daemon::live::ActivityState;
use serde_json::json;

use crate::common::acceptance::Harness;
use crate::worker_park::{attention, decide, get, needs_you, sweep};
use crate::worker_resume::{MAIN, TASK_WORKER, task_worker_for, until_runs, until_state, worker};
use crate::workers_support::FakeClaude;

const LEASE: Duration = Duration::from_secs(30 * 60);
const STALL: Duration = Duration::from_secs(5 * 60);

/// The names of the agents fold told with each stored event of `kind`.
async fn told(harness: &Harness, kind: &str) -> Vec<serde_json::Value> {
    harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == kind)
        .map(|event| event.data["told"].clone())
        .collect()
}

/// Acceptance 41, a parked worker waits for its lead's go: the web lead lets migrations into main one at a time. Two task workers finish their code; the lead parks each with stop_worker and wait_for until its go. Hours pass. fold never calls them stalled and never lets their claims go stale, the board and the task show "Waits for @web-lead's go" in the neutral tone, and nothing waits for a person. The lead's go to the first worker resumes it in its own session, and the second one still waits. When the lead itself ends, the second worker's claim goes stale: fold tells the lead first, and only one stall window later Needs you and the card show the stale claim in gold with what the person does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_41_a_parked_worker_waits_for_its_leads_go_and_nothing_waits_for_a_person() {
    let fake = FakeClaude::new("result");
    let mut harness = task_worker_for(&fake, &["Add the report table"], WorkerState::Idle).await;
    let first = harness.start_work(TASK_WORKER, "T-1").await;
    let created = harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Add the audit table", "labels": ["ui"]}),
        )
        .await;
    assert_eq!(created["id"], "T-2");
    let started = harness
        .call(
            "web-lead",
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-2"]}),
        )
        .await;
    let second_name = started["worker"]["name"].as_str().unwrap().to_owned();
    until_state(&harness, &second_name, WorkerState::Idle).await;
    harness.connect(&second_name).await;
    let second = harness.start_work(&second_name, "T-2").await;

    for (name, wait_for) in [
        (TASK_WORKER, "my go after the T-288 migration"),
        (second_name.as_str(), "my go after the T-1 migration"),
    ] {
        let parked = harness
            .call(
                "web-lead",
                "stop_worker",
                json!({"name": name, "wait_for": wait_for}),
            )
            .await;
        assert_eq!(parked["worker"]["parked"]["wait_for"], wait_for);
        until_state(&harness, name, WorkerState::Stopped).await;
        harness.disconnect(name).await;
    }
    let session = worker(&harness, TASK_WORKER).await.session_id;

    for _ in 0..6 {
        harness.project.clock().advance(LEASE).unwrap();
        assert_eq!(sweep(&harness).await, 0);
        for work in [&first, &second] {
            let seen = decide(&harness, work).await.expect("the wait shows");
            assert_eq!(seen.state, ActivityState::Parked);
            assert_eq!(seen.parked_by.as_deref(), Some("web-lead"));
        }
    }
    assert!(told(&harness, "worker.stalled").await.is_empty());
    assert!(told(&harness, "claim.stale").await.is_empty());
    for task in ["T-1", "T-2"] {
        assert!(attention(&harness, task).await.is_empty(), "{task}");
    }
    assert!(
        get(&harness, "needs-you").await["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let board = get(&harness, &format!("workspaces/{}", first.workspace)).await;
    assert_eq!(board["live"]["activity"]["state"], "parked");
    assert_eq!(
        board["live"]["activity"]["wait_for"],
        "my go after the T-288 migration"
    );
    let workers = get(&harness, "workers").await;
    let row = workers["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == TASK_WORKER)
        .unwrap()
        .clone();
    assert_eq!(row["parked"]["by"], "web-lead");

    harness
        .send(
            "web-lead",
            &[TASK_WORKER],
            "Go: the T-288 migration is in main",
        )
        .await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(session.as_str()));
    assert_eq!(
        decide(&harness, &second).await.map(|seen| seen.state),
        Some(ActivityState::Parked),
        "the second worker still waits for its go"
    );

    harness
        .call(MAIN, "stop_worker", json!({"name": "web-lead"}))
        .await;
    until_state(&harness, "web-lead", WorkerState::Stopped).await;
    harness.disconnect("web-lead").await;
    // The first worker works again after its go, and each of its fold calls renews its claim.
    harness
        .project
        .clock()
        .advance(LEASE - Duration::from_secs(60))
        .unwrap();
    harness.connect(TASK_WORKER).await;
    harness.inbox(TASK_WORKER).await;
    harness
        .project
        .clock()
        .advance(Duration::from_secs(60))
        .unwrap();
    assert_eq!(sweep(&harness).await, 1, "only the second claim lapses");
    assert_eq!(told(&harness, "claim.stale").await, [json!(["web-lead"])]);
    assert!(
        attention(&harness, "T-2").await.is_empty(),
        "the lead first"
    );
    harness.project.clock().advance(STALL).unwrap();
    sweep(&harness).await;
    assert_eq!(
        attention(&harness, "T-2").await,
        [json!({"kind": "stale_claim", "closable": false, "action": "message_worker"})]
    );
    let item = needs_you(&harness, "stale_claim").await.unwrap();
    assert_eq!(item["agent"], second_name.as_str());
    assert_eq!(item["action"], "message_worker");
}
