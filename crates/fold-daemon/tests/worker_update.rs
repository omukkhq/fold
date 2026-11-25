#![cfg(test)]

use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::roles::{Role, UPGRADE_HANDOFF_REQUEST, reload_note, update_note};
use fold_core::{Actor, AgentName, Clock, HumanName, WorkerState};
use fold_daemon::services::agents::{PAUSE_TURN, RESTART_TURN};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, WAIT};
use crate::project_pause::{LEAD, lead_running, until_not_running};
use crate::worker_resume::{
    MAIN, TASK_WORKER, proposed_by_a_task_worker, request_changes, until_runs, until_state, worker,
};
use crate::workers_support::{FakeClaude, Run};

/// The fingerprint of the installed fld's skill texts in these tests.
const CURRENT: &str = "1111111111111111";
/// The fingerprint of an older fld's skill texts, which a session recorded before the update.
const OLDER: &str = "0000000000000000";

/// The text of each user turn that `run` read, in order.
fn user_turns(run: &Run) -> Vec<String> {
    run.stdin
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["type"] == "user")
        .map(|line| line["message"]["content"].as_str().unwrap().to_owned())
        .collect()
}

/// Records that the current session of `agent` read the skill texts of [`OLDER`], as a session that an older fld started.
async fn read_by_older_fold(harness: &Harness, agent: &str) {
    let row = worker(harness, agent).await;
    harness
        .handle
        .store
        .write(async |tx| fold_store::agents::set_fold_fingerprint(tx, row.id, OLDER).await)
        .await
        .unwrap();
}

/// Records the tool schema hashes of [`OLDER`]: the running binary's, with `get_task` in another shape.
async fn older_tools(harness: &Harness) {
    let tools: Vec<(String, String)> = fold_daemon::mcp::tool_schema_hashes()
        .into_iter()
        .map(|(name, hash)| {
            let hash = if name == "get_task" {
                "0".repeat(16)
            } else {
                hash
            };
            (name, hash)
        })
        .collect();
    harness
        .handle
        .store
        .write(async |tx| fold_store::agents::record_tool_schema_hashes(tx, OLDER, &tools).await)
        .await
        .unwrap();
}

/// The user turns of the `index`th run of `agent` once it has read at least `count` of them.
async fn until_turns(fake: &FakeClaude, agent: &str, index: usize, count: usize) -> Vec<String> {
    eventually(WAIT, async || {
        let turns = fake
            .runs_of(agent)
            .get(index)
            .map(user_turns)
            .unwrap_or_default();
        if turns.len() >= count {
            Ok(turns)
        } else {
            Err(turns.len())
        }
    })
    .await
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

#[tokio::test]
async fn a_daemon_start_resumes_a_lead_of_an_older_fold_without_a_note_and_keeps_its_fingerprint() {
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
    let turns = until_turns(&fake, LEAD, 1, 1).await;
    assert_eq!(turns.len(), 1, "{turns:?}");
    assert!(turns[0].starts_with(RESTART_TURN), "{}", turns[0]);
    assert!(!turns[0].contains("fold was updated"), "{}", turns[0]);
    assert_eq!(
        worker(&harness, LEAD).await.fold_fingerprint.as_deref(),
        Some(OLDER),
        "the lead keeps the older fingerprint until its fresh session"
    );
    assert_eq!(older_fold(&harness, LEAD).await, true);
}

#[tokio::test]
async fn a_lead_of_an_older_fold_hands_off_into_a_fresh_session_with_its_note() {
    let fake = FakeClaude::new("result").with_fingerprint(CURRENT);
    let mut harness = lead_running(&fake, WorkerState::Idle).await;
    let started = worker(&harness, LEAD).await;
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

    harness.connect(MAIN).await;
    harness.connect(LEAD).await;
    let sent = harness
        .call(
            MAIN,
            "send_message",
            json!({"to": [LEAD], "body": "the plover task"}),
        )
        .await;
    let message = sent["id"].as_str().unwrap().to_owned();
    let turns = until_turns(&fake, LEAD, 1, 2).await;
    assert!(
        turns[0].contains(&format!("message {message} arrived")),
        "{}",
        turns[0]
    );
    assert!(!turns[0].contains("fold was updated"), "{}", turns[0]);
    assert_eq!(turns[1], UPGRADE_HANDOFF_REQUEST);
    assert_eq!(
        worker(&harness, LEAD).await.fold_fingerprint.as_deref(),
        Some(OLDER)
    );

    let note = "Working on the plover task. Next: split it.";
    let stored = harness
        .call(LEAD, "write_handoff", json!({"text": note}))
        .await;
    assert_eq!(stored["stored"], true);
    let runs = until_runs(&fake, LEAD, 3).await;
    let fresh = &runs[2];
    assert!(!fresh.has("--resume"));
    assert_ne!(
        fresh.flag("--session-id"),
        Some(started.session_id.as_str())
    );
    let first = until_turns(&fake, LEAD, 2, 1).await;
    assert!(
        first[0].contains("fold started you again as web-lead"),
        "{}",
        first[0]
    );
    assert!(first[0].contains(note), "{}", first[0]);
    assert!(!first[0].contains("fold was updated"), "{}", first[0]);
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
async fn the_resume_of_a_pause_gives_a_lead_of_an_older_fold_no_note() {
    let fake = FakeClaude::new("init").with_fingerprint(CURRENT);
    let harness = lead_running(&fake, WorkerState::Working).await;
    harness.call(MAIN, "pause_project", json!({})).await;
    until_not_running(&harness, LEAD).await;
    read_by_older_fold(&harness, LEAD).await;

    harness.call(MAIN, "resume_project", json!({})).await;
    let runs = until_runs(&fake, LEAD, 2).await;
    let turns = user_turns(&runs[1]);
    assert_eq!(turns.len(), 1, "{turns:?}");
    assert!(turns[0].starts_with(PAUSE_TURN), "{}", turns[0]);
    assert!(!turns[0].contains("fold was updated"), "{}", turns[0]);
    assert_eq!(
        worker(&harness, LEAD).await.fold_fingerprint.as_deref(),
        Some(OLDER)
    );
}

#[tokio::test]
async fn a_message_that_starts_a_stopped_lead_of_an_older_fold_asks_it_to_hand_off() {
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
    let turns = until_turns(&fake, LEAD, 1, 2).await;
    assert!(
        turns[0].contains("fold started you again for a message"),
        "{}",
        turns[0]
    );
    assert!(!turns[0].contains("fold was updated"), "{}", turns[0]);
    assert_eq!(turns[1], UPGRADE_HANDOFF_REQUEST);
}

#[tokio::test]
async fn requested_changes_keep_the_session_of_a_task_worker_of_an_older_fold_and_name_the_changed_tools()
 {
    let fake = FakeClaude::new("result").with_fingerprint(CURRENT);
    let (harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    until_fingerprint(&harness, TASK_WORKER, CURRENT).await;
    let started = worker(&harness, TASK_WORKER).await;
    read_by_older_fold(&harness, TASK_WORKER).await;
    older_tools(&harness).await;

    request_changes(&harness, "P-1", "Split the page.").await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert!(runs[1].has("--resume"));
    assert_eq!(runs[1].flag("--resume"), Some(started.session_id.as_str()));
    let turns = until_turns(&fake, TASK_WORKER, 1, 1).await;
    let note = reload_note(Role::TaskWorker, Some(&["get_task".to_owned()]));
    assert!(turns[0].starts_with(&format!("{note}\n\n")), "{}", turns[0]);
    assert!(turns[0].contains("Split the page."), "{}", turns[0]);
    assert_eq!(turns[0].matches(&update_note(Role::TaskWorker)).count(), 1);
    assert!(!turns[0].contains("`create_proposal`"), "{}", turns[0]);
    assert!(
        !turns
            .iter()
            .any(|turn| turn.contains(UPGRADE_HANDOFF_REQUEST))
    );
    until_fingerprint(&harness, TASK_WORKER, CURRENT).await;
    assert_eq!(
        worker(&harness, TASK_WORKER).await.session_id,
        started.session_id
    );
}

#[tokio::test]
async fn a_task_worker_whose_older_fold_recorded_no_tools_reads_every_tool_again() {
    let fake = FakeClaude::new("result").with_fingerprint(CURRENT);
    let (harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    until_fingerprint(&harness, TASK_WORKER, CURRENT).await;
    read_by_older_fold(&harness, TASK_WORKER).await;

    request_changes(&harness, "P-1", "Split the page.").await;
    until_runs(&fake, TASK_WORKER, 2).await;
    let turns = until_turns(&fake, TASK_WORKER, 1, 1).await;
    assert!(
        turns[0].starts_with(&reload_note(Role::TaskWorker, None)),
        "{}",
        turns[0]
    );
}

#[tokio::test]
async fn a_new_session_records_the_fingerprint_without_a_note() {
    let fake = FakeClaude::new("result").with_fingerprint(CURRENT);
    let harness = lead_running(&fake, WorkerState::Idle).await;
    read_by_older_fold(&harness, LEAD).await;

    let (status, restarted) = harness
        .http(
            Method::POST,
            &format!("/api/v1/projects/demo/workers/{LEAD}/restart"),
            Some(&json!({})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{restarted}");
    // The fake process can reach its first turn before the response is built.
    assert!(
        restarted["state"] == "starting" || restarted["state"] == "working",
        "{restarted}"
    );
    assert_eq!(
        restarted["older_fold"], false,
        "a starting session records its fingerprint once its process is in place"
    );
    let runs = until_runs(&fake, LEAD, 2).await;
    assert!(!runs[1].has("--resume"));
    let turns = user_turns(&runs[1]);
    assert!(!turns[0].contains("fold was updated"), "{}", turns[0]);
    until_fingerprint(&harness, LEAD, CURRENT).await;
    let mut conn = harness.handle.store.read().acquire().await.unwrap();
    assert_eq!(
        fold_store::agents::tool_schema_hashes(&mut *conn, CURRENT)
            .await
            .unwrap(),
        fold_daemon::mcp::tool_schema_hashes(),
        "the new session records the tool schemas of its fingerprint"
    );
}

#[tokio::test]
async fn a_stopping_worker_takes_no_more_turns_and_shows_no_mark() {
    let fake = FakeClaude::new("result")
        .stubborn()
        .with_fingerprint(CURRENT);
    let harness = lead_running(&fake, WorkerState::Idle).await;
    read_by_older_fold(&harness, LEAD).await;
    assert_eq!(older_fold(&harness, LEAD).await, true);

    let agents = harness.handle.agents();
    let human = Actor::human(HumanName::parse(HUMAN).unwrap());
    let lead = AgentName::parse(LEAD).unwrap();
    let stop = tokio::spawn(async move { agents.stop_worker(&human, &lead).await });
    until_state(&harness, LEAD, WorkerState::Stopping).await;
    assert_eq!(older_fold(&harness, LEAD).await, false);
    let clock = harness.project.clock().clone();
    eventually(WAIT, async || {
        if stop.is_finished() {
            Ok(())
        } else {
            clock.advance(Duration::from_secs(6)).unwrap();
            Err(clock.now())
        }
    })
    .await;
}
