#![cfg(test)]

use axum::body::Body;
use axum::http::{Method, Request};
use fold_core::roles::Role;
use fold_core::{WorkerState, codes};
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::router;
use fold_daemon::mcp::FoldServer;
use fold_daemon::services::events::EventService;
use fold_daemon::services::messages::MessageService;
use fold_store::testing::eventually;
use http_body_util::BodyExt;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tower::ServiceExt as _;

use crate::common::acceptance::{Harness, StoredEvent, WAIT};
use crate::common::roles::give_role;
use crate::proposal_tools_support::Fixture;
use crate::workers_support::FakeClaude;

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

async fn err(client: &Client, tool: &str, arguments: Value) -> Value {
    let result = call(client, tool, arguments).await;
    assert_eq!(result.is_error, Some(true), "{tool} succeeded: {result:?}");
    body(&result)["error"].clone()
}

/// `review` reviews `version` of `proposal` with `verdict`.
async fn verdict(review: &Client, proposal: &str, version: u32, verdict: &str) {
    ok(
        review,
        "submit_review",
        json!({"id": proposal, "version": version, "verdict": verdict, "body": "read"}),
    )
    .await;
}

/// `frontend` proposes `src/lib.rs` for T-1 as P-1 v1, the review lead asks for changes, and `frontend` sends each of `contents` as the next version, each but the last reviewed with request_changes again.
async fn revised(fx: &Fixture, frontend: &Client, review: &Client, contents: &[&str]) {
    ok(frontend, "create_workspace", json!({"task": "T-1"})).await;
    let first = (1..=40)
        .map(|n| format!("pub fn f{n}() {{}}\n"))
        .collect::<String>();
    fx.write(1, "src/lib.rs", &first);
    ok(
        frontend,
        "create_proposal",
        json!({"task": "T-1", "title": "Add the functions"}),
    )
    .await;
    for (index, content) in contents.iter().enumerate() {
        let reviewed = u32::try_from(index).unwrap() + 1;
        verdict(review, "P-1", reviewed, "request_changes").await;
        fx.write(1, "src/lib.rs", &format!("{first}{content}"));
        ok(frontend, "update_proposal", json!({"id": "P-1"})).await;
    }
}

/// The JSON answer of the project route `path` for the human `tanim`, which must succeed.
async fn http(fx: &Fixture, method: Method, path: &str, body: Option<Value>) -> Value {
    let request = Request::builder()
        .method(method)
        .uri(format!("/api/v1/projects/demo{path}"))
        .header(CLIENT_HEADER, "test")
        .header("cookie", "fold_human=tanim");
    let request = match body {
        Some(value) => request
            .header("content-type", "application/json")
            .body(Body::from(value.to_string())),
        None => request.body(Body::empty()),
    };
    let response = router(fx.state.clone())
        .oneshot(request.unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let answer: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(status.is_success(), "{status}: {answer}");
    answer
}

/// The scope of the newest review of `proposal`, as get_proposal shows the review history.
async fn last_scope(client: &Client, proposal: &str) -> Value {
    let view = ok(client, "get_proposal", json!({"id": proposal})).await;
    view["reviews"].as_array().unwrap().last().unwrap()["scope"].clone()
}

#[tokio::test]
async fn a_small_v2_gets_a_change_review_and_a_review_follows_the_suggestion() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = connect(&fx, "review").await;
    give_role(&fx.handle.store, "review", Role::ReviewLead, None).await;
    revised(&fx, &frontend, &review, &["pub fn g() {}\n"]).await;
    assert_eq!(
        last_scope(&review, "P-1").await,
        Value::Null,
        "a review of v1 reads the whole version"
    );

    let since = ok(
        &review,
        "get_proposal_diff",
        json!({"id": "P-1", "since": 1}),
    )
    .await;
    assert_eq!(since["suggested_scope"], "change");
    assert_eq!(since["scope_rules"], json!([]));
    let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(view["review_scope"], json!({"since": "v1", "chosen": null}));
    verdict(&review, "P-1", 2, "approve").await;
    assert_eq!(
        last_scope(&review, "P-1").await,
        json!({"scope": "change", "since": "v1", "suggested": "change", "rules": [], "reason": null})
    );
}

#[tokio::test]
async fn a_v3_suggests_a_full_review_that_the_review_lead_overrides_with_a_reason() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = connect(&fx, "review").await;
    give_role(&fx.handle.store, "review", Role::ReviewLead, None).await;
    revised(
        &fx,
        &frontend,
        &review,
        &["pub fn g() {}\n", "pub fn h() {}\n"],
    )
    .await;
    let since = ok(
        &review,
        "get_proposal_diff",
        json!({"id": "P-1", "since": 2}),
    )
    .await;
    assert_eq!(since["version"], "v3");
    assert_eq!(since["suggested_scope"], "full");
    assert_eq!(since["scope_rules"], json!(["v3_or_later"]));

    for (arguments, field) in [
        (json!({"id": "P-1", "scope": "change"}), "reason"),
        (
            json!({"id": "P-1", "scope": "change", "reason": "  "}),
            "reason",
        ),
        (
            json!({"id": "P-1", "scope": "partial", "reason": "small"}),
            "scope",
        ),
        (json!({"id": "P-1", "reason": "small"}), "reason"),
    ] {
        let refused = err(&review, "claim_review", arguments.clone()).await;
        assert_eq!(refused["code"], codes::INVALID_REQUEST, "{arguments}");
        assert_eq!(refused["details"]["field"], field, "{arguments}");
    }
    let unclaimed = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(
        unclaimed["claim"],
        Value::Null,
        "a refused choice claims nothing"
    );

    ok(
        &review,
        "claim_review",
        json!({"id": "P-1", "scope": "change", "reason": "v3 only adds one function"}),
    )
    .await;
    let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(
        view["review_scope"],
        json!({"since": "v2", "chosen": {"scope": "change", "reason": "v3 only adds one function", "by": "review"}})
    );
    verdict(&review, "P-1", 3, "approve").await;
    assert_eq!(
        last_scope(&review, "P-1").await,
        json!({
            "scope": "change",
            "since": "v2",
            "suggested": "full",
            "rules": ["v3_or_later"],
            "reason": "v3 only adds one function",
        })
    );

    let detail = http(&fx, Method::GET, "/proposals/P-1", None).await;
    let reviews = detail["reviews"].as_array().unwrap();
    assert_eq!(reviews[0]["scope"], Value::Null, "{detail}");
    assert_eq!(
        reviews.last().unwrap()["scope"],
        json!({
            "scope": "change",
            "since": 2,
            "suggested": "full",
            "rules": ["v3_or_later"],
            "reason": "v3 only adds one function",
        })
    );
}

#[tokio::test]
async fn a_first_version_takes_no_scope_and_a_scope_names_a_proposal_of_the_call() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let backend = connect(&fx, "backend").await;
    let review = connect(&fx, "review").await;
    give_role(&fx.handle.store, "review", Role::ReviewLead, None).await;
    revised(&fx, &frontend, &review, &["pub fn g() {}\n"]).await;
    ok(&backend, "create_workspace", json!({"task": "T-2"})).await;
    fx.write(2, "README.md", "# export\n");
    ok(
        &backend,
        "create_proposal",
        json!({"task": "T-2", "title": "Export"}),
    )
    .await;

    let first = err(
        &review,
        "claim_review",
        json!({"id": "P-2", "scope": "full", "reason": "a new endpoint"}),
    )
    .await;
    assert_eq!(first["code"], codes::INVALID_REQUEST);
    assert_eq!(first["details"]["field"], "scope");
    let unclaimed = ok(&review, "get_proposal", json!({"id": "P-2"})).await;
    assert_eq!(unclaimed["claim"], Value::Null);
    assert_eq!(unclaimed["review_scope"], Value::Null);

    let outside = err(
        &review,
        "create_review_batch",
        json!({"proposals": ["P-1", "P-2"], "scopes": [{"proposal": "P-9", "scope": "full", "reason": "wide"}]}),
    )
    .await;
    assert_eq!(outside["code"], codes::INVALID_REQUEST);
    assert_eq!(outside["details"]["field"], "scopes");
    let first = err(
        &review,
        "create_review_batch",
        json!({"proposals": ["P-1", "P-2"], "scopes": [{"proposal": "P-2", "scope": "full", "reason": "a new endpoint"}]}),
    )
    .await;
    assert_eq!(first["code"], codes::INVALID_REQUEST);
    assert_eq!(first["details"]["field"], "scope");
    for proposal in ["P-1", "P-2"] {
        let view = ok(&review, "get_proposal", json!({"id": proposal})).await;
        assert_eq!(view["claim"], Value::Null, "a refused batch claims nothing");
    }
    let batch = ok(
        &review,
        "create_review_batch",
        json!({"proposals": ["P-1", "P-2"], "scopes": [{"proposal": "P-1", "scope": "full", "reason": "it touches the parser"}]}),
    )
    .await;
    assert_eq!(batch["id"], "rb-1");
    // A later choice for the same version replaces the earlier one.
    ok(
        &review,
        "claim_review",
        json!({"id": "P-1", "scope": "full", "reason": "it touches the parser and the lexer"}),
    )
    .await;
    let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(
        view["review_scope"]["chosen"]["reason"],
        "it touches the parser and the lexer"
    );
    ok(&review, "create_batch_checkout", json!({"id": "rb-1"})).await;
    ok(
        &review,
        "submit_batch_review",
        json!({"id": "rb-1", "verdicts": [
            {"proposal": "P-1", "verdict": "approve"},
            {"proposal": "P-2", "verdict": "approve"},
        ]}),
    )
    .await;
    assert_eq!(
        last_scope(&review, "P-1").await,
        json!({"scope": "full", "since": "v1", "suggested": "change", "rules": [], "reason": "it touches the parser and the lexer"})
    );
    assert_eq!(last_scope(&review, "P-2").await, Value::Null);
}

/// The review lead gives a revised proposal to a reviewer with a full review chosen over fold's suggestion; the reviewer reads the choice, cannot change it by claiming, and its review follows it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reviewer_follows_the_scope_its_review_lead_chose_at_the_start() {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main("frontend").await;
    let task = h.create_task("frontend", "Add the functions").await;
    let work = h.start_work("frontend", &task).await;
    work.write(&[(
        "src/lib.rs",
        "pub fn a() {}\npub fn b() {}\npub fn c() {}\n",
    )]);
    let proposal = h.propose("frontend", &work, "Add the functions").await;
    h.call("frontend", "start_worker", json!({"role": "review_lead"}))
        .await;
    h.connect("review-lead").await;
    h.call(
        "review-lead",
        "submit_review",
        json!({"id": proposal, "version": 1, "verdict": "request_changes", "body": "add d"}),
    )
    .await;
    work.write(&[(
        "src/lib.rs",
        "pub fn a() {}\npub fn b() {}\npub fn c() {}\npub fn d() {}\n",
    )]);
    h.call(
        "frontend",
        "record_changes",
        json!({"ws": work.workspace, "message": "Add d"}),
    )
    .await;
    h.call("frontend", "update_proposal", json!({"id": proposal}))
        .await;
    let since = h
        .call(
            "review-lead",
            "get_proposal_diff",
            json!({"id": proposal, "since": 1}),
        )
        .await;
    assert_eq!(since["suggested_scope"], "change", "{since}");

    let refused = h
        .refused(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": [proposal], "scopes": [{"proposal": proposal, "scope": "full", "reason": ""}]}),
        )
        .await;
    assert_eq!(refused["code"], codes::INVALID_REQUEST);
    let reason = "d is a public function of the crate";
    let started = h
        .call(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": [proposal], "scopes": [{"proposal": proposal, "scope": "full", "reason": reason}]}),
        )
        .await;
    assert_eq!(started["worker"]["name"], "reviewer", "{started}");
    let chosen: Vec<Value> = h
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "review.scope_chosen")
        .map(|event| json!({"actor": event.actor, "proposal": event.proposal, "data": event.data}))
        .collect();
    assert_eq!(
        chosen,
        [json!({
            "actor": "review-lead",
            "proposal": proposal,
            "data": {"proposal": proposal, "version": 2, "scope": "full", "reason": reason},
        })]
    );
    eventually(WAIT, async || {
        let row = h
            .handle
            .agents()
            .get_worker(&fold_core::AgentName::parse("reviewer").unwrap())
            .await
            .unwrap();
        if row.state == WorkerState::Idle {
            Ok(())
        } else {
            Err(row.state)
        }
    })
    .await;
    h.connect("reviewer").await;
    let view = h
        .call("reviewer", "get_proposal", json!({"id": proposal}))
        .await;
    assert_eq!(
        view["review_scope"],
        json!({"since": "v1", "chosen": {"scope": "full", "reason": reason, "by": "review-lead"}})
    );
    let claimed = h
        .refused(
            "reviewer",
            "claim_review",
            json!({"id": proposal, "scope": "change", "reason": "small"}),
        )
        .await;
    assert_eq!(claimed["code"], codes::WORKER_CLAIM_DENIED);
    h.call(
        "reviewer",
        "create_review_checkout",
        json!({"id": proposal}),
    )
    .await;
    h.call(
        "reviewer",
        "submit_review",
        json!({"id": proposal, "version": 2, "verdict": "approve", "body": "Read every file."}),
    )
    .await;
    let view = h
        .call("review-lead", "get_proposal", json!({"id": proposal}))
        .await;
    let reviews = view["reviews"].as_array().unwrap();
    assert_eq!(reviews[0]["scope"], Value::Null);
    assert_eq!(
        reviews[1]["scope"],
        json!({"scope": "full", "since": "v1", "suggested": "change", "rules": [], "reason": reason})
    );
    let submitted: Vec<Value> = h
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "review.submitted")
        .map(|event| event.data["scope"].clone())
        .collect();
    assert_eq!(submitted, [Value::Null, json!("full")]);
}

/// The review lead chooses the scope of a version after the assignment, as for a reviewer's own fix version; the newest choice applies, and only the review lead or the main agent chooses.
#[tokio::test]
async fn the_review_lead_sets_the_scope_of_a_later_version_and_the_newest_choice_applies() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = connect(&fx, "review").await;
    let reviewer = connect(&fx, "reviewer").await;
    let lead = connect(&fx, "web-lead").await;
    give_role(&fx.handle.store, "review", Role::ReviewLead, None).await;
    give_role(&fx.handle.store, "reviewer", Role::Reviewer, None).await;
    give_role(&fx.handle.store, "web-lead", Role::AreaLead, None).await;
    revised(&fx, &frontend, &review, &["pub fn g() {}\n"]).await;

    for (arguments, code) in [
        (
            json!({"id": "P-1", "version": 1, "scope": "full", "reason": "wide"}),
            codes::INVALID_REQUEST,
        ),
        (
            json!({"id": "P-1", "version": 9, "scope": "full", "reason": "wide"}),
            codes::PROPOSAL_NOT_FOUND,
        ),
        (
            json!({"id": "P-1", "version": 2, "scope": "full", "reason": " "}),
            codes::INVALID_REQUEST,
        ),
        (
            json!({"id": "P-1", "version": 2, "scope": "most", "reason": "wide"}),
            codes::INVALID_REQUEST,
        ),
    ] {
        let refused = err(&review, "set_review_scope", arguments.clone()).await;
        assert_eq!(refused["code"], code, "{arguments}");
    }
    let arguments = json!({"id": "P-1", "version": 2, "scope": "change", "reason": "small"});
    for client in [&reviewer, &lead, &frontend] {
        let refused = err(client, "set_review_scope", arguments.clone()).await;
        assert_eq!(refused["code"], codes::INTEGRATOR_REQUIRED, "{refused}");
    }

    let set = ok(
        &review,
        "set_review_scope",
        json!({"id": "P-1", "version": 2, "scope": "full", "reason": "the fix moved the parser"}),
    )
    .await;
    assert_eq!(set, json!({"id": "P-1", "version": "v2", "scope": "full"}));
    ok(
        &review,
        "set_review_scope",
        json!({"id": "P-1", "version": 2, "scope": "change", "reason": "the move kept every line"}),
    )
    .await;
    let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    assert_eq!(
        view["review_scope"],
        json!({"since": "v1", "chosen": {"scope": "change", "reason": "the move kept every line", "by": "review"}})
    );
    verdict(&review, "P-1", 2, "approve").await;
    assert_eq!(
        last_scope(&review, "P-1").await,
        json!({"scope": "change", "since": "v1", "suggested": "change", "rules": [], "reason": "the move kept every line"})
    );
    let mut conn = fx.handle.store.read().acquire().await.unwrap();
    let chosen: Vec<(String, String)> =
        sqlx::query_as("SELECT scope, reason FROM review_scope_choices ORDER BY id")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        chosen,
        [
            ("full".to_owned(), "the fix moved the parser".to_owned()),
            ("change".to_owned(), "the move kept every line".to_owned()),
        ]
    );
    let events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE type = 'review.scope_chosen' AND json_extract(data, '$.version') = 2",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(events, 2);
}

/// The review lead gives two revised proposals to one reviewer with a scope for one of them. A scope for a version with no earlier verdict refuses the whole start, and the main agent chooses a scope too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reviewer_batch_takes_the_chosen_scopes_and_a_refused_scope_starts_nothing() {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main("frontend").await;
    h.call("frontend", "start_worker", json!({"role": "review_lead"}))
        .await;
    h.connect("review-lead").await;
    let mut proposals = Vec::new();
    for (title, path) in [
        ("Add a", "src/a.rs"),
        ("Add b", "src/b.rs"),
        ("Add c", "src/c.rs"),
    ] {
        let task = h.create_task("frontend", title).await;
        let work = h.start_work("frontend", &task).await;
        work.write(&[(path, "pub fn a() {}\npub fn b() {}\npub fn c() {}\n")]);
        let proposal = h.propose("frontend", &work, title).await;
        proposals.push((proposal, work, path));
    }
    for (proposal, work, path) in &proposals[..2] {
        h.call(
            "review-lead",
            "submit_review",
            json!({"id": proposal, "version": 1, "verdict": "request_changes", "body": "add d"}),
        )
        .await;
        work.write(&[(
            path,
            "pub fn a() {}\npub fn b() {}\npub fn c() {}\npub fn d() {}\n",
        )]);
        h.call(
            "frontend",
            "record_changes",
            json!({"ws": work.workspace, "message": "Add d"}),
        )
        .await;
        h.call("frontend", "update_proposal", json!({"id": proposal}))
            .await;
    }
    let [(first, ..), (second, ..), (fresh, ..)] = proposals.as_slice() else {
        panic!("three proposals");
    };

    let refused = h
        .refused(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": [first, fresh], "scopes": [{"proposal": fresh, "scope": "full", "reason": "a new module"}]}),
        )
        .await;
    assert_eq!(refused["code"], codes::INVALID_REQUEST, "{refused}");
    assert_eq!(refused["details"]["field"], "scope");
    for proposal in [first, fresh] {
        let view = h
            .call("review-lead", "get_proposal", json!({"id": proposal}))
            .await;
        assert_eq!(view["claim"], Value::Null, "a refused start claims nothing");
    }
    let kinds = |events: &[StoredEvent], kind: &str| {
        events.iter().filter(|event| event.kind == kind).count()
    };
    let events = h.events().await;
    assert_eq!(kinds(&events, "worker.started"), 1, "only the review lead");
    assert_eq!(kinds(&events, "review.batch_created"), 0);
    assert_eq!(kinds(&events, "review.scope_chosen"), 0);

    let reason = "d is a public function of the crate";
    h.call(
        "review-lead",
        "start_worker",
        json!({"role": "reviewer", "proposals": [first, second], "scopes": [{"proposal": first, "scope": "full", "reason": reason}]}),
    )
    .await;
    let events = h.events().await;
    assert_eq!(kinds(&events, "review.batch_created"), 1);
    let chosen: Vec<Value> = events
        .iter()
        .filter(|event| event.kind == "review.scope_chosen")
        .map(|event| json!({"actor": event.actor, "proposal": event.proposal, "data": event.data}))
        .collect();
    assert_eq!(
        chosen,
        [json!({
            "actor": "review-lead",
            "proposal": first,
            "data": {"proposal": first, "version": 2, "scope": "full", "reason": reason},
        })]
    );
    let view = h
        .call("review-lead", "get_proposal", json!({"id": second}))
        .await;
    assert_eq!(
        view["review_scope"],
        json!({"since": "v1", "chosen": null}),
        "the other member follows the suggestion"
    );

    h.call(
        "frontend",
        "set_review_scope",
        json!({"id": second, "version": 2, "scope": "full", "reason": "the user asked for a full review"}),
    )
    .await;
    let view = h
        .call("review-lead", "get_proposal", json!({"id": second}))
        .await;
    assert_eq!(
        view["review_scope"]["chosen"],
        json!({"scope": "full", "reason": "the user asked for a full review", "by": "frontend"})
    );
}

/// The review lead assigns only agents' reviews, so a human's review of a revised version has no scope: not in get_proposal, the HTTP view or review.submitted, even when the review lead chose one for the version.
#[tokio::test]
async fn a_human_review_of_a_revised_version_has_no_scope() {
    let fx = Fixture::new().await;
    let frontend = connect(&fx, "frontend").await;
    let review = connect(&fx, "review").await;
    give_role(&fx.handle.store, "review", Role::ReviewLead, None).await;
    revised(&fx, &frontend, &review, &["pub fn g() {}\n"]).await;
    ok(
        &review,
        "set_review_scope",
        json!({"id": "P-1", "version": 2, "scope": "full", "reason": "the parser moved"}),
    )
    .await;
    verdict(&review, "P-1", 2, "approve").await;
    http(
        &fx,
        Method::POST,
        "/proposals/P-1/reviews",
        Some(json!({"version": 2, "verdict": "approve", "body": "Read it."})),
    )
    .await;

    let view = ok(&review, "get_proposal", json!({"id": "P-1"})).await;
    let scopes: Vec<Value> = view["reviews"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| json!([line["reviewer"], line.get("scope")]))
        .collect();
    assert_eq!(
        scopes,
        [
            json!(["review", null]),
            json!(["review", {"scope": "full", "since": "v1", "suggested": "change", "rules": [], "reason": "the parser moved"}]),
            json!(["human:tanim", null]),
        ]
    );
    let detail = http(&fx, Method::GET, "/proposals/P-1", None).await;
    let scopes: Vec<Value> = detail["reviews"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["scope"]["scope"].clone())
        .collect();
    assert_eq!(scopes, [Value::Null, json!("full"), Value::Null]);
    let mut conn = fx.handle.store.read().acquire().await.unwrap();
    let submitted: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT actor, json_extract(data, '$.scope') FROM events WHERE type = 'review.submitted' ORDER BY seq",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(submitted[1].1.as_deref(), Some("full"), "{submitted:?}");
    let human = submitted.last().unwrap();
    assert!(human.0.contains("tanim"), "{submitted:?}");
    assert_eq!(human.1, None);
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_scopes")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(rows, 1);
}
