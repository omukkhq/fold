#![cfg(test)]

use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::WorkerState;
use fold_daemon::live::WorkspaceLive;
use fold_daemon::services::workspaces::check_stalls;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, SLUG, Work};
use crate::worker_resume::{TASK_WORKER, proposed_by_a_task_worker, task_worker_for};
use crate::workers_support::FakeClaude;

/// The default `stall_minutes`.
const STALL: Duration = Duration::from_secs(5 * 60);

async fn state(harness: &Harness, work: &Work) -> WorkspaceLive {
    check_stalls(&harness.handle, harness.project.now())
        .await
        .unwrap();
    let workspace = work.workspace.parse().unwrap();
    WorkspaceLive::clone(&harness.handle.live.get(workspace).unwrap())
}

fn advance(harness: &Harness, by: Duration) {
    harness.project.clock().advance(by).unwrap();
}

async fn stalled_events(harness: &Harness) -> usize {
    harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "worker.stalled")
        .count()
}

async fn stalled_item(harness: &Harness) -> Option<Value> {
    let (status, body) = harness
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/needs-you"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "worker_stalled")
        .cloned()
}

/// Over several stall windows, decided after each, the worker of `work` shows no stall, fold tells nobody and Needs you has no stall item.
async fn never_stalls(harness: &Harness, work: &Work) {
    for _ in 0..4 {
        advance(harness, STALL);
        let view = state(harness, work).await.view;
        assert!(
            view.activity.as_ref().is_none_or(|seen| !seen.is_stall()),
            "{:?}",
            view.activity
        );
        assert!(!view.stalled);
    }
    assert_eq!(stalled_events(harness).await, 0, "fold tells nobody");
    assert!(stalled_item(harness).await.is_none());
}

/// Acceptance 65, a worker that waits for others never stalls: a task worker sent its proposal and fold stopped it. It changed a file after the version, and the integration found a conflict that a reviewer fixes, yet every claim it holds waits for review, as its lease sees it, so over several stall windows nobody hears of a stall and Needs you stays empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_65_a_stopped_worker_whose_tasks_are_in_review_never_stalls() {
    let fake = FakeClaude::new("result");
    let (harness, work) = proposed_by_a_task_worker(&fake, &[("page.txt", "page\n")]).await;
    work.write(&[("notes.txt", "written after the version\n")]);
    harness
        .handle
        .live
        .rescan(work.workspace.parse().unwrap())
        .await
        .unwrap();
    never_stalls(&harness, &work).await;

    fold_store::testing::execute(&harness.handle.store, "UPDATE proposals SET conflicted = 1")
        .await;
    never_stalls(&harness, &work).await;
}

/// Acceptance 65, a running worker whose only task is in review: its process still runs after the proposal, and it never stalls while the proposal waits for review.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_65_a_running_worker_whose_tasks_are_in_review_never_stalls() {
    let fake = FakeClaude::new("bash_open");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Working).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    work.write(&[("page.txt", "page\n")]);
    assert_eq!(
        harness.propose(TASK_WORKER, &work, "Draw the page").await,
        "P-1"
    );
    work.write(&[("notes.txt", "written after the version\n")]);
    never_stalls(&harness, &work).await;
}

/// Acceptance 65, a worker that waits for its own ask to a person never stalls, however long the person takes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_65_a_worker_that_waits_for_its_own_ask_never_stalls() {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    fold_daemon::services::projects::ensure_human(
        harness.project.registry(),
        harness.project.clock(),
        &fold_core::HumanName::parse(HUMAN).unwrap(),
    )
    .await
    .unwrap();
    harness
        .send_with(
            TASK_WORKER,
            json!({"to": ["humans"], "body": "Which colour?", "task": "T-1", "asks_human": true}),
        )
        .await;
    never_stalls(&harness, &work).await;
}

/// Acceptance 65, a real stall is told once: a worker with its task in progress goes silent, and its lead hears of it once over several stall windows. Only after the worker changed its workspace again and went silent for one more window does fold tell of the new stall.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_65_a_real_stall_is_told_once_and_again_only_after_new_activity() {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    for _ in 0..4 {
        advance(&harness, STALL);
        state(&harness, &work).await;
    }
    assert_eq!(stalled_events(&harness).await, 1, "told once");

    work.write(&[("page.txt", "more work\n")]);
    let workspace = work.workspace.parse().unwrap();
    harness.handle.live.rescan(workspace).await.unwrap();
    let view = state(&harness, &work).await.view;
    assert!(view.activity.as_ref().is_none_or(|seen| !seen.is_stall()));
    assert_eq!(stalled_events(&harness).await, 1);

    for _ in 0..4 {
        advance(&harness, STALL);
        state(&harness, &work).await;
    }
    assert_eq!(
        stalled_events(&harness).await,
        2,
        "the new stall is told once"
    );
}
