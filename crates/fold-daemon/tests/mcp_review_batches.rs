#![cfg(test)]

use crate::common::roles::give_role;
use crate::proposal_tools_support::{Fixture, t};
use fold_core::roles::Role;
use fold_daemon::mcp::FoldServer;
use fold_daemon::services::events::EventService;
use fold_daemon::services::messages::MessageService;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

type Client = RunningService<RoleClient, ()>;

/// An adapter session over an in-memory stream, registered as `name`.
async fn connect(fx: &Fixture, name: &str) -> Client {
    let handle = &fx.handle;
    let agents = fx.agents();
    let session = agents.start_session(7).await.unwrap();
    let server = FoldServer::new(
        agents,
        EventService::new(handle.store.clone(), handle.bus.clone()),
        fx.workspaces(),
        handle.tasks.clone(),
        MessageService::new(
            handle.store.clone(),
            fx.state.registry.clone(),
            handle.bus.clone(),
            handle.clock.clone(),
        ),
        session,
    )
    .with_reviews(fx.reviews());
    let (client_end, server_end) = tokio::io::duplex(256 * 1024);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_end).await {
            let _ = running.waiting().await;
        }
    });
    let client = ().serve(client_end).await.unwrap();
    ok(&client, "register_agent", json!({"name": name})).await;
    client
}

async fn call(client: &Client, tool: &str, arguments: Value) -> CallToolResult {
    let Value::Object(arguments) = arguments else {
        panic!("arguments must be an object");
    };
    client
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
        .await
        .unwrap()
}

fn body(result: &CallToolResult) -> Value {
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

async fn ok(client: &Client, tool: &str, arguments: Value) -> Value {
    let result = call(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(false), "{tool} failed: {result:?}");
    body(&result)
}

/// The `error` object of a failed result, which names the next step.
async fn err(client: &Client, tool: &str, arguments: Value) -> Value {
    let result = call(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(true), "{tool} succeeded: {result:?}");
    let error = body(&result)["error"].clone();
    assert!(
        error["hint"].as_str().is_some_and(|hint| !hint.is_empty()),
        "{tool}: {error}"
    );
    error
}

/// `frontend` adds `src/one.rs` as P-1 and `backend` adds `src/two.rs` as P-2.
async fn two_proposals(fx: &Fixture) {
    for (task, name, path) in [(1, "frontend", "src/one.rs"), (2, "backend", "src/two.rs")] {
        let author = fx.agent(name).await;
        fx.workspaces()
            .create_workspace(&author, t(task))
            .await
            .unwrap();
        fx.write(task, path, "first\nsecond\n");
        fx.reviews()
            .create_proposal(&author, t(task), "Add a file", None)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_reviewer_batches_checks_out_and_reviews_two_proposals_through_the_tools() {
    let fx = Fixture::new().await;
    two_proposals(&fx).await;
    let review = connect(&fx, "review").await;
    give_role(&fx.handle.store, "review", Role::ReviewLead, None).await;

    let created = ok(
        &review,
        "create_review_batch",
        json!({"proposals": ["P-2", "P-1"]}),
    )
    .await;
    assert_eq!(created["id"], "rb-1");
    assert_eq!(created["holder"], "review");
    assert_eq!(created["state"], "open");
    assert_eq!(created["members"][0]["proposal"], "P-1");
    assert_eq!(created["members"][1]["proposal"], "P-2");
    assert_eq!(created["members"][0]["version"], "v1");
    assert_eq!(created["checkout"], Value::Null);

    let checkout = ok(&review, "create_batch_checkout", json!({"id": "rb-1"})).await;
    assert_eq!(checkout["path"], ".fold/ws/review-rb-1");
    assert_eq!(
        checkout["members"],
        json!([{"proposal": "P-1", "version": "v1"}, {"proposal": "P-2", "version": "v1"}])
    );
    let read = ok(&review, "get_review_batch", json!({"id": "rb-1"})).await;
    assert_eq!(read["checkout"]["path"], ".fold/ws/review-rb-1");
    let listed = ok(&review, "list_review_batches", json!({"open": true})).await;
    assert_eq!(listed["batches"].as_array().unwrap().len(), 1);

    let submitted = ok(
        &review,
        "submit_batch_review",
        json!({
            "id": "rb-1",
            "verdicts": [
                {"proposal": "P-1", "verdict": "approve"},
                {"proposal": "P-2", "verdict": "request_changes", "body": "split it"}
            ],
            "comments": [
                {"path": "src/two.rs", "start_line": 2, "end_line": 2, "body": "drop this"},
                {"members": ["P-1", "P-2"], "body": "both name the same thing"}
            ]
        }),
    )
    .await;
    assert_eq!(submitted["id"], "rb-1");
    assert_eq!(submitted["reviewed"][0]["verdict"], "approve");
    assert_eq!(submitted["reviewed"][1]["verdict"], "request_changes");
    assert_eq!(
        submitted["reviewed"][1]["comments"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(submitted["left_out"], json!([]));
    let comments = ok(&review, "get_review_comments", json!({"id": "P-2"})).await;
    assert_eq!(comments["comments"][0]["path"], "src/two.rs");
    assert_eq!(comments["comments"][0]["start_line"], 2);
    let listed = ok(&review, "list_review_batches", json!({})).await;
    assert_eq!(listed["batches"][0]["state"], "submitted");
    assert_eq!(
        listed["batches"][0]["verdicts"].as_array().unwrap().len(),
        2
    );

    fx.write(2, "src/two.rs", "first\n");
    let backend = fx.agent("backend").await;
    fx.reviews()
        .update_proposal(
            &backend,
            fold_core::ProposalId::from_number(2).unwrap(),
            None,
        )
        .await
        .unwrap();
    let read = ok(&review, "get_review_batch", json!({"id": "rb-1"})).await;
    assert_eq!(read["members"][0]["since"], Value::Null);
    assert_eq!(
        read["members"][1]["since"],
        json!({"version": "v1", "files": 1, "added": 0, "removed": 1})
    );
}

#[tokio::test]
async fn the_batch_tools_explain_their_refusals() {
    let fx = Fixture::new().await;
    two_proposals(&fx).await;
    let review = connect(&fx, "review").await;

    let id = err(&review, "get_review_batch", json!({"id": "P-1"})).await;
    assert_eq!(id["code"], "invalid_id");
    let missing = err(&review, "get_review_batch", json!({"id": "rb-4"})).await;
    assert_eq!(missing["code"], "review_batch_not_found");
    ok(
        &review,
        "create_review_batch",
        json!({"proposals": ["P-1", "P-2"]}),
    )
    .await;
    let verdict = err(
        &review,
        "submit_batch_review",
        json!({"id": "rb-1", "verdicts": [{"proposal": "P-1", "verdict": "maybe"}]}),
    )
    .await;
    assert_eq!(verdict["code"], "invalid_request");
    let lines = err(
        &review,
        "submit_batch_review",
        json!({"id": "rb-1", "verdicts": [], "comments": [{"path": "src/one.rs", "body": "x"}]}),
    )
    .await;
    assert_eq!(lines["code"], "invalid_request");
    let removed = ok(
        &review,
        "remove_from_batch",
        json!({"id": "rb-1", "proposal": "P-2"}),
    )
    .await;
    assert_eq!(removed["members"][1]["in_batch"], false);
    let gone = err(
        &review,
        "remove_from_batch",
        json!({"id": "rb-1", "proposal": "P-2"}),
    )
    .await;
    assert_eq!(gone["code"], "not_batch_member");
}

#[tokio::test]
async fn every_batch_tool_is_listed_with_described_fields() {
    let fx = Fixture::new().await;
    let review = connect(&fx, "review").await;
    let tools = review.list_all_tools().await.unwrap();
    for name in [
        "create_review_batch",
        "list_review_batches",
        "get_review_batch",
        "remove_from_batch",
        "create_batch_checkout",
        "submit_batch_review",
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name} is not listed"));
        assert!(
            tool.description
                .as_ref()
                .is_some_and(|text| !text.is_empty())
        );
        let schema = serde_json::to_value(tool.input_schema.as_ref()).unwrap();
        let mut objects = vec![schema.clone()];
        if let Some(defs) = schema.get("$defs").and_then(Value::as_object) {
            objects.extend(defs.values().cloned());
        }
        for object in objects {
            for (field, property) in object["properties"].as_object().unwrap() {
                assert!(
                    property["description"]
                        .as_str()
                        .is_some_and(|text| !text.is_empty()),
                    "{name}.{field} has no description"
                );
            }
        }
    }
}
