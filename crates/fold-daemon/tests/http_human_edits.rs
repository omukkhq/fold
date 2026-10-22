#![cfg(test)]
use crate::common::roles::give_role;
use crate::proposal_tools_support::{Fixture, t};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::reviews::Verdict;
use fold_core::roles::Role;
use fold_core::{HumanName, ProposalId};
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::{openapi, socket_router};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common::signed_in_router;

const BASE: &str = "/api/v1/projects/demo";
const TANIM: &str = "tanim";
const ANA: &str = "ana";
const EDITED: &str = "pub fn y() {}\n// checked\n";

/// Sends the request through the TCP router signed in as `human`, or over the socket without a human, the only transport where a request names none.
async fn send(
    fx: &Fixture,
    method: Method,
    path: &str,
    body: Option<Value>,
    human: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(method)
        .uri(format!("{BASE}{path}"));
    let body = match body {
        Some(value) => {
            request = request.header("content-type", "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let router = match human {
        Some(human) => signed_in_router(&fx.state, human).await,
        None => socket_router(fx.state.clone()),
    };
    let response = router.oneshot(request.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

fn code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap_or_default()
}

/// `frontend` opens P-1 on T-1, changing `src/lib.rs`; `review` is a reviewer and approved v1; the task's human review mark names `tanim`.
async fn marked() -> Fixture {
    let fx = Fixture::new().await;
    let frontend = fx.agent("frontend").await;
    fx.proposal(&frontend, 1, "pub fn y() {}\n").await;
    let review = fx.agent("review").await;
    give_role(&fx.handle.store, "review", Role::Reviewer, None).await;
    // A human is known once a request named them.
    send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
    send(&fx, Method::GET, "/proposals/P-1", None, Some(ANA)).await;
    fx.reviews()
        .require_human_review(
            &fx.agent("planner").await,
            t(1),
            Some(HumanName::parse("tanim").unwrap()),
            "schema work",
        )
        .await
        .unwrap();
    fx.reviews()
        .submit_review(
            &review,
            ProposalId::from_number(1).unwrap(),
            1,
            Verdict::Approve,
            "fine",
            Vec::new(),
        )
        .await
        .unwrap();
    fx
}

fn edit(version: u32, content: &str) -> Option<Value> {
    Some(json!({"version": version, "path": "src/lib.rs", "content": content}))
}

#[tokio::test]
async fn the_marked_human_reads_edits_and_sends_a_file_for_review_again() {
    let fx = marked().await;
    let (status, file) = send(
        &fx,
        Method::GET,
        "/proposals/P-1/file?path=src/lib.rs",
        None,
        Some(TANIM),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file["text"], "pub fn y() {}\n");
    assert_eq!(file["kind"], "file");

    let (status, version) = send(
        &fx,
        Method::POST,
        "/proposals/P-1/edits",
        edit(1, EDITED),
        Some(TANIM),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{version}");
    assert_eq!(version["version"], 2);
    assert_eq!(version["created_by"], "human:tanim");
    assert_eq!(version["human_edit"], true);
    assert_eq!(version["review_again"], false);
    assert_eq!(version["files_modified"], 1);

    let (_, detail) = send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
    assert_eq!(detail["summary"]["current_version"], 2);
    assert_eq!(detail["summary"]["latest_verdict"], "approve");
    assert_eq!(detail["agent_approved"], true);
    assert_eq!(detail["versions"][0]["human_edit"], false);
    let kept = &detail["reviews"][1];
    assert_eq!(kept["version"], 2);
    assert_eq!(kept["kept_from"], 1);
    assert_eq!(detail["reviews"][0]["kept_from"], Value::Null);
    let (_, file) = send(
        &fx,
        Method::GET,
        "/proposals/P-1/file?path=src/lib.rs&version=2",
        None,
        None,
    )
    .await;
    assert_eq!(file["text"], EDITED);

    let (status, stale) = send(
        &fx,
        Method::POST,
        "/proposals/P-1/review-again",
        Some(json!({"version": 1})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (status, code(&stale)),
        (StatusCode::CONFLICT, "stale_version")
    );
    let (status, summary) = send(
        &fx,
        Method::POST,
        "/proposals/P-1/review-again",
        Some(json!({"version": 2})),
        Some(TANIM),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["latest_verdict"], Value::Null);
    assert_eq!(summary["turn"], "with_reviewer");
    let (status, again) = send(
        &fx,
        Method::POST,
        "/proposals/P-1/review-again",
        Some(json!({"version": 2})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (
            status,
            code(&again),
            again["error"]["details"]["reason"].clone()
        ),
        (
            StatusCode::BAD_REQUEST,
            "invalid_request",
            json!("already_sent")
        )
    );
    let (_, detail) = send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
    assert_eq!(detail["versions"][1]["review_again"], true);
    assert_eq!(detail["agent_approved"], false);
}

#[tokio::test]
async fn edit_refusals_carry_their_declared_status() {
    let fx = marked().await;
    let cases = [
        (
            edit(1, EDITED),
            Some(ANA),
            StatusCode::FORBIDDEN,
            "human_edit_denied",
        ),
        (
            edit(2, EDITED),
            Some(TANIM),
            StatusCode::CONFLICT,
            "stale_version",
        ),
        (
            edit(1, "pub fn y() {}\n"),
            Some(TANIM),
            StatusCode::CONFLICT,
            "version_unchanged",
        ),
        (
            Some(json!({"version": 1, "path": "../x", "content": "a"})),
            Some(TANIM),
            StatusCode::BAD_REQUEST,
            "invalid_path",
        ),
        (
            Some(json!({"version": 1, "path": "src/none.rs", "content": "a"})),
            Some(TANIM),
            StatusCode::UNPROCESSABLE_ENTITY,
            "file_not_editable",
        ),
        (
            edit(1, EDITED),
            None,
            StatusCode::UNAUTHORIZED,
            "human_name_required",
        ),
    ];
    for (body, human, status, expected) in cases {
        let (got, answer) = send(&fx, Method::POST, "/proposals/P-1/edits", body, human).await;
        assert_eq!((got, code(&answer)), (status, expected), "{answer}");
        assert_declared("/edits", "post", got);
        assert!(!answer.to_string().contains("checked"), "{answer}");
    }
    send(
        &fx,
        Method::POST,
        "/proposals/P-1/edits",
        edit(1, EDITED),
        Some(TANIM),
    )
    .await;
    // The worker has not synced the first edit yet; a second edit replaces the write that waits.
    let (got, second) = send(
        &fx,
        Method::POST,
        "/proposals/P-1/edits",
        edit(2, "x\n"),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (got, second["version"].clone()),
        (StatusCode::CREATED, json!(3))
    );
    let (got, denied) = send(
        &fx,
        Method::POST,
        "/proposals/P-1/review-again",
        Some(json!({"version": 3})),
        Some(ANA),
    )
    .await;
    assert_eq!(
        (got, code(&denied)),
        (StatusCode::FORBIDDEN, "human_edit_denied")
    );
    assert_declared("/review-again", "post", got);
    let (got, missing) = send(
        &fx,
        Method::GET,
        "/proposals/P-1/file?path=src/none.rs",
        None,
        None,
    )
    .await;
    assert_eq!((got, code(&missing)), (StatusCode::NOT_FOUND, "not_found"));
    assert_declared("/file", "get", got);
}

#[test]
fn openapi_declares_the_edit_routes() {
    let doc = serde_json::to_value(openapi()).unwrap();
    for (path, method, operation, body, answer, ok) in [
        (
            "/file",
            "get",
            "read_proposal_file",
            None,
            "FileContentView",
            "200",
        ),
        (
            "/edits",
            "post",
            "edit_proposal_file",
            Some("EditFileBody"),
            "ProposalVersion",
            "201",
        ),
        (
            "/review-again",
            "post",
            "send_for_review_again",
            Some("ReviewAgainBody"),
            "ProposalSummary",
            "200",
        ),
    ] {
        let found =
            &doc["paths"][format!("/api/v1/projects/{{slug}}/proposals/{{id}}{path}")][method];
        assert_eq!(found["operationId"], operation);
        assert_eq!(
            found["responses"][ok]["content"]["application/json"]["schema"]["$ref"],
            format!("#/components/schemas/{answer}")
        );
        if let Some(body) = body {
            assert_eq!(
                found["requestBody"]["content"]["application/json"]["schema"]["$ref"],
                format!("#/components/schemas/{body}")
            );
        }
    }
}

/// Fails unless `status` is a declared response of the operation at `path` under a proposal.
fn assert_declared(path: &str, method: &str, status: StatusCode) {
    let doc = serde_json::to_value(openapi()).unwrap();
    let operation =
        &doc["paths"][format!("/api/v1/projects/{{slug}}/proposals/{{id}}{path}")][method];
    let response = &operation["responses"][status.as_str()];
    assert!(
        response.is_object(),
        "{method} {path} does not declare {status}"
    );
    assert_eq!(
        response["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/ErrorEnvelope"
    );
}
