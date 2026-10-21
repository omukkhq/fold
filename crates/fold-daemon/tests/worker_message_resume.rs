#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_core::WorkerState;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, WAIT};
use crate::worker_resume::{
    MAIN, TASK_WORKER, proposed_by_a_task_worker, until_runs, until_state, worker,
};
use crate::workers_support::FakeClaude;

const HUMAN: &str = "tanim";

/// A human's feedback on `T-1`, which goes to its claim holder, the stopped task worker; returns the message id.
async fn feedback(harness: &Harness, body: &str) -> String {
    let (status, sent) = harness
        .http(
            Method::POST,
            "/api/v1/projects/demo/feedback",
            Some(&json!({"task": "T-1", "body": body})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    sent["id"].as_str().unwrap().to_owned()
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

/// Fills the area's one place with a second task worker on a new task.
async fn fill_the_area(harness: &Harness) {
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
    until_state(harness, &other, WorkerState::Idle).await;
}

#[tokio::test]
async fn a_human_message_resumes_the_stopped_task_worker_once_and_it_stops_after_its_answer() {
    let fake = FakeClaude::new("result");
    let (harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    let stopped = worker(&harness, TASK_WORKER).await;

    let message = feedback(&harness, "Why a page and not a dialog?").await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(stopped.session_id.as_str()));
    let turn = &runs[1].stdin[1];
    for wanted in [
        "fold started you again to answer a message",
        &format!("- {message} from human:{HUMAN} about T-1"),
        "get_inbox",
        "Do not start new work",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
    let waiting = until_recipient(&harness, &message, TASK_WORKER, |row| {
        !row["pushed_at"].is_null()
    })
    .await;
    assert!(waiting["delivered_at"].is_null(), "{waiting}");
    assert!(waiting["forwarded_to"].is_null(), "{waiting}");

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
async fn a_message_from_the_area_lead_resumes_the_task_worker_and_one_from_another_worker_does_not()
{
    let fake = FakeClaude::new("result");
    let (harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;

    let other = harness
        .send("review", &[TASK_WORKER], "Which colours did you use?")
        .await;
    let lead = harness
        .send("web-lead", &[TASK_WORKER], "Did you test the page?")
        .await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    let turn = &runs[1].stdin[1];
    assert!(turn.contains(&format!("- {lead} from web-lead")), "{turn}");
    assert!(!turn.contains(&other), "{turn}");
    let row = until_recipient(&harness, &other, TASK_WORKER, |_| true).await;
    assert!(row["pushed_at"].is_null(), "{row}");
    assert!(row["forwarded_to"].is_null(), "{row}");
}

#[tokio::test]
async fn a_paused_project_holds_the_start_for_a_message_until_its_resume() {
    let fake = FakeClaude::new("result");
    let (harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
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
async fn a_worker_limit_sends_the_message_on_to_the_area_lead_and_says_so() {
    let fake = FakeClaude::new("result");
    let (harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    fill_the_area(&harness).await;

    let message = feedback(&harness, "Why a page?").await;
    let row = until_recipient(&harness, &message, TASK_WORKER, |row| {
        !row["forwarded_to"].is_null()
    })
    .await;
    assert_eq!(row["forwarded_to"], "web-lead", "{row}");
    assert!(row["pushed_at"].is_null(), "{row}");
    let lead_row = until_recipient(&harness, &message, "web-lead", |_| true).await;
    assert_eq!(lead_row["addressed_as"], "agent", "{lead_row}");
    let note = until_forward_note(&harness, &message).await;
    assert_eq!(
        note["body"],
        format!("@{TASK_WORKER} cannot start now, so fold sent {message} on to @web-lead.")
    );
    assert_eq!(
        names(&note),
        [format!("human:{HUMAN}").as_str(), "web-lead"]
    );
    assert_eq!(note["task"], "T-1");
    assert_eq!(fake.runs_of(TASK_WORKER).len(), 1);
    assert_eq!(
        worker(&harness, TASK_WORKER).await.state,
        WorkerState::Stopped
    );

    let inbox = harness.inbox("web-lead").await;
    assert!(inbox.contains(&message), "{inbox:?}");
    assert!(
        inbox.contains(&note["id"].as_str().unwrap().to_owned()),
        "{inbox:?}"
    );
}

#[tokio::test]
async fn a_task_worker_whose_session_is_gone_sends_the_message_on_to_the_area_lead() {
    let fake = FakeClaude::new("result").lost_sessions();
    let (harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;

    let message = feedback(&harness, "Why a page?").await;
    let row = until_recipient(&harness, &message, TASK_WORKER, |row| {
        !row["forwarded_to"].is_null()
    })
    .await;
    assert_eq!(row["forwarded_to"], "web-lead", "{row}");
    until_forward_note(&harness, &message).await;
    let runs = fake.runs_of(TASK_WORKER);
    assert_eq!(runs.len(), 2, "one resume, no new session");
    assert!(runs[1].has("--resume"));
    assert_eq!(
        worker(&harness, TASK_WORKER).await.state,
        WorkerState::Failed
    );
}
