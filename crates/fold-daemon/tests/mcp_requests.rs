use rmcp::model::CallToolResult;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, body, footer};

/// The JSON of a successful result, which ends with the footer line.
#[cfg(test)]
async fn ok(h: &Harness, agent: &str, tool: &str, arguments: Value) -> Value {
    let result = h.call_raw(agent, tool, arguments).await;
    assert_eq!(result.is_error, Some(false), "{agent} {tool}: {result:?}");
    assert_footer(&result, tool);
    body(&result)
}

/// The `error` of a refused call, which carries a code, a message and a hint, and ends with the footer line.
#[cfg(test)]
async fn err(h: &Harness, agent: &str, tool: &str, arguments: Value) -> Value {
    let result = h.call_raw(agent, tool, arguments).await;
    assert_eq!(result.is_error, Some(true), "{agent} {tool}: {result:?}");
    assert_footer(&result, tool);
    let error = body(&result)["error"].clone();
    assert!(
        error["message"].as_str().is_some_and(|m| !m.is_empty()),
        "{error}"
    );
    assert!(
        error["hint"].as_str().is_some_and(|h| !h.is_empty()),
        "{tool}: {error}"
    );
    error
}

#[cfg(test)]
fn assert_footer(result: &CallToolResult, tool: &str) {
    let line = footer(result).unwrap_or_else(|| panic!("{tool} has no footer"));
    assert!(line.starts_with("fold: "), "{tool}: {line}");
    assert!(line.contains("unread"), "{tool}: {line}");
}

/// The harness with one unacknowledged message in each agent's inbox, so every result carries the footer.
#[cfg(test)]
async fn harness() -> Harness {
    let h = Harness::start().await;
    h.send("review", &["frontend", "backend"], "hello").await;
    h.send("frontend", &["review"], "hello").await;
    h
}

#[cfg(test)]
fn cards() -> Value {
    json!([
        {"key": "c1", "title": "Index the board query", "description": "Add the index.\n\n**Acceptance**\n- fast"},
        {"key": "c2", "title": "Show the faster board", "description": "Use it in #T-1 and @web/src/board.ts."},
    ])
}

#[tokio::test]
async fn every_request_tool_succeeds_through_a_whole_request() {
    let h = harness().await;
    let task = h.create_task("frontend", "Profile the board").await;
    let work = h.start_work("frontend", &task).await;
    work.write(&[("logs/run.log", "slow query\n")]);

    let created = ok(
        &h,
        "frontend",
        "create_request",
        json!({
            "body": "The board is slow\nSee #T-1 and @web/src/board.ts.",
            "attachments": ["logs/run.log"],
            "ws": work.workspace,
        }),
    )
    .await;
    assert_eq!(
        created,
        json!({"id": "RQ-1", "state": "open", "attachments": 1})
    );

    let listed = ok(&h, "backend", "list_requests", json!({"state": "open"})).await;
    assert_eq!(
        listed["requests"],
        json!([{
            "id": "RQ-1",
            "state": "open",
            "kind": null,
            "title": null,
            "excerpt": "The board is slow",
            "requester": "frontend",
            "drafter": null,
            "latest_version": null,
        }])
    );
    let mine = ok(&h, "backend", "list_requests", json!({"mine": true})).await;
    assert_eq!(mine["requests"], json!([]));

    let claimed = ok(
        &h,
        "backend",
        "claim_request",
        json!({"id": "RQ-1", "kind": "bug", "title": "Board is slow"}),
    )
    .await;
    assert_eq!(
        claimed,
        json!({"id": "RQ-1", "state": "drafting", "kind": "bug", "title": "Board is slow", "drafter": "backend"})
    );

    let asked = ok(
        &h,
        "backend",
        "send_message",
        json!({"to": ["frontend"], "body": "Which board?", "request": "RQ-1"}),
    )
    .await;
    let answered = ok(
        &h,
        "review",
        "send_message",
        json!({"to": ["backend"], "body": "The main one.", "request": "RQ-1"}),
    )
    .await;
    assert_eq!(answered["thread"], asked["id"]);
    let thread = ok(&h, "frontend", "get_thread", json!({"request": "RQ-1"})).await;
    let messages = thread["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert!(messages.iter().all(|message| message["request"] == "RQ-1"));
    let by_id = ok(&h, "frontend", "get_thread", json!({"thread": asked["id"]})).await;
    assert_eq!(by_id, thread);

    let submitted = ok(
        &h,
        "backend",
        "submit_request_draft",
        json!({
            "id": "RQ-1",
            "cards": cards(),
            "edges": [{"prerequisite": "c1", "dependent": "c2"}],
            "note": "Two steps.",
        }),
    )
    .await;
    assert_eq!(
        submitted,
        json!({"id": "RQ-1", "state": "draft_ready", "version": 1})
    );

    let changes = ok(
        &h,
        "frontend",
        "request_draft_changes",
        json!({"id": "RQ-1", "version": 1, "comment": "Split c1."}),
    )
    .await;
    assert_eq!(
        changes,
        json!({"id": "RQ-1", "state": "changes_requested", "version": 1})
    );

    let released = ok(&h, "backend", "release_request", json!({"id": "RQ-1"})).await;
    assert_eq!(
        released,
        json!({"id": "RQ-1", "state": "changes_requested", "drafter": null})
    );
    ok(
        &h,
        "backend",
        "claim_request",
        json!({"id": "RQ-1", "kind": "bug", "title": "Board is slow"}),
    )
    .await;
    let second = ok(
        &h,
        "backend",
        "submit_request_draft",
        json!({"id": "RQ-1", "cards": cards(), "kind": "feature", "title": "Faster board"}),
    )
    .await;
    assert_eq!(second["version"], 2);

    let edited = ok(
        &h,
        "frontend",
        "edit_request_draft",
        json!({"id": "RQ-1", "cards": cards(), "edges": [{"prerequisite": "c1", "dependent": "c2"}]}),
    )
    .await;
    assert_eq!(
        edited,
        json!({"id": "RQ-1", "state": "draft_ready", "version": 3})
    );

    let first = ok(
        &h,
        "review",
        "get_request",
        json!({"id": "RQ-1", "version": 1}),
    )
    .await;
    assert_eq!(first["draft"]["version"], 1);
    assert_eq!(first["draft"]["note"], "Two steps.");
    assert_eq!(first["kind"], "feature");
    assert_eq!(first["title"], "Faster board");
    assert_eq!(first["versions"], json!([1, 2, 3]));
    assert_eq!(first["thread"], asked["id"]);
    assert_eq!(
        first["attachments"],
        json!([{"position": 1, "name": "run.log", "media_type": "text/plain", "size": 11}])
    );
    assert_eq!(
        first["comments"],
        json!([{"version": 1, "author": "frontend", "body": "Split c1."}])
    );

    let approved = ok(
        &h,
        "frontend",
        "approve_request_draft",
        json!({"id": "RQ-1", "expected_version": 3}),
    )
    .await;
    assert_eq!(
        approved,
        json!({"id": "RQ-1", "state": "approved", "tasks": ["T-2", "T-3"]})
    );
    let latest = ok(&h, "review", "get_request", json!({"id": "RQ-1"})).await;
    assert_eq!(latest["draft"]["version"], 3);
    assert_eq!(latest["tasks"], json!(["T-2", "T-3"]));

    let other = ok(
        &h,
        "review",
        "create_request",
        json!({"body": "Why two boards?"}),
    )
    .await;
    assert_eq!(other["id"], "RQ-2");
    let closed = ok(
        &h,
        "review",
        "close_request",
        json!({"id": "RQ-2", "reason": "Answered in #RQ-1."}),
    )
    .await;
    assert_eq!(closed, json!({"id": "RQ-2", "state": "closed"}));
}

#[tokio::test]
async fn every_request_tool_refuses_with_a_code_and_a_hint() {
    let h = harness().await;
    let task = h.create_task("frontend", "Profile the board").await;
    let work = h.start_work("frontend", &task).await;
    work.write(&[("run.log", "x")]);

    let error = err(
        &h,
        "frontend",
        "create_request",
        json!({"body": "slow", "attachments": ["run.log"]}),
    )
    .await;
    assert_eq!(error["code"], "invalid_request");
    assert!(error["hint"].as_str().unwrap().contains("create_request"));
    let error = err(
        &h,
        "backend",
        "create_request",
        json!({"body": "slow", "attachments": ["run.log"], "ws": work.workspace}),
    )
    .await;
    assert_eq!(error["code"], "not_workspace_owner");
    let error = err(
        &h,
        "frontend",
        "create_request",
        json!({"body": "slow", "attachments": ["../secret"], "ws": work.workspace}),
    )
    .await;
    assert_eq!(error["code"], "invalid_path");
    let error = err(
        &h,
        "frontend",
        "create_request",
        json!({"body": "slow", "attachments": ["absent.log"], "ws": work.workspace}),
    )
    .await;
    assert_eq!(error["code"], "not_found");
    let error = err(
        &h,
        "frontend",
        "create_request",
        json!({"body": "slow", "attachments": vec!["absent.log"; 11], "ws": work.workspace}),
    )
    .await;
    assert_eq!(error["details"]["reason"], "too_many_attachments");

    let error = err(&h, "frontend", "list_requests", json!({"state": "nope"})).await;
    assert_eq!(error["code"], "invalid_request");
    let error = err(&h, "frontend", "get_request", json!({"id": "RQ-9"})).await;
    assert_eq!(error["code"], "request_not_found");
    assert!(error["hint"].as_str().unwrap().contains("list_requests"));

    ok(&h, "frontend", "create_request", json!({"body": "slow"})).await;
    ok(&h, "frontend", "register_worker", json!({"name": "ui"})).await;
    let error = err(
        &h,
        "frontend",
        "claim_request",
        json!({"id": "RQ-1", "kind": "bug", "title": "Slow", "as": "frontend/ui"}),
    )
    .await;
    assert_eq!(error["code"], "orchestrator_required");
    assert!(error["hint"].as_str().unwrap().contains("claim_request"));
    let error = err(&h, "review", "release_request", json!({"id": "RQ-1"})).await;
    assert_eq!(error["code"], "drafter_required");

    ok(
        &h,
        "backend",
        "claim_request",
        json!({"id": "RQ-1", "kind": "bug", "title": "Slow"}),
    )
    .await;
    let error = err(
        &h,
        "backend",
        "submit_request_draft",
        json!({"id": "RQ-1", "cards": []}),
    )
    .await;
    assert_eq!(error["code"], "invalid_draft");
    assert_eq!(error["details"]["reason"], "empty");
    for version in [1, 2] {
        let submitted = ok(
            &h,
            "backend",
            "submit_request_draft",
            json!({"id": "RQ-1", "cards": cards()}),
        )
        .await;
        assert_eq!(submitted["version"], version);
    }
    let error = err(
        &h,
        "backend",
        "request_draft_changes",
        json!({"id": "RQ-1", "version": 2, "comment": "more"}),
    )
    .await;
    assert_eq!(error["code"], "requester_required");
    assert!(error["hint"].as_str().unwrap().contains("get_request"));
    let error = err(
        &h,
        "review",
        "edit_request_draft",
        json!({"id": "RQ-1", "cards": cards()}),
    )
    .await;
    assert_eq!(error["code"], "requester_required");
    let error = err(
        &h,
        "frontend",
        "approve_request_draft",
        json!({"id": "RQ-1", "expected_version": 1}),
    )
    .await;
    assert_eq!(error["code"], "stale_version");
    assert_eq!(error["details"], json!({"expected": 1, "latest": 2}));
    ok(
        &h,
        "frontend",
        "approve_request_draft",
        json!({"id": "RQ-1", "expected_version": 2}),
    )
    .await;
    let error = err(
        &h,
        "frontend",
        "close_request",
        json!({"id": "RQ-1", "reason": "done"}),
    )
    .await;
    assert_eq!(error["code"], "request_not_open");
    assert!(error["hint"].as_str().unwrap().contains("create_request"));
    let error = err(
        &h,
        "review",
        "get_request",
        json!({"id": "RQ-1", "version": 7}),
    )
    .await;
    assert_eq!(error["code"], "not_found");
}

#[tokio::test]
async fn a_clarification_joins_the_request_thread_and_a_reply_keeps_it() {
    let h = harness().await;
    ok(&h, "frontend", "create_request", json!({"body": "slow"})).await;
    let empty = ok(&h, "backend", "get_thread", json!({"request": "RQ-1"})).await;
    assert_eq!(empty, json!({"messages": []}));

    let first = h
        .send_with(
            "backend",
            json!({"to": ["frontend"], "body": "Which board?", "request": "RQ-1"}),
        )
        .await;
    let reply = ok(
        &h,
        "frontend",
        "send_message",
        json!({"to": ["backend"], "body": "Main.", "reply_to": first}),
    )
    .await;
    assert_eq!(reply["thread"], first);
    let inbox = ok(&h, "backend", "get_inbox", json!({})).await;
    let replied = inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == reply["id"])
        .unwrap();
    assert_eq!(replied["request"], "RQ-1");

    let unrelated = h.send("review", &["frontend"], "other").await;
    let error = err(
        &h,
        "frontend",
        "send_message",
        json!({"to": ["review"], "body": "x", "request": "RQ-1", "reply_to": unrelated}),
    )
    .await;
    assert_eq!(error["code"], "invalid_request");
    assert_eq!(error["details"]["reason"], "request_thread");
    let error = err(
        &h,
        "frontend",
        "send_message",
        json!({"to": ["review"], "body": "x", "request": "RQ-7"}),
    )
    .await;
    assert_eq!(error["code"], "request_not_found");
    let error = err(
        &h,
        "frontend",
        "get_thread",
        json!({"thread": first, "request": "RQ-1"}),
    )
    .await;
    assert_eq!(error["code"], "invalid_request");
    let error = err(&h, "frontend", "get_thread", json!({"request": "RQ-7"})).await;
    assert_eq!(error["code"], "request_not_found");

    let thread = ok(&h, "review", "get_thread", json!({"request": "RQ-1"})).await;
    assert_eq!(thread["messages"].as_array().unwrap().len(), 2);
    let detail = ok(&h, "review", "get_request", json!({"id": "RQ-1"})).await;
    assert_eq!(detail["thread"], first);
}

#[test]
fn every_input_field_has_a_description() {
    let mut tools = fold_daemon::mcp::tasks::requests::tools();
    assert_eq!(tools.len(), 10);
    tools.extend(fold_daemon::mcp::messages::tools());
    for tool in &tools {
        assert!(tool.description.is_some(), "{}", tool.name);
        let properties = tool.input_schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{} has no properties", tool.name));
        assert!(properties.contains_key("as"), "{}", tool.name);
        for (field, schema) in properties {
            let described = schema["description"]
                .as_str()
                .is_some_and(|text| !text.trim().is_empty());
            assert!(described, "{}.{field} has no description", tool.name);
        }
    }
    let send = tools
        .iter()
        .find(|tool| tool.name == "send_message")
        .unwrap();
    let marks = send.input_schema["properties"]["body"]["description"]
        .as_str()
        .unwrap();
    assert!(marks.contains("`#` names a fold item") && marks.contains("`@` names a file"));
    assert!(send.input_schema["properties"].get("request").is_some());
}
