#![cfg(test)]

use crate::common::acceptance::{Harness, StoredEvent};
use serde_json::{Value, json};

async fn status(harness: &Harness, task: &str) -> Value {
    harness
        .call("review", "get_task", json!({"id": task}))
        .await["status"]
        .clone()
}

/// The ids `get_ready_tasks` returns to `agent`.
async fn ready(harness: &Harness, agent: &str) -> Vec<String> {
    harness.call(agent, "get_ready_tasks", json!({})).await["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["id"].as_str().unwrap().to_owned())
        .collect()
}

/// The events about `task` from index `from` on, with their index.
fn about<'a>(events: &'a [StoredEvent], from: usize, task: &str) -> Vec<(usize, &'a StoredEvent)> {
    events
        .iter()
        .enumerate()
        .skip(from)
        .filter(|(_, event)| event.task.as_deref() == Some(task))
        .collect()
}

/// Acceptance 6: a frontend task that needs a backend task waits in Backlog and moves to Todo in the transaction that integrates the backend task, with no agent touching it; it then shows as ready work, and the author's inbox holds the integration message that names it.
#[tokio::test]
async fn acceptance_6_frontend_task_moves_from_backlog_to_todo_when_backend_reaches_done() {
    let harness = Harness::start().await;
    let endpoint = harness
        .create_task("frontend", "Add the login endpoint")
        .await;
    let screen = harness
        .create_task("frontend", "Build the login screen")
        .await;
    harness
        .call(
            "frontend",
            "add_dependency",
            json!({"task": screen, "needs": endpoint}),
        )
        .await;
    assert_eq!(status(&harness, &screen).await, "backlog");
    assert_eq!(ready(&harness, "frontend").await, [endpoint.as_str()]);
    let quiet_from = harness.events().await.len();

    let work = harness.start_work("backend", &endpoint).await;
    work.write(&[("src/login.txt", "POST /login\n")]);
    let proposal = harness
        .propose("backend", &work, "Add the login endpoint")
        .await;
    assert_eq!(status(&harness, &screen).await, "backlog");
    harness
        .call("review", "claim_review", json!({"id": proposal}))
        .await;
    harness.approve(&proposal).await;
    harness
        .call("review", "integrate_proposal", json!({"id": proposal}))
        .await;

    assert_eq!(status(&harness, &endpoint).await, "done");
    assert_eq!(status(&harness, &screen).await, "todo");
    assert_eq!(ready(&harness, "frontend").await, [screen.as_str()]);

    // The frontend task has no owner yet, so the integration message goes to backend, the author, and names the task it made ready.
    let inbox = harness.call("backend", "get_inbox", json!({})).await;
    let messages = inbox["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "{inbox}");
    assert_eq!(messages[0]["from"], "fold");
    assert_eq!(
        messages[0]["body"],
        format!("{proposal} for {endpoint} is integrated into main as r2. Now ready: {screen}.")
    );
    let inbox = harness.call("frontend", "get_inbox", json!({})).await;
    assert_eq!(inbox["messages"], json!([]), "{inbox}");

    // The only event about the frontend task since it was set up is the calculated move, in the integration's commit.
    let events = harness.events().await;
    let touched = about(&events, quiet_from, &screen);
    assert_eq!(touched.len(), 1, "{touched:?}");
    let (position, moved) = touched[0];
    assert_eq!(moved.kind, "task.status_changed");
    assert_eq!(moved.actor, "fold");
    assert_eq!(moved.data["from"], "backlog");
    assert_eq!(moved.data["to"], "todo");
    let integrated = events
        .iter()
        .position(|event| event.kind == "proposal.integrated")
        .unwrap();
    assert!(position > integrated, "{events:?}");
}

/// Acceptance 6, refusal: a proposal on a task whose prerequisite is not Done is approved but does not integrate; the refusal names the unfinished task and main stays where it was.
#[tokio::test]
async fn acceptance_6_integration_is_refused_while_a_prerequisite_is_open() {
    let harness = Harness::start().await;
    let endpoint = harness
        .create_task("frontend", "Add the login endpoint")
        .await;
    let screen = harness
        .create_task("frontend", "Build the login screen")
        .await;
    harness
        .call(
            "frontend",
            "add_dependency",
            json!({"task": screen, "needs": endpoint}),
        )
        .await;
    let work = harness.start_work("frontend", &screen).await;
    work.write(&[("src/screen.txt", "login screen\n")]);
    let proposal = harness
        .propose("frontend", &work, "Build the login screen")
        .await;
    harness.approve(&proposal).await;

    let error = harness
        .refused("review", "integrate_proposal", json!({"id": proposal}))
        .await;
    assert_eq!(error["code"], "prerequisites_not_done", "{error}");
    assert_eq!(
        error["details"]["prerequisites"],
        json!([{"task": endpoint, "status": "todo"}])
    );
    assert_eq!(harness.head().await.number.to_string(), "r1");
    assert_eq!(harness.completions(&screen).await, 0);
    let view = harness
        .call("review", "get_proposal", json!({"id": proposal}))
        .await;
    assert_eq!(view["state"], "open");
    assert!(
        harness
            .events()
            .await
            .iter()
            .any(|event| event.kind == "integration.refused"
                && event.data["code"] == "prerequisites_not_done")
    );
}
