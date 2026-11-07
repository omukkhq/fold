#![cfg(test)]

use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::WorkerState;
use fold_daemon::live::ActivityState;
use fold_daemon::services::workspaces::check_stalls;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, SLUG, WAIT, Work};
use crate::worker_resume::{TASK_WORKER, task_worker_for, until_runs};
use crate::workers_support::FakeClaude;

const MINUTE: Duration = Duration::from_secs(60);

async fn decide(harness: &Harness, work: &Work) -> Option<ActivityState> {
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
        .map(|seen| seen.state)
}

async fn get(harness: &Harness, path: &str) -> Value {
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

async fn stalled_item(harness: &Harness) -> Option<Value> {
    get(harness, "needs-you").await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "worker_stalled")
        .cloned()
}

async fn attention(harness: &Harness, task: &str) -> Vec<String> {
    get(harness, &format!("tasks/{task}")).await["attention"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["kind"].as_str().unwrap().to_owned())
        .collect()
}

/// Acceptance 40, a stall goes to the lead first: a task worker runs a long shell command. While it runs, fold shows it working and calls nothing a stall. Once the command has run for twice Claude Code's ten-minute limit, fold tells the worker's lead with the tool that starts it again, and no person hears of it. When the lead does not act within one more stall window, Needs you and the task show the stall with what fold saw, and the human's restart starts the worker in a new session, which ends the item.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_40_a_stall_goes_to_the_lead_first_and_to_a_person_only_when_it_lasts() {
    let fake = FakeClaude::new("bash_open");
    let harness = task_worker_for(&fake, &["Build the report"], WorkerState::Working).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;

    harness.project.clock().advance(8 * MINUTE).unwrap();
    assert_eq!(decide(&harness, &work).await, Some(ActivityState::Working));
    assert!(attention(&harness, "T-1").await.is_empty());
    assert!(stalled_item(&harness).await.is_none());

    harness.project.clock().advance(12 * MINUTE).unwrap();
    assert_eq!(decide(&harness, &work).await, Some(ActivityState::LongCall));
    let notice = eventually(WAIT, async || {
        let runs = fake.runs_of("web-lead");
        let line = runs.last().and_then(|run| {
            run.stdin
                .iter()
                .find(|line| line.contains("has run for"))
                .cloned()
        });
        line.ok_or(runs.len())
    })
    .await;
    assert!(
        notice.contains("a command of web-worker on T-1 has run for 20 min; stop_worker ends it, then send_message resumes it in its session"),
        "{notice}"
    );
    assert!(
        attention(&harness, "T-1").await.is_empty(),
        "the lead acts first"
    );
    assert!(stalled_item(&harness).await.is_none());

    harness.project.clock().advance(5 * MINUTE).unwrap();
    assert_eq!(decide(&harness, &work).await, Some(ActivityState::LongCall));
    assert_eq!(attention(&harness, "T-1").await, ["stalled"]);
    let item = stalled_item(&harness)
        .await
        .expect("the stall waits for a person");
    assert_eq!(item["agent"], TASK_WORKER);
    assert_eq!(item["task"], "T-1");
    assert_eq!(item["screen"], "task");
    assert_eq!(item["activity"]["state"], "long_call");
    assert_eq!(item["activity"]["call"], "command");
    assert_eq!(item["activity"]["supervised"], true);
    assert_eq!(item["activity"]["told"], json!(["web-lead"]));
    assert_eq!(
        item["reason"],
        "restart it in a new session: `fld worker restart web-worker`"
    );

    let (status, body) = harness
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/workers/{TASK_WORKER}/restart"),
            Some(&json!({})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    until_runs(&fake, TASK_WORKER, 2).await;
    eventually(WAIT, async || match decide(&harness, &work).await {
        None => Ok(()),
        other => Err(other),
    })
    .await;
    assert!(
        stalled_item(&harness).await.is_none(),
        "the restart ends the item"
    );
    assert!(attention(&harness, "T-1").await.is_empty());
}
