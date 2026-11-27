#![cfg(test)]

use std::path::Path;

use axum::http::{Method, StatusCode};
use fold_core::WorkerState;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, SLUG, WAIT, Work, read_tree};
use crate::worker_resume::{
    MAIN, REVIEWER, TASK_WORKER, task_worker_for, until_runs, until_state, worker,
};
use crate::workers_support::FakeClaude;

pub const LEAD: &str = "review-lead";
pub const SECOND_REVIEWER: &str = "reviewer-2";
/// Both tasks rewrite the one line of `README.md` in `r1`.
const SCREEN: &str = "# demo: the login screen\n";
const ENDPOINT: &str = "# demo: the login endpoint\n";
/// The reviewer's resolution keeps both changes.
const RESOLVED: &str = "# demo: the login screen and the login endpoint\n";

pub async fn status(harness: &Harness, task: &str) -> Value {
    harness.call(MAIN, "get_task", json!({"id": task})).await["status"].clone()
}

/// One task worker proposes `P-1` for `T-1` and `P-2` for `T-2`, which change the same line of `README.md`; fold then stops it. The review lead runs and is connected.
pub async fn two_conflicting_proposals(fake: &FakeClaude) -> (Harness, Work) {
    let mut harness = task_worker_for(
        fake,
        &["Name the login screen", "Name the login endpoint"],
        WorkerState::Idle,
    )
    .await;
    let first = harness.start_work(TASK_WORKER, "T-1").await;
    let second = harness.start_work(TASK_WORKER, "T-2").await;
    first.write(&[("README.md", SCREEN)]);
    second.write(&[("README.md", ENDPOINT)]);
    assert_eq!(
        harness
            .propose(TASK_WORKER, &first, "Name the login screen")
            .await,
        "P-1"
    );
    assert_eq!(
        harness
            .propose(TASK_WORKER, &second, "Name the login endpoint")
            .await,
        "P-2"
    );
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    until_state(&harness, LEAD, WorkerState::Idle).await;
    harness.connect(LEAD).await;
    (harness, second)
}

/// The review lead starts a reviewer for `proposal`, which checks it out and approves `v1`; fold then stops it with the review claim.
async fn approved_by_a_reviewer(harness: &mut Harness, proposal: &str, reviewer: &str) {
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
        .call(reviewer, "create_review_checkout", json!({"id": proposal}))
        .await;
    harness
        .call(
            reviewer,
            "submit_review",
            json!({"id": proposal, "version": 1, "verdict": "approve", "body": "Read and run."}),
        )
        .await;
    until_state(harness, reviewer, WorkerState::Stopped).await;
}

/// Every message sent to `agent`, read or not: a running agent's push can count a message as read, so the stored threads are read instead of the inbox.
pub async fn told(harness: &Harness, agent: &str) -> Vec<Value> {
    let mut found = Vec::new();
    for event in harness.events().await {
        let to_agent = event.kind == "message.sent"
            && event.data["recipients"]
                .as_array()
                .is_some_and(|to| to.iter().any(|name| name == agent));
        if !to_agent {
            continue;
        }
        let id = event.data["message"].clone();
        let thread = harness
            .call(MAIN, "get_thread", json!({"thread": event.data["thread"]}))
            .await;
        found.extend(
            thread["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|message| message["id"] == id)
                .cloned(),
        );
    }
    found
}

/// Acceptance 48: two approved proposals change the same line. When the second one conflicts at integration, fold resumes the reviewer that approved it in its own session with the conflict report. The reviewer checks the version out merged onto the current main, resolves the conflict markers, sends the fix with `submit_review_fix` and approves it, and the proposal integrates. The task stays in Review all the time, and the author is never told and never runs again.
#[tokio::test]
async fn acceptance_48_the_approving_reviewer_fixes_the_conflict_and_the_author_is_not_called() {
    let fake = FakeClaude::new("result");
    let (mut harness, second) = two_conflicting_proposals(&fake).await;
    approved_by_a_reviewer(&mut harness, "P-1", REVIEWER).await;
    approved_by_a_reviewer(&mut harness, "P-2", SECOND_REVIEWER).await;
    let stopped = worker(&harness, SECOND_REVIEWER).await;
    let author_runs = fake.runs_of(TASK_WORKER).len();

    let integrated = harness
        .call(LEAD, "integrate_proposal", json!({"id": "P-1"}))
        .await;
    assert_eq!(integrated["revision"], "r2", "{integrated}");
    let refused = harness
        .refused(LEAD, "integrate_proposal", json!({"id": "P-2"}))
        .await;
    assert_eq!(refused["code"], "integration_conflicted", "{refused}");
    assert!(
        refused["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("the reviewer that approved the version fixes it")),
        "{refused}"
    );
    assert_eq!(status(&harness, "T-2").await, "review");

    let runs = until_runs(&fake, SECOND_REVIEWER, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(stopped.session_id.as_str()));
    let turn = &runs[1].stdin[1];
    for wanted in [
        "fold resumed you because P-2 v1 for T-2, which you approved, conflicts with main at r2",
        "You fix the conflict, not the author",
        "Conflicting paths (1): README.md.",
        "create_review_checkout",
        "submit_review_fix",
    ] {
        assert!(turn.contains(wanted), "{wanted}: {turn}");
    }
    let resumed = eventually(WAIT, async || {
        let found: Vec<Value> = harness
            .events()
            .await
            .into_iter()
            .filter(|event| event.kind == "worker.resumed" && event.data["reason"] == "conflict")
            .map(|event| event.data)
            .collect();
        if found.len() == 1 {
            Ok(found)
        } else {
            Err(found.len())
        }
    })
    .await;
    assert_eq!(
        (&resumed[0]["agent"], &resumed[0]["version"]),
        (&json!(SECOND_REVIEWER), &json!(1))
    );
    until_state(&harness, SECOND_REVIEWER, WorkerState::Idle).await;

    let checkout = harness
        .call(
            SECOND_REVIEWER,
            "create_review_checkout",
            json!({"id": "P-2"}),
        )
        .await;
    let dir = Path::new(harness.root()).join(checkout["path"].as_str().unwrap());
    let merged = std::fs::read_to_string(dir.join("README.md")).unwrap();
    for wanted in [
        "<<<<<<< workspace",
        SCREEN.trim_end(),
        ENDPOINT.trim_end(),
        ">>>>>>> main r2",
    ] {
        assert!(merged.contains(wanted), "{wanted}: {merged}");
    }
    let unresolved = harness
        .refused(
            SECOND_REVIEWER,
            "submit_review_fix",
            json!({
                "intent": "Make the change the task asks for.",
                "id": "P-2",
                "version": 1,
                "fixes": [{"path": "README.md", "reason": "keep both names"}],
                "checks": "none",
            }),
        )
        .await;
    assert_eq!(unresolved["code"], "unresolved_conflicts", "{unresolved}");

    std::fs::write(dir.join("README.md"), RESOLVED).unwrap();
    let fixed = harness
        .call(
            SECOND_REVIEWER,
            "submit_review_fix",
            json!({
                "intent": "Make the change the task asks for.",
                "id": "P-2",
                "version": 1,
                "fixes": [{"path": "README.md", "reason": "keep the screen name of main and the endpoint name of v1"}],
                "checks": "read the merged README.md",
            }),
        )
        .await;
    assert_eq!(fixed["version"], "v2", "{fixed}");
    assert_eq!(status(&harness, "T-2").await, "review");
    let view = harness
        .call(LEAD, "get_proposal", json!({"id": "P-2"}))
        .await;
    assert_eq!(view["versions"][1]["base"], "r2", "{view}");
    assert_eq!(view["versions"][1]["created_by"], SECOND_REVIEWER, "{view}");
    // The author's workspace takes the merged main as its base, so its next sync finds the fix merged.
    let workspace = harness
        .call(LEAD, "get_workspace", json!({"task": "T-2"}))
        .await;
    assert_eq!(workspace["base"], "r2", "{workspace}");

    harness
        .call(
            SECOND_REVIEWER,
            "submit_review",
            json!({"id": "P-2", "version": 2, "verdict": "approve", "body": "The conflict keeps both names."}),
        )
        .await;
    let integrated = harness
        .call(
            LEAD,
            "integrate_proposal",
            json!({"id": "P-2", "expected_version": 2}),
        )
        .await;
    assert_eq!(integrated["result"], "succeeded", "{integrated}");
    assert_eq!(status(&harness, "T-2").await, "done");
    let readme = harness.project.paths().root().join("README.md");
    eventually(WAIT, async || {
        let text = std::fs::read_to_string(&readme).unwrap_or_default();
        if text == RESOLVED { Ok(()) } else { Err(text) }
    })
    .await;

    // The author was never resumed, its files stayed as it wrote them, and fold never wrote to it about the conflict.
    assert_eq!(fake.runs_of(TASK_WORKER).len(), author_runs);
    assert_eq!(
        read_tree(&second.dir).get("README.md").map(Vec::as_slice),
        Some(ENDPOINT.as_bytes())
    );
    let statuses: Vec<Value> = harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "task.status_changed" && event.task.as_deref() == Some("T-2"))
        .map(|event| event.data["to"].clone())
        .collect();
    let since_review: Vec<&Value> = statuses
        .iter()
        .skip_while(|status| **status != json!("review"))
        .collect();
    assert_eq!(
        since_review,
        [&json!("review"), &json!("done")],
        "{statuses:?}"
    );
    let author_resumes = harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "worker.resumed" && event.data["agent"] == TASK_WORKER)
        .count();
    assert_eq!(author_resumes, 0);
    let told = told(&harness, TASK_WORKER).await;
    assert!(
        told.iter()
            .all(|message| !message.to_string().contains("conflicts with main")),
        "{told:?}"
    );
}

/// Acceptance 48: a version that a human approved and integrated has no reviewer agent to fix its conflict, so the review lead gets the notice to start one, the task stays in Review and the author is not told.
#[tokio::test]
async fn acceptance_48_a_conflict_of_a_human_approval_goes_to_the_review_lead() {
    let fake = FakeClaude::new("result");
    let (harness, _second) = two_conflicting_proposals(&fake).await;
    fold_daemon::services::projects::ensure_human(
        harness.project.registry(),
        harness.project.clock(),
        &fold_core::HumanName::parse(HUMAN).unwrap(),
    )
    .await
    .unwrap();
    let integrate =
        |proposal: &str| format!("/api/v1/projects/{SLUG}/proposals/{proposal}/integrate");
    let (code, first) = harness
        .http(
            Method::POST,
            &integrate("P-1"),
            Some(&json!({"expected_version": 1})),
            HUMAN,
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{first}");
    let (code, second) = harness
        .http(
            Method::POST,
            &integrate("P-2"),
            Some(&json!({"expected_version": 1})),
            HUMAN,
        )
        .await;
    assert_eq!(code, StatusCode::CONFLICT, "{second}");
    assert_eq!(status(&harness, "T-2").await, "review");

    let notice = eventually(WAIT, async || {
        let found = told(&harness, LEAD)
            .await
            .into_iter()
            .find(|message| message.to_string().contains("conflicts with main"));
        found.ok_or(())
    })
    .await;
    let text = notice.to_string();
    for wanted in [
        "P-2 v1 for T-2 conflicts with main at r2 and was not integrated.",
        &format!("The human @{HUMAN} approved v1"),
        "start one for P-2 with start_worker",
        "its author gets no conflict notice",
    ] {
        assert!(text.contains(wanted), "{wanted}: {text}");
    }
    let to_reviewer = harness
        .events()
        .await
        .into_iter()
        .find(|event| event.kind == "integration.conflicted")
        .map(|event| event.data["fixer"].clone());
    assert_eq!(to_reviewer, Some(Value::Null));
    assert!(
        harness
            .events()
            .await
            .iter()
            .all(|event| event.kind != "worker.resumed" || event.data["agent"] != TASK_WORKER)
    );
}

/// Acceptance 48: a conflict that comes while the project's agents are paused waits for the end of the pause, which gives it to the reviewer that approved the version from the stored state; the review lead is not told.
#[tokio::test]
async fn acceptance_48_a_conflict_during_a_pause_reaches_its_reviewer_when_the_pause_ends() {
    let fake = FakeClaude::new("result");
    let (mut harness, _second) = two_conflicting_proposals(&fake).await;
    approved_by_a_reviewer(&mut harness, "P-1", REVIEWER).await;
    approved_by_a_reviewer(&mut harness, "P-2", SECOND_REVIEWER).await;
    harness.call(MAIN, "pause_project", json!({})).await;

    harness
        .call(LEAD, "integrate_proposal", json!({"id": "P-1"}))
        .await;
    let refused = harness
        .refused(LEAD, "integrate_proposal", json!({"id": "P-2"}))
        .await;
    assert_eq!(refused["code"], "integration_conflicted", "{refused}");
    assert_eq!(
        fake.runs_of(SECOND_REVIEWER).len(),
        1,
        "a paused project starts nobody"
    );

    harness.call(MAIN, "resume_project", json!({})).await;
    let runs = until_runs(&fake, SECOND_REVIEWER, 2).await;
    assert!(
        runs[1].stdin[1].contains("P-2 v1 for T-2, which you approved, conflicts with main at r2"),
        "{}",
        runs[1].stdin[1]
    );
    eventually(WAIT, async || {
        let resumed = harness
            .events()
            .await
            .into_iter()
            .filter(|event| event.kind == "worker.resumed" && event.data["reason"] == "conflict")
            .count();
        if resumed == 1 { Ok(()) } else { Err(resumed) }
    })
    .await;
    assert_eq!(status(&harness, "T-2").await, "review");
    assert!(
        told(&harness, LEAD)
            .await
            .iter()
            .all(|message| !message.to_string().contains("conflicts with main")),
        "the review lead is not told"
    );
}
