#![cfg(test)]

use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::{
    Actor, AgentActor, AgentName, HumanName, IdKind, PermissionQuestionId, ProposalId, Role,
    WorkerName, WorkerState,
};
use fold_daemon::services::agents::{PAUSE_TURN, StartWorker};
use fold_store::agents::{self, NewQuestion, QuestionKind};
use fold_store::testing::eventually;
use fold_store::{StoreError, counters};
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, SLUG, WAIT};
use crate::worker_resume::{TASK_WORKER, proposed_by_a_task_worker, request_changes};
use crate::workers_support::{FakeClaude, Run};

const BEFORE_SPAWN: &str = "worker_launch_before_spawn";
const BEFORE_STOP: &str = "worker_sweep_before_stop";
const BEFORE_WAITER: &str = "worker_question_before_waiter";
const MAIN: &str = "frontend";

/// Holds the code that reaches failpoint `name` until the returned sender sends; the receiver reports the arrival.
fn hold(name: &str) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (arrived, arrivals) = mpsc::channel();
    let (release, releases) = mpsc::channel::<()>();
    let arrived = Mutex::new(arrived);
    let releases = Mutex::new(releases);
    fail::cfg_callback(name, move || {
        let _ = arrived.lock().unwrap().send(());
        let _ = releases.lock().unwrap().recv_timeout(WAIT);
    })
    .unwrap();
    (arrivals, release)
}

async fn arrival(arrivals: mpsc::Receiver<()>) {
    tokio::task::spawn_blocking(move || arrivals.recv_timeout(WAIT))
        .await
        .unwrap()
        .expect("the failpoint was reached");
}

/// The answers fold wrote on a run's stdin.
fn responses(run: &Run) -> Vec<Value> {
    run.stdin
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["type"] == "control_response")
        .map(|line| line["response"]["response"].clone())
        .collect()
}

/// A stop that lands after the start committed but before its process is known finds nothing to end; the launch that follows must end the process itself and give it no turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_before_the_process_starts_leaves_no_process_running() {
    let scenario = fail::FailScenario::setup();
    let fake = FakeClaude::new("init");
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    let human = Actor::human(HumanName::parse("ana").unwrap());
    let lead = AgentName::parse("review-lead").unwrap();
    fail::cfg(BEFORE_SPAWN, "pause").unwrap();
    let agents = harness.handle.agents();
    let starter = human.clone();
    let start = tokio::spawn(async move {
        agents
            .start_worker(
                &starter,
                StartWorker {
                    role: Some(Role::ReviewLead),
                    ..StartWorker::default()
                },
            )
            .await
    });
    eventually(WAIT, async || {
        match harness.handle.agents().get_worker(&lead).await {
            Ok(row) if row.state == WorkerState::Starting => Ok(()),
            other => Err(other.map(|row| row.state).ok()),
        }
    })
    .await;
    let stopped = harness
        .handle
        .agents()
        .stop_worker(&human, &lead)
        .await
        .unwrap();
    assert_eq!(stopped.state, WorkerState::Stopped);
    fail::remove(BEFORE_SPAWN);
    let started = start.await.unwrap().unwrap();
    assert_eq!(started.state, WorkerState::Stopped);
    eventually(WAIT, async || {
        if harness.handle.workers.is_running(&lead) {
            Err(())
        } else {
            Ok(())
        }
    })
    .await;
    let row = harness.handle.agents().get_worker(&lead).await.unwrap();
    assert_eq!(row.state, WorkerState::Stopped);
    assert_eq!(row.pid, None);
    let runs = fake.runs_of(lead.as_str());
    assert_eq!(runs.len(), 1, "the launch started its process and ended it");
    assert!(
        runs[0]
            .stdin
            .iter()
            .all(|line| !line.contains("\"type\":\"user\"")),
        "{:?}",
        runs[0].stdin
    );
    let stops = harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "worker.stopped")
        .count();
    assert_eq!(stops, 1);
    scenario.teardown();
}

/// A pause that lands after the start committed but before its process is known finds nothing to end; the launch ends the process itself, keeps the row `starting` with its session, and the resume runs it in that session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pause_before_the_process_starts_keeps_the_row_for_the_resume() {
    let scenario = fail::FailScenario::setup();
    let fake = FakeClaude::new("init");
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    let human = Actor::human(HumanName::parse("ana").unwrap());
    let lead = AgentName::parse("review-lead").unwrap();
    fail::cfg(BEFORE_SPAWN, "pause").unwrap();
    let agents = harness.handle.agents();
    let starter = human.clone();
    let start = tokio::spawn(async move {
        agents
            .start_worker(
                &starter,
                StartWorker {
                    role: Some(Role::ReviewLead),
                    ..StartWorker::default()
                },
            )
            .await
    });
    eventually(WAIT, async || {
        match harness.handle.agents().get_worker(&lead).await {
            Ok(row) if row.state == WorkerState::Starting => Ok(()),
            other => Err(other.map(|row| row.state).ok()),
        }
    })
    .await;
    harness.handle.agents().pause_project(&human).await.unwrap();
    fail::remove(BEFORE_SPAWN);
    start.await.unwrap().unwrap();
    eventually(WAIT, async || {
        if harness.handle.workers.is_running(&lead) {
            Err(())
        } else {
            Ok(())
        }
    })
    .await;
    let kept = harness.handle.agents().get_worker(&lead).await.unwrap();
    assert_eq!(kept.state, WorkerState::Starting);
    assert_eq!(kept.pid, None);
    let runs = fake.runs_of(lead.as_str());
    assert_eq!(runs.len(), 1, "the launch started its process and ended it");
    assert!(
        runs[0]
            .stdin
            .iter()
            .all(|line| !line.contains("\"type\":\"user\"")),
        "{:?}",
        runs[0].stdin
    );
    let ended = harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "worker.stopped" || event.kind == "worker.failed")
        .count();
    assert_eq!(ended, 0);

    harness
        .handle
        .agents()
        .resume_project(&human)
        .await
        .unwrap();
    let runs = eventually(WAIT, async || {
        let runs = fake.runs_of(lead.as_str());
        match runs.get(1) {
            Some(run) if run.stdin.iter().any(|line| line.contains(PAUSE_TURN)) => Ok(runs),
            _ => Err(runs.len()),
        }
    })
    .await;
    assert_eq!(runs[1].flag("--resume"), Some(kept.session_id.as_str()));
    let turn = runs[1]
        .stdin
        .iter()
        .find(|line| line.contains(PAUSE_TURN))
        .unwrap();
    assert!(
        turn.contains("fold started you as review-lead, the review lead."),
        "the first turn the pause kept from the process follows the pause note: {turn}"
    );
    scenario.teardown();
}

/// A task worker that requested changes resume and a pause caught before its process read the turn still hears of the changes at the resume, although its row started after the review.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pause_that_catches_an_automatic_resume_keeps_the_owed_changes_for_the_resume() {
    let scenario = fail::FailScenario::setup();
    let fake = FakeClaude::new("result");
    let (harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    let task_worker = AgentName::parse(TASK_WORKER).unwrap();
    let human = Actor::human(HumanName::parse("ana").unwrap());
    harness
        .project
        .clock()
        .advance(Duration::from_secs(1))
        .unwrap();
    fail::cfg(BEFORE_SPAWN, "pause").unwrap();
    request_changes(&harness, "P-1", "Split the page.").await;
    eventually(WAIT, async || {
        match harness.handle.agents().get_worker(&task_worker).await {
            Ok(row) if row.state == WorkerState::Starting => Ok(()),
            other => Err(other.map(|row| row.state).ok()),
        }
    })
    .await;
    harness.handle.agents().pause_project(&human).await.unwrap();
    fail::remove(BEFORE_SPAWN);
    eventually(WAIT, async || {
        let row = harness
            .handle
            .agents()
            .get_worker(&task_worker)
            .await
            .unwrap();
        if fake.runs_of(TASK_WORKER).len() == 2 && !harness.handle.workers.is_running(&task_worker)
        {
            Ok(row)
        } else {
            Err(row.state)
        }
    })
    .await;
    assert_eq!(
        harness
            .handle
            .agents()
            .get_worker(&task_worker)
            .await
            .unwrap()
            .state,
        WorkerState::Starting
    );

    harness
        .handle
        .agents()
        .resume_project(&human)
        .await
        .unwrap();
    let runs = eventually(WAIT, async || {
        let runs = fake.runs_of(TASK_WORKER);
        match runs.get(2) {
            Some(run) if run.stdin.iter().any(|line| line.contains(PAUSE_TURN)) => Ok(runs),
            _ => Err(runs.len()),
        }
    })
    .await;
    let turn = runs[2]
        .stdin
        .iter()
        .find(|line| line.contains(PAUSE_TURN))
        .unwrap();
    assert!(turn.contains("Split the page."), "{turn}");
    scenario.teardown();
}

/// The idle sweep reads the rows first and stops each due worker after; a worker that took a turn in between is idle since a later moment and keeps running.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_idle_sweep_leaves_a_worker_that_took_a_turn_after_the_rows_were_read() {
    let scenario = fail::FailScenario::setup();
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
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
            "create_task",
            json!({"title": "Draw the page", "labels": ["ui"]}),
        )
        .await;
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    harness.connect("web-lead").await;
    harness
        .call(
            "web-lead",
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    let task_worker = AgentName::parse("web-worker").unwrap();
    let until_idle = async |turns: u64| {
        eventually(WAIT, async || {
            let row = harness
                .handle
                .agents()
                .get_worker(&task_worker)
                .await
                .unwrap();
            if row.state == WorkerState::Idle && row.turns == turns {
                Ok(row)
            } else {
                Err((row.state, row.turns))
            }
        })
        .await
    };
    let first = until_idle(1).await;
    let clock = harness.project.clock().clone();
    let due = clock.advance(Duration::from_secs(16 * 60)).unwrap();
    let (arrivals, release) = hold(BEFORE_STOP);
    let workers = harness.handle.workers.clone();
    let sweep = tokio::spawn(async move { workers.sweep_idle(due).await });
    arrival(arrivals).await;
    harness
        .call(
            "web-lead",
            "send_message",
            json!({"to": ["web-worker"], "body": "one more thing"}),
        )
        .await;
    let again = until_idle(2).await;
    assert!(again.idle_since > first.idle_since);
    release.send(()).unwrap();
    fail::remove(BEFORE_STOP);
    let stopped = sweep.await.unwrap().unwrap();
    assert!(stopped.is_empty(), "{stopped:?}");
    let row = harness
        .handle
        .agents()
        .get_worker(&task_worker)
        .await
        .unwrap();
    assert_eq!(row.state, WorkerState::Idle);
    assert!(harness.handle.workers.is_running(&task_worker));

    let later = clock.advance(Duration::from_secs(16 * 60)).unwrap();
    let stopped = harness.handle.workers.sweep_idle(later).await.unwrap();
    assert_eq!(stopped, [task_worker]);
    scenario.teardown();
}

/// A repeat of a call that finds its question open joins it as a waiter; an answer that lands between that read and the join must still reach the repeat.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_between_the_question_read_and_the_join_reaches_the_waiting_call() {
    let scenario = fail::FailScenario::setup();
    let fake = FakeClaude::new("permission_request");
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    let human = Actor::human(HumanName::parse("ana").unwrap());
    let lead = AgentName::parse("review-lead").unwrap();
    fail::cfg(BEFORE_SPAWN, "pause").unwrap();
    let service = harness.handle.agents();
    let starter = human.clone();
    let start = tokio::spawn(async move {
        service
            .start_worker(
                &starter,
                StartWorker {
                    role: Some(Role::ReviewLead),
                    ..StartWorker::default()
                },
            )
            .await
    });
    let row = eventually(WAIT, async || {
        match harness.handle.agents().get_worker(&lead).await {
            Ok(row) if row.state == WorkerState::Starting => Ok(row),
            other => Err(other.map(|row| row.state).ok()),
        }
    })
    .await;
    // The question an earlier process of the worker left open, for the call the fixture repeats.
    let input = json!({"command": "echo fold-probe-allow", "description": "Echo the fold-probe-allow string"});
    let now = harness.project.now();
    harness
        .project
        .store()
        .write(async |tx| {
            let number = counters::allocate(tx, IdKind::PermissionQuestion).await?;
            agents::insert_question(
                tx,
                &NewQuestion {
                    worker_process_id: row.id,
                    kind: QuestionKind::Tool,
                    number: PermissionQuestionId::from_number(number),
                    session_id: &row.session_id,
                    request_id: "earlier",
                    tool_use_id: None,
                    tool: "Bash",
                    input: &input,
                    now,
                },
            )
            .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let (arrivals, release) = hold(BEFORE_WAITER);
    fail::remove(BEFORE_SPAWN);
    start.await.unwrap().unwrap();
    arrival(arrivals).await;
    let (status, answered) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/worker-questions/pq-1/answer"),
            Some(&json!({"behavior": "allow"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    release.send(()).unwrap();
    fail::remove(BEFORE_WAITER);
    let run = eventually(WAIT, async || {
        let run = fake.runs_of(lead.as_str()).remove(0);
        if responses(&run).is_empty() {
            Err(run.stdin.len())
        } else {
            Ok(run)
        }
    })
    .await;
    let allowed = responses(&run).remove(0);
    assert_eq!(allowed["behavior"], "allow");
    assert_eq!(allowed["updatedInput"]["command"], "echo fold-probe-allow");
    scenario.teardown();
}

/// A daemon that stops after a start committed and before the worker's process ran leaves the worker `starting`. The next daemon start marks it failed, and a worker that never took a turn gives back the claims and the batch its lead gave it, so the lead gives them to a new reviewer at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reviewer_that_never_ran_before_a_restart_gives_back_its_claims_and_its_batch() {
    let scenario = fail::FailScenario::setup();
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    for number in 1..=2 {
        let task = harness.create_task(MAIN, &format!("Part {number}")).await;
        let work = harness.start_work(MAIN, &task).await;
        std::fs::write(work.dir.join(format!("part{number}.txt")), "part\n").unwrap();
        harness.propose(MAIN, &work, &task).await;
    }
    harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    let lead = AgentName::parse("review-lead").unwrap();
    let reviewer = AgentName::parse("reviewer").unwrap();
    fail::cfg(BEFORE_SPAWN, "panic").unwrap();
    let agents = harness.handle.agents();
    let starter = Actor::Agent(AgentActor {
        agent: lead.clone(),
        worker: WorkerName::main(&lead),
        tags: Vec::new(),
    });
    let proposals = ["P-1", "P-2"].map(|id| id.parse::<ProposalId>().unwrap());
    let start = tokio::spawn(async move {
        agents
            .start_worker(
                &starter,
                StartWorker {
                    role: Some(Role::Reviewer),
                    proposals: proposals.to_vec(),
                    ..StartWorker::default()
                },
            )
            .await
    });
    assert!(start.await.is_err(), "the start stopped after its commit");
    fail::remove(BEFORE_SPAWN);
    let left = harness.handle.agents().get_worker(&reviewer).await.unwrap();
    assert_eq!(left.state, WorkerState::Starting);

    harness.restart().await;
    eventually(WAIT, async || {
        match harness.handle.agents().get_worker(&reviewer).await {
            Ok(row) if row.state == WorkerState::Failed => Ok(()),
            other => Err(other.map(|row| row.state).ok()),
        }
    })
    .await;
    harness.connect(lead.as_str()).await;
    for proposal in ["P-1", "P-2"] {
        let view = harness
            .call(MAIN, "get_proposal", json!({"id": proposal}))
            .await;
        assert_eq!(view["claim"], Value::Null, "{view}");
    }
    let batch = harness
        .call(lead.as_str(), "get_review_batch", json!({"id": "rb-1"}))
        .await;
    assert_eq!(batch["state"], "closed", "{batch}");
    let again = harness
        .call(
            lead.as_str(),
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-1", "P-2"]}),
        )
        .await;
    assert_eq!(again["worker"]["name"], "reviewer-2", "{again}");
    let view = harness
        .call(MAIN, "get_proposal", json!({"id": "P-1"}))
        .await;
    assert_eq!(view["claim"]["holder"], "reviewer-2", "{view}");
    scenario.teardown();
}
