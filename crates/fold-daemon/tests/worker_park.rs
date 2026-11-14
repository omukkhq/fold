#![cfg(test)]

use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::{TaskId, WorkerState};
use fold_daemon::live::{ActivityState, LiveActivity};
use fold_daemon::services::tasks::sweep_leases;
use fold_daemon::services::workspaces::check_stalls;
use fold_store::tasks::claims;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, SLUG, WAIT, Work};
use crate::worker_resume::{MAIN, TASK_WORKER, task_worker_for, until_runs, until_state, worker};
use crate::workers_support::FakeClaude;

const LEASE: Duration = Duration::from_secs(30 * 60);
const STALL: Duration = Duration::from_secs(5 * 60);
const WAIT_FOR: &str = "my go after the T-2 migration";

/// What fold saw of the worker of `work` after one stall decision now.
pub async fn decide(harness: &Harness, work: &Work) -> Option<LiveActivity> {
    check_stalls(&harness.handle, harness.project.now())
        .await
        .unwrap();
    harness
        .handle
        .live
        .get(work.workspace.parse().unwrap())
        .unwrap()
        .view
        .activity
        .clone()
}

pub async fn sweep(harness: &Harness) -> usize {
    sweep_leases(&harness.handle, harness.project.now())
        .await
        .unwrap()
        .len()
}

pub async fn get(harness: &Harness, path: &str) -> Value {
    let (status, body) = harness
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/{path}"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// The attention items of `task` on the task read.
pub async fn attention(harness: &Harness, task: &str) -> Vec<Value> {
    get(harness, &format!("tasks/{task}")).await["attention"]
        .as_array()
        .unwrap()
        .clone()
}

pub async fn needs_you(harness: &Harness, kind: &str) -> Option<Value> {
    get(harness, "needs-you").await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == kind)
        .cloned()
}

pub async fn stale_at(harness: &Harness) -> Option<fold_core::Timestamp> {
    claims::unreleased_for_task(
        harness.project.store().read(),
        TaskId::from_number(1).unwrap(),
    )
    .await
    .unwrap()
    .unwrap()
    .stale_at
}

/// A task worker of `web-lead` that holds `T-1` with its workspace and stopped idle, which `web-lead` parks to wait for its go; its session has ended with its process.
pub async fn parked_worker(fake: &FakeClaude) -> (Harness, Work) {
    let mut harness = task_worker_for(fake, &["Add the report table"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    let parked = harness
        .call(
            "web-lead",
            "stop_worker",
            json!({"name": TASK_WORKER, "wait_for": format!("  {WAIT_FOR} ")}),
        )
        .await;
    assert_eq!(parked["worker"]["parked"]["by"], "web-lead");
    assert_eq!(parked["worker"]["parked"]["wait_for"], WAIT_FOR, "trimmed");
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    harness.disconnect(TASK_WORKER).await;
    (harness, work)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_worker_keeps_its_claim_past_the_lease_and_its_leads_go_resumes_it_in_its_session()
{
    let fake = FakeClaude::new("result");
    let (harness, work) = parked_worker(&fake).await;
    let stopped = worker(&harness, TASK_WORKER).await;
    let park = stopped.park.clone().expect("the row records the park");
    assert_eq!(park.by.as_str(), "web-lead");
    assert_eq!(park.wait_for, WAIT_FOR);
    let parked = harness
        .events()
        .await
        .into_iter()
        .find(|event| event.kind == "worker.parked")
        .expect("worker.parked is stored");
    assert_eq!(parked.actor, "web-lead");
    assert_eq!(
        parked.data,
        json!({"agent": TASK_WORKER, "by": "web-lead", "wait_for": WAIT_FOR})
    );

    let seen = decide(&harness, &work).await.expect("the wait shows");
    assert_eq!(seen.state, ActivityState::Parked);
    assert_eq!(seen.since, park.at);
    assert_eq!(seen.parked_by.as_deref(), Some("web-lead"));
    assert_eq!(seen.wait_for.as_deref(), Some(WAIT_FOR));
    assert_eq!(
        seen.text(TASK_WORKER, harness.project.now()).as_deref(),
        Some("Waits for @web-lead's go")
    );

    for _ in 0..3 {
        harness.project.clock().advance(LEASE).unwrap();
        assert_eq!(
            sweep(&harness).await,
            0,
            "the lead runs, so the lease holds"
        );
        assert_eq!(stale_at(&harness).await, None);
        assert_eq!(
            decide(&harness, &work).await.map(|seen| seen.state),
            Some(ActivityState::Parked),
            "never silent"
        );
    }
    assert!(attention(&harness, "T-1").await.is_empty());
    assert!(needs_you(&harness, "stale_claim").await.is_none());
    assert!(needs_you(&harness, "worker_stalled").await.is_none());
    let events = harness.events().await;
    assert!(
        !events
            .iter()
            .any(|event| event.kind == "worker.stalled" || event.kind == "claim.stale"),
        "nobody is told of a planned wait"
    );

    harness
        .send(
            "web-lead",
            &[TASK_WORKER],
            "Go: the T-2 migration is in main",
        )
        .await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert_eq!(
        runs[1].flag("--resume"),
        Some(stopped.session_id.as_str()),
        "the go resumes it in its own session"
    );
    let resumed = eventually(WAIT, async || {
        let row = worker(&harness, TASK_WORKER).await;
        if row.park.is_none() {
            Ok(row)
        } else {
            Err(row.state)
        }
    })
    .await;
    assert_eq!(resumed.session_id, stopped.session_id);
    assert_ne!(
        decide(&harness, &work).await.map(|seen| seen.state),
        Some(ActivityState::Parked),
        "the start ends the park"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_that_ended_already_is_parked_without_a_stop() {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(&fake, &["Add the report table"], WorkerState::Idle).await;
    harness
        .call("web-lead", "stop_worker", json!({"name": TASK_WORKER}))
        .await;
    let stopped = until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    assert_eq!(stopped.park, None, "a plain stop parks nothing");
    let parked = harness
        .call(
            "web-lead",
            "stop_worker",
            json!({"name": TASK_WORKER, "wait_for": WAIT_FOR}),
        )
        .await;
    assert_eq!(parked["worker"]["state"], "stopped");
    assert_eq!(parked["worker"]["parked"]["by"], "web-lead");
    let row = worker(&harness, TASK_WORKER).await;
    assert_eq!(row.stopped_at, stopped.stopped_at, "no second stop");
    assert!(row.park.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_park_without_words_is_refused_and_changes_nothing() {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(&fake, &["Add the report table"], WorkerState::Idle).await;
    let long = "x".repeat(201);
    for wait_for in ["", "   ", "my go\nafter T-2", long.as_str()] {
        let error = harness
            .refused(
                "web-lead",
                "stop_worker",
                json!({"name": TASK_WORKER, "wait_for": wait_for}),
            )
            .await;
        assert_eq!(error["code"], "invalid_request", "{wait_for:?}");
        assert!(
            error["hint"].as_str().unwrap().contains("wait_for"),
            "{error}"
        );
    }
    let row = worker(&harness, TASK_WORKER).await;
    assert_eq!(row.state, WorkerState::Idle, "the refusal stops nothing");
    assert_eq!(row.park, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn once_the_lead_ends_the_parked_claim_goes_stale_to_the_lead_first_and_names_the_persons_action()
 {
    let fake = FakeClaude::new("result");
    let (mut harness, work) = parked_worker(&fake).await;
    harness
        .call(MAIN, "stop_worker", json!({"name": "web-lead"}))
        .await;
    until_state(&harness, "web-lead", WorkerState::Stopped).await;
    harness.disconnect("web-lead").await;

    harness.project.clock().advance(LEASE).unwrap();
    assert_eq!(sweep(&harness).await, 1, "the lease counts again");
    let stale = harness
        .events()
        .await
        .into_iter()
        .find(|event| event.kind == "claim.stale")
        .unwrap();
    assert_eq!(
        stale.data["told"],
        json!(["web-lead"]),
        "the lead hears first"
    );
    assert!(
        attention(&harness, "T-1").await.is_empty(),
        "the lead gets one stall window"
    );
    assert!(needs_you(&harness, "stale_claim").await.is_none());
    assert_eq!(
        decide(&harness, &work).await.map(|seen| seen.state),
        Some(ActivityState::Parked),
        "the worker still waits for the go"
    );

    harness.project.clock().advance(STALL).unwrap();
    sweep(&harness).await;
    let needs_person = harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "claim.needs_person")
        .count();
    assert_eq!(needs_person, 1);
    assert_eq!(
        attention(&harness, "T-1").await,
        [json!({"kind": "stale_claim", "closable": false, "action": "message_worker"})]
    );
    let item = needs_you(&harness, "stale_claim")
        .await
        .expect("the stale claim waits for a person");
    assert_eq!(item["agent"], TASK_WORKER);
    assert_eq!(item["task"], "T-1");
    assert_eq!(item["screen"], "task");
    assert_eq!(item["action"], "message_worker");
    assert_eq!(
        item["reason"],
        "its process ended; message @web-worker to resume it in its session, or restart it in a new session: `fld worker restart web-worker`"
    );

    harness.connect(TASK_WORKER).await;
    harness.inbox(TASK_WORKER).await;
    assert!(
        attention(&harness, "T-1").await.is_empty(),
        "any fold call of the holder renews the claim and ends the item"
    );
    assert!(needs_you(&harness, "stale_claim").await.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_claim_whose_holder_waits_on_others_never_goes_stale_or_reaches_a_person() {
    let fake = FakeClaude::new("result");
    let (harness, _work) =
        crate::worker_resume::proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")])
            .await;
    for _ in 0..3 {
        harness.project.clock().advance(LEASE + STALL).unwrap();
        assert_eq!(sweep(&harness).await, 0, "the proposal waits for review");
    }
    assert_eq!(stale_at(&harness).await, None);
    assert!(attention(&harness, "T-1").await.is_empty());
    assert!(needs_you(&harness, "stale_claim").await.is_none());

    crate::worker_resume::request_changes(&harness, "P-1", "Use a dialog.").await;
    let back = harness.events().await;
    assert!(back.iter().any(|event| event.kind == "review.submitted"));
    harness.project.clock().advance(LEASE).unwrap();
    sweep(&harness).await;
    let stale: Vec<_> = harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "claim.stale")
        .collect();
    assert!(
        stale.iter().all(|event| event.data["worker"] != "review"),
        "the reviewer gave its verdict, so its claim waits on others"
    );
    assert!(
        stale
            .iter()
            .any(|event| event.data["worker"] == TASK_WORKER),
        "once the proposal is back with its author, the author's silence counts again"
    );
}
