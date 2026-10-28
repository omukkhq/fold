#![cfg(test)]

use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::projects::notify::sign;
use fold_daemon::jobs::spawn_notify;
use fold_store::testing::eventually;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::common::acceptance::{HUMAN, Harness};
use crate::webhook_notices::Receiver;

const WEB: &str = "https://fold.example.org";

const CHECKS: &str = r#"
[[check]]
name  = "endpoint"
run   = "grep -q 'POST /login' src/login.txt"
files = "^src/"
"#;

/// Adds a target of `kind` through the web routes as the human, and returns its id and secret.
async fn target(harness: &Harness, kind: &str, url: &str) -> (String, Option<String>) {
    let (status, body) = harness
        .http(
            Method::POST,
            "/api/v1/projects/demo/notify/targets",
            Some(&json!({"kind": kind, "name": format!("{kind} target"), "url": url})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (
        body["target"]["id"].as_str().unwrap().to_owned(),
        body["secret"].as_str().map(str::to_owned),
    )
}

/// The `checks.finished` event of `task` that `agent` waits for.
async fn finished(harness: &Harness, agent: &str, task: &str) -> Value {
    loop {
        let woke = harness
            .call(
                agent,
                "wait_for_events",
                json!({"types": ["checks.finished"], "task": task, "timeout_s": 60}),
            )
            .await;
        if let Some(event) = woke["events"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|event| event["type"] == "checks.finished")
        {
            return event.clone();
        }
    }
}

async fn waiting_notices(harness: &Harness) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM notify_outbox")
        .fetch_one(harness.handle.store.read())
        .await
        .unwrap()
}

/// Acceptance 29: a person adds a generic target and a Slack target in the Settings routes. backend's checks fail on its first run, then its fixed work becomes a proposal, which moves its card to Review. When the window of 15 s on the project clock ends, the generic receiver gets one signed send with both notices and the Slack receiver one message with a line for each, and nothing more comes.
#[tokio::test]
async fn acceptance_29_a_failed_check_and_a_card_move_reach_a_generic_and_a_slack_target() {
    let harness = Harness::start().await;
    let mut generic = Receiver::start(&[]).await;
    let mut slack = Receiver::start_at("/services/T0/B0/token", &[]).await;
    let (status, body) = harness
        .http(
            Method::PUT,
            "/api/v1/notify",
            Some(&json!({"web_url": WEB})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, secret) = target(&harness, "webhook", &generic.url).await;
    target(&harness, "slack", &slack.url).await;
    let (_stop, stopped) = watch::channel(false);
    let job = spawn_notify(harness.state.clone(), Duration::from_millis(10), stopped);

    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let work = harness.start_work("backend", &task).await;
    work.write(&[
        ("fold-checks.toml", CHECKS),
        ("src/login.txt", "GET /login\n"),
    ]);
    let started = harness
        .call("backend", "run_checks", json!({"task": task}))
        .await;
    assert_eq!(started["checks"], "running", "{started}");
    assert_eq!(
        finished(&harness, "backend", &task).await["data"]["state"],
        "failed"
    );
    work.write(&[("src/login.txt", "POST /login\n")]);
    harness
        .call(
            "backend",
            "create_proposal",
            json!({"task": task, "title": "Add the login endpoint", "evidence": "fold ran the checks"}),
        )
        .await;
    assert_eq!(
        finished(&harness, "backend", &task).await["data"]["state"],
        "passed"
    );
    let proposal = harness
        .call("review", "list_proposals", json!({"state": "open"}))
        .await["proposals"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    eventually(Duration::from_secs(20), async || {
        let waiting = waiting_notices(&harness).await;
        if waiting == 4 { Ok(()) } else { Err(waiting) }
    })
    .await;
    assert!(
        generic.none_waiting() && slack.none_waiting(),
        "the window still runs"
    );
    harness
        .project
        .clock()
        .advance(Duration::from_secs(15))
        .unwrap();
    let mut progress = job.progress();
    tokio::time::timeout(
        Duration::from_secs(20),
        progress.wait_for(|seen| seen.cycles >= 2),
    )
    .await
    .unwrap()
    .unwrap();

    let post = generic.next().await;
    let signature = sign(
        secret.as_deref().unwrap(),
        post.header("webhook-id").unwrap(),
        post.header("webhook-timestamp").unwrap().parse().unwrap(),
        &post.body,
    )
    .unwrap();
    assert_eq!(post.header("webhook-signature"), Some(signature.as_str()));
    let body = post.json();
    assert_eq!(body["type"], "fold.notices");
    let notices = body["data"]["notices"].as_array().unwrap();
    assert_eq!(post.types(), ["checks.failed", "task.moved"], "{body}");
    assert_eq!(notices[0]["stage"], "proposal");
    assert_eq!(notices[0]["task"]["id"], task);
    assert_eq!(notices[1]["task"]["id"], task);
    assert_eq!(notices[1]["task"]["title"], "Add the login endpoint");
    assert_eq!(
        (notices[1]["from"].as_str(), notices[1]["to"].as_str()),
        (Some("in_progress"), Some("review"))
    );
    assert_eq!(notices[1]["cause"]["proposal"], proposal);
    assert_eq!(notices[1]["task"]["url"], format!("{WEB}/p/demo/t/{task}"));

    let text = slack.next().await.json()["text"]
        .as_str()
        .unwrap()
        .to_owned();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(
        lines[0].contains("Check failed:") && lines[0].contains("stage proposal"),
        "{text}"
    );
    assert!(
        lines[1].contains("moved In progress → Review") && lines[1].contains(&proposal),
        "{text}"
    );

    harness
        .project
        .clock()
        .advance(Duration::from_secs(3600))
        .unwrap();
    let checks = progress.borrow().checks;
    harness.state.projects.notify().unwrap().wake(None);
    tokio::time::timeout(
        Duration::from_secs(20),
        progress.wait_for(|seen| seen.checks > checks),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        generic.none_waiting() && slack.none_waiting(),
        "a delivered send never goes again"
    );
}
