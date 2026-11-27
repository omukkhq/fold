#![cfg(test)]

use std::time::Duration;

use fold_core::WorkerState;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::acceptance_48_reviewer_fixes_a_conflict::{
    LEAD, SECOND_REVIEWER, status, told, two_conflicting_proposals,
};
use crate::auto_integrate::Running;
use crate::common::acceptance::{Harness, WAIT};
use crate::worker_resume::{MAIN, REVIEWER, until_runs, until_state, worker};
use crate::workers_support::FakeClaude;

/// The review lead starts `reviewer` for `proposal`, which approves `v1` without waiting for fold to stop it, so an automatic integration of the approval can conflict while fold stops the reviewer as finished.
async fn approve(harness: &mut Harness, proposal: &str, reviewer: &str) {
    let started = harness
        .call(
            LEAD,
            "start_worker",
            json!({"role": "reviewer", "proposals": [proposal]}),
        )
        .await;
    assert_eq!(started["worker"]["name"], reviewer, "{started}");
    until_state(harness, reviewer, WorkerState::Idle).await;
    harness.connect(reviewer).await;
    harness
        .call(
            reviewer,
            "submit_review",
            json!({"id": proposal, "version": 1, "verdict": "approve", "body": "Read and run."}),
        )
        .await;
}

/// Moves the project's clock, on which a stop's signals wait, until the stop of `agent` has ended.
async fn until_stopped_on_the_clock(harness: &Harness, agent: &str) {
    let clock = harness.project.clock().clone();
    eventually(WAIT, async || {
        let state = worker(harness, agent).await.state;
        if state == WorkerState::Stopped {
            Ok(())
        } else {
            clock.advance(Duration::from_secs(6)).unwrap();
            Err(state)
        }
    })
    .await;
}

/// The `worker.resumed` events of reason `conflict`, once there are `count` of them.
async fn conflict_resumes(harness: &Harness, count: usize) -> Vec<Value> {
    eventually(WAIT, async || {
        let found: Vec<Value> = harness
            .events()
            .await
            .into_iter()
            .filter(|event| event.kind == "worker.resumed" && event.data["reason"] == "conflict")
            .map(|event| event.data)
            .collect();
        if found.len() == count {
            Ok(found)
        } else {
            Err(found.len())
        }
    })
    .await
}

async fn lead_told_of_conflict(harness: &Harness) -> Option<String> {
    told(harness, LEAD)
        .await
        .into_iter()
        .map(|message| message.to_string())
        .find(|text| text.contains("conflicts with main"))
}

/// Acceptance 67: fold stops the reviewer as finished once its approval is in, and the automatic integration that approval starts conflicts while that stop runs. fold waits for the stop, then resumes the same reviewer in its own session with the conflict report; the review lead is not told. The reviewers' fake Claude Code ignores a closed stdin and SIGINT, so each stop lasts until the project's clock reaches its SIGTERM and the conflict always comes while the reviewer stops.
#[tokio::test]
async fn acceptance_67_a_conflict_during_the_reviewers_stop_resumes_the_same_reviewer_in_its_session()
 {
    let fake = FakeClaude::new("result");
    let (mut harness, _second) = two_conflicting_proposals(&fake).await;
    let mut stubborn = fake.launcher.clone();
    stubborn
        .env
        .push(("FOLD_FAKE_CLAUDE_STUBBORN".to_owned(), "1".to_owned()));
    harness.handle.workers.set_launcher(stubborn);
    approve(&mut harness, "P-1", REVIEWER).await;
    until_stopped_on_the_clock(&harness, REVIEWER).await;
    let integrated = harness
        .call(LEAD, "integrate_proposal", json!({"id": "P-1"}))
        .await;
    assert_eq!(integrated["revision"], "r2", "{integrated}");

    let integrate = Running::start(&harness.state);
    approve(&mut harness, "P-2", SECOND_REVIEWER).await;
    let session = worker(&harness, SECOND_REVIEWER).await.session_id;
    eventually(WAIT, async || {
        let events = harness.events().await;
        let conflicted = events
            .iter()
            .any(|event| event.kind == "integration.conflicted");
        let state = worker(&harness, SECOND_REVIEWER).await.state;
        if conflicted && state == WorkerState::Stopping {
            Ok(())
        } else {
            Err(state)
        }
    })
    .await;
    assert_eq!(
        fake.runs_of(SECOND_REVIEWER).len(),
        1,
        "the resume waits for the stop"
    );
    until_stopped_on_the_clock(&harness, SECOND_REVIEWER).await;
    let runs = until_runs(&fake, SECOND_REVIEWER, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(session.as_str()));
    assert!(
        runs[1].stdin[1].contains(
            "fold resumed you because P-2 v1 for T-2, which you approved, conflicts with main at r2"
        ),
        "{}",
        runs[1].stdin[1]
    );
    let resumed = conflict_resumes(&harness, 1).await;
    assert_eq!(
        (&resumed[0]["agent"], &resumed[0]["version"]),
        (&json!(SECOND_REVIEWER), &json!(1))
    );
    assert_eq!(status(&harness, "T-2").await, "review");
    assert_eq!(lead_told_of_conflict(&harness).await, None);
    integrate.stop().await;
}

/// Acceptance 67: the integration of a proposal releases its approving reviewer's review claim, and fold never resumes that reviewer, also when a later conflict of another proposal resumes its own reviewer and frees places.
#[tokio::test]
async fn acceptance_67_an_integrated_proposal_releases_its_reviewer_which_is_not_resumed() {
    let fake = FakeClaude::new("result");
    let (mut harness, _second) = two_conflicting_proposals(&fake).await;
    let integrate = Running::start(&harness.state);
    approve(&mut harness, "P-1", REVIEWER).await;
    let released = eventually(WAIT, async || {
        harness
            .events()
            .await
            .into_iter()
            .find(|event| event.kind == "claim.released" && event.data["proposal"] == "P-1")
            .map(|event| event.data)
            .ok_or(())
    })
    .await;
    assert_eq!(
        (&released["worker"], &released["reason"]),
        (&json!(REVIEWER), &json!("integrated"))
    );
    until_state(&harness, REVIEWER, WorkerState::Stopped).await;

    approve(&mut harness, "P-2", SECOND_REVIEWER).await;
    until_runs(&fake, SECOND_REVIEWER, 2).await;
    conflict_resumes(&harness, 1).await;
    until_state(&harness, SECOND_REVIEWER, WorkerState::Idle).await;
    assert_eq!(
        fake.runs_of(REVIEWER).len(),
        1,
        "the released reviewer never runs again"
    );
    assert_eq!(worker(&harness, REVIEWER).await.state, WorkerState::Stopped);
    integrate.stop().await;
}

/// Acceptance 67: when the approving reviewer's session is gone, its resume for the conflict fails, and only then does the review lead get the notice to start a new reviewer.
#[tokio::test]
async fn acceptance_67_a_reviewer_whose_session_is_gone_leaves_the_conflict_to_the_review_lead() {
    let fake = FakeClaude::new("result").lost_sessions();
    let (mut harness, _second) = two_conflicting_proposals(&fake).await;
    approve(&mut harness, "P-1", REVIEWER).await;
    until_state(&harness, REVIEWER, WorkerState::Stopped).await;
    harness
        .call(LEAD, "integrate_proposal", json!({"id": "P-1"}))
        .await;

    let integrate = Running::start(&harness.state);
    approve(&mut harness, "P-2", SECOND_REVIEWER).await;
    let notice = eventually(WAIT, async || {
        lead_told_of_conflict(&harness).await.ok_or(())
    })
    .await;
    for wanted in [
        "P-2 v1 for T-2 conflicts with main at r2 and was not integrated.",
        &format!("@{SECOND_REVIEWER} approved v1, but fold could not resume it"),
        "start one for P-2 with start_worker",
    ] {
        assert!(notice.contains(wanted), "{wanted}: {notice}");
    }
    let runs = fake.runs_of(SECOND_REVIEWER);
    assert_eq!(
        runs.len(),
        2,
        "one resume in the old session, no new session"
    );
    assert!(runs[1].has("--resume"));
    let failed = until_state(&harness, SECOND_REVIEWER, WorkerState::Failed).await;
    assert_eq!(failed.failed_reason.as_deref(), Some("session_lost"));
    assert_eq!(status(&harness, "T-2").await, "review");

    integrate.stop().await;
    let started = harness
        .call(
            LEAD,
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-2"]}),
        )
        .await;
    let third = started["worker"]["name"].as_str().unwrap().to_owned();
    until_state(&harness, &third, WorkerState::Idle).await;
    harness
        .call(MAIN, "stop_worker", json!({"name": third}))
        .await;
    until_state(&harness, &third, WorkerState::Stopped).await;
    let notices = told(&harness, LEAD)
        .await
        .into_iter()
        .filter(|message| message.to_string().contains("conflicts with main"))
        .count();
    assert_eq!(notices, 1, "a later stop gives no second notice");
    assert_eq!(fake.runs_of(SECOND_REVIEWER).len(), 2);
}

/// Acceptance 67: a conflict whose reviewer the worker limits hold back stays owed without a notice to the review lead, and the place a raised limit frees resumes that reviewer in its own session.
#[tokio::test]
async fn acceptance_67_a_full_reviewer_limit_keeps_the_conflict_owed_until_a_place_frees() {
    let fake = FakeClaude::new("result");
    let (mut harness, _second) = two_conflicting_proposals(&fake).await;
    approve(&mut harness, "P-1", REVIEWER).await;
    until_state(&harness, REVIEWER, WorkerState::Stopped).await;
    approve(&mut harness, "P-2", SECOND_REVIEWER).await;
    until_state(&harness, SECOND_REVIEWER, WorkerState::Stopped).await;
    let session = worker(&harness, SECOND_REVIEWER).await.session_id;
    harness
        .call(LEAD, "integrate_proposal", json!({"id": "P-1"}))
        .await;
    harness
        .call(MAIN, "set_worker_limits", json!({"reviewer": 0}))
        .await;

    let refused = harness
        .refused(LEAD, "integrate_proposal", json!({"id": "P-2"}))
        .await;
    assert_eq!(refused["code"], "integration_conflicted", "{refused}");
    // A worker stop is one of the events that give owed conflicts again; the limit still holds this one back.
    harness
        .call(MAIN, "stop_worker", json!({"name": LEAD}))
        .await;
    until_state(&harness, LEAD, WorkerState::Stopped).await;
    assert_eq!(
        fake.runs_of(SECOND_REVIEWER).len(),
        1,
        "the limit holds the resume"
    );
    assert_eq!(lead_told_of_conflict(&harness).await, None);

    harness
        .call(MAIN, "set_worker_limits", json!({"reviewer": 1}))
        .await;
    let runs = until_runs(&fake, SECOND_REVIEWER, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(session.as_str()));
    conflict_resumes(&harness, 1).await;
    assert_eq!(lead_told_of_conflict(&harness).await, None);
    assert_eq!(status(&harness, "T-2").await, "review");
}

/// Every notice to the review lead about a conflict, once there are `count` of them.
async fn conflict_notices(harness: &Harness, count: usize) -> Vec<String> {
    eventually(WAIT, async || {
        let found: Vec<String> = told(harness, LEAD)
            .await
            .into_iter()
            .map(|message| message.to_string())
            .filter(|text| text.contains("conflicts with main"))
            .collect();
        if found.len() == count {
            Ok(found)
        } else {
            Err(found.len())
        }
    })
    .await
}

/// Acceptance 67: a conflict that the limits held back is given when a place frees; when that resume's process ends before its turn, the review lead gets the notice once, and later stops and limit changes send no second one.
#[tokio::test]
async fn acceptance_67_a_held_conflict_whose_resume_fails_tells_the_review_lead_once() {
    let fake = FakeClaude::new("result");
    let (mut harness, _second) = two_conflicting_proposals(&fake).await;
    approve(&mut harness, "P-1", REVIEWER).await;
    until_state(&harness, REVIEWER, WorkerState::Stopped).await;
    approve(&mut harness, "P-2", SECOND_REVIEWER).await;
    until_state(&harness, SECOND_REVIEWER, WorkerState::Stopped).await;
    harness
        .call(LEAD, "integrate_proposal", json!({"id": "P-1"}))
        .await;
    harness
        .call(MAIN, "set_worker_limits", json!({"reviewer": 0}))
        .await;
    let refused = harness
        .refused(LEAD, "integrate_proposal", json!({"id": "P-2"}))
        .await;
    assert_eq!(refused["code"], "integration_conflicted", "{refused}");
    assert_eq!(lead_told_of_conflict(&harness).await, None);

    let mut failing = fake.launcher.clone();
    failing.env.push((
        "FOLD_FAKE_CLAUDE_STDERR".to_owned(),
        "cannot run".to_owned(),
    ));
    harness.handle.workers.set_launcher(failing);
    harness
        .call(MAIN, "set_worker_limits", json!({"reviewer": 1}))
        .await;
    let notices = conflict_notices(&harness, 1).await;
    assert!(
        notices[0].contains(&format!(
            "@{SECOND_REVIEWER} approved v1, but fold could not resume it"
        )),
        "{}",
        notices[0]
    );
    assert_eq!(fake.runs_of(SECOND_REVIEWER).len(), 2);

    harness
        .call(MAIN, "set_worker_limits", json!({"reviewer": 2}))
        .await;
    harness
        .call(MAIN, "set_worker_limits", json!({"reviewer": 3}))
        .await;
    conflict_notices(&harness, 1).await;
    assert_eq!(
        fake.runs_of(SECOND_REVIEWER).len(),
        2,
        "the conflict went to the review lead and is not given again"
    );
}
