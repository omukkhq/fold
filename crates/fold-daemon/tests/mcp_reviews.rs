#![cfg(test)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::proposal_tools_support::{Fixture, human};
use fold_core::AgentName;
use fold_daemon::mcp::{FoldServer, FooterPart, FooterSource};
use fold_daemon::services::agents::SessionContext;
use fold_daemon::services::events::EventService;
use fold_daemon::services::messages::MessageService;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

/// The fixed count always puts the unread part on the footer line; real counts can add to it.
const FOOTER: &str = "fold: ";

const REVIEW_TOOLS: [&str; 12] = [
    "create_proposal",
    "list_proposals",
    "get_proposal",
    "update_proposal",
    "close_proposal",
    "claim_review",
    "release_review",
    "create_review_checkout",
    "submit_review",
    "get_review_comments",
    "integrate_proposal",
    "require_human_review",
];

#[cfg(test)]
type Client = RunningService<RoleClient, ()>;

/// A footer source that always counts one, so every result must end with the footer line.
#[cfg(test)]
struct One;

impl FooterSource for One {
    fn part(&self) -> FooterPart {
        FooterPart::Unread
    }

    fn count<'a>(
        &'a self,
        _session: &'a SessionContext,
    ) -> Pin<Box<dyn Future<Output = u64> + Send + 'a>> {
        Box::pin(async { 1 })
    }
}

/// An adapter session built from the project handle as the handshake builds it, served over an in-memory stream and registered as `name`.
#[cfg(test)]
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
    .with_reviews(fx.reviews())
    .with_footer_source(Arc::new(One));
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

#[cfg(test)]
async fn call_raw(client: &Client, tool: &str, arguments: Value) -> CallToolResult {
    let Value::Object(arguments) = arguments else {
        panic!("arguments must be an object");
    };
    client
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
        .await
        .unwrap()
}

#[cfg(test)]
fn body(result: &CallToolResult) -> Value {
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[cfg(test)]
fn assert_footer(result: &CallToolResult, tool: &str) {
    assert_eq!(result.content.len(), 2, "{tool}: {result:?}");
    let line = &result.content[1].as_text().unwrap().text;
    assert!(
        line.starts_with(FOOTER) && line.contains(" unread"),
        "{tool}: {line}"
    );
}

/// The JSON of a successful result, which must end with the footer line.
#[cfg(test)]
async fn ok(client: &Client, tool: &str, arguments: Value) -> Value {
    let result = call_raw(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(false), "{tool} failed: {result:?}");
    assert_footer(&result, tool);
    body(&result)
}

/// The `error` object of a failed result, which also ends with the footer line and names the next step.
#[cfg(test)]
async fn err(client: &Client, tool: &str, arguments: Value) -> Value {
    let result = call_raw(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(true), "{tool} succeeded: {result:?}");
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

/// `frontend` creates the workspace of T-1 through MCP, changes `src/lib.rs` and opens P-1.
#[cfg(test)]
async fn open_p1(fx: &Fixture, frontend: &Client) -> Value {
    ok(frontend, "create_workspace", json!({"task": "T-1"})).await;
    fx.write(1, "src/lib.rs", "pub fn y() {}\n");
    ok(
        frontend,
        "create_proposal",
        json!({"task": "T-1", "title": "Rename x", "evidence": "tests pass"}),
    )
    .await
}

/// Gives an existing agent the `integrator` tag, as a human does.
#[cfg(test)]
async fn grant_integrator(fx: &Fixture, name: &str) {
    fx.agents()
        .set_integrator(&human("tanim"), &AgentName::parse(name).unwrap(), true)
        .await
        .unwrap();
}

/// `review` carries `integrator`; its client reads its tags on every call.
#[cfg(test)]
async fn integrator(fx: &Fixture) -> Client {
    let review = connect(fx, "review").await;
    grant_integrator(fx, "review").await;
    review
}

#[tokio::test]
async fn create_proposal_opens_v1_and_moves_the_task_to_review() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let created = open_p1(&fx, &frontend).await;
    assert_eq!(
        created,
        json!({"id": "P-1", "version": "v1", "state": "open", "task": "T-1", "status": "review"})
    );
    let error = err(
        &frontend,
        "create_proposal",
        json!({"task": "T-2", "title": "Export"}),
    )
    .await;
    assert_eq!(error["code"], "not_claim_holder");
}

#[tokio::test]
async fn list_proposals_shows_turn_claim_and_mark() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = connect(&fx, "review").await;
    open_p1(&fx, &frontend).await;
    ok(&review, "claim_review", json!({"id": "P-1"})).await;
    ok(
        &review,
        "require_human_review",
        json!({"task": "T-1", "reason": "touches the public API"}),
    )
    .await;
    let listed = ok(&review, "list_proposals", json!({"state": "open"})).await;
    assert_eq!(
        listed,
        json!({"proposals": [{
            "id": "P-1",
            "title": "Rename x",
            "task": "T-1",
            "author": "frontend",
            "state": "open",
            "version": "v1",
            "turn": "with_reviewer",
            "claim": "review",
            "human_review": {"reviewer": null, "reason": "touches the public API", "set_by": "review"},
        }]})
    );
    let unclaimed = ok(&review, "list_proposals", json!({"unclaimed": true})).await;
    assert_eq!(unclaimed, json!({"proposals": []}));
    let error = err(&review, "list_proposals", json!({"state": "merged"})).await;
    assert_eq!(error["code"], "invalid_request");
}

#[tokio::test]
async fn get_proposal_shows_versions_gates_and_files() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = integrator(&fx).await;
    open_p1(&fx, &frontend).await;
    let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(view["id"], "P-1");
    assert_eq!(view["state"], "open");
    assert_eq!(view["turn"], "with_reviewer");
    assert_eq!(view["version"], "v1");
    assert_eq!(view["current_version"], "v1");
    assert_eq!(view["base"], "r1");
    assert_eq!(view["evidence"], "tests pass");
    assert_eq!(view["versions"].as_array().unwrap().len(), 1);
    assert_eq!(
        view["task"],
        json!({"id": "T-1", "title": "report", "status": "review", "description": "what to build"})
    );
    assert_eq!(view["needs"], json!([]));
    assert_eq!(view["claim"], Value::Null);
    assert_eq!(view["human_review"], Value::Null);
    assert_eq!(view["conflict"], Value::Null);
    assert_eq!(
        view["files"],
        json!([{"path": "src/lib.rs", "change": "modified", "added": 1, "removed": 1}])
    );
    assert_eq!(
        view["totals"],
        json!({"files": 1, "added": 1, "removed": 1})
    );
    assert_eq!(
        view["gates"],
        json!([
            {"gate": "open", "status": "met"},
            {"gate": "version", "status": "met"},
            {"gate": "caller", "status": "met"},
            {"gate": "approval", "status": "unmet", "code": "not_approved"},
            {"gate": "human_review", "status": "met"},
            {"gate": "prerequisites", "status": "met"},
            {"gate": "merge", "status": "checked_at_integration"},
        ])
    );
    let own = ok(&frontend, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(
        own["gates"][2],
        json!({"gate": "caller", "status": "unmet", "code": "author_cannot_integrate"})
    );
    ok(
        &review,
        "submit_review",
        json!({"id": "P-1", "verdict": "approve", "body": "good", "comments": [
            {"path": "src/lib.rs", "start_line": 1, "end_line": 1, "side": "new", "body": "nice"},
        ]}),
    )
    .await;
    let approved = ok(&review, "get_proposal", json!({"id": "P-1", "version": 1})).await;
    assert_eq!(
        approved["gates"][3],
        json!({"gate": "approval", "status": "met"})
    );
    assert_eq!(
        approved["reviews"],
        json!([{"version": "v1", "verdict": "approve", "reviewer": "review", "body": "good", "comments": 1}])
    );
    assert_eq!(
        approved["comments"],
        json!({"total": 1, "on_version": 1, "outdated": 0})
    );
    let error = err(&review, "get_proposal", json!({"id": "P-9"})).await;
    assert_eq!(error["code"], "proposal_not_found");
    let error = err(&review, "get_proposal", json!({"id": "nine"})).await;
    assert_eq!(error["code"], "invalid_id");
}

#[tokio::test]
async fn update_proposal_adds_the_next_version() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let backend = connect(&fx, "backend").await;
    open_p1(&fx, &frontend).await;
    fx.write(1, "src/lib.rs", "pub fn z() {}\n");
    let updated = ok(
        &frontend,
        "update_proposal",
        json!({"id": "P-1", "evidence": "all tests pass"}),
    )
    .await;
    assert_eq!(
        updated,
        json!({"id": "P-1", "version": "v2", "state": "open", "task": "T-1", "status": "review"})
    );
    let error = err(&backend, "update_proposal", json!({"id": "P-1"})).await;
    assert_eq!(error["code"], "not_proposal_author");
}

#[tokio::test]
async fn close_proposal_closes_it() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    open_p1(&fx, &frontend).await;
    let error = err(
        &frontend,
        "close_proposal",
        json!({"id": "P-1", "reason": " "}),
    )
    .await;
    assert_eq!(error["code"], "invalid_request");
    let closed = ok(
        &frontend,
        "close_proposal",
        json!({"id": "P-1", "reason": "wrong approach"}),
    )
    .await;
    assert_eq!(closed["state"], "closed");
    assert_eq!(closed["status"], "in_progress");
}

#[tokio::test]
async fn claim_review_names_the_holder_and_release_review_gives_it_up() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = connect(&fx, "review").await;
    open_p1(&fx, &frontend).await;
    let error = err(&frontend, "claim_review", json!({"id": "P-1"})).await;
    assert_eq!(error["code"], "author_cannot_review");
    let error = err(&review, "release_review", json!({"id": "P-1"})).await;
    assert_eq!(error["code"], "claim_not_found");
    let claim = ok(&review, "claim_review", json!({"id": "P-1"})).await;
    assert_eq!(claim["id"], "P-1");
    assert_eq!(claim["holder"], "review");
    assert_eq!(claim["stale"], false);
    let released = ok(
        &review,
        "release_review",
        json!({"id": "P-1", "reason": "out of time"}),
    )
    .await;
    assert_eq!(released, json!({"id": "P-1", "released": true}));
    let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(view["claim"], Value::Null);
}

#[tokio::test]
async fn create_review_checkout_gives_the_path_and_version() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = connect(&fx, "review").await;
    open_p1(&fx, &frontend).await;
    let error = err(&review, "create_review_checkout", json!({"id": "P-1"})).await;
    assert_eq!(error["code"], "review_claim_required");
    ok(&review, "claim_review", json!({"id": "P-1"})).await;
    let checkout = ok(&review, "create_review_checkout", json!({"id": "P-1"})).await;
    assert_eq!(checkout["version"], "v1");
    assert_eq!(checkout["path"], "ws/review-p-1");
    assert!(checkout["workspace"].as_str().unwrap().starts_with("ws-"));
    let copied = fx.project.paths().root().join("ws/review-p-1/src/lib.rs");
    assert_eq!(std::fs::read_to_string(copied).unwrap(), "pub fn y() {}\n");
}

#[tokio::test]
async fn submit_review_returns_the_verdict_and_comment_ids() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let backend = connect(&fx, "backend").await;
    open_p1(&fx, &frontend).await;
    let error = err(
        &frontend,
        "submit_review",
        json!({"id": "P-1", "verdict": "comment", "body": "mine"}),
    )
    .await;
    assert_eq!(error["code"], "author_cannot_review");
    let error = err(
        &backend,
        "submit_review",
        json!({"id": "P-1", "verdict": "maybe", "body": "?"}),
    )
    .await;
    assert_eq!(error["code"], "invalid_request");
    let error = err(
        &backend,
        "submit_review",
        json!({"id": "P-1", "verdict": "comment", "body": "", "comments": [
            {"path": "src/lib.rs", "start_line": 1, "body": "no side"},
        ]}),
    )
    .await;
    assert_eq!(error["code"], "invalid_request");
    let error = err(
        &backend,
        "submit_review",
        json!({"id": "P-1", "verdict": "comment", "body": "", "comments": [
            {"path": "src/lib.rs", "start_line": 4, "end_line": 9, "side": "new", "body": "far"},
        ]}),
    )
    .await;
    assert_eq!(error["code"], "invalid_comment_range");
    let submitted = ok(
        &backend,
        "submit_review",
        json!({"id": "P-1", "verdict": "request_changes", "body": "rename back", "comments": [
            {"path": "src/lib.rs", "start_line": 1, "end_line": 1, "side": "old", "body": "keep x"},
            {"body": "and add a test"},
        ]}),
    )
    .await;
    assert_eq!(
        submitted,
        json!({
            "id": "P-1",
            "version": "v1",
            "verdict": "request_changes",
            "comments": ["rc-1", "rc-2"],
            "state": "open",
            "turn": "with_author",
        })
    );
}

#[tokio::test]
async fn get_review_comments_marks_outdated_comments() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let backend = connect(&fx, "backend").await;
    open_p1(&fx, &frontend).await;
    ok(
        &backend,
        "submit_review",
        json!({"id": "P-1", "verdict": "comment", "body": "", "comments": [
            {"path": "src/lib.rs", "start_line": 1, "end_line": 1, "side": "new", "body": "why y"},
        ]}),
    )
    .await;
    let comments = ok(&frontend, "get_review_comments", json!({"id": "P-1"})).await;
    assert_eq!(
        comments,
        json!({"comments": [{
            "id": "rc-1",
            "version": "v1",
            "author": "backend",
            "verdict": "comment",
            "body": "why y",
            "outdated": false,
            "path": "src/lib.rs",
            "side": "new",
            "start_line": 1,
            "end_line": 1,
        }]})
    );
    fx.write(1, "src/lib.rs", "pub fn z() {}\n");
    ok(&frontend, "update_proposal", json!({"id": "P-1"})).await;
    let outdated = ok(
        &frontend,
        "get_review_comments",
        json!({"id": "P-1", "version": 1}),
    )
    .await;
    assert_eq!(outdated["comments"][0]["outdated"], true);
    let error = err(
        &frontend,
        "get_review_comments",
        json!({"id": "P-1", "version": 9}),
    )
    .await;
    assert_eq!(error["code"], "proposal_not_found");
}

#[tokio::test]
async fn integrate_proposal_publishes_the_next_revision() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = integrator(&fx).await;
    open_p1(&fx, &frontend).await;
    let error = err(&review, "integrate_proposal", json!({"id": "P-1"})).await;
    assert_eq!(error["code"], "not_approved");
    ok(
        &review,
        "submit_review",
        json!({"id": "P-1", "verdict": "approve", "body": "good"}),
    )
    .await;
    grant_integrator(&fx, "frontend").await;
    let error = err(&frontend, "integrate_proposal", json!({"id": "P-1"})).await;
    assert_eq!(error["code"], "author_cannot_integrate");
    let error = err(
        &review,
        "integrate_proposal",
        json!({"id": "P-1", "expected_version": 2}),
    )
    .await;
    assert_eq!(error["code"], "stale_version");
    assert_eq!(
        error["details"],
        json!({"expected_version": 2, "current_version": 1})
    );
    let integrated = ok(
        &review,
        "integrate_proposal",
        json!({"id": "P-1", "expected_version": 1}),
    )
    .await;
    assert_eq!(
        integrated,
        json!({"id": "P-1", "result": "succeeded", "revision": "r2"})
    );
    let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(view["state"], "integrated");
    assert_eq!(view["task"]["status"], "done");
}

#[tokio::test]
async fn integration_conflicted_lists_files_and_kinds() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = integrator(&fx).await;
    open_p1(&fx, &frontend).await;
    let backend = fx.agent("backend").await;
    fx.proposal(&backend, 2, "pub fn w() {}\n").await;
    for id in ["P-1", "P-2"] {
        ok(
            &review,
            "submit_review",
            json!({"id": id, "verdict": "approve", "body": "good"}),
        )
        .await;
    }
    ok(&review, "integrate_proposal", json!({"id": "P-2"})).await;
    let error = err(&review, "integrate_proposal", json!({"id": "P-1"})).await;
    assert_eq!(error["code"], "integration_conflicted");
    assert_eq!(
        error["details"],
        json!({"proposal": "P-1", "files": [{"path": "src/lib.rs", "kind": "text"}]})
    );
    assert!(error["hint"].as_str().unwrap().contains("get_proposal"));
    let view = ok(&frontend, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(view["turn"], "with_author");
    assert_eq!(view["conflict"]["files"][0]["path"], "src/lib.rs");
    assert_eq!(view["conflict"]["files"][0]["kind"], "text");
    assert_eq!(view["conflict"]["main_revision"], "r2");
}

#[tokio::test]
async fn require_human_review_returns_the_mark() {
    let fx = Fixture::new().await;
    let planner = connect(&fx, "planner").await;
    let mark = ok(
        &planner,
        "require_human_review",
        json!({"task": "T-1", "reviewer": "tanim", "reason": "touches auth"}),
    )
    .await;
    assert_eq!(
        mark,
        json!({"task": "T-1", "reviewer": "tanim", "reason": "touches auth", "set_by": "planner"})
    );
    let error = err(
        &planner,
        "require_human_review",
        json!({"task": "T-2", "reviewer": "nobody", "reason": "touches auth"}),
    )
    .await;
    assert_eq!(error["code"], "unknown_human");
    let error = err(
        &planner,
        "require_human_review",
        json!({"task": "task-two", "reason": "touches auth"}),
    )
    .await;
    assert_eq!(error["code"], "invalid_id");
}

#[tokio::test]
async fn every_result_ends_with_the_footer() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    open_p1(&fx, &frontend).await;
    for tool in REVIEW_TOOLS {
        let arguments = match tool {
            "create_proposal" => json!({"task": "T-1", "title": "again"}),
            "require_human_review" => json!({"task": "T-1", "reason": "check"}),
            "list_proposals" => json!({}),
            "close_proposal" => json!({"id": "P-1", "reason": "done"}),
            _ => json!({"id": "P-1"}),
        };
        let result = call_raw(&frontend, tool, arguments).await;
        assert_footer(&result, tool);
    }
}

#[tokio::test]
async fn errors_carry_code_message_and_hint() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = connect(&fx, "review").await;
    open_p1(&fx, &frontend).await;
    for (client, tool, arguments, code, tool_in_hint) in [
        (
            &review,
            "get_proposal",
            json!({"id": "P-7"}),
            "proposal_not_found",
            "list_proposals",
        ),
        (
            &review,
            "claim_review",
            json!({"id": "P-x"}),
            "invalid_id",
            "list_proposals",
        ),
        (
            &frontend,
            "create_proposal",
            json!({"task": "T-1", "title": "twice"}),
            "proposal_exists",
            "update_proposal",
        ),
        (
            &review,
            "integrate_proposal",
            json!({"id": "P-1"}),
            "integrator_required",
            "integrate",
        ),
    ] {
        let error = err(client, tool, arguments).await;
        assert_eq!(error["code"], code, "{tool}");
        assert!(
            error["hint"].as_str().unwrap().contains(tool_in_hint),
            "{tool}: {error}"
        );
    }
}

#[tokio::test]
async fn as_acts_for_a_subagent_worker() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = connect(&fx, "review").await;
    open_p1(&fx, &frontend).await;
    ok(&review, "register_worker", json!({"name": "checker"})).await;
    let claim = ok(
        &review,
        "claim_review",
        json!({"id": "P-1", "as": "review/checker"}),
    )
    .await;
    assert_eq!(claim["holder"], "review/checker");
    let checkout = ok(
        &review,
        "create_review_checkout",
        json!({"id": "P-1", "as": "review/checker"}),
    )
    .await;
    assert_eq!(checkout["version"], "v1");
}

#[tokio::test]
async fn two_transports_open_one_proposal() {
    let fx = Fixture::new().await;
    let first = connect(&fx, "frontend").await;
    let second = connect(&fx, "frontend").await;
    ok(&first, "create_workspace", json!({"task": "T-1"})).await;
    fx.write(1, "src/lib.rs", "pub fn y() {}\n");
    let arguments = json!({"task": "T-1", "title": "Rename x"});
    let (a, b) = tokio::join!(
        call_raw(&first, "create_proposal", arguments.clone()),
        call_raw(&second, "create_proposal", arguments),
    );
    let mut errors: Vec<Value> = [a, b]
        .iter()
        .filter(|result| result.is_error == Some(true))
        .map(|result| body(result)["error"]["code"].clone())
        .collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors.pop().unwrap(), "proposal_exists");
    let listed = ok(&first, "list_proposals", json!({})).await;
    assert_eq!(listed["proposals"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn every_input_field_has_a_description() {
    let fx = Fixture::new().await;
    let client = connect(&fx, "frontend").await;
    let tools = client.list_all_tools().await.unwrap();
    for name in REVIEW_TOOLS {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name} is not listed"));
        assert!(tool.description.as_ref().is_some_and(|d| !d.is_empty()));
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
                        .is_some_and(|d| !d.is_empty()),
                    "{name}.{field} has no description"
                );
            }
        }
    }
    let integrate = tools
        .iter()
        .find(|tool| tool.name == "integrate_proposal")
        .unwrap();
    let description = integrate.description.as_deref().unwrap();
    for code in [
        "stale_version",
        "changes_requested",
        "not_approved",
        "human_approval_missing",
        "prerequisites_not_done",
        "author_cannot_integrate",
    ] {
        assert!(description.contains(code), "{code}");
    }
}

#[tokio::test]
async fn proposal_tools_need_a_review_service() {
    let fx = Fixture::new().await;
    let agents = fx.agents();
    let session = agents.start_session(7).await.unwrap();
    let handle = &fx.handle;
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
    );
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_end).await {
            let _ = running.waiting().await;
        }
    });
    let client: Client = ().serve(client_end).await.unwrap();
    call_raw(&client, "register_agent", json!({"name": "frontend"})).await;
    let tools = client.list_all_tools().await.unwrap();
    assert!(!tools.iter().any(|tool| tool.name == "list_proposals"));
    let Value::Object(arguments) = json!({}) else {
        unreachable!()
    };
    let refused = client
        .call_tool(CallToolRequestParams::new("list_proposals").with_arguments(arguments))
        .await;
    assert!(refused.is_err());
}
