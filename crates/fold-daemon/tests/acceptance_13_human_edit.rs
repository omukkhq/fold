#![cfg(test)]

use crate::common::acceptance::{FakeInbox, HUMAN, Harness, SLUG, WAIT, assert_in_order};
use axum::http::{Method, StatusCode};
use fold_store::testing::eventually;
use serde_json::{Value, json};

const ENDPOINT: (&str, &str) = ("src/login.txt", "POST /login\n// TODO remove\n");
const EDITED: &str = "POST /login\n";

/// The next post to `inbox` that contains `needle`; earlier notices, such as the one of the opened proposal, are skipped.
async fn notice_with(inbox: &mut FakeInbox, needle: &str) -> String {
    loop {
        let posted = inbox.next().await;
        if posted.contains(needle) {
            return posted;
        }
    }
}

/// Waits until the task view says whether the holder's files differ from the newest version; a proposal command returns before the live state compares them.
async fn wait_for_edits(harness: &Harness, task: &str, edited: bool) {
    let path = format!("/api/v1/projects/{SLUG}/tasks/{task}");
    eventually(WAIT, async || {
        let (_, view) = harness.http(Method::GET, &path, None, HUMAN).await;
        if view["edits_after_version"] == json!(edited) {
            Ok(())
        } else {
            Err(view["edits_after_version"].clone())
        }
    })
    .await;
}

fn proposal_path(proposal: &str, rest: &str) -> String {
    format!("/api/v1/projects/{SLUG}/proposals/{proposal}{rest}")
}

/// Acceptance 13: on a task with the human review mark, backend proposes and the review lead approves. The marked human changes one file over HTTP; the change is a new version under the human's name that keeps the review lead's approval, never a human's, and backend gets a push to sync. Backend cannot record until its sync writes the edit into its files. The human sends the edit for review again, which withdraws the kept approval and wakes the review lead; the review lead and the human approve the edited version, and it integrates with the human's change.
#[tokio::test]
async fn acceptance_13_a_human_edit_in_a_human_review_keeps_the_approval_until_it_is_sent_for_review_again()
 {
    let harness = Harness::start().await;
    let mut backend_inbox = harness.register_inbox("backend").await;
    let mut review_inbox = harness.register_inbox("review").await;
    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    harness
        .call(
            "review",
            "require_human_review",
            json!({"task": task, "reviewer": HUMAN, "reason": "touches auth"}),
        )
        .await;
    let work = harness.start_work("backend", &task).await;
    work.write(&[ENDPOINT]);
    let proposal = harness
        .propose("backend", &work, "Add the login endpoint")
        .await;
    wait_for_edits(&harness, &task, false).await;
    harness.approve(&proposal).await;

    let (code, refused) = harness
        .http(
            Method::POST,
            &proposal_path(&proposal, "/edits"),
            Some(&json!({"version": 1, "path": ENDPOINT.0, "content": EDITED})),
            "tanim",
        )
        .await;
    assert_eq!(
        (code, refused["error"]["code"].clone()),
        (StatusCode::FORBIDDEN, json!("human_edit_denied")),
        "a human the mark does not name is refused"
    );

    let (code, file) = harness
        .http(
            Method::GET,
            &proposal_path(&proposal, &format!("/file?path={}", ENDPOINT.0)),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{file}");
    assert_eq!(file["text"], ENDPOINT.1);
    let (code, made) = harness
        .http(
            Method::POST,
            &proposal_path(&proposal, "/edits"),
            Some(&json!({"version": 1, "path": ENDPOINT.0, "content": EDITED})),
            HUMAN,
        )
        .await;
    assert_eq!(code, StatusCode::CREATED, "{made}");
    assert_eq!(made["version"], 2);
    assert_eq!(made["created_by"], format!("human:{HUMAN}"));
    assert_eq!(made["human_edit"], true);

    // The worker hears about the edit, and the edit keeps the review lead's approval.
    let notice = notice_with(&mut backend_inbox, "a human edited").await;
    assert!(notice.contains(&format!("a human edited {proposal} as version 2")));
    assert!(notice.contains("sync_workspace"), "{notice}");
    // The human's version is not in backend's files, so backend edits after it until it syncs.
    wait_for_edits(&harness, &task, true).await;
    let view = harness
        .call("backend", "get_proposal", json!({"id": proposal}))
        .await;
    assert_eq!(view["current_version"], "v2");
    assert_eq!(view["versions"][1]["created_by"], format!("human:{HUMAN}"));
    assert_eq!(view["task"]["status"], "review");
    let kept: Vec<&Value> = view["reviews"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|review| review["version"] == "v2")
        .collect();
    assert_eq!(kept.len(), 1, "{view}");
    assert_eq!(kept[0]["verdict"], "approve");
    assert_eq!(kept[0]["reviewer"], "review");
    assert_eq!(kept[0]["kept_from"], "v1");

    // fold leaves the worker's files alone; its sync writes the edit before it records again.
    let on_disk = work.dir.join(ENDPOINT.0);
    assert_eq!(std::fs::read_to_string(&on_disk).unwrap(), ENDPOINT.1);
    let error = harness
        .refused(
            "backend",
            "record_changes",
            json!({"ws": work.workspace, "message": "more"}),
        )
        .await;
    assert_eq!(error["code"], "workspace_write_unfinished", "{error}");
    harness
        .call("backend", "sync_workspace", json!({"ws": work.workspace}))
        .await;
    assert_eq!(std::fs::read_to_string(&on_disk).unwrap(), EDITED);
    wait_for_edits(&harness, &task, false).await;

    // Sent for review again, the kept approval stops counting and the review lead gets the turn.
    let (code, summary) = harness
        .http(
            Method::POST,
            &proposal_path(&proposal, "/review-again"),
            Some(&json!({"version": 2})),
            HUMAN,
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{summary}");
    assert_eq!(summary["latest_verdict"], Value::Null);
    assert_eq!(summary["turn"], "with_reviewer");
    let notice = notice_with(&mut review_inbox, "for review again").await;
    assert!(notice.contains("create_review_checkout"), "{notice}");
    let view = harness
        .call("review", "get_proposal", json!({"id": proposal}))
        .await;
    assert!(
        view["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .all(|review| review["version"] == "v1"),
        "{view}"
    );

    harness.approve(&proposal).await;
    let (code, approved) = harness
        .http(
            Method::POST,
            &proposal_path(&proposal, "/reviews"),
            Some(&json!({"version": 2, "verdict": "approve", "body": "seen"})),
            HUMAN,
        )
        .await;
    assert_eq!(code, StatusCode::CREATED, "{approved}");
    assert_eq!(approved["summary"]["human_approved"], true);
    let integrated = harness
        .call(
            "review",
            "integrate_proposal",
            json!({"id": proposal, "expected_version": 2}),
        )
        .await;
    assert_eq!(integrated["result"], "succeeded", "{integrated}");
    let revision = harness
        .call("review", "get_revision", json!({"r": "r2"}))
        .await;
    assert_eq!(revision["proposal"], proposal.as_str());
    assert_eq!(harness.completions(&task).await, 1);
    let (_, main_file) = harness
        .http(
            Method::GET,
            &format!(
                "/api/v1/projects/{SLUG}/revisions/r2/file?path={}",
                ENDPOINT.0
            ),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(main_file["text"], EDITED);

    let events = harness.events().await;
    assert_in_order(
        &events,
        &[
            "proposal.opened",
            "review.submitted",
            "proposal.human_edited",
            "checkpoint.restored",
            "proposal.review_again_requested",
            "review.submitted",
            "review.submitted",
            "integration.succeeded",
        ],
    );
    let edited = events
        .iter()
        .find(|event| event.kind == "proposal.human_edited")
        .unwrap();
    assert_eq!(edited.actor, format!("human:{HUMAN}"));
    assert_eq!(edited.data["kept_approvals"], 1);
}
