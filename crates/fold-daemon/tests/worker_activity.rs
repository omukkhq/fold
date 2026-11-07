#![cfg(test)]

use std::time::Duration;

use fold_core::WorkerState;
use fold_daemon::live::{ActivityState, LiveActivity, LiveCall, WorkspaceLive};
use fold_daemon::services::workspaces::check_stalls;
use fold_store::testing::eventually;
use serde_json::json;

use crate::common::acceptance::{HUMAN, Harness, WAIT, Work};
use crate::worker_resume::{TASK_WORKER, task_worker_for, until_state};
use crate::workers_support::FakeClaude;

const MINUTE: Duration = Duration::from_secs(60);

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

async fn stalled_events(harness: &Harness) -> Vec<serde_json::Value> {
    harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "worker.stalled")
        .map(|event| event.data)
        .collect()
}

/// A task worker whose turn ends: with no call open and no output for `stall_minutes` it is silent, fold tells its lead once, and the stall waits for a person one window later.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_without_output_for_the_window_is_silent_and_its_lead_hears_first() {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    let start = harness.project.now();

    advance(&harness, STALL - Duration::from_millis(1));
    assert_eq!(state(&harness, &work).await.view.activity, None);

    advance(&harness, Duration::from_millis(1));
    let silent = state(&harness, &work).await.view;
    let seen = silent.activity.expect("a silent worker");
    assert_eq!(seen.state, ActivityState::Silent);
    assert_eq!(seen.call, None);
    assert!(seen.since >= start, "counted from the newest step");
    assert!(seen.supervised);
    assert_eq!(seen.told, ["web-lead"]);
    assert!(!silent.stalled, "the lead acts first");
    let told = stalled_events(&harness).await;
    assert_eq!(told.len(), 1);
    assert_eq!(told[0]["agent"], TASK_WORKER);
    assert_eq!(told[0]["reason"], "silent");
    assert_eq!(told[0]["told"], json!(["web-lead"]));

    advance(&harness, STALL - Duration::from_millis(1));
    assert!(!state(&harness, &work).await.view.stalled);
    advance(&harness, Duration::from_millis(1));
    assert!(
        state(&harness, &work).await.view.stalled,
        "one window after the notice"
    );
    assert_eq!(stalled_events(&harness).await.len(), 1, "told once");
}

/// A Bash call without a result is work: the worker never stalls by silence while it runs, shows as working after a minute, and its call counts as long at twice Claude Code's ten-minute limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_open_command_is_work_until_it_runs_twice_claude_codes_limit() {
    let fake = FakeClaude::new("bash_open");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Working).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;

    advance(&harness, 2 * MINUTE);
    let working = state(&harness, &work).await.view;
    assert_eq!(
        working
            .activity
            .as_ref()
            .map(|seen| (seen.state, seen.call)),
        Some((ActivityState::Working, Some(LiveCall::Command)))
    );
    assert!(
        working
            .activity
            .as_ref()
            .is_some_and(|seen| seen.told.is_empty())
    );
    assert!(!working.stalled);

    advance(&harness, 17 * MINUTE);
    let still: Option<LiveActivity> = state(&harness, &work).await.view.activity;
    assert_eq!(
        still.map(|seen| seen.state),
        Some(ActivityState::Working),
        "19 min"
    );
    assert!(stalled_events(&harness).await.is_empty());

    advance(&harness, MINUTE);
    let long = state(&harness, &work).await.view;
    let seen = long.activity.expect("a long call");
    assert_eq!(
        (seen.state, seen.call),
        (ActivityState::LongCall, Some(LiveCall::Command))
    );
    assert!(!long.stalled);
    let told = stalled_events(&harness).await;
    assert_eq!(told[0]["reason"], "long_call");
    assert_eq!(told[0]["call"], "command");
    assert_eq!(told[0]["minutes"], 20);
}

/// A process that ended without a report is stopped at once, and its lead hears of it; a session of the agent that fold does not run keeps it from counting as stopped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_process_that_ended_without_a_report_is_stopped_at_once() {
    let fake = FakeClaude::new("result");
    let mut harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    harness
        .call("web-lead", "stop_worker", json!({"name": TASK_WORKER}))
        .await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    assert_eq!(
        state(&harness, &work).await.view.activity,
        None,
        "the harness's own session of the worker still acts as it"
    );

    harness.disconnect(TASK_WORKER).await;
    let stopped = eventually(WAIT, async || {
        let view = state(&harness, &work).await.view;
        match view.activity {
            Some(seen) if seen.state == ActivityState::Stopped => Ok(seen),
            other => Err(other),
        }
    })
    .await;
    assert_eq!(stopped.told, ["web-lead"]);
    assert_eq!(stalled_events(&harness).await[0]["reason"], "stopped");
}

/// A worker whose ask about its task waits for a person is never stalled, however long it waits and whatever call it left open.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_that_waits_for_an_answer_is_not_stalled() {
    let fake = FakeClaude::new("bash_open");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Working).await;
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
            json!({"to": [format!("human:{HUMAN}")], "body": "Which colour?", "task": "T-1", "asks_human": true}),
        )
        .await;
    advance(&harness, 60 * MINUTE);
    let view = state(&harness, &work).await.view;
    assert_eq!(view.activity, None);
    assert!(!view.stalled);
    assert!(stalled_events(&harness).await.is_empty());
}

/// The lead that fold tells of a stall started the worker, so it may restart it with restart_worker, which ends the stall; the lead of another area may not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lead_that_hears_of_a_stall_restarts_its_task_worker() {
    let fake = FakeClaude::new("result");
    let mut harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    advance(&harness, STALL);
    let told = state(&harness, &work).await.view.activity.expect("a stall");
    assert_eq!(told.told, ["web-lead"]);

    crate::common::roles::give_role(
        &harness.handle.store,
        "api-lead",
        fold_core::Role::AreaLead,
        Some("api"),
    )
    .await;
    harness.connect("api-lead").await;
    let refused = harness
        .refused("api-lead", "restart_worker", json!({"name": TASK_WORKER}))
        .await;
    assert_eq!(refused["code"], fold_core::codes::MAIN_AGENT_REQUIRED);

    harness
        .call("web-lead", "restart_worker", json!({"name": TASK_WORKER}))
        .await;
    crate::worker_resume::until_runs(&fake, TASK_WORKER, 2).await;
    eventually(WAIT, async || {
        match state(&harness, &work).await.view.activity {
            None => Ok(()),
            other => Err(other),
        }
    })
    .await;
    assert!(!state(&harness, &work).await.view.stalled);
}
