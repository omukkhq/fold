#![cfg(test)]

use std::collections::BTreeMap;
use std::path::Path;

use crate::common::acceptance::{Harness, SLUG, WAIT, assert_in_order, read_tree};
use axum::http::{Method, StatusCode};
use fold_store::testing::eventually;
use serde_json::{Value, json};

const ENDPOINT: (&str, &str) = ("src/login.txt", "POST /login\n");

/// `README.md` of `r1` plus `files`, as a workspace or a checkout holds them.
fn tree_with(files: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
    let mut tree = BTreeMap::from([("README.md".to_owned(), b"# demo\n".to_vec())]);
    for (path, content) in files {
        tree.insert((*path).to_owned(), content.as_bytes().to_vec());
    }
    tree
}

async fn status(harness: &Harness, task: &str) -> Value {
    harness
        .call("review", "get_task", json!({"id": task}))
        .await["status"]
        .clone()
}

/// Acceptance 5: backend opens a proposal and its task moves to Review; review finds it, claims the review, reads a checkout of the version, approves and integrates. The task reaches Done through the integration alone, main moves to `r2` with the file, the project folder shows the file, the workspace is archived, and the events tell the story in order.
#[tokio::test]
async fn acceptance_5_backend_proposal_is_reviewed_and_integrated_with_no_card_moved_by_hand() {
    let harness = Harness::start().await;
    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let work = harness.start_work("backend", &task).await;
    work.write(&[ENDPOINT]);
    let proposal = harness
        .propose("backend", &work, "Add the login endpoint")
        .await;
    assert_eq!(status(&harness, &task).await, "review");

    let listed = harness
        .call("review", "list_proposals", json!({"state": "open"}))
        .await;
    let listed = listed["proposals"].as_array().unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["id"], proposal.as_str());
    assert_eq!(listed[0]["author"], "backend");
    assert_eq!(listed[0]["claim"], Value::Null);

    harness.give_review("review", &proposal).await;
    let checkout = harness
        .call("review", "create_review_checkout", json!({"id": proposal}))
        .await;
    assert_eq!(checkout["version"], "v1");
    let checkout_dir = Path::new(harness.root()).join(checkout["path"].as_str().unwrap());
    assert_eq!(read_tree(&checkout_dir), tree_with(&[ENDPOINT]));

    harness.approve(&proposal).await;
    let integrated = harness
        .call(
            "review",
            "integrate_proposal",
            json!({"id": proposal, "expected_version": 1}),
        )
        .await;
    assert_eq!(
        integrated,
        json!({"id": proposal, "result": "succeeded", "revision": "r2"})
    );

    assert_eq!(status(&harness, &task).await, "done");
    assert_eq!(harness.completions(&task).await, 1);
    let head = harness.head().await;
    assert_eq!(head.number.to_string(), "r2");
    let shown = harness.project.paths().root().join(ENDPOINT.0);
    let content = eventually(WAIT, async || {
        std::fs::read_to_string(&shown).map_err(|error| error.kind())
    })
    .await;
    assert_eq!(content, ENDPOINT.1);
    let revision = harness
        .call("review", "get_revision", json!({"r": "r2"}))
        .await;
    assert_eq!(revision["proposal"], proposal.as_str());
    let files: Vec<(&str, &str)> = revision["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap(),
                file["change"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(files, [(ENDPOINT.0, "added")]);
    let workspace = harness
        .call("backend", "get_workspace", json!({"ws": work.workspace}))
        .await;
    assert_eq!(workspace["state"], "archived");

    // Every status change came from the calculation, never from an agent.
    let events = harness.events().await;
    let changes: Vec<(&str, &str, &str)> = events
        .iter()
        .filter(|event| event.kind == "task.status_changed")
        .map(|event| {
            (
                event.actor.as_str(),
                event.data["from"].as_str().unwrap(),
                event.data["to"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        changes,
        [
            ("fold", "backlog", "todo"),
            ("fold", "todo", "in_progress"),
            ("fold", "in_progress", "review"),
            ("fold", "review", "done"),
        ]
    );
    let from_opened: Vec<_> = events
        .iter()
        .skip_while(|event| event.kind != "proposal.opened")
        .cloned()
        .collect();
    assert_in_order(
        &from_opened,
        &[
            "proposal.opened",
            "review.submitted",
            "integration.succeeded",
            "main.advanced",
            "proposal.integrated",
            "task.status_changed",
        ],
    );
    let done = from_opened
        .iter()
        .rev()
        .find(|event| event.kind == "task.status_changed")
        .unwrap();
    assert_eq!(done.task.as_deref(), Some(task.as_str()));
    assert_eq!(done.data["to"], "done");
}

/// Acceptance 5, human case: on a project with no main agent and no review lead, where `review` is a session opened without a role and may not approve or integrate, a named human integrates over `review`'s earlier `request_changes` through the HTTP route. The integration counts as the human's approval of that version; an agent without the right to decide is still refused.
#[tokio::test]
async fn acceptance_5_named_human_integrates_without_an_agent_approval_over_an_earlier_request_changes()
 {
    let mut harness = Harness::start_without_roles().await;
    // `backend` claims its task as the lead of its area; no agent may decide.
    harness.lead_the_work_area().await;
    let agents = harness.call("review", "list_agents", json!({})).await;
    assert!(
        agents["agents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|agent| agent["tags"].as_array().is_none_or(Vec::is_empty)),
        "{agents}"
    );

    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let work = harness.start_work("backend", &task).await;
    work.write(&[ENDPOINT]);
    let proposal = harness
        .propose("backend", &work, "Add the login endpoint")
        .await;
    let requested = harness
        .call(
            "review",
            "submit_review",
            json!({"id": proposal, "version": 1, "verdict": "request_changes", "body": "Add a test."}),
        )
        .await;
    assert_eq!(requested["verdict"], "request_changes");
    assert_eq!(status(&harness, &task).await, "in_progress");
    let before = harness.head().await;

    let (code, answer) = harness
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/proposals/{proposal}/integrate"),
            Some(&json!({"expected_version": 1})),
            "ana",
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{answer}");
    assert_eq!(
        answer,
        json!({"proposal": proposal, "result": "succeeded", "revision": "r2", "run": null})
    );

    let after = harness.head().await;
    assert_eq!(after.number.to_string(), "r2");
    assert_eq!(after.parent, Some(before.number));
    let view = harness
        .call("review", "get_proposal", json!({"id": proposal}))
        .await;
    assert_eq!(view["state"], "integrated");
    let verdicts: Vec<(&str, &str, &str)> = view["reviews"]
        .as_array()
        .unwrap()
        .iter()
        .map(|review| {
            (
                review["version"].as_str().unwrap(),
                review["verdict"].as_str().unwrap(),
                review["reviewer"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        verdicts,
        [
            ("v1", "request_changes", "review"),
            ("v1", "approve", "human:ana"),
        ]
    );
    assert_eq!(status(&harness, &task).await, "done");
    assert_eq!(harness.completions(&task).await, 1);

    let other = harness
        .create_task("backend", "Add the logout endpoint")
        .await;
    let work = harness.start_work("backend", &other).await;
    work.write(&[("src/logout.txt", "POST /logout\n")]);
    let second = harness
        .propose("backend", &work, "Add the logout endpoint")
        .await;
    let error = harness
        .refused("review", "integrate_proposal", json!({"id": second}))
        .await;
    assert_eq!(error["code"], "integrator_required", "{error}");
    assert_eq!(harness.head().await.number, after.number);
    assert_eq!(status(&harness, &other).await, "review");
}
