#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_core::WorkerState;
use fold_core::roles::{Role, update_note};
use fold_daemon::services::agents::{PAUSE_TURN, RESTART_TURN};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, WAIT};
use crate::project_pause::{LEAD, lead_running, until_not_running};
use crate::worker_resume::{
    MAIN, TASK_WORKER, proposed_by_a_task_worker, request_changes, until_runs, until_state, worker,
};
use crate::workers_support::{FakeClaude, Run};

/// The fingerprint of the installed fld in these tests.
const CURRENT: &str = "0.5.0+1111111111111111";
/// A fingerprint of an older fld, which a session recorded before the update.
const OLDER: &str = "0.4.0+0000000000000000";

/// The text of each user turn that `run` read, in order.
fn user_turns(run: &Run) -> Vec<String> {
    run.stdin
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["type"] == "user")
        .map(|line| line["message"]["content"].as_str().unwrap().to_owned())
        .collect()
}

/// Records that the current session of `agent` read the role text of [`OLDER`], as a session that an older fld started.
async fn read_by_older_fold(harness: &Harness, agent: &str) {
    let row = worker(harness, agent).await;
    harness
        .handle
        .store
        .write(async |tx| fold_store::agents::set_fold_fingerprint(tx, row.id, OLDER).await)
        .await
        .unwrap();
}

/// The fingerprint that `agent`'s row holds once it is `wanted`.
async fn until_fingerprint(harness: &Harness, agent: &str, wanted: &str) {
    eventually(WAIT, async || {
        let found = worker(harness, agent).await.fold_fingerprint;
        if found.as_deref() == Some(wanted) {
            Ok(())
        } else {
            Err(found)
        }
    })
    .await;
}

/// `older_fold` of `agent` on the Agents screen's list.
async fn older_fold(harness: &Harness, agent: &str) -> Value {
    let (status, body) = harness
        .http(Method::GET, "/api/v1/projects/demo/workers", None, HUMAN)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == agent)
        .map(|row| row["older_fold"].clone())
        .unwrap()
}

/// Asserts that `turn` starts with the update note of `role` and goes on with `rest`.
fn noted(turn: &str, role: Role, rest: &str) {
    let note = format!("{}\n\n", update_note(role));
    assert!(turn.starts_with(&note), "{turn}");
    assert!(turn.contains(rest), "{rest} in {turn}");
    assert_eq!(turn.matches(&update_note(role)).count(), 1, "{turn}");
}

#[tokio::test]
async fn a_daemon_start_tells_a_worker_of_an_older_fold_once_and_records_the_new_fingerprint() {
    let fake = FakeClaude::new("init").with_fingerprint(CURRENT);
    let mut harness = lead_running(&fake, WorkerState::Working).await;
    until_fingerprint(&harness, LEAD, CURRENT).await;
    assert_eq!(older_fold(&harness, LEAD).await, false);
    read_by_older_fold(&harness, LEAD).await;
    assert_eq!(older_fold(&harness, LEAD).await, true);
    harness.stop().await;

    harness.start_again().await;
    let runs = until_runs(&fake, LEAD, 2).await;
    assert!(runs[1].has("--resume"));
    let turns = user_turns(&runs[1]);
    assert_eq!(turns.len(), 1, "{turns:?}");
    noted(&turns[0], Role::AreaLead, RESTART_TURN);
    until_fingerprint(&harness, LEAD, CURRENT).await;
    assert_eq!(older_fold(&harness, LEAD).await, false);

    harness.stop().await;
    harness.start_again().await;
    let runs = until_runs(&fake, LEAD, 3).await;
    let turns = user_turns(&runs[2]);
    assert_eq!(turns.len(), 1, "{turns:?}");
    assert!(turns[0].starts_with(RESTART_TURN), "{}", turns[0]);
    assert!(!turns[0].contains(&update_note(Role::AreaLead)), "once");
}

#[tokio::test]
async fn a_worker_resumed_idle_gets_the_note_with_its_next_turn_and_shows_until_then() {
    let fake = FakeClaude::new("result").with_fingerprint(CURRENT);
    let mut harness = lead_running(&fake, WorkerState::Idle).await;
    read_by_older_fold(&harness, LEAD).await;
    harness.stop().await;

    harness.start_again().await;
    let resumed = eventually(WAIT, async || {
        let runs = fake.runs_of(LEAD);
        match runs.get(1) {
            Some(run) if !run.stdin.is_empty() => Ok(run.clone()),
            _ => Err(runs.len()),
        }
    })
    .await;
    assert!(resumed.has("--resume"));
    until_state(&harness, LEAD, WorkerState::Idle).await;
    assert!(
        user_turns(&resumed).is_empty(),
        "nothing waits for the lead"
    );
    assert_eq!(older_fold(&harness, LEAD).await, true);
    assert_eq!(
        worker(&harness, LEAD).await.fold_fingerprint.as_deref(),
        Some(OLDER)
    );

    harness.connect(MAIN).await;
    let sent = harness
        .call(
            MAIN,
            "send_message",
            json!({"to": [LEAD], "body": "the plover task"}),
        )
        .await;
    let message = sent["id"].as_str().unwrap().to_owned();
    let turn = eventually(WAIT, async || {
        let runs = fake.runs_of(LEAD);
        let turns = runs.get(1).map(user_turns).unwrap_or_default();
        turns.first().cloned().ok_or(turns.len())
    })
    .await;
    noted(&turn, Role::AreaLead, &format!("message {message} arrived"));
    until_fingerprint(&harness, LEAD, CURRENT).await;
    assert_eq!(older_fold(&harness, LEAD).await, false);
}

#[tokio::test]
async fn a_daemon_start_under_the_same_fingerprint_says_nothing_of_an_update() {
    let fake = FakeClaude::new("init").with_fingerprint(CURRENT);
    let mut harness = lead_running(&fake, WorkerState::Working).await;
    until_fingerprint(&harness, LEAD, CURRENT).await;
    harness.stop().await;

    harness.start_again().await;
    let runs = until_runs(&fake, LEAD, 2).await;
    let turns = user_turns(&runs[1]);
    assert_eq!(turns.len(), 1, "{turns:?}");
    assert!(turns[0].starts_with(RESTART_TURN), "{}", turns[0]);
    assert!(!turns[0].contains("fold was updated"), "{}", turns[0]);
    assert_eq!(older_fold(&harness, LEAD).await, false);
}

#[tokio::test]
async fn a_launcher_without_fingerprints_records_none_and_says_nothing() {
    let fake = FakeClaude::new("init");
    let mut harness = lead_running(&fake, WorkerState::Working).await;
    read_by_older_fold(&harness, LEAD).await;
    harness.stop().await;

    harness.start_again().await;
    let runs = until_runs(&fake, LEAD, 2).await;
    let turns = user_turns(&runs[1]);
    assert!(!turns[0].contains("fold was updated"), "{}", turns[0]);
    assert_eq!(older_fold(&harness, LEAD).await, false);
    assert_eq!(
        worker(&harness, LEAD).await.fold_fingerprint.as_deref(),
        Some(OLDER)
    );
}

#[tokio::test]
async fn the_resume_of_a_pause_tells_a_worker_of_an_older_fold() {
    let fake = FakeClaude::new("init").with_fingerprint(CURRENT);
    let harness = lead_running(&fake, WorkerState::Working).await;
    harness.call(MAIN, "pause_project", json!({})).await;
    until_not_running(&harness, LEAD).await;
    read_by_older_fold(&harness, LEAD).await;

    harness.call(MAIN, "resume_project", json!({})).await;
    let runs = until_runs(&fake, LEAD, 2).await;
    let turns = user_turns(&runs[1]);
    assert_eq!(turns.len(), 1, "{turns:?}");
    noted(&turns[0], Role::AreaLead, PAUSE_TURN);
    until_fingerprint(&harness, LEAD, CURRENT).await;
}

#[tokio::test]
async fn a_message_that_starts_a_stopped_lead_again_tells_it_of_an_older_fold() {
    let fake = FakeClaude::new("result").with_fingerprint(CURRENT);
    let harness = lead_running(&fake, WorkerState::Idle).await;
    harness
        .call(MAIN, "stop_worker", json!({"name": LEAD}))
        .await;
    until_state(&harness, LEAD, WorkerState::Stopped).await;
    read_by_older_fold(&harness, LEAD).await;

    harness
        .call(
            MAIN,
            "send_message",
            json!({"to": [LEAD], "body": "the plover task"}),
        )
        .await;
    let runs = until_runs(&fake, LEAD, 2).await;
    assert!(runs[1].has("--resume"));
    let turns = user_turns(&runs[1]);
    noted(
        &turns[0],
        Role::AreaLead,
        "fold started you again for a message",
    );
    until_fingerprint(&harness, LEAD, CURRENT).await;
}

#[tokio::test]
async fn requested_changes_tell_the_resumed_task_worker_of_an_older_fold() {
    let fake = FakeClaude::new("result").with_fingerprint(CURRENT);
    let (harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    until_fingerprint(&harness, TASK_WORKER, CURRENT).await;
    read_by_older_fold(&harness, TASK_WORKER).await;

    request_changes(&harness, "P-1", "Split the page.").await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert!(runs[1].has("--resume"));
    let turns = user_turns(&runs[1]);
    noted(&turns[0], Role::TaskWorker, "Split the page.");
    until_fingerprint(&harness, TASK_WORKER, CURRENT).await;
}

#[tokio::test]
async fn a_new_session_records_the_fingerprint_without_a_note() {
    let fake = FakeClaude::new("result").with_fingerprint(CURRENT);
    let harness = lead_running(&fake, WorkerState::Idle).await;
    read_by_older_fold(&harness, LEAD).await;

    harness
        .call(MAIN, "restart_worker", json!({"name": LEAD}))
        .await;
    let runs = until_runs(&fake, LEAD, 2).await;
    assert!(!runs[1].has("--resume"));
    let turns = user_turns(&runs[1]);
    assert!(!turns[0].contains("fold was updated"), "{}", turns[0]);
    until_fingerprint(&harness, LEAD, CURRENT).await;
}
