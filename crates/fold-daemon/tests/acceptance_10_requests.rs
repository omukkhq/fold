#![cfg(test)]

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, TOKEN, message_notice, wire};

const BASE: &str = "/api/v1/projects/demo";

/// A draft of the board search: `c1` builds the query, `c2` shows it and needs `c1`.
fn search_draft(id: &str, filter_title: &str) -> Value {
    json!({
        "id": id,
        "cards": [
            {"key": "c1", "title": filter_title, "description": "Add the search query.\n\n**Acceptance**: the query test passes."},
            {"key": "c2", "title": "Show the board search box", "description": "Add the box.\n\n**Acceptance**: the box filters the cards."},
        ],
        "edges": [{"prerequisite": "c1", "dependent": "c2"}],
        "note": "Two cards; the box needs the query.",
    })
}

/// How many tasks name the request `RQ-<number>`.
async fn linked_tasks(h: &Harness, number: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks t JOIN requests r ON r.id = t.request_id WHERE r.number = ?",
    )
    .bind(number)
    .fetch_one(h.handle.store.read())
    .await
    .unwrap()
}

/// Acceptance 10, a human's request: a human creates a request with an attachment through HTTP, an orchestrator claims it from the push, asks in the request thread, drafts two cards with one edge, revises after the human asks for changes, and the human's approval of v2 creates two linked tasks whose columns fold calculates.
#[tokio::test]
async fn acceptance_10_a_human_request_becomes_two_tasks_through_a_drafted_and_revised_plan() {
    let h = Harness::start().await;
    let board = h.create_task("backend", "Board layout").await;
    assert_eq!(board, "T-1");
    let mut frontend = h.register_inbox("frontend").await;

    let screenshot: &[u8] = b"\x89PNG\r\n\x1a\nboard";
    let message = "I cannot find cards on a big board.\nThe board of #T-1 needs a search.";
    let (status, created) = h
        .create_request_form(
            &[
                ("body", None, None, message.as_bytes()),
                ("files", Some("board.png"), Some("image/png"), screenshot),
            ],
            Some(HUMAN),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["request"]["id"], "RQ-1");
    assert_eq!(created["request"]["kind"], Value::Null);
    assert_eq!(created["request"]["title"], Value::Null);
    assert_eq!(created["request"]["requester"], "human:ana");
    assert_eq!(
        frontend.next().await,
        wire(TOKEN, "fold: RQ-1 created. Call claim_request.")
    );

    let read = h
        .call("frontend", "get_request", json!({"id": "RQ-1"}))
        .await;
    assert_eq!(read["body"], message);
    assert_eq!(
        read["attachments"],
        json!([{"position": 1, "name": "board.png", "media_type": "image/png", "size": screenshot.len()}])
    );
    let claimed = h
        .call(
            "frontend",
            "claim_request",
            json!({"id": "RQ-1", "kind": "feature", "title": "Search the board"}),
        )
        .await;
    assert_eq!(
        claimed,
        json!({"id": "RQ-1", "state": "drafting", "kind": "feature", "title": "Search the board", "drafter": "frontend"})
    );

    let question = h
        .send_with(
            "frontend",
            json!({"to": ["human:ana"], "body": "Search by title only, or also by description?", "request": "RQ-1"}),
        )
        .await;
    let (status, answer) = h
        .http(
            Method::POST,
            &format!("{BASE}/feedback"),
            Some(&json!({"body": "By title is enough.", "request": "RQ-1"})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{answer}");
    let answer = answer["id"].as_str().unwrap().to_owned();
    assert_eq!(frontend.next().await, wire(TOKEN, &message_notice(&answer)));
    let thread = h
        .call("frontend", "get_thread", json!({"request": "RQ-1"}))
        .await;
    let messages = thread["messages"].as_array().unwrap();
    assert_eq!(
        messages
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [question.as_str(), answer.as_str()]
    );
    assert_eq!(messages[1]["body"], "By title is enough.");

    let v1 = h
        .call(
            "frontend",
            "submit_request_draft",
            search_draft("RQ-1", "Add the board search query"),
        )
        .await;
    assert_eq!(
        v1,
        json!({"id": "RQ-1", "state": "draft_ready", "version": 1})
    );
    let (status, changed) = h
        .http(
            Method::POST,
            &format!("{BASE}/requests/RQ-1/changes"),
            Some(&json!({"version": 1, "comment": "Name the query card after titles."})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["request"]["state"], "changes_requested");
    assert_eq!(
        frontend.next().await,
        wire(TOKEN, "fold: RQ-1 changes requested. Call get_request.")
    );
    let read = h
        .call("frontend", "get_request", json!({"id": "RQ-1"}))
        .await;
    assert_eq!(
        read["comments"],
        json!([{"version": 1, "author": "human:ana", "body": "Name the query card after titles."}])
    );

    let v2 = h
        .call(
            "frontend",
            "submit_request_draft",
            search_draft("RQ-1", "Add the title search query"),
        )
        .await;
    assert_eq!(
        v2,
        json!({"id": "RQ-1", "state": "draft_ready", "version": 2})
    );
    let (status, approved) = h
        .http(
            Method::POST,
            &format!("{BASE}/requests/RQ-1/approve"),
            Some(&json!({"expected_version": 2})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["request"]["state"], "approved");
    assert_eq!(approved["tasks"], json!(["T-2", "T-3"]));
    assert_eq!(
        frontend.next().await,
        wire(
            TOKEN,
            "fold: RQ-1 approved as T-2, T-3. Call get_ready_tasks."
        )
    );

    assert_eq!(linked_tasks(&h, 1).await, 2);
    let query = h.call("frontend", "get_task", json!({"id": "T-2"})).await;
    assert_eq!(query["title"], "Add the title search query");
    assert_eq!(query["status"], "todo");
    let search_box = h.call("frontend", "get_task", json!({"id": "T-3"})).await;
    assert_eq!(search_box["title"], "Show the board search box");
    assert_eq!(search_box["status"], "backlog");
    assert_eq!(search_box["needs"][0]["id"], "T-2");
    assert_eq!(search_box["needs"].as_array().unwrap().len(), 1);
    let ready: Vec<String> = h.call("frontend", "get_ready_tasks", json!({})).await["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["id"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        ready.iter().any(|id| id == "T-2") && !ready.iter().any(|id| id == "T-3"),
        "{ready:?}"
    );

    let events = h.events().await;
    let approvals: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "request.approved")
        .collect();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].data["request"], "RQ-1");
    assert_eq!(approvals[0].data["version"], 2);
    assert_eq!(approvals[0].data["tasks"], json!(["T-2", "T-3"]));
    assert_eq!(approvals[0].actor, "human:ana");
}

/// Acceptance 10, an agent's request: `backend` asks for a bug fix, `review` claims and drafts it, a human who did not create the request cannot approve it, and `backend` approves it.
#[tokio::test]
async fn acceptance_10_an_agent_request_is_approved_only_by_its_requester() {
    let h = Harness::start().await;
    let created = h
        .call(
            "backend",
            "create_request",
            json!({"body": "Saving a task with an empty title gives a 500 error."}),
        )
        .await;
    assert_eq!(
        created,
        json!({"id": "RQ-1", "state": "open", "attachments": 0})
    );

    h.call(
        "review",
        "claim_request",
        json!({"id": "RQ-1", "kind": "bug", "title": "Empty task title fails"}),
    )
    .await;
    let drafted = h
        .call(
            "review",
            "submit_request_draft",
            json!({
                "id": "RQ-1",
                "cards": [{"key": "fix", "title": "Refuse an empty task title", "description": "Return invalid_task_title.\n\n**Acceptance**: a test saves an empty title and gets 400."}],
            }),
        )
        .await;
    assert_eq!(drafted["version"], 1);

    let (status, refused) = h
        .http(
            Method::POST,
            &format!("{BASE}/requests/RQ-1/approve"),
            Some(&json!({"expected_version": 1})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["error"]["code"], "requester_required");
    let refused = h
        .refused(
            "review",
            "approve_request_draft",
            json!({"id": "RQ-1", "expected_version": 1}),
        )
        .await;
    assert_eq!(refused["code"], "requester_required");

    let approved = h
        .call(
            "backend",
            "approve_request_draft",
            json!({"id": "RQ-1", "expected_version": 1}),
        )
        .await;
    assert_eq!(
        approved,
        json!({"id": "RQ-1", "state": "approved", "tasks": ["T-1"]})
    );
    let read = h
        .call("backend", "get_request", json!({"id": "RQ-1"}))
        .await;
    assert_eq!(read["kind"], "bug");
    assert_eq!(read["requester"], "backend");
    assert_eq!(read["drafter"], "review");
    assert_eq!(read["tasks"], json!(["T-1"]));
    assert_eq!(linked_tasks(&h, 1).await, 1);
    let task = h.call("backend", "get_task", json!({"id": "T-1"})).await;
    assert_eq!(task["status"], "todo");
}
