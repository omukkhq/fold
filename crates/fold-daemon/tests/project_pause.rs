#![cfg(test)]

use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::roles::Role;
use fold_core::{AgentName, HumanName, WorkerState, codes};
use fold_daemon::services::agents::PAUSE_TURN;
use fold_daemon::services::tasks::sweep_leases;
use fold_daemon::services::workspaces::check_stalls;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::auto_integrate::{Running, approve, head, review_lead, state_of};
use crate::common::acceptance::{HUMAN, Harness, WAIT};
use crate::common::roles::give_role;
use crate::proposal_tools_support::{Fixture, human, t};
use crate::worker_resume::{
    MAIN, TASK_WORKER, proposed_by_a_task_worker, request_changes, task_worker_for, until_runs,
    until_state, worker,
};
use crate::workers_support::FakeClaude;

const LEAD: &str = "web-lead";
const PAUSE: &str = "/api/v1/projects/demo/pause";
const RESUME: &str = "/api/v1/projects/demo/resume";

/// A harness whose worker processes run `fake`, with the human [`HUMAN`] known and `frontend` as the main agent, the area `web` over label `ui`, and its lead started and in `state`.
async fn lead_running(fake: &FakeClaude, state: WorkerState) -> Harness {
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    fold_daemon::services::projects::ensure_human(
        harness.project.registry(),
        harness.project.clock(),
        &HumanName::parse(HUMAN).unwrap(),
    )
    .await
    .unwrap();
    harness.make_main(MAIN).await;
    harness
        .call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    harness
        .call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    until_state(&harness, LEAD, state).await;
    until_runs(fake, LEAD, 1).await;
    harness
}

async fn http_post(harness: &Harness, path: &str) -> Value {
    let (status, body) = harness.http(Method::POST, path, None, HUMAN).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// Waits until the supervisor runs no process for `agent`.
async fn until_not_running(harness: &Harness, agent: &str) {
    let name = AgentName::parse(agent).unwrap();
    eventually(WAIT, async || {
        if harness.handle.workers.is_running(&name) {
            Err("running")
        } else {
            Ok(())
        }
    })
    .await;
}

fn kinds(events: &[crate::common::acceptance::StoredEvent], prefix: &str) -> Vec<String> {
    events
        .iter()
        .filter(|event| event.kind.starts_with(prefix))
        .map(|event| event.kind.clone())
        .collect()
}

#[tokio::test]
async fn pause_stops_every_worker_and_resume_runs_each_again_in_its_own_session() {
    let fake = FakeClaude::new("init");
    let harness = lead_running(&fake, WorkerState::Working).await;
    let before = worker(&harness, LEAD).await;

    let paused = harness.call(MAIN, "pause_project", json!({})).await;
    assert_eq!(paused["paused"], true, "{paused}");
    assert_eq!(paused["paused_by"], MAIN);
    until_not_running(&harness, LEAD).await;
    let kept = worker(&harness, LEAD).await;
    assert_eq!(kept.state, WorkerState::Working, "the row keeps its state");
    assert_eq!(kept.pid, None);
    assert_eq!(kept.session_id, before.session_id);
    let refused = harness
        .refused(
            MAIN,
            "start_worker",
            json!({"role": "review_lead", "name": "reviews"}),
        )
        .await;
    assert_eq!(refused["code"], codes::PROJECT_PAUSED);
    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    assert_eq!(listed["pause"]["paused"], true, "{listed}");
    assert_eq!(fake.runs_of(LEAD).len(), 1);

    let resumed = harness.call(MAIN, "resume_project", json!({})).await;
    assert_eq!(resumed["paused"], false, "{resumed}");
    let runs = until_runs(&fake, LEAD, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(before.session_id.as_str()));
    assert!(
        runs[1].stdin[1].contains(PAUSE_TURN),
        "{}",
        runs[1].stdin[1]
    );
    until_state(&harness, LEAD, WorkerState::Working).await;
    let events = harness.events().await;
    assert_eq!(
        kinds(&events, "project."),
        ["project.paused", "project.resumed"]
    );
}

#[tokio::test]
async fn a_resume_the_pause_held_back_runs_at_the_resume() {
    let fake = FakeClaude::new("result");
    let (harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    let stopped = worker(&harness, TASK_WORKER).await;
    harness.call(MAIN, "pause_project", json!({})).await;
    until_not_running(&harness, LEAD).await;
    harness
        .project
        .clock()
        .advance(Duration::from_secs(1))
        .unwrap();

    request_changes(&harness, "P-1", "Split the page.").await;
    let refused = harness
        .refused(
            LEAD,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    assert_eq!(refused["code"], codes::PROJECT_PAUSED);
    assert_eq!(
        worker(&harness, TASK_WORKER).await.state,
        WorkerState::Stopped
    );
    assert_eq!(
        fake.runs_of(TASK_WORKER).len(),
        1,
        "the pause holds the resume"
    );

    harness.call(MAIN, "resume_project", json!({})).await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(stopped.session_id.as_str()));
    assert!(
        runs[1].stdin[1].contains("Split the page."),
        "{}",
        runs[1].stdin[1]
    );
}

#[tokio::test]
async fn a_claim_held_through_a_long_pause_stays_fresh_and_gets_a_full_lease_at_the_resume() {
    let harness = Harness::start().await;
    harness.make_main(MAIN).await;
    let task = harness.create_task(MAIN, "Draw the page").await;
    let work = harness.start_work("backend", &task).await;
    http_post(&harness, PAUSE).await;
    let clock = harness.project.clock();
    clock.advance(Duration::from_secs(10 * 60 * 60)).unwrap();

    let read = harness.call(MAIN, "get_task", json!({"id": task})).await;
    assert_eq!(read["claim"]["holder"], "backend");
    assert_eq!(read["claim"]["stale"], false, "{read}");
    let taken = harness
        .refused(MAIN, "takeover_claim", json!({"id": task}))
        .await;
    assert_eq!(taken["code"], codes::CLAIM_NOT_STALE);
    let marked = sweep_leases(&harness.handle, harness.project.now())
        .await
        .unwrap();
    assert!(marked.is_empty());
    assert_eq!(
        check_stalls(&harness.handle, harness.project.now())
            .await
            .unwrap(),
        0
    );
    let workspace = work.workspace.parse().unwrap();
    assert!(!harness.handle.live.get(workspace).unwrap().view.stalled);

    http_post(&harness, RESUME).await;
    check_stalls(&harness.handle, harness.project.now())
        .await
        .unwrap();
    assert!(
        !harness.handle.live.get(workspace).unwrap().view.stalled,
        "stall time counts from the resume"
    );
    clock.advance(Duration::from_secs(29 * 60)).unwrap();
    let read = harness.call(MAIN, "get_task", json!({"id": task})).await;
    assert_eq!(read["claim"]["stale"], false, "a full lease of 30 minutes");
    check_stalls(&harness.handle, harness.project.now())
        .await
        .unwrap();
    assert!(harness.handle.live.get(workspace).unwrap().view.stalled);
    clock.advance(Duration::from_secs(60)).unwrap();
    let read = harness.call(MAIN, "get_task", json!({"id": task})).await;
    assert_eq!(read["claim"]["stale"], true, "{read}");
}

#[tokio::test]
async fn the_idle_sweep_waits_while_paused_and_counts_from_the_resume() {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    http_post(&harness, PAUSE).await;
    until_not_running(&harness, TASK_WORKER).await;
    let clock = harness.project.clock();
    clock.advance(Duration::from_secs(24 * 60 * 60)).unwrap();
    let sweep = async || {
        harness
            .handle
            .workers
            .sweep_idle(harness.project.now())
            .await
            .unwrap()
    };
    assert!(sweep().await.is_empty());
    assert_eq!(worker(&harness, TASK_WORKER).await.state, WorkerState::Idle);

    http_post(&harness, RESUME).await;
    until_runs(&fake, TASK_WORKER, 2).await;
    until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
    assert!(sweep().await.is_empty(), "the idle time starts again");
}

#[tokio::test]
async fn a_daemon_restart_while_paused_stays_paused_and_resumes_no_worker() {
    let fake = FakeClaude::new("init");
    let mut harness = lead_running(&fake, WorkerState::Working).await;
    http_post(&harness, PAUSE).await;
    until_not_running(&harness, LEAD).await;
    harness.stop().await;

    harness.start_again().await;
    let (status, project) = harness
        .http(Method::GET, "/api/v1/projects/demo", None, HUMAN)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(project["overview"]["paused_since"].is_i64(), "{project}");
    assert_eq!(project["overview"]["paused_by"], "human:ana");
    assert_eq!(worker(&harness, LEAD).await.state, WorkerState::Working);

    http_post(&harness, RESUME).await;
    let runs = until_runs(&fake, LEAD, 2).await;
    assert!(
        runs[1].stdin[1].contains(PAUSE_TURN),
        "the resume, not the restart, ran the lead: {}",
        runs[1].stdin[1]
    );
}

#[tokio::test]
async fn approved_proposals_still_integrate_while_paused() {
    let fx = Fixture::new().await;
    let author = fx.agent("backend").await;
    let review = review_lead(&fx, "review").await;
    fx.workspaces()
        .create_workspace(&author, t(1))
        .await
        .unwrap();
    fx.write(1, "src/lib.rs", "pub fn y() {}\n");
    fx.reviews()
        .create_proposal(&author, t(1), "Change it", None)
        .await
        .unwrap();
    fx.agents().pause_project(&human("tanim")).await.unwrap();
    let job = Running::start(&fx.state);
    job.runs(1).await;

    approve(&fx, &review, 1).await;
    job.cycles(1).await;
    assert_eq!(head(&fx).await.to_string(), "r2");
    assert_eq!(state_of(&fx, 1).await, "integrated");
    assert!(
        fx.agents()
            .pause_state()
            .await
            .unwrap()
            .paused_since
            .is_some()
    );
    job.stop().await;
}

#[tokio::test]
async fn only_a_human_or_the_main_agent_pauses_or_resumes() {
    let harness = Harness::start().await;
    give_role(
        &harness.handle.store,
        "backend",
        Role::AreaLead,
        Some("web"),
    )
    .await;
    let lead = harness.refused("backend", "pause_project", json!({})).await;
    assert_eq!(lead["code"], codes::MAIN_AGENT_REQUIRED);
    let reviewer = harness.refused("review", "resume_project", json!({})).await;
    assert_eq!(reviewer["code"], codes::MAIN_AGENT_REQUIRED);
    let unsupervised = harness
        .refused("frontend", "pause_project", json!({}))
        .await;
    assert_eq!(unsupervised["code"], codes::ORCHESTRATOR_REQUIRED);

    harness.make_main(MAIN).await;
    harness.call(MAIN, "pause_project", json!({})).await;
    let again = harness.call(MAIN, "pause_project", json!({})).await;
    assert_eq!(again["paused"], true);
    let resumed = http_post(&harness, RESUME).await;
    assert_eq!(resumed, json!({"paused_since": null, "paused_by": null}));
    let events = harness.events().await;
    let actors: Vec<(String, String)> = events
        .iter()
        .filter(|event| event.kind.starts_with("project."))
        .map(|event| (event.kind.clone(), event.actor.clone()))
        .collect();
    assert_eq!(
        actors,
        [
            ("project.paused".to_owned(), MAIN.to_owned()),
            ("project.resumed".to_owned(), "human:ana".to_owned()),
        ]
    );
}
