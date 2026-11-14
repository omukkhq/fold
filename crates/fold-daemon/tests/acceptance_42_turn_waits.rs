#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_core::WorkerState;
use fold_core::turn_waits::{LIMIT_RERUN_TURN, NETWORK_FIRST_WAIT, NETWORK_RERUN_TURN};
use fold_daemon::live::ActivityState;
use fold_daemon::services::workspaces::check_stalls;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::common::acceptance::{Harness, Work};
use crate::turn_waits::{
    advance, events_of, fake_with_turn, limit_turn, needs_you, read_fixture, set_turn, until_idle,
    until_turns,
};
use crate::worker_resume::{TASK_WORKER, task_worker_for};

async fn activity(harness: &Harness, work: &Work) -> Option<ActivityState> {
    check_stalls(&harness.handle, harness.project.now())
        .await
        .unwrap();
    let workspace = work.workspace.parse().unwrap();
    harness
        .handle
        .live
        .get(workspace)
        .unwrap()
        .view
        .activity
        .as_ref()
        .map(|activity| activity.state)
}

fn items_of<'a>(items: &'a [Value], kind: &str) -> Vec<&'a Value> {
    items.iter().filter(|item| item["kind"] == kind).collect()
}

/// The network goes down while a lead and its task worker work: their turns wait, the person sees one project item and neutral chips, and once the model answers fold runs each turn again in its own session. Later the account reaches its session limit: the worker waits for the limit, the item offers Pause, and the resume of the pause runs the turn again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_42_turns_that_end_for_the_network_or_a_usage_limit_run_again_in_their_sessions()
{
    let dir = TempDir::new().unwrap();
    let (fake, turn) = fake_with_turn(&dir, &read_fixture("network_error"));
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    let waiting = until_idle(&harness, TASK_WORKER, |row| row.wait.is_some()).await;
    until_idle(&harness, "web-lead", |row| row.wait.is_some()).await;

    assert_eq!(
        activity(&harness, &work).await,
        Some(ActivityState::WaitsForNetwork)
    );
    let items = needs_you(&harness).await;
    let unreachable = items_of(&items, "worker_model_unreachable");
    assert_eq!(unreachable.len(), 1, "{items:?}");
    assert_eq!(unreachable[0]["agent"], Value::Null);
    assert_eq!(unreachable[0]["agents"], json!(["web-lead", TASK_WORKER]));
    assert!(events_of(&harness, "worker.stalled").await.is_empty());

    set_turn(&turn, &read_fixture("result"));
    advance(&harness, NETWORK_FIRST_WAIT);
    until_turns(&fake, "web-lead", NETWORK_RERUN_TURN, 1).await;
    until_turns(&fake, TASK_WORKER, NETWORK_RERUN_TURN, 1).await;
    let back = until_idle(&harness, TASK_WORKER, |row| row.wait.is_none()).await;
    until_idle(&harness, "web-lead", |row| row.wait.is_none()).await;
    assert_eq!(back.session_id, waiting.session_id);
    assert_eq!(fake.runs_of(TASK_WORKER).len(), 1);
    assert!(items_of(&needs_you(&harness).await, "worker_model_unreachable").is_empty());
    assert_ne!(
        activity(&harness, &work).await,
        Some(ActivityState::WaitsForNetwork)
    );

    set_turn(
        &turn,
        &limit_turn("You've hit your session limit · resets 1:50am (Asia/Tokyo)"),
    );
    harness
        .call(
            "web-lead",
            "send_message",
            json!({"to": [TASK_WORKER], "body": "Add a footer too."}),
        )
        .await;
    until_idle(&harness, TASK_WORKER, |row| row.wait.is_some()).await;
    assert_eq!(
        activity(&harness, &work).await,
        Some(ActivityState::WaitsForUsageLimit)
    );
    let items = needs_you(&harness).await;
    let limits = items_of(&items, "usage_limit");
    assert_eq!(limits.len(), 1, "{items:?}");
    assert_eq!(limits[0]["limit"], "session");
    assert_eq!(limits[0]["reset"], "1:50am (Asia/Tokyo)");
    assert_eq!(limits[0]["actions"], json!([{"action": "pause"}]));

    let (status, paused) = harness
        .http(Method::POST, "/api/v1/projects/demo/pause", None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    set_turn(&turn, &read_fixture("result"));
    let (status, resumed) = harness
        .http(Method::POST, "/api/v1/projects/demo/resume", None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    let after = until_idle(&harness, TASK_WORKER, |row| row.wait.is_none()).await;
    assert_eq!(after.session_id, waiting.session_id);
    let runs = fake.runs_of(TASK_WORKER);
    assert_eq!(
        runs.last().and_then(|run| run.flag("--resume")),
        Some(waiting.session_id.as_str())
    );
    assert!(items_of(&needs_you(&harness).await, "usage_limit").is_empty());
    assert_eq!(
        crate::turn_waits::reruns_of(&fake, TASK_WORKER, LIMIT_RERUN_TURN),
        0,
        "the pause's own turn ran the waiting turn again"
    );
    assert!(events_of(&harness, "worker.stalled").await.is_empty());
}
