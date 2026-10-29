#![cfg(test)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::common::roles::{give_main_tag, give_review_claim, give_role};
use crate::proposal_tools_support::Fixture;
use fold_core::roles::Role;
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

const REVIEW_TOOLS: [&str; 18] = [
    "create_proposal",
    "list_proposals",
    "get_proposal",
    "get_proposal_diff",
    "update_proposal",
    "close_proposal",
    "claim_review",
    "release_review",
    "create_review_checkout",
    "submit_review",
    "get_review_comments",
    "integrate_proposal",
    "require_human_review",
    "submit_main_change",
    "submit_review_fix",
    "run_checks",
    "get_check_run",
    "set_review_scope",
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
        json!({"intent": "Make the change the task asks for.", "task": "T-1", "title": "Rename x", "evidence": "tests pass"}),
    )
    .await
}

/// Makes an existing agent a worker that fold started in `role`.
#[cfg(test)]
async fn grant_role(fx: &Fixture, name: &str, role: Role) {
    give_role(&fx.handle.store, name, role, None).await;
}

/// `review` is a reviewer; its client reads its standing on every call.
#[cfg(test)]
async fn reviewer(fx: &Fixture) -> Client {
    let review = connect(fx, "review").await;
    grant_role(fx, "review", Role::Reviewer).await;
    review
}

/// Gives the reviewer `review` the review claim of `P-1`, as the review lead's `start_worker` gives it.
#[cfg(test)]
async fn give_p1(fx: &Fixture) {
    let p1 = "P-1".parse().unwrap();
    give_review_claim(&fx.handle.store, "review", p1, fx.handle.clock.now()).await;
}

/// `review` is the main agent, which claims a review itself when the user tells it to.
#[cfg(test)]
async fn main_reviewer(fx: &Fixture) -> Client {
    let review = connect(fx, "review").await;
    give_main_tag(&fx.handle.store, "review").await;
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
        json!({"intent": "Make the change the task asks for.", "task": "T-2", "title": "Export"}),
    )
    .await;
    assert_eq!(error["code"], "not_claim_holder");
}

#[tokio::test]
async fn list_proposals_shows_turn_claim_and_mark() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = reviewer(&fx).await;
    open_p1(&fx, &frontend).await;
    give_p1(&fx).await;
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
    let review = reviewer(&fx).await;
    open_p1(&fx, &frontend).await;
    let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(view["id"], "P-1");
    assert_eq!(view["state"], "open");
    assert_eq!(view["turn"], "with_reviewer");
    assert_eq!(view["version"], "v1");
    assert_eq!(view["current_version"], "v1");
    assert_eq!(view["base"], "r1");
    assert_eq!(view.get("evidence"), None);
    assert_eq!(view["evidence_chars"], "tests pass".len());
    assert_eq!(view["versions"].as_array().unwrap().len(), 1);
    assert_eq!(
        view["task"],
        json!({"id": "T-1", "title": "report", "status": "review"})
    );
    let full = ok(
        &review,
        "get_proposal",
        json!({"id": "P-1", "detail": "full"}),
    )
    .await;
    assert_eq!(full["evidence"], "tests pass");
    assert_eq!(full.get("evidence_chars"), None);
    assert_eq!(full["task"]["description"], "what to build");
    let error = err(
        &review,
        "get_proposal",
        json!({"id": "P-1", "detail": "all"}),
    )
    .await;
    assert_eq!(error["code"], "invalid_request");
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
        json!({"id": "P-1", "version": 1, "verdict": "approve", "body": "good", "comments": [
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

/// After `request_changes` on v1 the reviewer reads only what v2 changed, and a `new` comment on those lines lands on v2. Another agent, such as a new reviewer, reads the same earlier verdict.
#[tokio::test]
async fn a_reviewer_reads_only_the_change_since_the_last_verdict() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = connect(&fx, "review").await;
    ok(&frontend, "create_workspace", json!({"task": "T-1"})).await;
    fx.write(1, "src/lib.rs", "pub fn y() {}\n");
    fx.write(1, "README.md", "# demo v1\n");
    ok(
        &frontend,
        "create_proposal",
        json!({"intent": "Make the change the task asks for.", "task": "T-1", "title": "Rename x"}),
    )
    .await;
    let first = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(first["last_reviewed"], Value::Null);
    ok(
        &review,
        "submit_review",
        json!({"id": "P-1", "version": 1, "verdict": "request_changes", "body": "add z"}),
    )
    .await;
    fx.write(1, "src/lib.rs", "pub fn y() {}\npub fn z() {}\n");
    ok(
        &frontend,
        "update_proposal",
        json!({"intent": "Make the change the task asks for.", "id": "P-1"}),
    )
    .await;

    let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(view["last_reviewed"], "v1");
    assert_eq!(view["reviews"][0]["body"], "add z");
    assert_eq!(
        view["reviews"][0]["body_chars"], 5,
        "a review of an earlier version"
    );
    let full = ok(
        &review,
        "get_proposal",
        json!({"id": "P-1", "detail": "full"}),
    )
    .await;
    assert_eq!(full["reviews"][0].get("body_chars"), None);
    let since = ok(
        &review,
        "get_proposal_diff",
        json!({"id": "P-1", "since": 1}),
    )
    .await;
    assert_eq!(since["version"], "v2");
    assert_eq!(since["since"], "v1");
    assert_eq!(since["last_reviewed"], "v1");
    assert_eq!(
        since["files"],
        json!([{"path": "src/lib.rs", "change": "modified", "added": 1, "removed": 0}])
    );
    assert_eq!(
        since["earlier"],
        json!({"files": 2, "added": 2, "removed": 2})
    );
    assert_eq!(since["new_files"], json!([]));
    assert_eq!(since["base_moved"], false);
    assert_eq!(since["base_conflicts"], json!([]));
    assert_eq!(since["suggested_scope"], "change", "one line of four");
    assert_eq!(since["scope_rules"], json!([]));
    let full = ok(&review, "get_proposal_diff", json!({"id": "P-1"})).await;
    assert_eq!(full["since"], Value::Null);
    assert_eq!(full.get("earlier"), None);
    assert_eq!(full.get("suggested_scope"), None);
    assert_eq!(full["from"], "r1");
    assert_eq!(
        full["totals"],
        json!({"files": 2, "added": 3, "removed": 2})
    );

    let hunks = ok(
        &review,
        "get_proposal_diff",
        json!({"id": "P-1", "since": 1, "path": "src/lib.rs"}),
    )
    .await;
    assert_eq!(
        hunks["hunks"],
        json!([{"header": "@@ -1,1 +1,2 @@", "lines": [" 1 pub fn y() {}\n", "+2 pub fn z() {}\n"]}])
    );
    ok(
        &review,
        "submit_review",
        json!({"id": "P-1", "version": 2, "verdict": "comment", "body": "", "comments": [
            {"path": "src/lib.rs", "start_line": 2, "end_line": 2, "side": "new", "body": "document z"},
        ]}),
    )
    .await;
    let comments = ok(&review, "get_review_comments", json!({"id": "P-1"})).await;
    let comment = comments["comments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|comment| comment["body"] == "document z")
        .unwrap();
    assert_eq!(comment["version"], "v2");
    assert_eq!(comment["side"], "new");
    assert_eq!(
        (&comment["start_line"], &comment["end_line"]),
        (&json!(2), &json!(2))
    );
    assert_eq!(comment["outdated"], false);

    let next = connect(&fx, "reviewer-2").await;
    let view = ok(&next, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(view["last_reviewed"], "v1");
    let stale = err(
        &next,
        "submit_review",
        json!({"id": "P-1", "version": 1, "verdict": "request_changes", "body": "test z"}),
    )
    .await;
    assert_eq!(stale["code"], "stale_version");
    assert_eq!(stale["details"]["current_version"], 2);
    ok(
        &next,
        "submit_review",
        json!({"id": "P-1", "version": 2, "verdict": "request_changes", "body": "test z"}),
    )
    .await;
    let view = ok(&next, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(view["last_reviewed"], "v1");
    let first = ok(&next, "get_proposal", json!({"id": "P-1", "version": 1})).await;
    assert_eq!(first["last_reviewed"], Value::Null);
    let since = ok(
        &next,
        "get_proposal_diff",
        json!({"id": "P-1", "since": 1, "path": "src/lib.rs"}),
    )
    .await;
    assert_eq!(
        (&since["version"], &since["since"], &since["last_reviewed"]),
        (&json!("v2"), &json!("v1"), &json!("v1"))
    );

    let error = err(
        &review,
        "get_proposal_diff",
        json!({"id": "P-1", "since": 2}),
    )
    .await;
    assert_eq!(error["code"], "invalid_request");
    let error = err(
        &review,
        "get_proposal_diff",
        json!({"id": "P-1", "since": 1, "version": 5}),
    )
    .await;
    assert_eq!(error["code"], "proposal_not_found");
}

/// Opens `P-2` for `T-2` with `files` written by `backend`, approves it and integrates it as `r2`, so main moves under the frontend's workspace.
#[cfg(test)]
async fn integrate_p2(fx: &Fixture, backend: &Client, review: &Client, files: &[(&str, &str)]) {
    ok(backend, "create_workspace", json!({"task": "T-2"})).await;
    for (path, content) in files {
        fx.write(2, path, content);
    }
    ok(
        backend,
        "create_proposal",
        json!({"intent": "Make the change the task asks for.", "task": "T-2", "title": "Export"}),
    )
    .await;
    ok(
        review,
        "submit_review",
        json!({"id": "P-2", "version": 1, "verdict": "approve", "body": "good"}),
    )
    .await;
    let integrated = ok(review, "integrate_proposal", json!({"id": "P-2"})).await;
    assert_eq!(integrated["revision"], "r2");
}

/// The worker syncs with a newer main between versions: the change since v1 holds only the worker's own edits, never the files main gained, and names the file the earlier version did not change.
#[tokio::test]
async fn a_change_since_an_earlier_version_leaves_out_what_main_gained() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let backend = connect(&fx, "backend").await;
    let review = reviewer(&fx).await;
    open_p1(&fx, &frontend).await;
    ok(
        &review,
        "submit_review",
        json!({"id": "P-1", "version": 1, "verdict": "request_changes", "body": "add z"}),
    )
    .await;
    integrate_p2(
        &fx,
        &backend,
        &review,
        &[
            ("README.md", "# demo from main\n"),
            ("docs/guide.md", "guide\n"),
        ],
    )
    .await;
    let synced = ok(&frontend, "sync_workspace", json!({"ws": "ws-1"})).await;
    assert_eq!(synced["base"], "r2");
    assert_eq!(synced["conflicted"], json!([]));
    fx.write(1, "src/lib.rs", "pub fn y() {}\npub fn z() {}\n");
    fx.write(1, "src/extra.rs", "pub fn extra() {}\n");
    ok(
        &frontend,
        "update_proposal",
        json!({"intent": "Make the change the task asks for.", "id": "P-1"}),
    )
    .await;
    let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(view["base"], "r2");

    let since = ok(
        &review,
        "get_proposal_diff",
        json!({"id": "P-1", "since": 1}),
    )
    .await;
    assert_eq!(
        since["files"],
        json!([
            {"path": "src/extra.rs", "change": "added", "added": 1, "removed": 0},
            {"path": "src/lib.rs", "change": "modified", "added": 1, "removed": 0},
        ])
    );
    assert_eq!(
        since["totals"],
        json!({"files": 2, "added": 2, "removed": 0})
    );
    assert_eq!(
        since["earlier"],
        json!({"files": 1, "added": 1, "removed": 1})
    );
    assert_eq!(since["new_files"], json!(["src/extra.rs"]));
    assert_eq!(since["base_moved"], true);
    assert_eq!(since["base_conflicts"], json!([]));
    assert_eq!(since["suggested_scope"], "full");
    assert_eq!(since["scope_rules"], json!(["new_files", "large_delta"]));
    let hunks = ok(
        &review,
        "get_proposal_diff",
        json!({"id": "P-1", "since": 1, "path": "src/lib.rs"}),
    )
    .await;
    assert_eq!(
        hunks["hunks"],
        json!([{"header": "@@ -1,1 +1,2 @@", "lines": [" 1 pub fn y() {}\n", "+2 pub fn z() {}\n"]}])
    );
    let error = err(
        &review,
        "get_proposal_diff",
        json!({"id": "P-1", "since": 1, "path": "README.md"}),
    )
    .await;
    assert_eq!(error["code"], "path_not_in_comparison");
    let full = ok(&review, "get_proposal_diff", json!({"id": "P-1"})).await;
    assert_eq!(full["from"], "r2");
    assert_eq!(
        full["totals"],
        json!({"files": 2, "added": 3, "removed": 1})
    );
}

/// When the earlier version and main changed the same lines, the worker merged main by hand: the change since v1 names that file, and shows it against the earlier version's own content.
#[tokio::test]
async fn a_change_since_an_earlier_version_names_the_files_merged_by_hand() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let backend = connect(&fx, "backend").await;
    let review = reviewer(&fx).await;
    open_p1(&fx, &frontend).await;
    ok(
        &review,
        "submit_review",
        json!({"id": "P-1", "version": 1, "verdict": "request_changes", "body": "rebase"}),
    )
    .await;
    integrate_p2(
        &fx,
        &backend,
        &review,
        &[("src/lib.rs", "pub fn w() {}\n"), ("README.md", "# main\n")],
    )
    .await;
    let synced = ok(&frontend, "sync_workspace", json!({"ws": "ws-1"})).await;
    assert_eq!(synced["conflicted"], json!(["src/lib.rs"]));
    fx.write(1, "src/lib.rs", "pub fn w() {}\npub fn y() {}\n");
    ok(
        &frontend,
        "record_changes",
        json!({"ws": "ws-1", "message": "Keep both functions"}),
    )
    .await;
    ok(
        &frontend,
        "update_proposal",
        json!({"intent": "Make the change the task asks for.", "id": "P-1"}),
    )
    .await;

    let since = ok(
        &review,
        "get_proposal_diff",
        json!({"id": "P-1", "since": 1}),
    )
    .await;
    assert_eq!(
        since["files"],
        json!([{"path": "src/lib.rs", "change": "modified", "added": 1, "removed": 0}])
    );
    assert_eq!(since["new_files"], json!([]));
    assert_eq!(since["base_moved"], true);
    assert_eq!(since["base_conflicts"], json!(["src/lib.rs"]));
    assert_eq!(since["suggested_scope"], "full");
    assert_eq!(since["scope_rules"], json!(["base_conflicts"]));
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
        json!({"intent": "Make the change the task asks for.", "id": "P-1", "evidence": "all tests pass"}),
    )
    .await;
    assert_eq!(
        updated,
        json!({"id": "P-1", "version": "v2", "state": "open", "task": "T-1", "status": "review"})
    );
    let error = err(
        &backend,
        "update_proposal",
        json!({"intent": "Make the change the task asks for.", "id": "P-1"}),
    )
    .await;
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
    let review = main_reviewer(&fx).await;
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
    let review = reviewer(&fx).await;
    open_p1(&fx, &frontend).await;
    let error = err(&review, "create_review_checkout", json!({"id": "P-1"})).await;
    assert_eq!(error["code"], "review_claim_required");
    give_p1(&fx).await;
    let checkout = ok(&review, "create_review_checkout", json!({"id": "P-1"})).await;
    assert_eq!(checkout["version"], "v1");
    assert_eq!(checkout["path"], ".fold/ws/review-p-1");
    assert!(checkout["workspace"].as_str().unwrap().starts_with("ws-"));
    let copied = fx
        .project
        .paths()
        .root()
        .join(".fold/ws/review-p-1/src/lib.rs");
    assert_eq!(std::fs::read_to_string(copied).unwrap(), "pub fn y() {}\n");
}

#[tokio::test]
async fn submit_review_fix_makes_the_reviewers_version_and_the_reviewer_approves_it() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = reviewer(&fx).await;
    open_p1(&fx, &frontend).await;
    give_p1(&fx).await;
    ok(&review, "create_review_checkout", json!({"id": "P-1"})).await;
    let file = fx
        .project
        .paths()
        .root()
        .join(".fold/ws/review-p-1/src/lib.rs");
    std::fs::write(file, "pub fn y() -> u8 { 1 }\n").unwrap();
    let arguments = json!({
        "id": "P-1",
        "version": 1,
        "fixes": [{"path": "src/lib.rs", "reason": "y returns 1 as the task asks"}],
        "checks": "cargo test: 1 passed",
        "intent": "Make y return 1 as the task asks; nothing else changes.",
    });
    let error = err(&frontend, "submit_review_fix", arguments.clone()).await;
    assert_eq!(error["code"], "author_cannot_review");
    assert!(error["hint"].as_str().unwrap().contains("update_proposal"));

    let fixed = ok(&review, "submit_review_fix", arguments.clone()).await;
    assert_eq!(fixed["id"], "P-1");
    assert_eq!(fixed["version"], "v2");
    assert_eq!(fixed["from_version"], "v1");
    assert_eq!(fixed["checkout"], "ws-2");
    assert_eq!(fixed["status"], "review");
    assert!(fixed["changeset"].as_str().unwrap().starts_with("C-"));
    let view = ok(
        &review,
        "get_proposal",
        json!({"id": "P-1", "detail": "full"}),
    )
    .await;
    assert_eq!(view["versions"][1]["created_by"], "review");
    assert_eq!(view["versions"][1]["reviewer_fix"], true);
    assert_eq!(view["versions"][0].get("reviewer_fix"), None);
    assert!(
        view["evidence"]
            .as_str()
            .unwrap()
            .starts_with("Review fixes by review on v1:")
    );
    let error = err(&review, "submit_review_fix", arguments).await;
    assert_eq!(error["code"], "stale_version");

    ok(
        &review,
        "submit_review",
        json!({"id": "P-1", "version": 2, "verdict": "approve", "body": "Fix: src/lib.rs (y returns 1). cargo test: 1 passed"}),
    )
    .await;
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
        json!({"id": "P-1", "version": 1, "verdict": "comment", "body": "mine"}),
    )
    .await;
    assert_eq!(error["code"], "author_cannot_review");
    let error = err(
        &backend,
        "submit_review",
        json!({"id": "P-1", "version": 1, "verdict": "maybe", "body": "?"}),
    )
    .await;
    assert_eq!(error["code"], "invalid_request");
    let error = err(
        &backend,
        "submit_review",
        json!({"id": "P-1", "version": 1, "verdict": "comment", "body": "", "comments": [
            {"path": "src/lib.rs", "start_line": 1, "body": "no side"},
        ]}),
    )
    .await;
    assert_eq!(error["code"], "invalid_request");
    let error = err(
        &backend,
        "submit_review",
        json!({"id": "P-1", "version": 1, "verdict": "comment", "body": "", "comments": [
            {"path": "src/lib.rs", "start_line": 4, "end_line": 9, "side": "new", "body": "far"},
        ]}),
    )
    .await;
    assert_eq!(error["code"], "invalid_comment_range");
    let submitted = ok(
        &backend,
        "submit_review",
        json!({"id": "P-1", "version": 1, "verdict": "request_changes", "body": "rename back", "comments": [
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
        json!({"id": "P-1", "version": 1, "verdict": "comment", "body": "", "comments": [
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
    ok(
        &frontend,
        "update_proposal",
        json!({"intent": "Make the change the task asks for.", "id": "P-1"}),
    )
    .await;
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
    let review = reviewer(&fx).await;
    open_p1(&fx, &frontend).await;
    let error = err(&review, "integrate_proposal", json!({"id": "P-1"})).await;
    assert_eq!(error["code"], "not_approved");
    ok(
        &review,
        "submit_review",
        json!({"id": "P-1", "version": 1, "verdict": "approve", "body": "good"}),
    )
    .await;
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
    let review = reviewer(&fx).await;
    open_p1(&fx, &frontend).await;
    let backend = fx.agent("backend").await;
    fx.proposal(&backend, 2, "pub fn w() {}\n").await;
    for id in ["P-1", "P-2"] {
        ok(
            &review,
            "submit_review",
            json!({"id": id, "version": 1, "verdict": "approve", "body": "good"}),
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
            "create_proposal" => json!({"task": "T-1", "title": "again", "intent": "again"}),
            "require_human_review" => json!({"task": "T-1", "reason": "check"}),
            "list_proposals" => json!({}),
            "close_proposal" => json!({"id": "P-1", "reason": "done"}),
            "submit_main_change" => json!({"changeset": "C-1", "message": "m"}),
            "submit_review_fix" => json!({
                "id": "P-1",
                "version": 1,
                "fixes": [{"path": "src/lib.rs", "reason": "r"}],
                "checks": "ok",
                "intent": "i",
            }),
            "set_review_scope" => {
                json!({"id": "P-1", "version": 1, "scope": "full", "reason": "r"})
            }
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
    grant_role(&fx, "review", Role::TaskWorker).await;
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
            json!({"intent": "Make the change the task asks for.", "task": "T-1", "title": "twice"}),
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
async fn two_transports_open_one_proposal() {
    let fx = Fixture::new().await;
    let first = connect(&fx, "frontend").await;
    let second = connect(&fx, "frontend").await;
    ok(&first, "create_workspace", json!({"task": "T-1"})).await;
    fx.write(1, "src/lib.rs", "pub fn y() {}\n");
    let arguments = json!({"task": "T-1", "title": "Rename x", "intent": "Renames x."});
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

/// The result of `get_check_run` once the run has ended.
#[cfg(test)]
async fn ended_run(client: &Client, run: &Value) -> Value {
    fold_store::testing::eventually(std::time::Duration::from_secs(30), async || {
        let value = ok(client, "get_check_run", json!({"run": run})).await;
        if value["state"] == "running" {
            Err(value)
        } else {
            Ok(value)
        }
    })
    .await
}

#[tokio::test]
async fn run_checks_answers_at_once_and_get_check_run_reads_the_result() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    ok(&frontend, "create_workspace", json!({"task": "T-1"})).await;
    let none = ok(&frontend, "run_checks", json!({"task": "T-1"})).await;
    assert_eq!(none, json!({"task": "T-1", "checks": "none"}));
    fx.write(
        1,
        "fold-checks.toml",
        "[[check]]\nname = \"unit\"\nrun = \"echo nope; exit 2\"\n",
    );
    let started = ok(&frontend, "run_checks", json!({"task": "T-1"})).await;
    assert_eq!(started["checks"], "running");
    assert_eq!(started["run"], "cr-1");
    assert_eq!(started["workspace"], "ws-1");
    let run = ended_run(&frontend, &started["run"]).await;
    assert_eq!(run["state"], "failed");
    assert_eq!(run["reason"], "check_failed");
    assert_eq!(run["stage"], "proposal");
    assert_eq!(run["failed"], json!(["unit"]));
    assert_eq!(run["results"][0]["exit_code"], 2);
    assert_eq!(run["results"][0]["log_tail"], "nope");
    let missing = err(&frontend, "get_check_run", json!({"run": "cr-9"})).await;
    assert_eq!(missing["code"], "check_run_not_found");
    let bad = err(&frontend, "get_check_run", json!({"run": "P-1"})).await;
    assert_eq!(bad["code"], "invalid_id");
    let review = connect(&fx, "review").await;
    let other = err(&review, "run_checks", json!({"task": "T-1"})).await;
    assert_eq!(other["code"], "not_workspace_owner");
}

#[tokio::test]
async fn get_proposal_lists_the_check_runs_of_the_version() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    ok(&frontend, "create_workspace", json!({"task": "T-1"})).await;
    fx.write(1, "src/lib.rs", "pub fn y() {}\n");
    fx.write(
        1,
        "fold-checks.toml",
        "[[check]]\nname = \"unit\"\nrun = \"true\"\n",
    );
    let started = ok(&frontend, "run_checks", json!({"task": "T-1"})).await;
    ended_run(&frontend, &started["run"]).await;
    ok(
        &frontend,
        "create_proposal",
        json!({"intent": "Make the change the task asks for.", "task": "T-1", "title": "Add checks"}),
    )
    .await;
    let review = connect(&fx, "review").await;
    let proposal = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(
        proposal["checks"],
        json!([{"run": "cr-1", "stage": "proposal", "state": "passed", "reason": null, "failed": []}])
    );
}

#[tokio::test]
async fn create_proposal_answers_with_the_run_and_wait_for_events_wakes_on_its_end() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    ok(&frontend, "create_workspace", json!({"task": "T-1"})).await;
    fx.write(1, "src/lib.rs", "pub fn y() {}\n");
    fx.write(
        1,
        "fold-checks.toml",
        "[[check]]\nname = \"unit\"\nrun = \"true\"\n",
    );
    let created = ok(
        &frontend,
        "create_proposal",
        json!({"intent": "Make the change the task asks for.", "task": "T-1", "title": "Add checks"}),
    )
    .await;
    assert_eq!(
        created,
        json!({"task": "T-1", "workspace": "ws-1", "checks": "running", "run": "cr-1"})
    );
    let woke = ok(
        &frontend,
        "wait_for_events",
        json!({"types": ["checks.finished"], "task": "T-1", "timeout_s": 60}),
    )
    .await;
    let events = woke["events"].as_array().unwrap();
    assert_eq!(events[0]["type"], "checks.finished", "{woke}");
    assert_eq!(events[0]["data"]["state"], "passed");
    let proposal = ok(&frontend, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(proposal["version"], "v1");
    assert_eq!(proposal["human_review"]["set_by"], "fold");
}

#[tokio::test]
async fn an_approve_waits_while_an_update_runs_the_checks_of_the_next_version() {
    let fx = Fixture::new().await;
    let review = reviewer(&fx).await;
    let author = fx.agent("frontend").await;
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    let update = fx.updating_proposal(&author, &go).await;
    let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(
        view["update_run"],
        json!({"run": update.to_string(), "purpose": "update_proposal"})
    );
    let held = err(
        &review,
        "submit_review",
        json!({"id": "P-1", "version": 1, "verdict": "approve", "body": "ok"}),
    )
    .await;
    assert_eq!(held["code"], "update_checks_running");
    assert_eq!(held["details"]["run"], json!(update.to_string()));
    let hint = held["hint"].as_str().unwrap();
    assert!(hint.contains("wait_for_events"), "{hint}");
    assert!(hint.contains("checks.finished"), "{hint}");
    ok(
        &review,
        "submit_review",
        json!({"id": "P-1", "version": 1, "verdict": "comment", "body": "reading"}),
    )
    .await;
    std::fs::write(&go, "").unwrap();
    let view = fold_store::testing::eventually(std::time::Duration::from_secs(30), async || {
        let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
        if view["current_version"] == "v2" {
            Ok(view)
        } else {
            Err(view)
        }
    })
    .await;
    assert_eq!(view.get("update_run"), None);
    ok(
        &review,
        "submit_review",
        json!({"id": "P-1", "version": 2, "verdict": "approve", "body": "ok"}),
    )
    .await;
}

#[tokio::test]
async fn an_approve_waits_while_the_reviewers_fix_runs_and_the_reviewer_approves_the_fix_version() {
    let fx = Fixture::new().await;
    let review = reviewer(&fx).await;
    let author = fx.agent("frontend").await;
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    fx.gated_proposal(&author, &go).await;
    give_p1(&fx).await;
    let checkout = ok(&review, "create_review_checkout", json!({"id": "P-1"})).await;
    let dir = fx
        .project
        .paths()
        .root()
        .join(checkout["path"].as_str().unwrap());
    std::fs::write(dir.join("src/lib.rs"), "pub fn y() -> u8 { 1 }\n").unwrap();
    let fixing = ok(
        &review,
        "submit_review_fix",
        json!({
            "intent": "Make the change the task asks for.",
            "id": "P-1",
            "version": 1,
            "fixes": [{"path": "src/lib.rs", "reason": "y returned nothing"}],
            "checks": "cargo test: ok",
        }),
    )
    .await;
    assert_eq!(fixing["checks"], "running");
    let run = fixing["run"].clone();
    let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(
        view["update_run"],
        json!({"run": run, "purpose": "review_fix"})
    );
    let held = err(
        &review,
        "submit_review",
        json!({"id": "P-1", "version": 1, "verdict": "approve", "body": "ok"}),
    )
    .await;
    assert_eq!(held["code"], "update_checks_running");
    assert_eq!(held["details"]["purpose"], "review_fix");
    let hint = held["hint"].as_str().unwrap();
    assert!(hint.contains("wait_for_events"), "{hint}");
    std::fs::write(&go, "").unwrap();
    fold_store::testing::eventually(std::time::Duration::from_secs(30), async || {
        let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
        if view["current_version"] == "v2" && view.get("update_run").is_none() {
            Ok(())
        } else {
            Err(view)
        }
    })
    .await;
    ok(
        &review,
        "submit_review",
        json!({"id": "P-1", "version": 2, "verdict": "approve", "body": "Fixes: src/lib.rs (y returns 1). cargo test: ok"}),
    )
    .await;
}

#[tokio::test]
async fn every_proposal_version_tool_requires_an_intent_and_get_proposal_shows_it() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let tools = frontend.list_all_tools().await.unwrap();
    for name in ["create_proposal", "update_proposal", "submit_review_fix"] {
        let tool = tools.iter().find(|tool| tool.name == name).unwrap();
        let schema = serde_json::to_value(tool.input_schema.as_ref()).unwrap();
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("intent")),
            "{name}: {schema}"
        );
        assert_eq!(schema["properties"]["intent"]["type"], "string", "{name}");
    }

    ok(&frontend, "create_workspace", json!({"task": "T-1"})).await;
    fx.write(1, "src/lib.rs", "pub fn y() {}\n");
    let missing = call_raw(
        &frontend,
        "create_proposal",
        json!({"task": "T-1", "title": "Rename x"}),
    )
    .await;
    assert_eq!(missing.is_error, Some(true));
    let missing = &body(&missing)["error"];
    assert_eq!(missing["code"], "invalid_intent", "{missing}");
    assert_eq!(missing["details"]["reason"], "missing");
    assert!(
        missing["hint"]
            .as_str()
            .unwrap()
            .starts_with("call create_proposal again with `intent`"),
        "{missing}"
    );
    let fix = json!({"id": "P-1", "version": 1, "checks": "cargo test: ok",
                     "fixes": [{"path": "src/lib.rs", "reason": "typo"}]});
    for (tool, arguments) in [
        ("update_proposal", json!({"id": "P-1"})),
        ("submit_review_fix", fix),
    ] {
        let refused = err(&frontend, tool, arguments).await;
        assert_eq!(refused["code"], "invalid_intent", "{tool}: {refused}");
        assert_eq!(refused["details"]["reason"], "missing", "{tool}");
        assert!(
            refused["hint"]
                .as_str()
                .unwrap()
                .starts_with(&format!("call {tool} again with `intent`")),
            "{refused}"
        );
    }
    let control = err(
        &frontend,
        "create_proposal",
        json!({"task": "T-1", "title": "Rename x", "intent": "Renames x.\u{1b}[2J"}),
    )
    .await;
    assert_eq!(control["code"], "invalid_intent");
    assert_eq!(control["details"]["reason"], "control_character");
    let long = "x".repeat(2001);
    let refused = err(
        &frontend,
        "create_proposal",
        json!({"task": "T-1", "title": "Rename x", "intent": long}),
    )
    .await;
    assert_eq!(refused["code"], "invalid_intent");
    assert_eq!(refused["details"]["reason"], "too_long");
    assert!(
        !refused.to_string().contains(&long),
        "an error never repeats the intent"
    );
    assert!(
        refused["hint"]
            .as_str()
            .unwrap()
            .starts_with("call create_proposal again")
    );

    let intent = "Renames x to y so the name says what it holds. It changes no behaviour.";
    ok(
        &frontend,
        "create_proposal",
        json!({"task": "T-1", "title": "Rename x", "intent": intent}),
    )
    .await;
    let short = ok(&frontend, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(short["intent"], intent);
    assert_eq!(short["intent_chars"], intent.chars().count());
    let full = ok(
        &frontend,
        "get_proposal",
        json!({"id": "P-1", "detail": "full"}),
    )
    .await;
    assert_eq!(full["intent"], intent);
    let blank = err(
        &frontend,
        "update_proposal",
        json!({"id": "P-1", "intent": "  "}),
    )
    .await;
    assert_eq!(blank["code"], "invalid_intent");
    assert_eq!(blank["details"]["reason"], "empty");
}
