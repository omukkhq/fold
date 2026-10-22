#![cfg(test)]

use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::{AgentName, WorkerState};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, StoredEvent, WAIT};
use crate::worker_resume::{
    MAIN, TASK_WORKER, proposed_by_a_task_worker, request_changes, task_worker_for, until_runs,
    until_state, worker,
};
use crate::workers_support::FakeClaude;

const HUMAN: &str = "tanim";
const LEAD: &str = "web-lead";
const REVIEW_LEAD: &str = "review";
const REVIEWER: &str = "reviewer";

/// A human's message from the web app, returned by id: feedback on `T-1`, which goes to its claim holder, or with `reply_to` a reply in that message's thread.
async fn from_human(harness: &Harness, body: Value) -> String {
    let (status, sent) = harness
        .http(
            Method::POST,
            "/api/v1/projects/demo/feedback",
            Some(&body),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    sent["id"].as_str().unwrap().to_owned()
}

/// A human's feedback on `T-1`, which goes to its claim holder, the stopped task worker; returns the message id.
async fn feedback(harness: &Harness, body: &str) -> String {
    from_human(harness, json!({"task": "T-1", "body": body})).await
}

/// The thread of `message` as the web reads it.
async fn thread(harness: &Harness, message: &str) -> Value {
    let (status, thread) = harness
        .http(
            Method::GET,
            &format!("/api/v1/projects/demo/threads/{message}"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    thread
}

/// The recipient row of `name` on `message`, once `pick` holds for it.
async fn until_recipient(
    harness: &Harness,
    message: &str,
    name: &str,
    pick: impl Fn(&Value) -> bool,
) -> Value {
    eventually(WAIT, async || {
        let thread = thread(harness, message).await;
        let row = thread["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|sent| sent["id"] == message)
            .and_then(|sent| {
                sent["recipients"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["name"] == name)
                    .cloned()
            });
        match row {
            Some(row) if pick(&row) => Ok(row),
            other => Err(other),
        }
    })
    .await
}

/// The forward of `message` on the row of `name`, once fold recorded it.
async fn until_forwarded(harness: &Harness, message: &str, name: &str) -> Value {
    until_recipient(harness, message, name, |row| !row["forwarded_to"].is_null()).await
}

/// fold's note in the thread of `message` that it sent the message on, once it is there.
async fn until_forward_note(harness: &Harness, message: &str) -> Value {
    eventually(WAIT, async || {
        let thread = thread(harness, message).await;
        thread["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|sent| sent["sender"] == "fold" && sent["reply_to"] == message)
            .cloned()
            .ok_or(thread)
    })
    .await
}

fn names(message: &Value) -> Vec<&str> {
    message["recipients"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["name"].as_str().unwrap())
        .collect()
}

fn write_config(harness: &Harness, text: &str) {
    std::fs::write(
        std::path::Path::new(harness.root()).join(".fold/config.toml"),
        text,
    )
    .unwrap();
}

/// Fills the area's one place with a second task worker on a new task.
async fn fill_the_area(harness: &Harness) {
    write_config(harness, "[workers]\nmax_per_area = 1\n");
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Draw the menu", "labels": ["ui"]}),
        )
        .await;
    let other = harness
        .call(
            LEAD,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-2"]}),
        )
        .await;
    let other = other["worker"]["name"].as_str().unwrap().to_owned();
    until_state(harness, &other, WorkerState::Idle).await;
}

/// The idle sweep after the idle time of a short-lived worker; returns the agents it stopped.
async fn sweep_after_idle_time(harness: &Harness) -> Vec<AgentName> {
    harness
        .project
        .clock()
        .advance(Duration::from_secs(16 * 60))
        .unwrap();
    harness
        .handle
        .workers
        .sweep_idle(harness.project.now())
        .await
        .unwrap()
}

/// The kinds of the stored events among `kinds` in order, with only the stops of `agent`.
fn kinds_of(events: &[StoredEvent], agent: &str, kinds: &[&str]) -> Vec<String> {
    events
        .iter()
        .filter(|event| kinds.contains(&event.kind.as_str()))
        .filter(|event| event.kind != "worker.stopped" || event.data["agent"] == agent)
        .map(|event| event.kind.clone())
        .collect()
}

/// [`proposed_by_a_task_worker`] of `page.html`; the stopped task worker's session ends with its process.
async fn stopped_task_worker(fake: &FakeClaude) -> (Harness, crate::common::acceptance::Work) {
    let (mut harness, work) =
        proposed_by_a_task_worker(fake, &[("page.html", "<p>page</p>\n")]).await;
    harness.disconnect(TASK_WORKER).await;
    (harness, work)
}

/// Makes [`HUMAN`] a human of the project, whom a message to `humans` reaches.
async fn know_the_human(harness: &Harness) {
    fold_daemon::services::projects::ensure_human(
        harness.project.registry(),
        harness.project.clock(),
        &fold_core::HumanName::parse(HUMAN).unwrap(),
    )
    .await
    .unwrap();
}

/// `review`, the review lead, starts the reviewer of `P-1` and its session connects.
async fn start_the_reviewer(harness: &mut Harness) {
    let started = harness
        .call(
            REVIEW_LEAD,
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-1"]}),
        )
        .await;
    assert_eq!(started["worker"]["name"], REVIEWER, "{started}");
    until_state(harness, REVIEWER, WorkerState::Idle).await;
    harness.connect(REVIEWER).await;
}

#[tokio::test]
async fn acceptance_23_a_human_message_resumes_the_stopped_task_worker_once_and_it_stops_after_its_answer()
 {
    let fake = FakeClaude::new("result");
    let (mut harness, _work) = stopped_task_worker(&fake).await;
    let stopped = worker(&harness, TASK_WORKER).await;

    let message = feedback(&harness, "Why a page and not a dialog?").await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(stopped.session_id.as_str()));
    let turn = &runs[1].stdin[1];
    for wanted in [
        "fold started you again for a message",
        &format!("- {message} from human:{HUMAN} about T-1"),
        "get_inbox",
        "If your bound task or review still waits for your work, continue it.",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
    let waiting = until_recipient(&harness, &message, TASK_WORKER, |row| {
        !row["pushed_at"].is_null()
    })
    .await;
    assert!(waiting["delivered_at"].is_null(), "{waiting}");
    assert!(waiting["forwarded_to"].is_null(), "{waiting}");

    harness.connect(TASK_WORKER).await;
    assert_eq!(
        harness.inbox(TASK_WORKER).await,
        std::slice::from_ref(&message)
    );
    harness
        .send_with(
            TASK_WORKER,
            json!({"to": [format!("human:{HUMAN}")], "reply_to": message, "body": "A dialog hides the board."}),
        )
        .await;
    let done = until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    assert_eq!(done.session_id, stopped.session_id);
    assert_eq!(
        fake.runs_of(TASK_WORKER).len(),
        2,
        "one start for the message"
    );
    let delivered = until_recipient(&harness, &message, TASK_WORKER, |row| {
        !row["delivered_at"].is_null()
    })
    .await;
    assert!(delivered["forwarded_to"].is_null(), "{delivered}");
    let events = harness.events().await;
    assert!(
        !events.iter().any(|event| event.kind == "worker.resumed"),
        "the start for a message records no resume of work"
    );
}

#[tokio::test]
async fn acceptance_23_a_message_from_any_sender_resumes_the_stopped_task_worker() {
    let fake = FakeClaude::new("result");
    let (harness, _work) = stopped_task_worker(&fake).await;
    harness.call(MAIN, "pause_project", json!({})).await;

    let from_review = harness
        .send(REVIEW_LEAD, &[TASK_WORKER], "Which colours did you use?")
        .await;
    let from_lead = harness
        .send(LEAD, &[TASK_WORKER], "Did you test the page?")
        .await;
    harness.call(MAIN, "resume_project", json!({})).await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    let turn = &runs[1].stdin[1];
    assert!(
        turn.contains(&format!("- {from_review} from {REVIEW_LEAD}")),
        "{turn}"
    );
    assert!(
        turn.contains(&format!("- {from_lead} from {LEAD}")),
        "{turn}"
    );
    for message in [&from_review, &from_lead] {
        let row = until_recipient(&harness, message, TASK_WORKER, |row| {
            !row["pushed_at"].is_null()
        })
        .await;
        assert!(row["forwarded_to"].is_null(), "{row}");
    }
}

#[tokio::test]
async fn acceptance_23_a_paused_project_holds_the_start_for_a_message_until_its_resume() {
    let fake = FakeClaude::new("result");
    let (harness, _work) = stopped_task_worker(&fake).await;
    harness.call(MAIN, "pause_project", json!({})).await;

    let message = feedback(&harness, "Why a page?").await;
    assert_eq!(
        fake.runs_of(TASK_WORKER).len(),
        1,
        "a paused project starts nobody"
    );
    let row = until_recipient(&harness, &message, TASK_WORKER, |_| true).await;
    assert!(
        row["forwarded_to"].is_null(),
        "a pause sends nothing on: {row}"
    );

    harness.call(MAIN, "resume_project", json!({})).await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert!(runs[1].has("--resume"));
    assert!(
        runs[1].stdin[1].contains(&format!("- {message} from human:{HUMAN}")),
        "{}",
        runs[1].stdin[1]
    );
}

#[tokio::test]
async fn acceptance_23_a_worker_limit_sends_the_message_on_to_the_area_lead_and_says_so() {
    let fake = FakeClaude::new("result");
    let (harness, _work) = stopped_task_worker(&fake).await;
    fill_the_area(&harness).await;

    let message = feedback(&harness, "Why a page?").await;
    let row = until_forwarded(&harness, &message, TASK_WORKER).await;
    assert_eq!(row["forwarded_to"], LEAD, "{row}");
    assert!(row["pushed_at"].is_null(), "{row}");
    let lead_row = until_recipient(&harness, &message, LEAD, |_| true).await;
    assert_eq!(lead_row["addressed_as"], "agent", "{lead_row}");
    let note = until_forward_note(&harness, &message).await;
    assert_eq!(
        note["body"],
        format!("@{TASK_WORKER} cannot start now, so fold sent {message} on to @{LEAD}.")
    );
    assert_eq!(names(&note), [format!("human:{HUMAN}").as_str(), LEAD]);
    assert_eq!(note["task"], "T-1");
    assert_eq!(fake.runs_of(TASK_WORKER).len(), 1);
    assert_eq!(
        worker(&harness, TASK_WORKER).await.state,
        WorkerState::Stopped
    );

    let inbox = harness.inbox(LEAD).await;
    assert!(inbox.contains(&message), "{inbox:?}");
    assert!(
        inbox.contains(&note["id"].as_str().unwrap().to_owned()),
        "{inbox:?}"
    );
}

#[tokio::test]
async fn acceptance_23_the_leads_reply_to_a_forwarded_message_acknowledges_only_the_leads_row() {
    let fake = FakeClaude::new("result");
    let (harness, _work) = stopped_task_worker(&fake).await;
    fill_the_area(&harness).await;

    let message = feedback(&harness, "Why a page?").await;
    until_forwarded(&harness, &message, TASK_WORKER).await;
    until_recipient(&harness, &message, LEAD, |_| true).await;
    harness
        .send_with(
            LEAD,
            json!({"to": ["humans"], "body": "A page links better.", "reply_to": message}),
        )
        .await;

    let lead_row = until_recipient(&harness, &message, LEAD, |_| true).await;
    assert!(!lead_row["acknowledged_at"].is_null(), "{lead_row}");
    assert!(!harness.inbox(LEAD).await.contains(&message));
    let worker_row = until_recipient(&harness, &message, TASK_WORKER, |_| true).await;
    assert_eq!(worker_row["forwarded_to"], LEAD, "{worker_row}");
    assert!(worker_row["acknowledged_at"].is_null(), "{worker_row}");
    assert_eq!(fake.runs_of(TASK_WORKER).len(), 1);
}

#[tokio::test]
async fn acceptance_23_a_worker_limit_that_holds_the_task_workers_own_resume_sends_the_message_on_at_once()
 {
    let fake = FakeClaude::new("result");
    let (harness, _work) = stopped_task_worker(&fake).await;
    fill_the_area(&harness).await;
    request_changes(&harness, "P-1", "Split the page.").await;

    let message = feedback(&harness, "Why a page?").await;
    let row = until_forwarded(&harness, &message, TASK_WORKER).await;
    assert_eq!(row["forwarded_to"], LEAD, "{row}");
    until_forward_note(&harness, &message).await;
    assert_eq!(fake.runs_of(TASK_WORKER).len(), 1);
}

#[tokio::test]
async fn acceptance_23_a_task_worker_whose_session_is_gone_sends_its_messages_on_without_another_start()
 {
    let fake = FakeClaude::new("result").lost_sessions();
    let (harness, _work) = stopped_task_worker(&fake).await;

    let message = feedback(&harness, "Why a page?").await;
    let row = until_forwarded(&harness, &message, TASK_WORKER).await;
    assert_eq!(row["forwarded_to"], LEAD, "{row}");
    until_forward_note(&harness, &message).await;
    let runs = fake.runs_of(TASK_WORKER);
    assert_eq!(runs.len(), 2, "one resume, no new session");
    assert!(runs[1].has("--resume"));
    let failed = until_state(&harness, TASK_WORKER, WorkerState::Failed).await;
    assert_eq!(failed.failed_reason.as_deref(), Some("session_lost"));

    let second = feedback(&harness, "And the menu?").await;
    let row = until_forwarded(&harness, &second, TASK_WORKER).await;
    assert_eq!(row["forwarded_to"], LEAD, "{row}");
    assert_eq!(
        fake.runs_of(TASK_WORKER).len(),
        2,
        "a session known gone is not resumed again"
    );
}

#[tokio::test]
async fn acceptance_23_a_message_from_the_lead_that_cannot_start_the_worker_goes_back_to_the_lead()
{
    let fake = FakeClaude::new("result");
    let (harness, _work) = stopped_task_worker(&fake).await;
    fill_the_area(&harness).await;

    let message = harness
        .send(LEAD, &[TASK_WORKER], "Did you test the page?")
        .await;
    let row = until_forwarded(&harness, &message, TASK_WORKER).await;
    assert_eq!(row["forwarded_to"], LEAD, "{row}");
    let note = until_forward_note(&harness, &message).await;
    assert_eq!(names(&note), [LEAD]);
    assert_eq!(
        note["body"],
        format!(
            "@{TASK_WORKER} cannot start now to answer {message}, so it stays unread. Answer it yourself or start the worker again."
        )
    );
    let sent = thread(&harness, &message).await;
    assert_eq!(
        names(&sent["messages"][0]),
        [TASK_WORKER],
        "the lead gets no row of its own message"
    );
    assert_eq!(fake.runs_of(TASK_WORKER).len(), 1);
}

#[tokio::test]
async fn acceptance_23_a_message_about_a_task_of_the_area_main_goes_on_to_the_main_agent() {
    let fake = FakeClaude::new("result");
    let (harness, _work) = stopped_task_worker(&fake).await;
    harness
        .call(
            MAIN,
            "update_task",
            json!({"id": "T-1", "labels": ["main"]}),
        )
        .await;
    write_config(&harness, "[workers]\ntask_worker = 0\n");

    let message = feedback(&harness, "Why a page?").await;
    let row = until_forwarded(&harness, &message, TASK_WORKER).await;
    assert_eq!(row["forwarded_to"], MAIN, "{row}");
    until_recipient(&harness, &message, MAIN, |_| true).await;
    let note = until_forward_note(&harness, &message).await;
    assert_eq!(names(&note), [format!("human:{HUMAN}").as_str(), MAIN]);
    assert_eq!(
        note["body"],
        format!("@{TASK_WORKER} cannot start now, so fold sent {message} on to @{MAIN}.")
    );
    assert_eq!(fake.runs_of(TASK_WORKER).len(), 1);
}

#[tokio::test]
async fn acceptance_23_a_task_worker_that_asked_the_human_and_was_stopped_resumes_with_the_answer_and_goes_on()
 {
    let fake = FakeClaude::new("result");
    let mut harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    know_the_human(&harness).await;
    let started = worker(&harness, TASK_WORKER).await;
    let ask = harness
        .send_with(
            TASK_WORKER,
            json!({"to": ["humans"], "body": "A page or a dialog?", "task": "T-1", "choices": ["Page", "Dialog"], "asks_human": true}),
        )
        .await;
    assert_eq!(
        sweep_after_idle_time(&harness).await,
        [AgentName::parse(TASK_WORKER).unwrap()]
    );
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    harness.disconnect(TASK_WORKER).await;

    let answer = from_human(&harness, json!({"reply_to": ask, "body": "Dialog"})).await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    harness.connect(TASK_WORKER).await;
    assert_eq!(runs[1].flag("--resume"), Some(started.session_id.as_str()));
    let turn = &runs[1].stdin[1];
    for wanted in [
        &format!("- {answer} from human:{HUMAN} about T-1"),
        "If your bound task or review still waits for your work, continue it.",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
    assert_eq!(
        harness.inbox(TASK_WORKER).await,
        std::slice::from_ref(&answer)
    );
    harness
        .send(TASK_WORKER, &[LEAD], "The human chose a dialog.")
        .await;

    let work = harness.start_work(TASK_WORKER, "T-1").await;
    work.write(&[("dialog.html", "<dialog>page</dialog>\n")]);
    harness.propose(TASK_WORKER, &work, "Draw the dialog").await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    assert_eq!(fake.runs_of(TASK_WORKER).len(), 2);
    assert_eq!(
        kinds_of(
            &harness.events().await,
            TASK_WORKER,
            &["worker.stopped", "proposal.opened"]
        ),
        ["worker.stopped", "proposal.opened", "worker.stopped"],
        "the answer leaves the worker running until its proposal"
    );
}

#[tokio::test]
async fn acceptance_23_a_reviewer_that_asked_the_human_and_was_stopped_resumes_with_the_answer_and_gives_its_verdict()
 {
    let fake = FakeClaude::new("result");
    let (mut harness, _work) = stopped_task_worker(&fake).await;
    know_the_human(&harness).await;
    start_the_reviewer(&mut harness).await;
    let started = worker(&harness, REVIEWER).await;
    let ask = harness
        .send_with(
            REVIEWER,
            json!({"to": ["humans"], "body": "Does the page replace the dialog?", "proposal": "P-1", "asks_human": true}),
        )
        .await;
    assert_eq!(
        sweep_after_idle_time(&harness).await,
        [AgentName::parse(REVIEWER).unwrap()]
    );
    until_state(&harness, REVIEWER, WorkerState::Stopped).await;
    harness.disconnect(REVIEWER).await;

    let answer = from_human(&harness, json!({"reply_to": ask, "body": "Yes"})).await;
    let runs = until_runs(&fake, REVIEWER, 2).await;
    harness.connect(REVIEWER).await;
    assert_eq!(runs[1].flag("--resume"), Some(started.session_id.as_str()));
    let turn = &runs[1].stdin[1];
    assert!(
        turn.contains(&format!("- {answer} from human:{HUMAN}")),
        "{turn}"
    );
    assert!(
        turn.contains("If your bound task or review still waits for your work, continue it."),
        "{turn}"
    );
    assert_eq!(harness.inbox(REVIEWER).await, std::slice::from_ref(&answer));
    harness
        .send(REVIEWER, &[REVIEW_LEAD], "The page replaces the dialog.")
        .await;

    harness
        .call(REVIEWER, "create_review_checkout", json!({"id": "P-1"}))
        .await;
    harness
        .call(
            REVIEWER,
            "submit_review",
            json!({"id": "P-1", "version": 1, "verdict": "approve", "body": "The page replaces the dialog."}),
        )
        .await;
    until_state(&harness, REVIEWER, WorkerState::Stopped).await;
    assert_eq!(fake.runs_of(REVIEWER).len(), 2);
    assert_eq!(
        kinds_of(
            &harness.events().await,
            REVIEWER,
            &["worker.stopped", "review.submitted"]
        ),
        ["worker.stopped", "review.submitted", "worker.stopped"],
        "the answer leaves the reviewer running until its verdict"
    );
}

#[tokio::test]
async fn acceptance_23_a_message_resumes_a_stopped_lead() {
    let fake = FakeClaude::new("result");
    let (mut harness, _work) = stopped_task_worker(&fake).await;
    harness
        .call(MAIN, "stop_worker", json!({"name": LEAD}))
        .await;
    let stopped = until_state(&harness, LEAD, WorkerState::Stopped).await;
    harness.disconnect(LEAD).await;
    let runs_before = fake.runs_of(LEAD).len();

    let message = harness.send(MAIN, &[LEAD], "Is the page done?").await;
    let runs = until_runs(&fake, LEAD, runs_before + 1).await;
    let resumed = runs.last().unwrap();
    assert_eq!(resumed.flag("--resume"), Some(stopped.session_id.as_str()));
    assert!(
        resumed.stdin[1].contains(&format!("- {message} from {MAIN}")),
        "{}",
        resumed.stdin[1]
    );
}

#[tokio::test]
async fn acceptance_23_a_reviewer_that_cannot_start_sends_the_message_on_to_the_review_lead() {
    let fake = FakeClaude::new("result");
    let (mut harness, _work) = stopped_task_worker(&fake).await;
    start_the_reviewer(&mut harness).await;
    harness
        .call(MAIN, "stop_worker", json!({"name": REVIEWER}))
        .await;
    until_state(&harness, REVIEWER, WorkerState::Stopped).await;
    harness.disconnect(REVIEWER).await;
    write_config(&harness, "[workers]\nreviewer = 0\n");

    let message = harness
        .send(MAIN, &[REVIEWER], "Which version did you read?")
        .await;
    let row = until_forwarded(&harness, &message, REVIEWER).await;
    assert_eq!(row["forwarded_to"], REVIEW_LEAD, "{row}");
    let note = until_forward_note(&harness, &message).await;
    assert_eq!(names(&note), [MAIN, REVIEW_LEAD]);
    assert_eq!(fake.runs_of(REVIEWER).len(), 1);
}

#[tokio::test]
async fn acceptance_23_a_lead_whose_session_is_gone_with_no_main_agent_holds_the_message_and_says_so()
 {
    let fake = FakeClaude::new("result").lost_sessions();
    let (mut harness, _work) = stopped_task_worker(&fake).await;
    harness
        .call(MAIN, "stop_worker", json!({"name": LEAD}))
        .await;
    until_state(&harness, LEAD, WorkerState::Stopped).await;
    harness.disconnect(LEAD).await;
    let main = AgentName::parse(MAIN).unwrap();
    harness
        .handle
        .store
        .write(async |tx| {
            let agent = fold_store::agents::find_agent(tx.conn(), &main)
                .await?
                .unwrap();
            fold_store::agents::remove_tag(tx, agent.id, &fold_core::Tag::parse("main").unwrap())
                .await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    let runs = fake.runs_of(LEAD).len();

    let message = harness
        .send(REVIEW_LEAD, &[LEAD], "Is the page reviewed?")
        .await;
    let note = until_forward_note(&harness, &message).await;
    assert_eq!(names(&note), [REVIEW_LEAD]);
    assert_eq!(
        note["body"],
        format!(
            "@{LEAD} cannot start now to answer {message}, and no lead can take it, so it stays unread."
        )
    );
    let row = until_recipient(&harness, &message, LEAD, |_| true).await;
    assert!(row["forwarded_to"].is_null(), "{row}");
    assert_eq!(fake.runs_of(LEAD).len(), runs + 1, "one resume");
    let failed = until_state(&harness, LEAD, WorkerState::Failed).await;
    assert_eq!(failed.failed_reason.as_deref(), Some("session_lost"));

    let second = harness.send(REVIEW_LEAD, &[LEAD], "And the menu?").await;
    until_forward_note(&harness, &second).await;
    assert_eq!(
        fake.runs_of(LEAD).len(),
        runs + 1,
        "a session known gone is not resumed again"
    );
}
