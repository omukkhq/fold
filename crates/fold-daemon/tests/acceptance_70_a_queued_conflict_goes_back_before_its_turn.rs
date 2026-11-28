#![cfg(test)]

use fold_core::{RevisionId, WorkerState};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::acceptance_48_reviewer_fixes_a_conflict::{LEAD, status};
use crate::auto_integrate::Running;
use crate::common::acceptance::{Harness, WAIT};
use crate::worker_resume::{MAIN, TASK_WORKER, task_worker_for, until_runs, until_state, worker};
use crate::workers_support::FakeClaude;

/// `P-1` and `P-2` add a line at the same place of `README.md`, which holds `# demo` in `r1`; `P-3` adds a file of its own.
const FILES: [(&str, &str); 3] = [
    ("README.md", "# demo\nthe login screen\n"),
    ("README.md", "# demo\nthe login endpoint\n"),
    ("docs/notes.md", "notes\n"),
];

/// Three proposals by one task worker, each approved by its own reviewer, which fold then stops with its review claim. The automatic integration does not run, so each waits in the queue. Returns the reviewers in proposal order.
async fn three_approved_proposals(fake: &FakeClaude) -> (Harness, Vec<String>) {
    let mut harness = task_worker_for(
        fake,
        &[
            "Name the login screen",
            "Name the login endpoint",
            "Add notes",
        ],
        WorkerState::Idle,
    )
    .await;
    for (index, (path, content)) in FILES.iter().enumerate() {
        let task = format!("T-{}", index + 1);
        let work = harness.start_work(TASK_WORKER, &task).await;
        work.write(&[(path, content)]);
        let proposal = harness.propose(TASK_WORKER, &work, &task).await;
        assert_eq!(proposal, format!("P-{}", index + 1));
    }
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    until_state(&harness, LEAD, WorkerState::Idle).await;
    harness.connect(LEAD).await;
    let mut reviewers = Vec::new();
    for proposal in ["P-1", "P-2", "P-3"] {
        let started = harness
            .call(
                LEAD,
                "start_worker",
                json!({"role": "reviewer", "proposals": [proposal]}),
            )
            .await;
        let reviewer = started["worker"]["name"].as_str().unwrap().to_owned();
        until_state(&harness, &reviewer, WorkerState::Idle).await;
        harness.connect(&reviewer).await;
        harness
            .call(
                &reviewer,
                "submit_review",
                json!({"id": proposal, "version": 1, "verdict": "approve", "body": "Read and run."}),
            )
            .await;
        until_state(&harness, &reviewer, WorkerState::Stopped).await;
        reviewers.push(reviewer);
    }
    (harness, reviewers)
}

async fn conflicted_events(harness: &Harness) -> Vec<Value> {
    harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "integration.conflicted")
        .map(|event| event.data)
        .collect()
}

/// Acceptance 70: after `P-1` integrates, fold test-merges the queue onto the new main before any other turn. `P-2`, which adds its line where `P-1` added one, leaves the queue and goes to its own approving reviewer, which fold resumes in its session; `get_proposal` names the conflicting file and that reviewer. `P-3` merges cleanly, keeps its place and integrates in the next turn, while `P-2` waits for its fix.
#[tokio::test]
async fn acceptance_70_a_queued_proposal_that_would_conflict_goes_to_its_reviewer_before_its_turn()
{
    let fake = FakeClaude::new("result");
    let (harness, reviewers) = three_approved_proposals(&fake).await;
    let second = reviewers[1].as_str();
    let session = worker(&harness, second).await.session_id;
    let integrated = harness
        .call(LEAD, "integrate_proposal", json!({"id": "P-1"}))
        .await;
    assert_eq!(integrated["revision"], "r2", "{integrated}");

    let conflicted = eventually(WAIT, async || {
        let found = conflicted_events(&harness).await;
        if found.is_empty() { Err(()) } else { Ok(found) }
    })
    .await;
    assert_eq!(conflicted.len(), 1, "{conflicted:?}");
    for (key, wanted) in [
        ("proposal", json!("P-2")),
        ("main_revision", json!("r2")),
        ("fixer", json!(second)),
        ("before_turn", json!(true)),
        ("files", json!(1)),
    ] {
        assert_eq!(conflicted[0][key], wanted, "{key}: {}", conflicted[0]);
    }
    let runs = until_runs(&fake, second, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(session.as_str()));
    assert!(
        runs[1].stdin[1].contains("P-2 v1 for T-2, which you approved, conflicts with main at r2"),
        "{}",
        runs[1].stdin[1]
    );

    let proposal = harness
        .call(LEAD, "get_proposal", json!({"id": "P-2"}))
        .await;
    assert_eq!(
        proposal["conflict"]["files"][0]["path"], "README.md",
        "{proposal}"
    );
    let hunk = &proposal["conflict"]["files"][0]["hunks"][0];
    assert_eq!(
        hunk["main"]["lines"],
        json!(["the login screen\n"]),
        "{hunk}"
    );
    assert_eq!(
        hunk["version"]["lines"],
        json!(["the login endpoint\n"]),
        "{hunk}"
    );

    assert_eq!(
        proposal["conflicted_reason"],
        json!({"kind": "merge_conflict", "fixer": second, "before_turn": true})
    );
    let clean = harness
        .call(LEAD, "get_proposal", json!({"id": "P-3"}))
        .await;
    assert_eq!(clean["conflict"], Value::Null, "{clean}");
    assert_eq!(clean["conflicted_reason"], Value::Null, "{clean}");
    assert_eq!(proposal["waits_for"]["reason"], "conflict", "{proposal}");
    assert_eq!(clean["queue"]["position"], 1, "{clean}");
    assert_eq!(clean["waits_for"], Value::Null, "{clean}");
    assert_eq!(status(&harness, "T-2").await, "review");

    let reviews = harness.handle.reviews(&harness.state.registry);
    let again = reviews
        .find_queue_conflicts(RevisionId::from_number(2).unwrap())
        .await
        .unwrap();
    assert!(
        again.is_empty(),
        "P-3 merges cleanly and P-2 left the queue: {again:?}"
    );

    let integrate = Running::start(&harness.state);
    let integrated = eventually(WAIT, async || {
        let proposal = harness
            .call(LEAD, "get_proposal", json!({"id": "P-3"}))
            .await;
        if proposal["state"] == "integrated" {
            Ok(proposal)
        } else {
            Err(proposal["state"].clone())
        }
    })
    .await;
    assert_eq!(integrated["integrated"]["revision"], "r3", "{integrated}");
    integrate.stop().await;
    harness.handle.checks.wait_idle().await;
    let waiting = harness
        .call(LEAD, "get_proposal", json!({"id": "P-2"}))
        .await;
    assert_eq!(waiting["state"], "open");
    assert_eq!(
        conflicted_events(&harness).await.len(),
        1,
        "P-2 waits for its fix and is not merged again"
    );
}
