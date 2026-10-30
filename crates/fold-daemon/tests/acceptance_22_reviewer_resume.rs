#![cfg(test)]

use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::WorkerState;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, SLUG, WAIT};
use crate::worker_resume::{
    MAIN, REVIEWER, TASK_WORKER, post_next_version, review_claim_holder,
    reviewed_by_a_started_reviewer, task_worker_for, until_runs, until_state, worker,
};
use crate::workers_support::FakeClaude;

const LEAD: &str = "review-lead";

/// The data of every stored `worker.resumed` of reason `new_version`, once there are `count`.
async fn until_new_version_resumes(harness: &Harness, count: usize) -> Vec<Value> {
    eventually(WAIT, async || {
        let resumed: Vec<Value> = harness
            .events()
            .await
            .into_iter()
            .filter(|event| event.kind == "worker.resumed" && event.data["reason"] == "new_version")
            .map(|event| event.data)
            .collect();
        if resumed.len() == count {
            Ok(resumed)
        } else {
            Err(resumed.len())
        }
    })
    .await
}

/// Waits until the running review lead got a notice that contains `needle`.
async fn until_lead_hears(fake: &FakeClaude, needle: &str) {
    eventually(WAIT, async || {
        let runs = fake.runs_of(LEAD);
        let heard = runs
            .last()
            .is_some_and(|run| run.stdin.iter().any(|line| line.contains(needle)));
        if heard { Ok(()) } else { Err(runs.len()) }
    })
    .await;
}

/// Starts the review lead as the main agent does, and the reviewer of `P-1` as the review lead does; both are connected and idle.
async fn start_the_reviewer(harness: &mut Harness) {
    harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    until_state(harness, LEAD, WorkerState::Idle).await;
    harness.connect(LEAD).await;
    let started = harness
        .call(
            LEAD,
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-1"]}),
        )
        .await;
    assert_eq!(started["worker"]["name"], REVIEWER, "{started}");
    until_state(harness, REVIEWER, WorkerState::Idle).await;
    harness.connect(REVIEWER).await;
    harness
        .call(REVIEWER, "create_review_checkout", json!({"id": "P-1"}))
        .await;
}

async fn submit(harness: &Harness, version: u32, verdict: &str) {
    harness
        .call(
            REVIEWER,
            "submit_review",
            json!({"id": "P-1", "version": version, "verdict": verdict, "body": "Read and run."}),
        )
        .await;
}

/// Acceptance 22: on a task with the human review mark, the reviewer that the review lead started approves, and fold stops it with the review claim. A human edit keeps that approval, so nothing resumes the reviewer; when the human sends the edit for review again, fold resumes the same reviewer in its own session for that version.
#[tokio::test]
async fn acceptance_22_a_version_a_human_sends_for_review_again_resumes_the_stopped_reviewer() {
    let fake = FakeClaude::new("result");
    let mut harness = task_worker_for(&fake, &["Draw the login page"], WorkerState::Idle).await;
    fold_daemon::services::projects::ensure_human(
        harness.project.registry(),
        harness.project.clock(),
        &fold_core::HumanName::parse(HUMAN).unwrap(),
    )
    .await
    .unwrap();
    harness
        .call(
            MAIN,
            "require_human_review",
            json!({"task": "T-1", "reviewer": HUMAN, "reason": "touches auth"}),
        )
        .await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    work.write(&[("login.html", "<form>login</form>\n")]);
    harness
        .propose(TASK_WORKER, &work, "Draw the login page")
        .await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    start_the_reviewer(&mut harness).await;
    submit(&harness, 1, "approve").await;
    until_state(&harness, REVIEWER, WorkerState::Stopped).await;
    let stopped = worker(&harness, REVIEWER).await;

    let proposal = format!("/api/v1/projects/{SLUG}/proposals/P-1");
    let (code, made) = harness
        .http(
            Method::POST,
            &format!("{proposal}/edits"),
            Some(&json!({"version": 1, "path": "login.html", "content": "<form>sign in</form>\n"})),
            HUMAN,
        )
        .await;
    assert_eq!(code, StatusCode::CREATED, "{made}");
    assert_eq!(made["version"], 2);
    // The edit keeps the reviewer's approval, so the reviewer stays stopped.
    harness.send(MAIN, &[LEAD], "after the edit").await;
    assert_eq!(fake.runs_of(REVIEWER).len(), 1);

    let (code, summary) = harness
        .http(
            Method::POST,
            &format!("{proposal}/review-again"),
            Some(&json!({"version": 2})),
            HUMAN,
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{summary}");
    let runs = until_runs(&fake, REVIEWER, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(stopped.session_id.as_str()));
    assert!(runs[1].stdin[1].contains("P-1 v2"), "{}", runs[1].stdin[1]);
    let resumed = until_new_version_resumes(&harness, 1).await;
    assert_eq!(
        (
            &resumed[0]["agent"],
            &resumed[0]["version"],
            &resumed[0]["resumed"]
        ),
        (&json!(REVIEWER), &json!(2), &json!(true))
    );
    until_state(&harness, REVIEWER, WorkerState::Idle).await;
    harness
        .call(REVIEWER, "create_review_checkout", json!({"id": "P-1"}))
        .await;
    submit(&harness, 2, "approve").await;
    until_state(&harness, REVIEWER, WorkerState::Stopped).await;
}

/// Acceptance 22: fold resumes a reviewer once for each version. A resumed reviewer that ends again without a verdict is not resumed again for that version, and the review lead hears of the version instead; the reviewer it then starts takes over the claim.
#[tokio::test]
async fn acceptance_22_a_reviewer_that_ends_again_without_a_verdict_is_resumed_once_and_the_lead_is_told()
 {
    let fake = FakeClaude::new("result");
    let (mut harness, work) = reviewed_by_a_started_reviewer(&fake).await;
    work.write(&[("page.html", "<h1>page</h1>\n")]);
    post_next_version(&harness, &work).await;
    until_runs(&fake, REVIEWER, 2).await;
    until_state(&harness, REVIEWER, WorkerState::Idle).await;

    harness
        .call(LEAD, "stop_worker", json!({"name": REVIEWER}))
        .await;
    until_state(&harness, REVIEWER, WorkerState::Stopped).await;
    until_lead_hears(
        &fake,
        "reviewer ended with no verdict on P-1; a reviewer you start takes over its claim. Call start_worker.",
    )
    .await;
    assert_eq!(fake.runs_of(REVIEWER).len(), 2, "resumed once for v2");
    assert_eq!(review_claim_holder(&harness).await, REVIEWER);

    let started = harness
        .call(
            LEAD,
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-1"]}),
        )
        .await;
    assert_eq!(started["worker"]["name"], "reviewer-2", "{started}");
    assert_eq!(review_claim_holder(&harness).await, "reviewer-2");
    until_state(&harness, "reviewer-2", WorkerState::Idle).await;
    harness.connect("reviewer-2").await;
    harness
        .call("reviewer-2", "create_review_checkout", json!({"id": "P-1"}))
        .await;
    harness
        .call(
            "reviewer-2",
            "submit_review",
            json!({"id": "P-1", "version": 2, "verdict": "approve", "body": "Split now."}),
        )
        .await;
    until_state(&harness, "reviewer-2", WorkerState::Stopped).await;
    assert_eq!(fake.runs_of(REVIEWER).len(), 2);
}

/// Acceptance 22: a new version whose reviewer no limit lets fold resume stays owed in the stored state, and the next daemon start resumes that reviewer in its own session for the version.
#[tokio::test]
async fn acceptance_22_a_daemon_start_resumes_the_reviewer_that_a_new_version_waits_for() {
    let fake = FakeClaude::new("result");
    let (mut harness, work) = reviewed_by_a_started_reviewer(&fake).await;
    let stopped = worker(&harness, REVIEWER).await;
    let config = std::path::Path::new(harness.root()).join(".fold/config.toml");
    std::fs::write(&config, "[workers]\nreviewer = 0\n").unwrap();
    harness
        .project
        .clock()
        .advance(Duration::from_secs(1))
        .unwrap();
    work.write(&[("page.html", "<h1>page</h1>\n")]);
    post_next_version(&harness, &work).await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    assert_eq!(worker(&harness, REVIEWER).await.state, WorkerState::Stopped);
    assert_eq!(
        fake.runs_of(REVIEWER).len(),
        1,
        "a limit of 0 holds the resume"
    );

    // The limit rises while the daemon is down, so only the next start can serve the version.
    harness.stop().await;
    std::fs::write(&config, "[workers]\nreviewer = 3\n").unwrap();
    harness.start_again().await;
    harness.connect(MAIN).await;
    let runs = until_runs(&fake, REVIEWER, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(stopped.session_id.as_str()));
    assert!(runs[1].stdin[1].contains("P-1 v2"), "{}", runs[1].stdin[1]);
    let resumed = until_new_version_resumes(&harness, 1).await;
    assert_eq!(resumed[0]["version"], 2);
    assert_eq!(review_claim_holder(&harness).await, REVIEWER);
}

/// Acceptance 22: when the session of the reviewer that a new version goes back to is gone, fold starts the reviewer in a new session with the same name, claim and turn, and records the resume for that version.
#[tokio::test]
async fn acceptance_22_a_reviewer_whose_session_is_gone_reviews_the_new_version_in_a_new_session() {
    let fake = FakeClaude::new("result").lost_sessions();
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.lead_area("backend").await;
    harness.make_main(MAIN).await;
    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let work = harness.start_work("backend", &task).await;
    work.write(&[("login.txt", "POST /login\n")]);
    let proposal = harness
        .propose("backend", &work, "Add the login endpoint")
        .await;
    assert_eq!(proposal, "P-1");
    start_the_reviewer(&mut harness).await;
    submit(&harness, 1, "request_changes").await;
    let stopped = until_state(&harness, REVIEWER, WorkerState::Stopped).await;

    work.write(&[("login.txt", "POST /login\nPOST /logout\n")]);
    harness
        .call(
            "backend",
            "record_changes",
            json!({"ws": work.workspace, "message": "Add the logout"}),
        )
        .await;
    let updated = harness
        .call(
            "backend",
            "update_proposal",
            json!({"intent": "Make the change the task asks for.", "id": "P-1"}),
        )
        .await;
    assert_eq!(updated["version"], "v2", "{updated}");

    let runs = until_runs(&fake, REVIEWER, 3).await;
    assert_eq!(runs[1].flag("--resume"), Some(stopped.session_id.as_str()));
    let fresh = &runs[2];
    assert!(!fresh.has("--resume"));
    assert_ne!(
        fresh.flag("--session-id"),
        Some(stopped.session_id.as_str())
    );
    let turn = &fresh.stdin[1];
    for wanted in ["P-1 v2", "You still hold the review claim"] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
    let resumed = until_new_version_resumes(&harness, 1).await;
    assert_eq!(
        resumed[0],
        json!({"agent": REVIEWER, "proposal": "P-1", "version": 2, "reason": "new_version", "resumed": false})
    );
    until_state(&harness, REVIEWER, WorkerState::Idle).await;
    assert_eq!(review_claim_holder(&harness).await, REVIEWER);
    // fold starts the reviewer again at once, so the review lead hears nothing of the version.
    let lead_lines: Vec<String> = fake
        .runs_of(LEAD)
        .into_iter()
        .flat_map(|run| run.stdin)
        .collect();
    assert!(
        lead_lines
            .iter()
            .all(|line| !line.contains("ended with no verdict")),
        "{lead_lines:?}"
    );
    harness
        .call(REVIEWER, "create_review_checkout", json!({"id": "P-1"}))
        .await;
    submit(&harness, 2, "approve").await;
    until_state(&harness, REVIEWER, WorkerState::Stopped).await;
}

/// Acceptance 22: a message to a reviewer that a new version waits for never takes the place of that version. The reviewer resumes once, for the version; the message waits until the reviewer stops again and then starts it for the message.
#[tokio::test]
async fn acceptance_22_a_message_to_a_reviewer_owed_a_new_version_resumes_it_for_the_version() {
    let fake = FakeClaude::new("result");
    let (mut harness, work) = reviewed_by_a_started_reviewer(&fake).await;
    let config = std::path::Path::new(harness.root()).join(".fold/config.toml");
    std::fs::write(&config, "[workers]\nreviewer = 0\n").unwrap();
    harness
        .project
        .clock()
        .advance(Duration::from_secs(1))
        .unwrap();
    work.write(&[("page.html", "<h1>page</h1>\n")]);
    post_next_version(&harness, &work).await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    assert_eq!(
        fake.runs_of(REVIEWER).len(),
        1,
        "a limit of 0 holds the resume"
    );

    // The limit rises without an event, so only the message serves the owed version. The reviewer's own session has ended, as it does when fold stops it.
    std::fs::write(&config, "[workers]\nreviewer = 3\n").unwrap();
    harness.disconnect(REVIEWER).await;
    harness
        .send(MAIN, &[REVIEWER], "Check the header too")
        .await;
    let runs = until_runs(&fake, REVIEWER, 2).await;
    let turn = &runs[1].stdin[1];
    assert!(turn.contains("P-1 v2"), "{turn}");
    assert!(!turn.contains("for a message"), "{turn}");
    let resumed = until_new_version_resumes(&harness, 1).await;
    assert_eq!(resumed[0]["version"], 2);
    until_state(&harness, REVIEWER, WorkerState::Idle).await;
    assert_eq!(fake.runs_of(REVIEWER).len(), 2, "resumed once");
    harness.connect(REVIEWER).await;

    harness
        .call(REVIEWER, "create_review_checkout", json!({"id": "P-1"}))
        .await;
    submit(&harness, 2, "approve").await;
    until_state(&harness, REVIEWER, WorkerState::Stopped).await;
    // Its session ends with its process; a second message starts it for both.
    harness.disconnect(REVIEWER).await;
    harness.send(MAIN, &[REVIEWER], "And the footer").await;
    let runs = until_runs(&fake, REVIEWER, 3).await;
    let turn = &runs[2].stdin[1];
    assert!(
        turn.contains("for a message") && turn.contains("m-1") && turn.contains("m-2"),
        "{turn}"
    );
}

/// Acceptance 22: a reviewer whose session is gone at a daemon start and whose new session the reviewer limit refuses leaves its version to nobody, so fold tells the review lead which version waits for a review.
#[tokio::test]
async fn acceptance_22_a_reviewer_that_cannot_start_in_a_new_session_leaves_its_version_to_the_review_lead()
 {
    let fake = FakeClaude::new("result").lost_sessions();
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.lead_area("backend").await;
    harness.make_main(MAIN).await;
    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let work = harness.start_work("backend", &task).await;
    work.write(&[("login.txt", "POST /login\n")]);
    harness
        .propose("backend", &work, "Add the login endpoint")
        .await;
    start_the_reviewer(&mut harness).await;
    // The reviewer is in a turn when the daemon stops, so the next start resumes it with a turn, which finds its session gone.
    harness
        .handle
        .store
        .write(async |tx| {
            sqlx::query(
                "UPDATE worker_processes SET state = 'working', idle_since = NULL WHERE agent_id = (SELECT id FROM agents WHERE name = 'reviewer')",
            )
            .execute(tx.conn())
            .await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();

    harness.stop().await;
    let config = std::path::Path::new(harness.root()).join(".fold/config.toml");
    std::fs::write(&config, "[workers]\nreviewer = 0\n").unwrap();
    harness.start_again().await;
    until_runs(&fake, REVIEWER, 2).await;
    until_state(&harness, REVIEWER, WorkerState::Failed).await;
    harness.connect(LEAD).await;
    let note = eventually(WAIT, async || {
        let inbox = harness.call(LEAD, "get_inbox", json!({})).await;
        inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["from"] == "fold")
            .cloned()
            .ok_or(inbox)
    })
    .await;
    assert_eq!(
        note["body"],
        "@reviewer lost its session and the reviewer limit refuses it a new one now, so P-1 v1 waits for a review. Give it to a reviewer with start_worker when a place is free; the start takes over the claim."
    );
    assert_eq!(fake.runs_of(REVIEWER).len(), 2, "no new session");
}
