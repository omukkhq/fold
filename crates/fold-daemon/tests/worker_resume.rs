#![cfg(test)]

use crate::common::roles::give_role;
use fold_core::roles::Role;
use std::time::Duration;

use fold_core::events::types;
use fold_core::reviews::Verdict;
use fold_core::{Actor, AgentName, HumanName, LiveEvent, WorkerState, codes};
use fold_daemon::services::events::EventBus;
use fold_daemon::services::reviews::ReviewService;
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::agents::WorkerProcessRow;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, SLUG, StoredEvent, WAIT, Work};
use crate::workers_support::{FakeClaude, Run};

pub const MAIN: &str = "frontend";
pub const TASK_WORKER: &str = "web-worker";

pub async fn worker(harness: &Harness, agent: &str) -> WorkerProcessRow {
    harness
        .handle
        .agents()
        .get_worker(&AgentName::parse(agent).unwrap())
        .await
        .unwrap()
}

pub async fn until_state(harness: &Harness, agent: &str, state: WorkerState) -> WorkerProcessRow {
    eventually(WAIT, async || {
        let row = worker(harness, agent).await;
        if row.state == state {
            Ok(row)
        } else {
            Err(row.state)
        }
    })
    .await
}

/// The runs of `agent` once there are `count` of them and the newest has read its first turn.
pub async fn until_runs(fake: &FakeClaude, agent: &str, count: usize) -> Vec<Run> {
    eventually(WAIT, async || {
        let runs = fake.runs_of(agent);
        match runs.last() {
            Some(last) if runs.len() == count && last.stdin.len() >= 2 => Ok(runs),
            _ => Err(runs.len()),
        }
    })
    .await
}

/// The data of every stored `worker.resumed`, once there are `count`.
async fn until_resumed(harness: &Harness, count: usize) -> Vec<(Option<String>, Value)> {
    eventually(WAIT, async || {
        let resumed: Vec<(Option<String>, Value)> = harness
            .events()
            .await
            .into_iter()
            .filter(|event| event.kind == "worker.resumed")
            .map(|event| (event.proposal, event.data))
            .collect();
        if resumed.len() == count {
            Ok(resumed)
        } else {
            Err(resumed.len())
        }
    })
    .await
}

/// Area `web` over label `ui` with one task per title, its lead `web-lead`, and the task worker `web-worker` that the lead started for every task, connected and waiting for work. `review` is the review lead.
pub async fn task_worker_for(fake: &FakeClaude, titles: &[&str], state: WorkerState) -> Harness {
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    give_role(&harness.handle.store, "review", Role::ReviewLead, None).await;
    harness
        .call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    harness
        .call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    let mut tasks = Vec::new();
    for title in titles {
        let created = harness
            .call(
                MAIN,
                "create_task",
                json!({"title": title, "labels": ["ui"]}),
            )
            .await;
        tasks.push(created["id"].clone());
    }
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    harness.connect("web-lead").await;
    let started = harness
        .call(
            "web-lead",
            "start_worker",
            json!({"role": "task_worker", "tasks": tasks}),
        )
        .await;
    assert_eq!(started["worker"]["name"], TASK_WORKER);
    until_state(&harness, TASK_WORKER, state).await;
    harness.connect(TASK_WORKER).await;
    harness
}

/// [`task_worker_for`] one task `T-1`, for which the task worker writes `files` in its workspace and proposes `P-1`, after which fold stops it.
pub async fn proposed_by_a_task_worker(
    fake: &FakeClaude,
    files: &[(&str, &str)],
) -> (Harness, Work) {
    let harness = task_worker_for(fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    work.write(files);
    let proposal = harness.propose(TASK_WORKER, &work, "Draw the page").await;
    assert_eq!(proposal, "P-1");
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    (harness, work)
}

/// `review` reads `proposal` and asks for changes on the version it read.
pub async fn request_changes(harness: &Harness, proposal: &str, body: &str) {
    let version = harness.version_of("review", proposal).await;
    harness
        .call(
            "review",
            "submit_review",
            json!({"id": proposal, "version": version, "verdict": "request_changes", "body": body}),
        )
        .await;
}

/// The position of the first stored event of `kind` for which `pick` holds.
fn position(events: &[StoredEvent], kind: &str, pick: impl Fn(&StoredEvent) -> bool) -> usize {
    events
        .iter()
        .position(|event| event.kind == kind && pick(event))
        .unwrap_or_else(|| panic!("no {kind}"))
}

/// The task worker records its workspace and sends the next version of `P-1`.
async fn post_next_version(harness: &Harness, work: &Work) {
    harness
        .call(
            TASK_WORKER,
            "record_changes",
            json!({"ws": work.workspace, "message": "Address the review"}),
        )
        .await;
    let updated = harness
        .call(TASK_WORKER, "update_proposal", json!({"id": "P-1"}))
        .await;
    assert_eq!(updated["version"], "v2", "{updated}");
}

#[tokio::test]
async fn requested_changes_resume_the_stopped_task_worker_with_the_comments_until_it_posts_version_2()
 {
    let fake = FakeClaude::new("result");
    let (harness, work) = proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    let stopped = worker(&harness, TASK_WORKER).await;
    harness
        .call(
            "review",
            "submit_review",
            json!({
                "id": "P-1",
                "version": 1,
                "verdict": "request_changes",
                "body": "Split the page.",
                "comments": [{"path": "page.html", "side": "new", "start_line": 1, "end_line": 1, "body": "Use a heading."}]
            }),
        )
        .await;

    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    let resumed_run = &runs[1];
    assert_eq!(
        resumed_run.flag("--resume"),
        Some(stopped.session_id.as_str())
    );
    let turn = &resumed_run.stdin[1];
    for wanted in [
        "P-1 v1 for T-1: changes requested",
        &format!("Your task workspace is {}", work.workspace),
        "Split the page.",
        "page.html:1 (review): Use a heading.",
        "update_proposal for P-1 to send v2",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
    let resumed = until_resumed(&harness, 1).await;
    assert_eq!(resumed[0].0.as_deref(), Some("P-1"));
    assert_eq!(
        resumed[0].1,
        json!({"agent": TASK_WORKER, "proposal": "P-1", "version": 1, "reason": "changes", "resumed": true})
    );
    let running = until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
    assert_eq!(running.tasks.len(), 1);
    assert_eq!(running.session_id, stopped.session_id);

    work.write(&[("page.html", "<h1>Page</h1>\n<p>page</p>\n")]);
    post_next_version(&harness, &work).await;
    let done = until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    assert_eq!(done.exit_code, Some(0));
    let view = harness
        .call("review", "get_proposal", json!({"id": "P-1"}))
        .await;
    assert_eq!(view["version"], "v2", "{view}");
    assert_eq!(view["task"]["status"], "review", "{view}");
    assert!(
        view["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .all(|review| review["version"] == "v1"),
        "the new version starts unapproved: {view}"
    );
}

#[tokio::test]
async fn a_conflict_at_integration_resumes_the_stopped_task_worker_with_the_sync_step() {
    let fake = FakeClaude::new("result");
    let (harness, work) =
        proposed_by_a_task_worker(&fake, &[("README.md", "# demo: the page\n")]).await;
    let other = harness.create_task(MAIN, "Name the endpoint").await;
    let theirs = harness.start_work(MAIN, &other).await;
    theirs.write(&[("README.md", "# demo: the endpoint\n")]);
    let their_proposal = harness.propose(MAIN, &theirs, "Name the endpoint").await;
    harness.approve(&their_proposal).await;
    harness.approve("P-1").await;
    harness
        .call(
            "review",
            "integrate_proposal",
            json!({"id": their_proposal}),
        )
        .await;
    let refused = harness
        .refused("review", "integrate_proposal", json!({"id": "P-1"}))
        .await;
    assert_eq!(refused["code"], codes::INTEGRATION_CONFLICTED);

    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert!(runs[1].has("--resume"));
    let turn = &runs[1].stdin[1];
    for wanted in [
        "P-1 v1 for T-1: conflicts with main",
        &format!("Your task workspace is {}", work.workspace),
        "README.md",
        "sync_workspace",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
    let resumed = until_resumed(&harness, 1).await;
    assert_eq!(resumed[0].1["reason"], "conflict");
    assert_eq!(resumed[0].1["resumed"], true);
    until_state(&harness, TASK_WORKER, WorkerState::Idle).await;

    let synced = harness
        .call(TASK_WORKER, "sync_workspace", json!({"ws": work.workspace}))
        .await;
    assert_eq!(synced["conflicted"], json!(["README.md"]), "{synced}");
    work.write(&[("README.md", "# demo: the page and the endpoint\n")]);
    post_next_version(&harness, &work).await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
}

#[tokio::test]
async fn a_task_worker_whose_session_is_gone_starts_fresh_in_a_new_session() {
    let fake = FakeClaude::new("result").lost_sessions();
    let (harness, work) = proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    let stopped = worker(&harness, TASK_WORKER).await;
    harness
        .call(
            "review",
            "submit_review",
            json!({"id": "P-1", "version": 1, "verdict": "request_changes", "body": "Split the page."}),
        )
        .await;

    let runs = until_runs(&fake, TASK_WORKER, 3).await;
    assert_eq!(runs[1].flag("--resume"), Some(stopped.session_id.as_str()));
    let fresh = &runs[2];
    assert!(!fresh.has("--resume"));
    let session = fresh.flag("--session-id").unwrap();
    assert_ne!(session, stopped.session_id);
    let turn = &fresh.stdin[1];
    for wanted in [
        "task worker for T-1",
        "Your earlier session is gone",
        "P-1 v1 for T-1: changes requested",
        &format!("Your task workspace is {}", work.workspace),
        "Split the page.",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
    let resumed = until_resumed(&harness, 1).await;
    assert_eq!(
        resumed[0].1,
        json!({"agent": TASK_WORKER, "proposal": "P-1", "version": 1, "reason": "changes", "resumed": false})
    );
    let row = until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
    assert_eq!(row.session_id, session);
    assert_eq!(row.turns, stopped.turns + 1, "the lost session ran no turn");
    let kinds: Vec<String> = harness
        .events()
        .await
        .into_iter()
        .map(|event| event.kind)
        .filter(|kind| kind.starts_with("worker."))
        .collect();
    assert!(kinds.contains(&"worker.failed".to_owned()), "{kinds:?}");

    work.write(&[("page.html", "<h1>Page</h1>\n")]);
    post_next_version(&harness, &work).await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
}

#[tokio::test]
async fn a_task_worker_stays_while_a_proposal_has_changes_requested_and_an_integrated_task_counts_as_finished()
 {
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
    request_changes(&harness, "P-1", "Split the page.").await;
    let menu = harness.start_work(TASK_WORKER, "T-2").await;
    menu.write(&[("menu.html", "<p>menu</p>\n")]);
    assert_eq!(
        harness.propose(TASK_WORKER, &menu, "Draw the menu").await,
        "P-2"
    );
    harness.approve("P-2").await;
    harness
        .call("review", "integrate_proposal", json!({"id": "P-2"}))
        .await;

    page.write(&[("page.html", "<h1>Page</h1>\n")]);
    post_next_version(&harness, &page).await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    let events = harness.events().await;
    let stopped = position(&events, "worker.stopped", |event| {
        event.data["agent"] == TASK_WORKER
    });
    let sent = position(&events, "proposal.version_added", |_| true);
    assert!(
        stopped > sent,
        "the task worker stops only once the requested change is sent"
    );
    assert_eq!(
        fake.runs_of(TASK_WORKER).len(),
        1,
        "a running task worker gets the review as a notice, not a resume"
    );
}

/// A supervisor whose watcher falls behind its bus still tells a running task worker about changes requested while it fell behind, from the stored reviews.
#[tokio::test]
async fn a_running_task_worker_hears_of_changes_its_supervisor_missed() {
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
    until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
    harness
        .project
        .clock()
        .advance(Duration::from_secs(1))
        .unwrap();
    let handle = &harness.handle;
    let ana = HumanName::parse("ana").unwrap();
    let now = harness.project.now();
    harness
        .state
        .registry
        .write(async |tx| fold_store::projects::ensure_human(tx, &ana, now).await)
        .await
        .unwrap();
    // The review goes out on a bus of its own, so neither the supervisor nor the session push receives its event.
    let reviews = ReviewService::new(
        handle.store.clone(),
        EventBus::new(),
        handle.clock.clone(),
        WorkspaceService::for_project(handle),
        handle.integration.clone(),
    )
    .with_registry(harness.state.registry.clone());
    reviews
        .submit_review(
            &Actor::human(ana),
            "P-1".parse().unwrap(),
            1,
            Verdict::RequestChanges,
            "Split the page.",
            Vec::new(),
        )
        .await
        .unwrap();
    // More messages than the bus holds, sent while the watcher cannot run, so it falls behind.
    for _ in 0..1100 {
        handle
            .bus
            .publish_live(LiveEvent::new(types::WORKER_OUTPUT, None, json!({})).unwrap());
    }
    let runs = eventually(WAIT, async || {
        let runs = fake.runs_of(TASK_WORKER);
        let told = runs[0]
            .stdin
            .iter()
            .any(|line| line.contains("Split the page."));
        if told {
            Ok(runs)
        } else {
            Err(runs[0].stdin.len())
        }
    })
    .await;
    assert_eq!(
        runs.len(),
        1,
        "a running task worker gets a notice, not a resume"
    );
}

#[tokio::test]
async fn a_batch_review_resumes_the_task_worker_once_with_every_member_it_wrote() {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(
        &fake,
        &["Draw the page", "Draw the menu"],
        WorkerState::Idle,
    )
    .await;
    let page = harness.start_work(TASK_WORKER, "T-1").await;
    page.write(&[("page.html", "<p>page</p>\n")]);
    harness.propose(TASK_WORKER, &page, "Draw the page").await;
    let menu = harness.start_work(TASK_WORKER, "T-2").await;
    menu.write(&[("menu.html", "<p>menu</p>\n")]);
    harness.propose(TASK_WORKER, &menu, "Draw the menu").await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    harness
        .call(
            "review",
            "create_review_batch",
            json!({"proposals": ["P-1", "P-2"]}),
        )
        .await;
    harness
        .call(
            "review",
            "submit_batch_review",
            json!({
                "id": "rb-1",
                "verdicts": [
                    {"proposal": "P-1", "verdict": "request_changes", "body": "Split the page."},
                    {"proposal": "P-2", "verdict": "request_changes", "body": "Sort the menu."}
                ],
                "comments": [
                    {"members": ["P-1"], "body": "Use a heading."},
                    {"members": ["P-1", "P-2"], "body": "Share the styles."}
                ]
            }),
        )
        .await;

    let resumed = until_resumed(&harness, 2).await;
    let proposals: Vec<Option<&str>> = resumed.iter().map(|(p, _)| p.as_deref()).collect();
    assert_eq!(proposals, [Some("P-1"), Some("P-2")]);
    let runs = fake.runs_of(TASK_WORKER);
    assert_eq!(runs.len(), 2, "one resume for the whole batch");
    let turn = &runs[1].stdin[1];
    for wanted in [
        "P-1 v1 for T-1: changes requested",
        "P-2 v1 for T-2: changes requested",
        "Split the page.",
        "Sort the menu.",
        "Use a heading.",
        "Share the styles.",
        "update_proposal for P-2 to send v2",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
}

#[tokio::test]
async fn a_resume_the_worker_limits_refuse_runs_once_a_worker_stops() {
    let fake = FakeClaude::new("result");
    let (harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    std::fs::write(
        std::path::Path::new(harness.root()).join(".fold/config.toml"),
        "[workers]\nmax_per_area = 1\n",
    )
    .unwrap();
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
    until_state(&harness, &other, WorkerState::Idle).await;
    harness
        .project
        .clock()
        .advance(Duration::from_secs(1))
        .unwrap();
    request_changes(&harness, "P-1", "Split the page.").await;
    assert_eq!(
        worker(&harness, TASK_WORKER).await.state,
        WorkerState::Stopped,
        "the area holds the other task worker"
    );

    harness
        .call("web-lead", "stop_worker", json!({"name": other}))
        .await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert!(runs[1].has("--resume"));
    assert!(
        runs[1].stdin[1].contains("Split the page."),
        "{}",
        runs[1].stdin[1]
    );
    let resumed = until_resumed(&harness, 1).await;
    assert_eq!(resumed[0].1["resumed"], true);
}

#[tokio::test]
async fn a_daemon_start_serves_a_resume_the_worker_limits_refused() {
    let fake = FakeClaude::new("result");
    let (mut harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    let config = std::path::Path::new(harness.root()).join(".fold/config.toml");
    std::fs::write(&config, "[workers]\ntask_worker = 1\n").unwrap();
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
    until_state(&harness, &other, WorkerState::Idle).await;
    harness
        .project
        .clock()
        .advance(Duration::from_secs(1))
        .unwrap();
    request_changes(&harness, "P-1", "Split the page.").await;
    harness.stop().await;
    assert_eq!(
        fake.runs_of(TASK_WORKER).len(),
        1,
        "the other task worker holds the only place"
    );
    std::fs::write(&config, "[workers]\ntask_worker = 2\n").unwrap();

    harness.start_again().await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert!(runs[1].has("--resume"));
    assert!(runs[1].stdin[1].contains("Split the page."));
    until_resumed(&harness, 1).await;
}

#[tokio::test]
async fn a_daemon_start_that_finds_a_task_workers_session_gone_starts_it_fresh() {
    let fake = FakeClaude::new("init").lost_sessions();
    let mut harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Working).await;
    let working = worker(&harness, TASK_WORKER).await;
    until_runs(&fake, TASK_WORKER, 1).await;
    harness.stop().await;

    harness.start_again().await;
    let runs = until_runs(&fake, TASK_WORKER, 3).await;
    assert_eq!(runs[1].flag("--resume"), Some(working.session_id.as_str()));
    let fresh = &runs[2];
    assert!(!fresh.has("--resume"));
    let session = fresh.flag("--session-id").unwrap();
    assert_ne!(session, working.session_id);
    for wanted in ["task worker for T-1", "your earlier session is gone"] {
        assert!(
            fresh.stdin[1].contains(wanted),
            "{wanted} in {}",
            fresh.stdin[1]
        );
    }
    let row = until_state(&harness, TASK_WORKER, WorkerState::Working).await;
    assert_eq!(row.session_id, session);
    let started: Vec<Value> = harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "worker.started" && event.data["agent"] == TASK_WORKER)
        .map(|event| event.data["resumed"].clone())
        .collect();
    assert_eq!(
        started,
        [Value::Null, json!(false)],
        "the first start and the new session"
    );
}

const LEAD: &str = "web-lead";

/// Area `web` over label `ui` and its lead, started by the main agent and in `state`.
async fn lead_in(fake: &FakeClaude, state: WorkerState) -> (Harness, WorkerProcessRow) {
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
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
    until_runs(fake, LEAD, 1).await;
    let row = until_state(&harness, LEAD, state).await;
    (harness, row)
}

/// The data of every stored `worker.resumed` of reason `session_lost`, once there is one.
async fn until_session_lost(harness: &Harness) -> Vec<Value> {
    eventually(WAIT, async || {
        let lost: Vec<Value> = harness
            .events()
            .await
            .into_iter()
            .filter(|event| {
                event.kind == "worker.resumed" && event.data["reason"] == "session_lost"
            })
            .map(|event| {
                assert_eq!(event.proposal, None);
                event.data
            })
            .collect();
        if lost.is_empty() { Err(0) } else { Ok(lost) }
    })
    .await
}

#[tokio::test]
async fn a_daemon_start_that_finds_a_leads_session_gone_starts_it_fresh_with_its_note() {
    let fake = FakeClaude::new("init").lost_sessions();
    let (mut harness, working) = lead_in(&fake, WorkerState::Working).await;
    let now = harness.project.now();
    harness
        .handle
        .store
        .write(async |tx| {
            fold_store::agents::insert_handoff(
                tx,
                working.agent_id,
                &working.session_id,
                "Split the menu task next.",
                now,
            )
            .await
        })
        .await
        .unwrap();
    harness.stop().await;

    harness.start_again().await;
    let runs = until_runs(&fake, LEAD, 3).await;
    assert_eq!(runs[1].flag("--resume"), Some(working.session_id.as_str()));
    let fresh = &runs[2];
    assert!(!fresh.has("--resume"));
    let session = fresh.flag("--session-id").unwrap();
    assert_ne!(session, working.session_id);
    let turn = &fresh.stdin[1];
    for wanted in [
        "the area lead of area web",
        "Your earlier session is gone",
        "Your latest hand-off note:",
        "Split the menu task next.",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
    assert_eq!(
        until_session_lost(&harness).await,
        [json!({"agent": LEAD, "reason": "session_lost", "resumed": false})]
    );
    let row = until_state(&harness, LEAD, WorkerState::Working).await;
    assert_eq!(row.session_id, session);
}

#[tokio::test]
async fn a_start_of_a_stopped_lead_whose_session_is_gone_starts_it_fresh() {
    let fake = FakeClaude::new("result").lost_sessions();
    let (harness, idle) = lead_in(&fake, WorkerState::Idle).await;
    harness
        .call(MAIN, "stop_worker", json!({"name": LEAD}))
        .await;
    until_state(&harness, LEAD, WorkerState::Stopped).await;

    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web", "instructions": "Plan the menu first."}),
        )
        .await;
    let runs = until_runs(&fake, LEAD, 3).await;
    assert_eq!(runs[1].flag("--resume"), Some(idle.session_id.as_str()));
    let fresh = &runs[2];
    assert!(!fresh.has("--resume"));
    let session = fresh.flag("--session-id").unwrap();
    assert_ne!(session, idle.session_id);
    let turn = &fresh.stdin[1];
    for wanted in [
        "the area lead of area web",
        "Your earlier session is gone",
        "Plan the menu first.",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
    assert!(!turn.contains("hand-off note"), "{turn}");
    assert_eq!(
        until_session_lost(&harness).await,
        [json!({"agent": LEAD, "reason": "session_lost", "resumed": false})]
    );
    let row = until_state(&harness, LEAD, WorkerState::Idle).await;
    assert_eq!(row.session_id, session);
}

#[tokio::test]
async fn a_daemon_start_that_finds_a_reviewers_session_gone_starts_it_fresh() {
    let fake = FakeClaude::new("init").lost_sessions();
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
    let working = until_state(&harness, reviewer, WorkerState::Working).await;
    harness.stop().await;

    harness.start_again().await;
    let runs = until_runs(&fake, reviewer, 3).await;
    assert_eq!(runs[1].flag("--resume"), Some(working.session_id.as_str()));
    let fresh = &runs[2];
    assert!(!fresh.has("--resume"));
    let session = fresh.flag("--session-id").unwrap();
    assert_ne!(session, working.session_id);
    let turn = &fresh.stdin[1];
    for wanted in [
        "reviewer for P-1",
        "Your earlier session is gone",
        "give a verdict only to those that have none from you",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
    assert!(
        until_session_lost(&harness)
            .await
            .contains(&json!({"agent": reviewer, "reason": "session_lost", "resumed": false}))
    );
    let row = until_state(&harness, reviewer, WorkerState::Working).await;
    assert_eq!(row.session_id, session);
    assert_eq!(
        row.proposals,
        [fold_core::ProposalId::from_number(1).unwrap()]
    );
}

#[tokio::test]
async fn a_raised_limit_serves_a_resume_the_worker_limits_refused() {
    let fake = FakeClaude::new("result");
    let (harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    harness
        .call(MAIN, "set_worker_limits", json!({"max_per_area": 1}))
        .await;
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
    until_state(&harness, &other, WorkerState::Idle).await;
    harness
        .project
        .clock()
        .advance(Duration::from_secs(1))
        .unwrap();
    request_changes(&harness, "P-1", "Split the page.").await;
    assert_eq!(
        worker(&harness, TASK_WORKER).await.state,
        WorkerState::Stopped,
        "the area holds the other task worker"
    );

    harness
        .call(MAIN, "set_worker_limits", json!({"max_per_area": 2}))
        .await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert!(runs[1].has("--resume"));
    assert!(
        runs[1].stdin[1].contains("Split the page."),
        "{}",
        runs[1].stdin[1]
    );
    assert_eq!(worker(&harness, &other).await.state, WorkerState::Idle);
    until_resumed(&harness, 1).await;
}

/// A stand-in for Claude Code that records whether the daemon's socket exists as it starts, which is when its `fld mcp` looks for the daemon, then reads its input until fold closes it.
const SOCKET_PROBE: &str = r#"#!/bin/sh
if [ -S "$FOLD_TEST_SOCKET" ]; then state=bound; else state=missing; fi
printf '%s\n' "$state" > "$FOLD_TEST_PROBE.tmp" && mv "$FOLD_TEST_PROBE.tmp" "$FOLD_TEST_PROBE"
cat > /dev/null
"#;

#[tokio::test]
async fn a_daemon_start_resumes_a_working_task_worker_once_its_socket_is_bound() {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    use fold_daemon::services::agents::Launcher;
    use fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME;
    use fold_daemon::{Daemon, DaemonConfig};
    use fold_store::projects::{self, ProjectRow};
    use fold_store::{Store, StoreError};

    let fake = FakeClaude::new("init");
    let mut harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Working).await;
    until_runs(&fake, TASK_WORKER, 1).await;
    harness.stop().await;

    let dir = tempfile::TempDir::new().unwrap();
    let program = dir.path().join("claude");
    std::fs::write(&program, SOCKET_PROBE).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let probe = dir.path().join("probe");
    let config = DaemonConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        socket: dir.path().join("run").join("fold.sock"),
        data_dir: dir.path().join("data"),
        runtime_dir: dir.path().join("run"),
        web_dir: None,
        session_lifetime: DEFAULT_SESSION_LIFETIME,
    };
    let registry = Store::open_registry(&config.registry_path()).await.unwrap();
    let row = ProjectRow {
        slug: SLUG.to_owned(),
        name: "Demo".to_owned(),
        root: harness.root().to_owned(),
    };
    let now = harness.project.now();
    registry
        .write(async |tx| {
            projects::insert_project(tx, &row, now).await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    registry.close().await.unwrap();
    let launcher = Launcher {
        claude: program,
        fold: Some(std::path::PathBuf::from("/usr/local/bin/fld")),
        env: vec![
            (
                "FOLD_TEST_SOCKET".to_owned(),
                config.socket.to_string_lossy().into_owned(),
            ),
            (
                "FOLD_TEST_PROBE".to_owned(),
                probe.to_string_lossy().into_owned(),
            ),
        ],
    };
    let clock = Arc::new(harness.project.clock().clone());
    let daemon = Daemon::start_with(config, clock, launcher).await.unwrap();
    let seen = eventually(WAIT, async || {
        std::fs::read_to_string(&probe).map_err(|error| error.kind())
    })
    .await;
    assert_eq!(
        seen, "bound\n",
        "the resumed task worker found the daemon's socket"
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn red_proposal_checks_resume_a_stopped_task_worker_with_the_failed_check() {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    let outside = tempfile::tempdir().unwrap();
    let gate = outside.path().join("go");
    work.write(&[
        ("page.html", "<p>page</p>\n"),
        (
            "fold-checks.toml",
            &format!(
                "[[check]]\nname = \"render\"\nrun = \"until [ -e '{}' ]; do sleep 0.05; done; exit 1\"\n",
                gate.display()
            ),
        ),
    ]);
    let created = harness
        .call(
            TASK_WORKER,
            "create_proposal",
            json!({"task": "T-1", "title": "Draw the page"}),
        )
        .await;
    assert_eq!(created["checks"], "running", "{created}");
    harness
        .call("web-lead", "stop_worker", json!({"name": TASK_WORKER}))
        .await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    std::fs::write(&gate, "").unwrap();

    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert!(runs[1].has("--resume"));
    let turn = &runs[1].stdin[1];
    for wanted in [
        "The checks of T-1 (cr-1) did not pass, so fold made no proposal: render failed.",
        "call create_proposal again",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
    let resumed = until_resumed(&harness, 1).await;
    assert_eq!(resumed[0].1["reason"], "checks");
    assert_eq!(resumed[0].1["stage"], "proposal");
    assert_eq!(resumed[0].1["run"], "cr-1");
}
