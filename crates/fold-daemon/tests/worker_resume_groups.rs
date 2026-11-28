#![cfg(test)]

use std::time::Duration;

use fold_core::WorkerState;
use fold_daemon::live::{ActivityState, WorkspaceLive};
use fold_daemon::services::agents::{DEFAULT_RESUME_GAP, DEFAULT_RESUME_GROUP};
use fold_daemon::services::workspaces::check_stalls;
use fold_store::testing::eventually;
use serde_json::json;

use crate::common::acceptance::{Harness, WAIT, Work};
use crate::worker_resume::{
    MAIN, TASK_WORKER, proposed_by_a_task_worker, request_changes, task_worker_for, until_runs,
    until_state, worker,
};
use crate::workers_support::FakeClaude;

/// The default `stall_minutes`.
const STALL: Duration = Duration::from_secs(5 * 60);

fn advance(harness: &Harness, by: Duration) {
    harness.project.clock().advance(by).unwrap();
}

/// A ready task of area web, so a daemon start owes `web-lead` a turn.
async fn web_work(harness: &Harness) {
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Draw the footer", "labels": ["ui"]}),
        )
        .await;
}

/// How many of `agents` a start ran again: each ran once before the daemon stopped.
fn resumed(fake: &FakeClaude, agents: &[String]) -> usize {
    agents
        .iter()
        .filter(|agent| fake.runs_of(agent).len() >= 2)
        .count()
}

async fn until_resumed(fake: &FakeClaude, agents: &[String], count: usize) {
    eventually(WAIT, async || {
        let seen = resumed(fake, agents);
        if seen == count { Ok(()) } else { Err(seen) }
    })
    .await;
}

/// Starts a lead for each of `count` new areas, each with a ready task of its own so a daemon start owes the lead a turn, and gives their names.
async fn zone_leads(harness: &Harness, count: usize) -> Vec<String> {
    let mut leads = Vec::new();
    for n in 1..=count {
        let zone = format!("zone-{n}");
        harness
            .call(MAIN, "create_label", json!({"name": zone, "color": "sage"}))
            .await;
        harness
            .call(MAIN, "define_area", json!({"name": zone, "label": zone}))
            .await;
        harness
            .call(
                MAIN,
                "create_task",
                json!({"title": format!("Pave {zone}"), "labels": [zone]}),
            )
            .await;
        harness
            .call(
                MAIN,
                "start_worker",
                json!({"role": "area_lead", "area": zone}),
            )
            .await;
        leads.push(format!("{zone}-lead"));
    }
    leads
}

fn limit_task_workers(harness: &Harness, limit: u32) {
    std::fs::write(
        std::path::Path::new(harness.root()).join(".fold/config.toml"),
        format!("[workers]\ntask_worker = {limit}\n"),
    )
    .unwrap();
}

async fn run_due(harness: &Harness) -> usize {
    harness
        .state
        .projects
        .resume_groups()
        .run_due(harness.project.now())
        .await
}

async fn activity(harness: &Harness, work: &Work) -> Option<ActivityState> {
    check_stalls(&harness.handle, harness.project.now())
        .await
        .unwrap();
    let workspace = work.workspace.parse().unwrap();
    WorkspaceLive::clone(&harness.handle.live.get(workspace).unwrap())
        .view
        .activity
        .map(|activity| activity.state)
}

async fn stalled_events(harness: &Harness) -> usize {
    harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "worker.stalled")
        .count()
}

/// Ten idle workers, nine leads and a task worker with a workspace: after a daemon start four run again at once, four more after the pause, then the last two, leads first. The task worker that waits for its group is never a stall.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_daemon_start_resumes_workers_a_group_at_a_time() {
    assert_eq!(DEFAULT_RESUME_GROUP, 4);
    let fake = FakeClaude::new("result");
    let mut harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    web_work(&harness).await;
    let mut agents = vec!["web-lead".to_owned()];
    agents.extend(zone_leads(&harness, 8).await);
    agents.push(TASK_WORKER.to_owned());
    for agent in &agents {
        until_state(&harness, agent, WorkerState::Idle).await;
    }
    assert_eq!(resumed(&fake, &agents), 0);

    harness.stop().await;
    harness.start_again().await;
    until_resumed(&fake, &agents, 4).await;
    let groups = harness.state.projects.resume_groups().clone();
    assert_eq!(groups.waiting(), 6);
    assert_eq!(fake.runs_of(TASK_WORKER).len(), 1, "a lead goes first");
    assert_eq!(
        run_due(&harness).await,
        0,
        "the next group waits for the pause"
    );

    // A reopened workspace gets its first scan with its watch group, which the live job's tick starts.
    let workspace = work.workspace.parse().unwrap();
    eventually(WAIT, async || {
        harness.handle.live.tick();
        harness.handle.live.get(workspace).ok_or(())
    })
    .await;

    advance(&harness, DEFAULT_RESUME_GAP - Duration::from_millis(1));
    assert_eq!(run_due(&harness).await, 0);
    advance(&harness, Duration::from_millis(1));
    assert_eq!(run_due(&harness).await, 4);
    until_resumed(&fake, &agents, 8).await;
    assert_eq!(groups.waiting(), 2);
    assert_eq!(
        fake.runs_of(TASK_WORKER).len(),
        1,
        "the task worker comes last"
    );

    advance(&harness, STALL);
    assert_eq!(
        activity(&harness, &work).await,
        Some(ActivityState::WaitsForSlot),
        "a worker that waits for its group waits for a place"
    );
    assert_eq!(stalled_events(&harness).await, 0);

    assert_eq!(run_due(&harness).await, 2);
    until_resumed(&fake, &agents, 10).await;
    assert_eq!(groups.waiting(), 0);
    until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
    assert_ne!(
        activity(&harness, &work).await,
        Some(ActivityState::WaitsForSlot),
        "its own run ends the wait"
    );
    assert_eq!(stalled_events(&harness).await, 0, "never reported stalled");
}

/// A start that owes a stopped task worker its requested changes and runs seven idle workers again: the owed resume goes in the first group, before the leads, and its start checks the worker limits when its group comes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_resumes_fold_owes_go_in_the_first_group() {
    let fake = FakeClaude::new("result");
    let (mut harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    limit_task_workers(&harness, 1);
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Draw the menu", "labels": ["ui"]}),
        )
        .await;
    let other = harness
        .call(
            "web-lead",
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-2"]}),
        )
        .await;
    let other = other["worker"]["name"].as_str().unwrap().to_owned();
    web_work(&harness).await;
    let mut rows = vec!["web-lead".to_owned(), other];
    rows.extend(zone_leads(&harness, 5).await);
    for agent in &rows {
        until_state(&harness, agent, WorkerState::Idle).await;
    }
    advance(&harness, Duration::from_secs(1));
    request_changes(&harness, "P-1", "Split the page.").await;
    assert_eq!(
        worker(&harness, TASK_WORKER).await.state,
        WorkerState::Stopped,
        "the limit holds the resume back"
    );

    harness.stop().await;
    limit_task_workers(&harness, 2);
    harness.start_again().await;
    let groups = harness.state.projects.resume_groups().clone();
    assert_eq!(groups.waiting(), 4, "the owed resume and three rows went");
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert!(runs[1].stdin[1].contains("Split the page."));
    until_resumed(&fake, &rows, 3).await;
    assert_eq!(
        fake.runs_of(&rows[1]).len(),
        1,
        "the idle task worker waits"
    );

    advance(&harness, DEFAULT_RESUME_GAP);
    assert_eq!(run_due(&harness).await, 4);
    until_resumed(&fake, &rows, 7).await;
    assert_eq!(groups.waiting(), 0);
}
