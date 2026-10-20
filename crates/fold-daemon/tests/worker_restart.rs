#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_core::{WorkerState, codes};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, WAIT};
use crate::worker_resume::{
    MAIN, TASK_WORKER, request_changes, task_worker_for, until_runs, until_state, worker,
};
use crate::workers_support::{FakeClaude, Run};

const LEAD: &str = "web-lead";
const FRESH: &str = "A human or the main agent asked fold to start you again in a new session";

fn restart_path(agent: &str) -> String {
    format!("/api/v1/projects/demo/workers/{agent}/restart")
}

/// The stored events of `kind` with their actor and data.
async fn events_of(harness: &Harness, kind: &str) -> Vec<(String, Value)> {
    harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == kind)
        .map(|event| (event.actor, event.data))
        .collect()
}

fn first_turn(run: &Run) -> String {
    run.stdin.join("\n")
}

#[tokio::test]
async fn a_human_restarts_a_working_lead_in_a_new_session_and_its_task_worker_runs_on() {
    let fake = FakeClaude::new("init");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Working).await;
    let before = until_state(&harness, LEAD, WorkerState::Working).await;
    until_runs(&fake, LEAD, 1).await;
    harness
        .handle
        .store
        .write(async |tx| {
            sqlx::query("UPDATE worker_processes SET model = '<synthetic>' WHERE id = ?")
                .bind(before.id)
                .execute(tx.conn())
                .await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    let (status, restarted) = harness
        .socket(
            Method::POST,
            &restart_path(LEAD),
            Some(&json!({"instructions": "Plan the page again."})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{restarted}");
    let session = restarted["session_id"].as_str().unwrap().to_owned();
    assert_ne!(session, before.session_id);
    assert_eq!(
        restarted["previous_sessions"],
        json!([{
            "session_id": before.session_id,
            "reason": "requested",
            "ended_at": restarted["previous_sessions"][0]["ended_at"],
        }])
    );
    let runs = until_runs(&fake, LEAD, 2).await;
    assert_eq!(runs[1].flag("--session-id"), Some(session.as_str()));
    assert!(!runs[1].has("--resume"));
    assert_ne!(runs[1].flag("--model"), Some("<synthetic>"));
    let turn = first_turn(&runs[1]);
    assert!(turn.contains("the area lead of area web"), "{turn}");
    assert!(turn.contains(FRESH), "{turn}");
    assert!(turn.contains("Plan the page again."), "{turn}");
    let after = worker(&harness, LEAD).await;
    assert_eq!(after.session_id, session);
    assert_ne!(after.model.as_deref(), Some("<synthetic>"));
    let stopped = events_of(&harness, "worker.stopped").await;
    assert!(
        !stopped.iter().any(|(_, data)| data["agent"] == LEAD),
        "the restart ends the process without a stop: {stopped:?}"
    );
    let refreshed = events_of(&harness, "worker.refreshed").await;
    assert_eq!(refreshed.len(), 1);
    assert_eq!(refreshed[0].0, "human:ana");
    assert_eq!(refreshed[0].1["requested"], true);
    assert_eq!(refreshed[0].1["old_session"], before.session_id.as_str());
    assert_eq!(refreshed[0].1["new_session"], session.as_str());
    assert_eq!(
        worker(&harness, TASK_WORKER).await.state,
        WorkerState::Working
    );
    assert!(
        harness
            .handle
            .workers
            .is_running(&fold_core::AgentName::parse(TASK_WORKER).unwrap())
    );
    let (status, listed) = harness
        .socket(Method::GET, "/api/v1/projects/demo/workers", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["areas"][0]["lead"], LEAD);
    let lead = listed["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == LEAD)
        .unwrap();
    assert_eq!(
        lead["previous_sessions"][0]["session_id"],
        before.session_id.as_str()
    );
    let tool = harness.call(MAIN, "list_workers", json!({})).await;
    let lead = tool["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == LEAD)
        .unwrap();
    assert_eq!(
        lead["previous_sessions"][0]["session"],
        before.session_id.as_str()
    );
    assert_eq!(lead["previous_sessions"][0]["reason"], "requested");
}

#[tokio::test]
async fn the_main_agent_restarts_a_stopped_task_worker_that_keeps_its_claim_within_the_limits() {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    harness
        .call(MAIN, "stop_worker", json!({"name": TASK_WORKER}))
        .await;
    let before = until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    let lead = harness
        .refused(LEAD, "restart_worker", json!({"name": TASK_WORKER}))
        .await;
    assert_eq!(lead["code"], codes::MAIN_AGENT_REQUIRED);
    harness
        .call(MAIN, "set_worker_limits", json!({"task_worker": 0}))
        .await;
    let full = harness
        .refused(MAIN, "restart_worker", json!({"name": TASK_WORKER}))
        .await;
    assert_eq!(full["code"], codes::WORKER_LIMIT_REACHED);
    assert_eq!(
        worker(&harness, TASK_WORKER).await.session_id,
        before.session_id
    );
    harness
        .call(MAIN, "set_worker_limits", json!({"task_worker": 4}))
        .await;
    let restarted = harness
        .call(MAIN, "restart_worker", json!({"name": TASK_WORKER}))
        .await;
    assert_eq!(restarted["worker"]["tasks"], json!(["T-1"]));
    assert_eq!(
        restarted["worker"]["previous_sessions"][0]["session"],
        before.session_id.as_str()
    );
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert!(!runs[1].has("--resume"));
    let turn = first_turn(&runs[1]);
    assert!(turn.contains("a task worker for T-1"), "{turn}");
    assert!(turn.contains(FRESH), "{turn}");
    let after = until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
    assert_ne!(after.session_id, before.session_id);
    assert_eq!(after.tasks, before.tasks);
    let task = harness.call(MAIN, "get_task", json!({"id": "T-1"})).await;
    assert_eq!(task["claim"]["holder"], TASK_WORKER);
    let refreshed = events_of(&harness, "worker.refreshed").await;
    assert_eq!(refreshed[0].0, MAIN);
}

#[tokio::test]
async fn a_paused_project_refuses_a_new_session_and_the_worker_keeps_its_own() {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let before = until_state(&harness, LEAD, WorkerState::Idle).await;
    harness.call(MAIN, "pause_project", json!({})).await;
    eventually(WAIT, async || {
        let name = fold_core::AgentName::parse(LEAD).unwrap();
        if harness.handle.workers.is_running(&name) {
            Err(())
        } else {
            Ok(())
        }
    })
    .await;
    let (status, refused) = harness
        .socket(Method::POST, &restart_path(LEAD), Some(&json!({})))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"]["code"], codes::PROJECT_PAUSED);
    let after = worker(&harness, LEAD).await;
    assert_eq!(after.session_id, before.session_id);
    assert_eq!(after.state, before.state);
    assert!(events_of(&harness, "worker.refreshed").await.is_empty());
    harness.call(MAIN, "resume_project", json!({})).await;
    let (status, unknown) = harness
        .socket(Method::POST, &restart_path("nobody"), Some(&json!({})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{unknown}");
    assert_eq!(unknown["error"]["code"], codes::WORKER_PROCESS_NOT_FOUND);
}

#[tokio::test]
async fn a_running_task_worker_with_requested_changes_starts_fresh_with_them_and_keeps_its_place() {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(
        &fake,
        &["Draw the page", "Draw the menu"],
        WorkerState::Idle,
    )
    .await;
    let page = harness.start_work(TASK_WORKER, "T-1").await;
    page.write(&[("page.html", "<p>page</p>\n")]);
    assert_eq!(
        harness.propose(TASK_WORKER, &page, "Draw the page").await,
        "P-1"
    );
    let before = until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
    harness
        .project
        .clock()
        .advance(std::time::Duration::from_secs(1))
        .unwrap();
    request_changes(&harness, "P-1", "Split the page.").await;
    // A full limit holds no running worker back: the restart keeps the worker's place.
    harness
        .call(MAIN, "set_worker_limits", json!({"task_worker": 0}))
        .await;
    let restarted = harness
        .call(MAIN, "restart_worker", json!({"name": TASK_WORKER}))
        .await;
    assert_eq!(restarted["worker"]["previous_session_count"], 1);
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert!(!runs[1].has("--resume"));
    assert_ne!(
        runs[1].flag("--session-id"),
        Some(before.session_id.as_str())
    );
    let turn = first_turn(&runs[1]);
    for wanted in [
        FRESH,
        "P-1 v1 for T-1: changes requested",
        "Split the page.",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
    let after = until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
    assert_ne!(after.session_id, before.session_id);
    assert_eq!(
        fake.runs_of(TASK_WORKER).len(),
        2,
        "never resumed in its old session"
    );
    let stopped = events_of(&harness, "worker.stopped").await;
    assert!(
        !stopped.iter().any(|(_, data)| data["agent"] == TASK_WORKER),
        "{stopped:?}"
    );
}

#[tokio::test]
async fn a_reviewer_starts_fresh_with_its_proposals_and_the_review_lead_cannot_ask_for_it() {
    let fake = FakeClaude::new("init");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    let task = harness.create_task(MAIN, "Draw the page").await;
    let work = harness.start_work(MAIN, &task).await;
    work.write(&[("page.html", "<p>page</p>\n")]);
    let proposal = harness.propose(MAIN, &work, "Draw the page").await;
    harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    harness.connect("review-lead").await;
    harness
        .call(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": [proposal]}),
        )
        .await;
    let reviewer = "reviewer";
    until_runs(&fake, reviewer, 1).await;
    until_state(&harness, reviewer, WorkerState::Working).await;
    let refused = harness
        .refused("review-lead", "restart_worker", json!({"name": reviewer}))
        .await;
    assert_eq!(refused["code"], codes::MAIN_AGENT_REQUIRED);
    harness
        .call(MAIN, "restart_worker", json!({"name": reviewer}))
        .await;
    let runs = until_runs(&fake, reviewer, 2).await;
    assert!(!runs[1].has("--resume"));
    let turn = first_turn(&runs[1]);
    for wanted in [
        "reviewer for P-1",
        FRESH,
        "give a verdict only to those that have none from you",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
    let row = until_state(&harness, reviewer, WorkerState::Working).await;
    assert_eq!(
        row.proposals,
        [fold_core::ProposalId::from_number(1).unwrap()]
    );
}

#[tokio::test]
async fn an_agent_sees_the_three_newest_previous_sessions_and_a_human_sees_all() {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let first = until_state(&harness, LEAD, WorkerState::Idle).await;
    let mut sessions = vec![first.session_id];
    for count in 2..=5 {
        let (status, restarted) = harness
            .socket(Method::POST, &restart_path(LEAD), Some(&json!({})))
            .await;
        assert_eq!(status, StatusCode::OK, "{restarted}");
        sessions.push(restarted["session_id"].as_str().unwrap().to_owned());
        until_runs(&fake, LEAD, count).await;
        until_state(&harness, LEAD, WorkerState::Idle).await;
    }
    let tool = harness.call(MAIN, "list_workers", json!({})).await;
    let lead = tool["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == LEAD)
        .unwrap();
    assert_eq!(lead["previous_session_count"], 4);
    let shown: Vec<&str> = lead["previous_sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["session"].as_str().unwrap())
        .collect();
    assert_eq!(shown, [&sessions[1], &sessions[2], &sessions[3]]);
    let (status, listed) = harness
        .socket(Method::GET, "/api/v1/projects/demo/workers", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let lead = listed["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == LEAD)
        .unwrap();
    assert_eq!(lead["previous_sessions"].as_array().unwrap().len(), 4);
    assert_eq!(lead["session_id"], sessions[4].as_str());
}
