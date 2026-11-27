#![cfg(test)]

use fold_store::testing::eventually;
use serde_json::json;

use crate::common::acceptance::{Harness, WAIT};

/// The messages from fold to `agent` that a task watch sent, oldest first.
async fn watch_messages(h: &Harness, agent: &str) -> Vec<String> {
    h.fold_messages(agent)
        .await
        .into_iter()
        .filter(|body| body.contains("watch"))
        .collect()
}

/// Waits until `agent` holds `count` watch messages and returns them.
async fn until_watch_messages(h: &Harness, agent: &str, count: usize) -> Vec<String> {
    eventually(WAIT, async || {
        let messages = watch_messages(h, agent).await;
        if messages.len() == count {
            Ok(messages)
        } else {
            Err(messages)
        }
    })
    .await
}

/// `backend` builds `task` and the reviewer integrates it, so the task is Done.
async fn integrate(h: &Harness, task: &str, file: &str) {
    let work = h.start_work("backend", task).await;
    work.write(&[(file, "done\n")]);
    let proposal = h.propose("backend", &work, "Build the task").await;
    h.give_review("review", &proposal).await;
    h.approve(&proposal).await;
    h.call("review", "integrate_proposal", json!({"id": proposal}))
        .await;
}

/// Acceptance 60: an agent watches two tasks without blocking its turn. The frontend agent calls `watch_tasks` with two backend tasks and goes on with its work. When the first task is integrated, fold sends it one message that names the task; when the second is, one more message and a last one that says every watched task is Done, and the watch ends.
#[tokio::test]
async fn acceptance_60_a_watch_tells_each_done_task_and_the_end() {
    let h = Harness::start().await;
    let login = h
        .create_task_for("frontend", "backend", "Add the login endpoint")
        .await;
    let logout = h
        .create_task_for("frontend", "backend", "Add the logout endpoint")
        .await;
    let watched = h
        .call(
            "frontend",
            "watch_tasks",
            json!({"tasks": [login, logout, login]}),
        )
        .await;
    assert_eq!(watched["watching"], json!([login, logout]), "{watched}");
    assert_eq!(watched["done"], json!([]), "{watched}");
    assert!(watch_messages(&h, "frontend").await.is_empty());

    integrate(&h, &login, "src/login.txt").await;
    let first = until_watch_messages(&h, "frontend", 1).await;
    assert!(
        first[0].starts_with(&format!("{login} is Done: Add the login endpoint.")),
        "{first:?}"
    );

    integrate(&h, &logout, "src/logout.txt").await;
    let all = until_watch_messages(&h, "frontend", 3).await;
    assert!(
        all[1].starts_with(&format!("{logout} is Done: Add the logout endpoint.")),
        "{all:?}"
    );
    assert_eq!(
        all[2],
        format!("Every task you watched is Done: {login}, {logout}. The watch has ended.")
    );
    let ended = h.call("frontend", "unwatch_tasks", json!({})).await;
    assert_eq!(ended["watching"], json!([]), "{ended}");
    assert!(watch_messages(&h, "backend").await.is_empty());
}

/// Acceptance 60, at once: a task that is Done already is reported in the call itself, with the last message, so the watch ends at once.
#[tokio::test]
async fn acceptance_60_a_done_task_is_reported_at_once() {
    let h = Harness::start().await;
    let login = h
        .create_task_for("frontend", "backend", "Add the login endpoint")
        .await;
    integrate(&h, &login, "src/login.txt").await;
    let watched = h
        .call("frontend", "watch_tasks", json!({"tasks": [login]}))
        .await;
    assert_eq!(watched["watching"], json!([]), "{watched}");
    assert_eq!(watched["done"], json!([login]), "{watched}");
    let messages = watch_messages(&h, "frontend").await;
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(
        messages[1],
        format!("Every task you watched is Done: {login}. The watch has ended.")
    );
}

/// Acceptance 60, unwatch and refusals: `unwatch_tasks` ends a watch without a message, and an empty list or an unknown task watches nothing.
#[tokio::test]
async fn acceptance_60_unwatch_ends_the_watch_and_bad_input_is_refused() {
    let h = Harness::start().await;
    let login = h
        .create_task_for("frontend", "backend", "Add the login endpoint")
        .await;
    let logout = h
        .create_task_for("frontend", "backend", "Add the logout endpoint")
        .await;
    let empty = h
        .refused("frontend", "watch_tasks", json!({"tasks": []}))
        .await;
    assert_eq!(empty["code"], "invalid_request", "{empty}");
    let unknown = h
        .refused("frontend", "watch_tasks", json!({"tasks": [login, "T-99"]}))
        .await;
    assert_eq!(unknown["code"], "task_not_found", "{unknown}");
    let none = h.call("frontend", "unwatch_tasks", json!({})).await;
    assert_eq!(none["watching"], json!([]), "{none}");

    h.call("frontend", "watch_tasks", json!({"tasks": [login, logout]}))
        .await;
    let left = h
        .call("frontend", "unwatch_tasks", json!({"tasks": [login]}))
        .await;
    assert_eq!(left["watching"], json!([logout]), "{left}");
    integrate(&h, &login, "src/login.txt").await;
    let gone = h.call("frontend", "unwatch_tasks", json!({})).await;
    assert_eq!(gone["watching"], json!([]), "{gone}");
    integrate(&h, &logout, "src/logout.txt").await;
    assert!(watch_messages(&h, "frontend").await.is_empty());
}

/// Acceptance 60, archived tasks: an archived task never becomes Done, so a watch refuses it with `task_closed`, and a watched task that is archived later leaves the watch without a message, so the watch can still end.
#[tokio::test]
async fn acceptance_60_an_archived_task_leaves_the_watch() {
    let h = Harness::start().await;
    let login = h
        .create_task_for("frontend", "backend", "Add the login endpoint")
        .await;
    let logout = h
        .create_task_for("frontend", "backend", "Add the logout endpoint")
        .await;
    let old = h
        .create_task_for("frontend", "backend", "Add the old endpoint")
        .await;
    h.call("frontend", "archive_task", json!({"id": old})).await;
    let closed = h
        .refused("frontend", "watch_tasks", json!({"tasks": [login, old]}))
        .await;
    assert_eq!(closed["code"], "task_closed", "{closed}");

    let watched = h
        .call("frontend", "watch_tasks", json!({"tasks": [login, logout]}))
        .await;
    assert_eq!(watched["watching"], json!([login, logout]), "{watched}");
    h.call("frontend", "archive_task", json!({"id": logout}))
        .await;
    eventually(WAIT, async || {
        let left = h
            .call("frontend", "unwatch_tasks", json!({"tasks": []}))
            .await;
        if left["watching"] == json!([login]) {
            Ok(())
        } else {
            Err(left)
        }
    })
    .await;
    integrate(&h, &login, "src/login.txt").await;
    let all = until_watch_messages(&h, "frontend", 2).await;
    assert_eq!(
        all[1],
        format!("Every task you watched is Done: {login}. The watch has ended.")
    );
}
