#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_core::{Actor, AgentName, HumanName, WorkerState, codes};
use fold_store::agents::WorkerProcessRow;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, SLUG, WAIT};
use crate::workers_support::{FakeClaude, Run};

const MAIN: &str = "frontend";
const LEAD: &str = "web-lead";
const TASK_WORKER: &str = "web-worker";

async fn worker(harness: &Harness, agent: &str) -> WorkerProcessRow {
    harness
        .handle
        .agents()
        .get_worker(&AgentName::parse(agent).unwrap())
        .await
        .unwrap()
}

async fn until_state(harness: &Harness, agent: &str, state: WorkerState) -> WorkerProcessRow {
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
async fn until_runs(fake: &FakeClaude, agent: &str, count: usize) -> Vec<Run> {
    eventually(WAIT, async || {
        let runs = fake.runs_of(agent);
        match runs.last() {
            Some(last) if runs.len() == count && last.stdin.len() >= 2 => Ok(runs),
            _ => Err(runs.len()),
        }
    })
    .await
}

/// Every stdin line of `agent`'s runs once one holds `text`.
async fn until_stdin(fake: &FakeClaude, agent: &str, text: &str) -> Vec<String> {
    eventually(WAIT, async || {
        let lines: Vec<String> = fake
            .runs_of(agent)
            .into_iter()
            .flat_map(|run| run.stdin)
            .collect();
        if lines.iter().any(|line| line.contains(text)) {
            Ok(lines)
        } else {
            Err(lines.len())
        }
    })
    .await
}

async fn task(harness: &Harness, id: &str) -> Value {
    harness.call(MAIN, "get_task", json!({"id": id})).await
}

/// Area `web` over label `ui` with T-1, a task without a proposal, and T-2, which needs T-1; the area lead `web-lead` and the task worker `web-worker` it started for T-1, both connected.
async fn designed_by_a_task_worker(fake: &FakeClaude) -> Harness {
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    harness
        .call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    harness
        .call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    let created = harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Update the board screens in Pencil", "labels": ["ui"], "kind": "no_proposal"}),
        )
        .await;
    assert_eq!(created, json!({"id": "T-1", "status": "todo"}));
    let dependent = harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Build the board", "labels": ["ui"], "needs": ["T-1"]}),
        )
        .await;
    assert_eq!(dependent["status"], "backlog");
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    harness.connect(LEAD).await;
    let started = harness
        .call(
            LEAD,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    assert_eq!(started["worker"]["name"], TASK_WORKER);
    until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
    harness.connect(TASK_WORKER).await;
    harness
}

/// The task worker, which holds the claim on T-1 from its start, reports it finished, after which fold stops it.
async fn reported(harness: &Harness, note: &str) -> Value {
    let done = harness
        .call(
            TASK_WORKER,
            "report_task",
            json!({"id": "T-1", "note": note}),
        )
        .await;
    assert_eq!(done, json!({"id": "T-1", "status": "review"}));
    until_state(harness, TASK_WORKER, WorkerState::Stopped).await;
    task(harness, "T-1").await
}

#[tokio::test]
async fn a_task_without_a_proposal_goes_from_todo_to_done_through_its_claim_report_and_check() {
    let fake = FakeClaude::new("result");
    let mut harness = designed_by_a_task_worker(&fake).await;
    assert_eq!(task(&harness, "T-1").await["kind"], "no_proposal");

    let claimed = task(&harness, "T-1").await;
    assert_eq!(claimed["claim"]["holder"], TASK_WORKER, "{claimed}");
    assert_eq!(claimed["status"], "in_progress", "{claimed}");
    assert_eq!(claimed["workspace"], Value::Null);
    assert_eq!(
        claimed["status_reason"],
        "Calculated: a task worker holds the claim, no report yet"
    );
    for (tool, arguments) in [
        ("create_workspace", json!({"task": "T-1"})),
        (
            "create_proposal",
            json!({"intent": "Make the change the task asks for.", "task": "T-1", "title": "Draw the board"}),
        ),
    ] {
        let refused = harness.refused(TASK_WORKER, tool, arguments).await;
        assert_eq!(refused["code"], codes::TASK_KIND_MISMATCH, "{tool}");
    }
    let early = harness
        .refused(LEAD, "complete_task", json!({"id": "T-1"}))
        .await;
    assert_eq!(early["code"], codes::TASK_NOT_REPORTED);

    let done = harness
        .call(
            TASK_WORKER,
            "report_task",
            json!({"id": "T-1", "note": "Drew the five board screens."}),
        )
        .await;
    assert_eq!(done, json!({"id": "T-1", "status": "review"}));
    let again = harness
        .refused(
            TASK_WORKER,
            "report_task",
            json!({"id": "T-1", "note": "Again."}),
        )
        .await;
    assert_eq!(again["code"], codes::TASK_ALREADY_REPORTED);
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    let waiting = task(&harness, "T-1").await;
    assert_eq!(waiting["status"], "review");
    assert_eq!(
        waiting["reports"],
        json!([{"by": TASK_WORKER, "note": "Drew the five board screens.", "resolution": null, "resolved_by": null, "send_back_note": null}])
    );

    let own = harness
        .refused(TASK_WORKER, "complete_task", json!({"id": "T-1"}))
        .await;
    assert_eq!(own["code"], codes::TASK_CHECK_DENIED);
    harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    harness.connect("review-lead").await;
    let reviewer = harness
        .refused("review-lead", "complete_task", json!({"id": "T-1"}))
        .await;
    assert_eq!(reviewer["code"], codes::TASK_CHECK_DENIED);
    let proposals = harness
        .call("review-lead", "list_proposals", json!({}))
        .await;
    assert_eq!(proposals["proposals"], json!([]), "{proposals}");

    let completed = harness
        .call(LEAD, "complete_task", json!({"id": "T-1"}))
        .await;
    assert_eq!(completed, json!({"id": "T-1", "status": "done"}));
    assert_eq!(harness.completions("T-1").await, 1);
    let done = task(&harness, "T-1").await;
    assert_eq!(done["claim"], Value::Null);
    assert_eq!(done["reports"][0]["resolution"], "completed");
    assert_eq!(done["reports"][0]["resolved_by"], LEAD);
    assert_eq!(task(&harness, "T-2").await["status"], "todo");
    let events = harness.events().await;
    let completion = events
        .iter()
        .find(|event| event.kind == "task.completed")
        .unwrap();
    assert_eq!(completion.data, json!({"kind": "check"}));
    assert_eq!(completion.actor, LEAD);
    assert!(
        !events
            .iter()
            .any(|event| event.kind == "workspace.created" || event.kind == "proposal.opened")
    );
}

#[tokio::test]
async fn completing_a_task_without_a_proposal_stops_its_running_task_worker() {
    let fake = FakeClaude::new("result");
    let harness = designed_by_a_task_worker(&fake).await;
    reported(&harness, "Drew the board screens.").await;
    let human = Actor::human(HumanName::parse(HUMAN).unwrap());
    harness
        .handle
        .agents()
        .resume_worker(
            &human,
            &AgentName::parse(TASK_WORKER).unwrap(),
            "Wait for the check.".to_owned(),
        )
        .await
        .unwrap();
    let running = until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
    harness
        .call(LEAD, "complete_task", json!({"id": "T-1"}))
        .await;
    let stopped = until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    assert_eq!(stopped.exit_code, Some(0));
    assert_eq!(
        stopped.session_id, running.session_id,
        "the session stays for its history"
    );
}

#[tokio::test]
async fn a_send_back_resumes_the_same_task_worker_session_with_the_note() {
    let fake = FakeClaude::new("result");
    let harness = designed_by_a_task_worker(&fake).await;
    reported(&harness, "Drew the board screens.").await;
    let stopped = worker(&harness, TASK_WORKER).await;

    let (status, sent) = harness
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/tasks/T-1/send-back"),
            Some(&json!({"note": "Use the new colour tokens."})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    assert_eq!(sent["status"], "in_progress");
    assert_eq!(sent["reports"][0]["resolution"], "sent_back");
    assert_eq!(
        sent["reports"][0]["send_back_note"],
        "Use the new colour tokens."
    );

    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(stopped.session_id.as_str()));
    let turn = &runs[1].stdin[1];
    for wanted in [
        "T-1 was sent back by human:ana",
        "Use the new colour tokens.",
        "call report_task for T-1 again",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
    let resumed = eventually(WAIT, async || {
        let resumed: Vec<Value> = harness
            .events()
            .await
            .into_iter()
            .filter(|event| event.kind == "worker.resumed" && event.task.as_deref() == Some("T-1"))
            .map(|event| event.data)
            .collect();
        if resumed.is_empty() {
            Err(0)
        } else {
            Ok(resumed)
        }
    })
    .await;
    assert_eq!(
        resumed[0],
        json!({"agent": TASK_WORKER, "reason": "sent_back", "resumed": true})
    );
    let running = until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
    assert_eq!(running.session_id, stopped.session_id);

    harness
        .call(
            TASK_WORKER,
            "report_task",
            json!({"id": "T-1", "note": "Switched to the colour tokens."}),
        )
        .await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    let (status, completed) = harness
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/tasks/T-1/complete"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{completed}");
    assert_eq!(completed["status"], "done");
    assert_eq!(completed["reports"].as_array().unwrap().len(), 2);
    assert_eq!(task(&harness, "T-2").await["status"], "todo");
}

#[tokio::test]
async fn the_kind_is_chosen_by_whoever_may_check_the_task() {
    let fake = FakeClaude::new("result");
    let harness = designed_by_a_task_worker(&fake).await;
    let by_task_worker = harness
        .refused(
            TASK_WORKER,
            "create_task",
            json!({"title": "Draw icons", "labels": ["ui"], "kind": "no_proposal"}),
        )
        .await;
    assert_eq!(by_task_worker["code"], codes::TASK_KIND_DENIED);
    let outside = harness
        .refused(
            LEAD,
            "create_task",
            json!({"title": "Draw icons", "kind": "no_proposal"}),
        )
        .await;
    assert_eq!(outside["code"], codes::TASK_KIND_DENIED);
    let unknown = harness
        .refused(
            MAIN,
            "create_task",
            json!({"title": "Draw icons", "kind": "design"}),
        )
        .await;
    assert_eq!(unknown["code"], codes::INVALID_TASK_KIND);
    let created = harness
        .call(
            LEAD,
            "create_task",
            json!({"title": "Draw icons", "labels": ["ui"], "kind": "no_proposal"}),
        )
        .await;
    assert_eq!(
        task(&harness, created["id"].as_str().unwrap()).await["kind"],
        "no_proposal"
    );

    let (status, made) = harness
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/tasks"),
            Some(&json!({"title": "Draw the logo", "kind": "no_proposal"})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["kind"], "no_proposal");
    assert_eq!(made["reports"], json!([]));
    let (status, board) = harness
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/board"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let card = board["columns"][1]["cards"]
        .as_array()
        .unwrap()
        .iter()
        .find(|card| card["task"] == made["task"])
        .unwrap()
        .clone();
    assert_eq!(card["kind"], "no_proposal");
    assert_eq!(card["report"], Value::Null);
    let (status, refused) = harness
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/tasks/T-2/complete"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["error"]["code"], codes::TASK_KIND_MISMATCH);
}

#[tokio::test]
async fn the_board_card_of_a_reported_task_carries_the_report() {
    let fake = FakeClaude::new("result");
    let harness = designed_by_a_task_worker(&fake).await;
    let reported = reported(&harness, "Drew the board screens.").await;
    assert_eq!(
        reported["status_reason"],
        "Calculated: the task worker reported the work finished and it waits for a lead check"
    );
    let (_, board) = harness
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/board"),
            None,
            HUMAN,
        )
        .await;
    let review = &board["columns"][3];
    assert_eq!(review["status"], "review");
    assert_eq!(review["cards"][0]["task"], "T-1");
    assert_eq!(review["cards"][0]["kind"], "no_proposal");
    assert_eq!(review["cards"][0]["report"], "Drew the board screens.");
    let released = harness
        .call(TASK_WORKER, "release_task", json!({"id": "T-1"}))
        .await;
    assert_eq!(released["holder"], Value::Null);
    assert_eq!(task(&harness, "T-1").await["status"], "review");
    let (status, sent) = harness
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/tasks/T-1/send-back"),
            Some(&json!({"note": "  "})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{sent}");
    harness
        .call(
            LEAD,
            "send_back_task",
            json!({"id": "T-1", "note": "Add the empty states."}),
        )
        .await;
    assert_eq!(task(&harness, "T-1").await["status"], "todo");
}

#[tokio::test]
async fn a_send_back_the_worker_limits_refuse_runs_once_a_worker_stops() {
    let fake = FakeClaude::new("result");
    let harness = designed_by_a_task_worker(&fake).await;
    reported(&harness, "Drew the board screens.").await;
    std::fs::write(
        std::path::Path::new(harness.root()).join(".fold/config.toml"),
        "[workers]\nmax_per_area = 1\n",
    )
    .unwrap();
    let other = harness
        .call(
            LEAD,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-2"]}),
        )
        .await;
    let other = other["worker"]["name"].as_str().unwrap().to_owned();
    until_state(&harness, &other, WorkerState::Idle).await;
    harness
        .project
        .clock()
        .advance(std::time::Duration::from_secs(1))
        .unwrap();
    harness
        .call(
            LEAD,
            "send_back_task",
            json!({"id": "T-1", "note": "Add the empty states."}),
        )
        .await;
    assert_eq!(
        worker(&harness, TASK_WORKER).await.state,
        WorkerState::Stopped,
        "the area holds the other task worker"
    );

    harness
        .call(LEAD, "stop_worker", json!({"name": other}))
        .await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert!(runs[1].has("--resume"));
    assert!(
        runs[1].stdin[1].contains("Add the empty states."),
        "{}",
        runs[1].stdin[1]
    );
}

#[tokio::test]
async fn a_report_reaches_the_area_lead_and_its_check_waits_for_the_prerequisites() {
    let fake = FakeClaude::new("result");
    let harness = designed_by_a_task_worker(&fake).await;
    reported(&harness, "Drew the board screens.").await;
    until_stdin(
        &fake,
        LEAD,
        "fold: T-1 is reported finished and waits for your check. Call get_task.",
    )
    .await;

    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Pick the colours", "labels": ["ui"]}),
        )
        .await;
    harness
        .call(
            MAIN,
            "add_dependency",
            json!({"task": "T-1", "needs": "T-3"}),
        )
        .await;
    let early = harness
        .refused(LEAD, "complete_task", json!({"id": "T-1"}))
        .await;
    assert_eq!(early["code"], codes::PREREQUISITES_NOT_DONE, "{early}");
    assert_eq!(
        early["details"]["prerequisites"],
        json!([{"task": "T-3", "status": "todo"}])
    );
    let waiting = task(&harness, "T-1").await;
    assert_eq!(waiting["status"], "review");
    assert_eq!(waiting["reports"][0]["resolution"], Value::Null);
    assert_eq!(harness.completions("T-1").await, 0);

    harness
        .call(
            MAIN,
            "remove_dependency",
            json!({"task": "T-1", "needs": "T-3"}),
        )
        .await;
    let completed = harness
        .call(LEAD, "complete_task", json!({"id": "T-1"}))
        .await;
    assert_eq!(completed, json!({"id": "T-1", "status": "done"}));
}

#[tokio::test]
async fn a_task_with_the_human_review_mark_is_completed_only_by_a_human() {
    let fake = FakeClaude::new("result");
    let harness = designed_by_a_task_worker(&fake).await;
    harness
        .call(
            MAIN,
            "require_human_review",
            json!({"task": "T-1", "reason": "The screens set the brand"}),
        )
        .await;
    reported(&harness, "Drew the board screens.").await;

    harness
        .call(
            LEAD,
            "send_back_task",
            json!({"id": "T-1", "note": "Add the empty states."}),
        )
        .await;
    until_runs(&fake, TASK_WORKER, 2).await;
    until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
    harness
        .call(
            TASK_WORKER,
            "report_task",
            json!({"id": "T-1", "note": "Added the empty states."}),
        )
        .await;

    for agent in [LEAD, MAIN] {
        let refused = harness
            .refused(agent, "complete_task", json!({"id": "T-1"}))
            .await;
        assert_eq!(refused["code"], codes::HUMAN_APPROVAL_MISSING, "{agent}");
        assert_eq!(refused["details"]["reviewer"], Value::Null, "{agent}");
    }
    let waiting = task(&harness, "T-1").await;
    assert_eq!(waiting["status"], "review");
    assert_eq!(harness.completions("T-1").await, 0);

    let (status, completed) = harness
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/tasks/T-1/complete"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{completed}");
    assert_eq!(completed["status"], "done");
    assert_eq!(harness.completions("T-1").await, 1);
}
