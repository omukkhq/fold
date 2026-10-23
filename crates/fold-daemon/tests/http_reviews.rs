#![cfg(test)]

use crate::common::roles::give_role;
use crate::proposal_tools_support::{Fixture, human, t};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::roles::Role;
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::{openapi, socket_router};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common::signed_in_router;

const BASE: &str = "/api/v1/projects/demo";
const TANIM: &str = "tanim";
const ANA: &str = "ana";

#[cfg(test)]
struct Answer {
    status: StatusCode,
    body: Value,
}

#[cfg(test)]
impl Answer {
    fn code(&self) -> &str {
        self.body["error"]["code"].as_str().unwrap_or_default()
    }
}

#[cfg(test)]
async fn send_to(
    fx: &Fixture,
    method: Method,
    uri: &str,
    body: Option<Value>,
    human: Option<&str>,
) -> Answer {
    let mut request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(method)
        .uri(uri);
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
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    Answer { status, body }
}

#[cfg(test)]
async fn send(
    fx: &Fixture,
    method: Method,
    path: &str,
    body: Option<Value>,
    human: Option<&str>,
) -> Answer {
    send_to(fx, method, &format!("{BASE}{path}"), body, human).await
}

/// `frontend` opens P-1 on T-1, changing `src/lib.rs`.
#[cfg(test)]
async fn with_p1() -> Fixture {
    let fx = Fixture::new().await;
    let frontend = fx.agent("frontend").await;
    fx.proposal(&frontend, 1, "pub fn y() {}\n").await;
    fx
}

#[tokio::test]
async fn list_proposals_returns_summaries_with_the_mark() {
    let fx = with_p1().await;
    fx.reviews()
        .require_human_review(&fx.agent("planner").await, t(1), None, "touches auth")
        .await
        .unwrap();
    let answer = send(&fx, Method::GET, "/proposals?state=open", None, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let row = &answer.body["proposals"][0];
    assert_eq!(row["id"], "P-1");
    assert_eq!(row["task"], "T-1");
    assert_eq!(row["author"], "frontend");
    assert_eq!(row["state"], "open");
    assert_eq!(row["turn"], "with_reviewer");
    assert_eq!(row["current_version"], 1);
    assert_eq!(row["review_claim"], Value::Null);
    assert_eq!(
        row["human_review"],
        json!({"reviewer": null, "reason": "touches auth", "set_by": "planner"})
    );
    let answer = send(&fx, Method::GET, "/proposals?unclaimed=true", None, None).await;
    assert_eq!(answer.body["proposals"].as_array().unwrap().len(), 1);
    let answer = send(&fx, Method::GET, "/proposals?state=merged", None, None).await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
}

#[tokio::test]
async fn an_agent_approval_waits_for_the_human_the_mark_names() {
    let fx = with_p1().await;
    let p1 = fold_core::ProposalId::from_number(1).unwrap();
    let review = fx.agent("review").await;
    give_role(&fx.handle.store, "review", Role::Reviewer, None).await;
    // A human is known once a request named them.
    send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
    fx.reviews()
        .require_human_review(
            &fx.agent("planner").await,
            t(1),
            Some(fold_core::HumanName::parse("tanim").unwrap()),
            "schema work",
        )
        .await
        .unwrap();
    let row = |answer: &Answer| answer.body["proposals"][0].clone();
    let answer = send(&fx, Method::GET, "/proposals", None, None).await;
    assert_eq!(row(&answer)["latest_verdict"], Value::Null);
    assert_eq!(row(&answer)["human_approved"], false);
    fx.reviews()
        .submit_review(
            &review,
            p1,
            1,
            fold_core::reviews::Verdict::Approve,
            "fine",
            Vec::new(),
        )
        .await
        .unwrap();
    let answer = send(&fx, Method::GET, "/proposals", None, None).await;
    assert_eq!(row(&answer)["latest_verdict"], "approve");
    assert_eq!(row(&answer)["human_approved"], false);
    assert_eq!(answer.body["waiting_for_you"], json!([]), "no human named");
    let waiting = async |human: &'static str| {
        send(
            &fx,
            Method::GET,
            "/proposals?state=closed",
            None,
            Some(human),
        )
        .await
        .body["waiting_for_you"]
            .clone()
    };
    assert_eq!(
        waiting(TANIM).await,
        json!(["P-1"]),
        "whatever the filter, P-1 waits for the human the mark names"
    );
    assert_eq!(waiting(ANA).await, json!([]));
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/reviews",
        Some(json!({"version": 1, "verdict": "approve", "body": "seen"})),
        Some(ANA),
    )
    .await;
    assert_eq!(answer.status, StatusCode::CREATED, "{}", answer.body);
    assert_eq!(answer.body["summary"]["human_approved"], false);
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/reviews",
        Some(json!({"version": 1, "verdict": "approve", "body": "seen"})),
        Some(TANIM),
    )
    .await;
    assert_eq!(answer.body["summary"]["human_approved"], true);
    assert_eq!(waiting(TANIM).await, json!([]), "tanim's approval stands");
    let detail = send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
    assert_eq!(detail.body["summary"]["latest_verdict"], "approve");
    assert_eq!(detail.body["summary"]["human_approved"], true);
    fx.reviews()
        .submit_review(
            &review,
            p1,
            1,
            fold_core::reviews::Verdict::RequestChanges,
            "one more thing",
            Vec::new(),
        )
        .await
        .unwrap();
    let answer = send(&fx, Method::GET, "/proposals", None, None).await;
    assert_eq!(row(&answer)["latest_verdict"], "request_changes");
    assert_eq!(row(&answer)["human_approved"], false);
}

#[tokio::test]
async fn a_proposal_without_a_mark_counts_as_human_approved() {
    let fx = with_p1().await;
    let answer = send(&fx, Method::GET, "/proposals", None, None).await;
    assert_eq!(answer.body["proposals"][0]["human_approved"], true);
}

#[tokio::test]
async fn the_proposal_diff_reads_the_change_since_an_earlier_version() {
    let fx = with_p1().await;
    let p1 = fold_core::ProposalId::from_number(1).unwrap();
    fx.reviews()
        .submit_review(
            &human("tanim"),
            p1,
            1,
            fold_core::reviews::Verdict::RequestChanges,
            "add z",
            Vec::new(),
        )
        .await
        .unwrap();
    fx.write(1, "src/lib.rs", "pub fn y() {}\npub fn z() {}\n");
    let frontend = fx.agent("frontend").await;
    fx.reviews()
        .update_proposal(&frontend, p1, None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    let stale = send(
        &fx,
        Method::POST,
        "/proposals/P-1/comments",
        Some(json!({"version": 1, "body": "on what I read"})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (stale.status, stale.code()),
        (StatusCode::CONFLICT, "stale_version")
    );
    let stale = send(
        &fx,
        Method::POST,
        "/proposals/P-1/reviews",
        Some(json!({"version": 1, "verdict": "approve", "body": "fine"})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (stale.status, stale.code()),
        (StatusCode::CONFLICT, "stale_version")
    );

    let answer = send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
    assert_eq!(answer.body["last_reviewed_version"], 1, "{}", answer.body);
    let answer = send(&fx, Method::GET, "/proposals/P-1?version=1", None, None).await;
    assert_eq!(
        answer.body["last_reviewed_version"],
        Value::Null,
        "{}",
        answer.body
    );

    let answer = send(&fx, Method::GET, "/proposals/P-1/diff?since=1", None, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let since = &answer.body;
    assert_eq!(since["view"], "files");
    assert_eq!(since["source"], "recorded");
    assert_eq!(since["from"]["kind"], "changeset");
    assert_eq!(since["from"]["task"], "T-1");
    assert_eq!(since["to"]["kind"], "changeset");
    assert_ne!(since["from"]["id"], since["to"]["id"]);
    assert_eq!(since["files"][0]["path"], "src/lib.rs");
    assert_eq!(
        (
            &since["files"][0]["added_lines"],
            &since["files"][0]["removed_lines"]
        ),
        (&json!(1), &json!(0))
    );

    let answer = send(
        &fx,
        Method::GET,
        "/proposals/P-1/diff?version=2",
        None,
        None,
    )
    .await;
    assert_eq!(
        answer.body["from"],
        json!({"kind": "revision", "id": "r1", "base": null, "task": null, "worker": null})
    );
    assert_eq!(
        answer.body["totals"],
        json!({"files": 1, "added_lines": 2, "removed_lines": 1})
    );

    let answer = send(
        &fx,
        Method::GET,
        "/proposals/P-1/diff?since=1&path=src/lib.rs",
        None,
        None,
    )
    .await;
    assert_eq!(answer.body["view"], "file", "{}", answer.body);
    let lines = &answer.body["hunks"][0]["lines"];
    assert_eq!(lines[0]["kind"], "context");
    assert_eq!(lines[1]["kind"], "added");
    assert_eq!(lines[1]["new_no"], 2);
    assert_eq!(lines.as_array().unwrap().len(), 2);

    for (query, status, code) in [
        ("since=2", StatusCode::BAD_REQUEST, "invalid_request"),
        (
            "since=1&version=1",
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            "since=7&version=9",
            StatusCode::NOT_FOUND,
            "proposal_not_found",
        ),
        (
            "since=1&path=README.md",
            StatusCode::NOT_FOUND,
            "path_not_in_comparison",
        ),
    ] {
        let answer = send(
            &fx,
            Method::GET,
            &format!("/proposals/P-1/diff?{query}"),
            None,
            None,
        )
        .await;
        assert_eq!((answer.status, answer.code()), (status, code), "{query}");
    }
    let answer = send(&fx, Method::GET, "/proposals/P-4/diff", None, None).await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::NOT_FOUND, "proposal_not_found")
    );
}

/// The worker syncs with a newer main between versions: the HTTP change since v1 holds only the worker's own edit, never the file main gained, and every read of it shares one carry of v1 onto the new base.
#[tokio::test]
async fn the_proposal_diff_since_an_earlier_version_leaves_out_what_main_gained() {
    let fx = with_p1().await;
    let (p1, p2) = (
        fold_core::ProposalId::from_number(1).unwrap(),
        fold_core::ProposalId::from_number(2).unwrap(),
    );
    fx.reviews()
        .submit_review(
            &human("tanim"),
            p1,
            1,
            fold_core::reviews::Verdict::RequestChanges,
            "add z",
            Vec::new(),
        )
        .await
        .unwrap();
    let backend = fx.agent("backend").await;
    fx.workspaces()
        .create_workspace(&backend, t(2))
        .await
        .unwrap();
    fx.write(2, "docs/guide.md", "guide\n");
    fx.reviews()
        .create_proposal(&backend, t(2), "Add a guide", None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    let review = fx.agent("review").await;
    give_role(&fx.handle.store, "review", Role::Reviewer, None).await;
    fx.reviews()
        .submit_review(
            &review,
            p2,
            1,
            fold_core::reviews::Verdict::Approve,
            "good",
            Vec::new(),
        )
        .await
        .unwrap();
    fx.reviews()
        .integrate_proposal(&review, p2, None)
        .await
        .unwrap();
    let frontend = fx.agent("frontend").await;
    let synced = fx
        .workspaces()
        .sync_workspace(&frontend, fold_core::WorkspaceId::from_number(1).unwrap())
        .await
        .unwrap();
    assert_eq!(synced.base.to_string(), "r2");
    fx.write(1, "src/lib.rs", "pub fn y() {}\npub fn z() {}\n");
    fx.reviews()
        .update_proposal(&frontend, p1, None)
        .await
        .unwrap()
        .detail()
        .unwrap();

    let answer = send(&fx, Method::GET, "/proposals/P-1/diff?since=1", None, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let paths: Vec<&Value> = answer.body["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| &file["path"])
        .collect();
    assert_eq!(paths, [&json!("src/lib.rs")]);
    assert_eq!(
        answer.body["totals"],
        json!({"files": 1, "added_lines": 1, "removed_lines": 0})
    );
    let answer = send(
        &fx,
        Method::GET,
        "/proposals/P-1/diff?since=1&path=src/lib.rs",
        None,
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let lines = &answer.body["hunks"][0]["lines"];
    assert_eq!(lines.as_array().unwrap().len(), 2);
    assert_eq!(
        (&lines[1]["kind"], &lines[1]["new_no"]),
        (&json!("added"), &json!(2))
    );
    let answer = send(
        &fx,
        Method::GET,
        "/proposals/P-1/diff?since=1&path=docs/guide.md",
        None,
        None,
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::NOT_FOUND, "path_not_in_comparison")
    );
    assert_eq!(fx.handle.carries.misses(), 1);
    assert_eq!(fx.handle.carries.hits(), 2);

    let answer = send(
        &fx,
        Method::GET,
        "/proposals/P-1/diff?version=2",
        None,
        None,
    )
    .await;
    assert_eq!(answer.body["from"]["id"], "r2");
    assert_eq!(
        answer.body["totals"],
        json!({"files": 1, "added_lines": 2, "removed_lines": 1})
    );
}

#[tokio::test]
async fn get_proposal_reports_the_gates_for_the_requesting_human() {
    let fx = with_p1().await;
    let answer = send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let detail = &answer.body;
    assert_eq!(detail["summary"]["id"], "P-1");
    assert_eq!(detail["task_status"], "review");
    assert_eq!(detail["task"]["title"], "report");
    assert_eq!(detail["selected"]["version"], 1);
    assert_eq!(detail["selected"]["base"], "r1");
    assert_eq!(detail["selected"]["evidence"], "tests pass");
    assert_eq!(detail["versions"].as_array().unwrap().len(), 1);
    assert_eq!(detail["conflict"], Value::Null);
    assert_eq!(detail["review_claim"], Value::Null);
    assert_eq!(detail["files"][0]["path"], "src/lib.rs");
    assert_eq!(detail["files"][0]["change"], "modified");
    assert_eq!(detail["totals"]["files"], 1);
    let gates: Vec<(String, String)> = detail["gates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|gate| {
            (
                gate["gate"].as_str().unwrap().to_owned(),
                gate["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let expected: Vec<(String, String)> = [
        ("open", "met"),
        ("version", "met"),
        ("caller", "met"),
        ("approval", "met"),
        ("human_review", "met"),
        ("prerequisites", "met"),
        ("merge", "checked_at_integration"),
    ]
    .iter()
    .map(|(gate, status)| ((*gate).to_owned(), (*status).to_owned()))
    .collect();
    assert_eq!(gates, expected);

    let answer = send(&fx, Method::GET, "/proposals/P-1", None, None).await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::UNAUTHORIZED, "human_name_required")
    );
    let answer = send(&fx, Method::GET, "/proposals/P-9", None, Some(TANIM)).await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::NOT_FOUND, "proposal_not_found")
    );
    let answer = send(
        &fx,
        Method::GET,
        "/proposals/P-1?version=4",
        None,
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::NOT_FOUND, "proposal_not_found")
    );
    let answer = send(&fx, Method::GET, "/proposals/nine", None, Some(TANIM)).await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::BAD_REQUEST, "invalid_id")
    );
}

#[tokio::test]
async fn a_human_comment_is_added_and_listed() {
    let fx = with_p1().await;
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/comments",
        Some(json!({"version": 1, "path": "src/lib.rs", "side": "new", "start_line": 1, "end_line": 1, "body": "why y"})),
        Some(TANIM),
    )
    .await;
    assert_eq!(answer.status, StatusCode::CREATED, "{}", answer.body);
    assert_eq!(
        answer.body,
        json!({
            "id": "rc-1",
            "version": 1,
            "verdict": "comment",
            "author": "human:tanim",
            "path": "src/lib.rs",
            "side": "new",
            "start_line": 1,
            "end_line": 1,
            "reply_to": null,
            "body": "why y",
            "created_at": answer.body["created_at"],
            "outdated": false,
            "batch_comment": null,
        })
    );
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/comments",
        Some(json!({"version": 1, "reply_to": "rc-1", "body": "because"})),
        Some(ANA),
    )
    .await;
    assert_eq!(answer.status, StatusCode::CREATED, "{}", answer.body);
    assert_eq!(answer.body["reply_to"], "rc-1");
    assert_eq!(answer.body["path"], Value::Null);

    let answer = send(&fx, Method::GET, "/proposals/P-1/comments", None, None).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.body["comments"].as_array().unwrap().len(), 2);
    let answer = send(
        &fx,
        Method::GET,
        "/proposals/P-1/comments?version=1",
        None,
        None,
    )
    .await;
    assert_eq!(answer.body["comments"].as_array().unwrap().len(), 2);
    let answer = send(
        &fx,
        Method::GET,
        "/proposals/P-1/comments?version=3",
        None,
        None,
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::NOT_FOUND, "proposal_not_found")
    );

    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/comments",
        Some(json!({"version": 1, "path": "src/lib.rs", "side": "new", "start_line": 5, "end_line": 6, "body": "far"})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid_comment_range")
    );
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/comments",
        Some(json!({"version": 1, "path": "src/lib.rs", "body": "no lines"})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/comments",
        Some(json!({"version": 1, "body": "anonymous"})),
        None,
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::UNAUTHORIZED, "human_name_required")
    );
}

#[tokio::test]
async fn a_human_verdict_moves_the_turn() {
    let fx = with_p1().await;
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/reviews",
        Some(json!({"version": 1, "verdict": "comment", "body": ""})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::UNPROCESSABLE_ENTITY, "empty_review")
    );
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/reviews",
        Some(json!({"version": 1, "verdict": "request_changes", "body": "rename back", "comments": [
            {"path": "src/lib.rs", "side": "old", "start_line": 1, "end_line": 1, "body": "keep x"},
        ]})),
        Some(TANIM),
    )
    .await;
    assert_eq!(answer.status, StatusCode::CREATED, "{}", answer.body);
    assert_eq!(answer.body["proposal"], "P-1");
    assert_eq!(answer.body["version"], 1);
    assert_eq!(answer.body["verdict"], "request_changes");
    assert_eq!(answer.body["comments"], json!(["rc-1"]));
    assert_eq!(answer.body["summary"]["turn"], "with_author");
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/reviews",
        Some(json!({"version": 1, "verdict": "approve", "body": "fine after all"})),
        Some(ANA),
    )
    .await;
    assert_eq!(answer.status, StatusCode::CREATED, "{}", answer.body);
    assert_eq!(answer.body["summary"]["turn"], "with_reviewer");
    let detail = send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
    assert_eq!(detail.body["reviews"].as_array().unwrap().len(), 2);
    assert_eq!(detail.body["reviews"][0]["reviewer"], "human:tanim");
    assert_eq!(detail.body["reviews"][0]["comments"], 1);
    assert_eq!(detail.body["comments"]["total"], 1);
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/reviews",
        Some(json!({"version": 1, "verdict": "reject", "body": "not this way"})),
        Some(TANIM),
    )
    .await;
    assert_eq!(answer.status, StatusCode::CREATED);
    assert_eq!(answer.body["summary"]["state"], "closed");
    assert_eq!(answer.body["summary"]["close_reason"], "not this way");
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/reviews",
        Some(json!({"version": 1, "verdict": "approve", "body": "late"})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::CONFLICT, "proposal_not_open")
    );
}

#[tokio::test]
async fn human_integrates_through_the_route_with_a_session_cookie() {
    let fx = with_p1().await;
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/integrate",
        Some(json!({"expected_version": 1})),
        Some(TANIM),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert_eq!(
        answer.body,
        json!({"proposal": "P-1", "result": "succeeded", "revision": "r2", "run": null})
    );
    let detail = send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
    assert_eq!(detail.body["summary"]["state"], "integrated");
    assert_eq!(detail.body["task_status"], "done");
    assert_eq!(detail.body["reviews"][0]["verdict"], "approve");
    assert_eq!(detail.body["reviews"][0]["reviewer"], "human:tanim");
}

#[tokio::test]
async fn human_integrate_without_the_cookie_is_refused() {
    let fx = with_p1().await;
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/integrate",
        Some(json!({"expected_version": 1})),
        None,
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::UNAUTHORIZED, "human_name_required")
    );
    let answer = send(&fx, Method::GET, "/proposals?state=integrated", None, None).await;
    assert_eq!(answer.body["proposals"], json!([]));
}

#[tokio::test]
async fn integrate_with_a_stale_expected_version_gives_stale_version() {
    let fx = with_p1().await;
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/integrate",
        Some(json!({"expected_version": 2})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::CONFLICT, "stale_version")
    );
    assert_eq!(answer.body["error"]["details"]["expected_version"], 2);
    assert_eq!(answer.body["error"]["details"]["current_version"], 1);
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/integrate",
        Some(json!({})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
}

#[tokio::test]
async fn integrate_refusals_answer_with_their_gate_codes() {
    let fx = with_p1().await;
    fx.reviews()
        .require_human_review(
            &human("tanim"),
            t(1),
            Some(fold_core::HumanName::parse("tanim").unwrap()),
            "touches auth",
        )
        .await
        .unwrap();
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/integrate",
        Some(json!({"expected_version": 1})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::CONFLICT, "agent_approval_required"),
        "a human integrates a marked task only after a reviewer agent approved it"
    );
    let review = fx.agent("review").await;
    give_role(&fx.handle.store, "review", Role::Reviewer, None).await;
    fx.reviews()
        .submit_review(
            &review,
            fold_core::ProposalId::from_number(1).unwrap(),
            1,
            fold_core::reviews::Verdict::Approve,
            "fine",
            Vec::new(),
        )
        .await
        .unwrap();
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/integrate",
        Some(json!({"expected_version": 1})),
        Some(ANA),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::CONFLICT, "human_approval_missing")
    );
    assert_eq!(answer.body["error"]["details"]["reviewer"], "tanim");
    fx.handle
        .tasks
        .add_dependency(&human("tanim"), t(1), t(2))
        .await
        .unwrap();
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/integrate",
        Some(json!({"expected_version": 1})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::CONFLICT, "prerequisites_not_done")
    );
    assert_eq!(
        answer.body["error"]["details"]["prerequisites"],
        json!([{"task": "T-2", "status": "todo"}])
    );
}

#[tokio::test]
async fn a_conflicting_integration_answers_integration_conflicted() {
    let fx = with_p1().await;
    let backend = fx.agent("backend").await;
    fx.proposal(&backend, 2, "pub fn w() {}\n").await;
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-2/integrate",
        Some(json!({"expected_version": 1})),
        Some(TANIM),
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let answer = send(
        &fx,
        Method::POST,
        "/proposals/P-1/integrate",
        Some(json!({"expected_version": 1})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::CONFLICT, "integration_conflicted")
    );
    let details = &answer.body["error"]["details"];
    assert_eq!(details["proposal"], "P-1");
    assert_eq!(
        details["files"],
        json!([{"path": "src/lib.rs", "kind": "text"}])
    );
    assert!(details["hint"].as_str().unwrap().contains("get_proposal"));
    let detail = send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
    let report = &detail.body["conflict"];
    assert_eq!(report["proposal"], "P-1");
    assert_eq!(report["version"], 1);
    assert_eq!(report["base_revision"], "r1");
    assert_eq!(report["main_revision"], "r2");
    let file = &report["files"][0];
    assert_eq!(file["path"], "src/lib.rs");
    assert_eq!(file["kind"], "text");
    assert_eq!(file["base"]["kind"], "file");
    let hunk = &file["hunks"][0];
    assert_eq!(
        hunk["base"],
        json!({"start": 1, "count": 1, "lines": ["pub fn x() {}\n"]})
    );
    assert_eq!(hunk["main"]["lines"], json!(["pub fn w() {}\n"]));
    assert_eq!(hunk["version"]["lines"], json!(["pub fn y() {}\n"]));
    assert_eq!(detail.body["summary"]["conflicted"], true);
    assert_eq!(detail.body["summary"]["turn"], "with_author");
}

#[test]
fn the_openapi_document_declares_every_route_and_error_schema() {
    let doc = serde_json::to_value(openapi()).unwrap();
    let paths = &doc["paths"];
    for (path, method, operation) in [
        ("/api/v1/projects/{slug}/proposals", "get", "list_proposals"),
        (
            "/api/v1/projects/{slug}/proposals/{id}",
            "get",
            "get_proposal",
        ),
        (
            "/api/v1/projects/{slug}/proposals/{id}/comments",
            "get",
            "list_review_comments",
        ),
        (
            "/api/v1/projects/{slug}/proposals/{id}/comments",
            "post",
            "add_review_comment",
        ),
        (
            "/api/v1/projects/{slug}/proposals/{id}/reviews",
            "post",
            "submit_review",
        ),
        (
            "/api/v1/projects/{slug}/proposals/{id}/integrate",
            "post",
            "integrate_proposal",
        ),
    ] {
        let declared = &paths[path][method];
        assert_eq!(declared["operationId"], operation, "{method} {path}");
        let responses = declared["responses"].as_object().unwrap();
        for (status, response) in responses {
            if status.starts_with('4') || status.starts_with('5') {
                assert_eq!(
                    response["content"]["application/json"]["schema"]["$ref"],
                    "#/components/schemas/ErrorEnvelope",
                    "{operation} {status}"
                );
            }
        }
    }
    let integrate = &paths["/api/v1/projects/{slug}/proposals/{id}/integrate"]["post"]["responses"];
    let conflict = integrate["409"]["description"].as_str().unwrap();
    for code in [
        "stale_version",
        "changes_requested",
        "not_approved",
        "human_approval_missing",
        "prerequisites_not_done",
        "integration_conflicted",
        "head_moved",
    ] {
        assert!(conflict.contains(code), "{code}");
    }
    assert!(
        integrate["403"]["description"]
            .as_str()
            .unwrap()
            .contains("author_cannot_integrate")
    );
    let schemas = &doc["components"]["schemas"];
    for name in [
        "ProposalList",
        "ProposalSummary",
        "ProposalDetail",
        "ProposalGate",
        "ConflictReportView",
        "ConflictHunkSide",
        "CommentList",
        "ReviewComment",
        "CommentBody",
        "ReviewBody",
        "SubmittedReview",
        "IntegrateBody",
        "IntegrationResult",
        "ErrorEnvelope",
    ] {
        assert!(schemas[name].is_object(), "{name} is missing");
    }
}

#[tokio::test]
async fn a_check_run_is_read_with_its_logs_and_listed_on_its_proposal() {
    let fx = Fixture::new().await;
    let author = fx.agent("frontend").await;
    fx.workspaces()
        .create_workspace(&author, t(1))
        .await
        .unwrap();
    fx.write(1, "src/lib.rs", "pub fn y() {}\n");
    fx.write(
        1,
        "fold-checks.toml",
        "[[check]]\nname = \"unit\"\nrun = \"echo fine\"\n",
    );
    let reviews = fx.reviews();
    let started = reviews.run_checks(&author, t(1)).await.unwrap();
    let fold_daemon::services::reviews::checks::Started::Running { run, .. } = started else {
        panic!("expected a run");
    };
    let answer = fold_store::testing::eventually(std::time::Duration::from_secs(30), async || {
        let answer = send(&fx, Method::GET, &format!("/check-runs/{run}"), None, None).await;
        if answer.body["state"] == "running" {
            Err(answer.body)
        } else {
            Ok(answer)
        }
    })
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert_eq!(answer.body["id"], "cr-1");
    assert_eq!(answer.body["state"], "passed");
    assert_eq!(answer.body["purpose"], "check");
    let result = &answer.body["results"][0];
    assert_eq!(result["name"], "unit");
    assert_eq!(result["setup"], false);
    assert_eq!(result["outcome"], "passed");
    assert_eq!(result["log"], "fine\n");
    assert_eq!(result["truncated"], false);
    let missing = send(&fx, Method::GET, "/check-runs/cr-7", None, None).await;
    assert_eq!(
        (missing.status, missing.code()),
        (StatusCode::NOT_FOUND, "check_run_not_found")
    );

    reviews
        .create_proposal(&author, t(1), "Add checks", None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    let proposal = send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
    let checks = &proposal.body["checks"];
    assert_eq!(checks.as_array().unwrap().len(), 1, "{checks}");
    assert_eq!(checks[0]["id"], "cr-1");
    assert_eq!(
        checks[0]["results"][0]["log"], "",
        "a proposal leaves the logs out"
    );
}

#[tokio::test]
async fn approve_and_integrate_wait_while_an_update_runs_its_checks() {
    let fx = Fixture::new().await;
    let author = fx.agent("frontend").await;
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    let update = fx.updating_proposal(&author, &go).await;
    let detail = send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
    assert_eq!(
        detail.body["update_run"],
        json!({"run": update.to_string(), "purpose": "update_proposal"})
    );
    let approve = send(
        &fx,
        Method::POST,
        "/proposals/P-1/reviews",
        Some(json!({"version": 1, "verdict": "approve", "body": "fine"})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (approve.status, approve.code()),
        (StatusCode::CONFLICT, "update_checks_running"),
        "{}",
        approve.body
    );
    assert_eq!(
        approve.body["error"]["details"]["run"],
        json!(update.to_string())
    );
    assert!(
        approve.body["error"]["details"]["hint"]
            .as_str()
            .unwrap()
            .contains("checks.finished")
    );
    let integrate = send(
        &fx,
        Method::POST,
        "/proposals/P-1/integrate",
        Some(json!({"expected_version": 1})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (integrate.status, integrate.code()),
        (StatusCode::CONFLICT, "update_checks_running")
    );
    let changes = send(
        &fx,
        Method::POST,
        "/proposals/P-1/reviews",
        Some(json!({"version": 1, "verdict": "request_changes", "body": "and a test"})),
        Some(TANIM),
    )
    .await;
    assert_eq!(changes.status, StatusCode::CREATED, "{}", changes.body);
    std::fs::write(&go, "").unwrap();
    let detail = fold_store::testing::eventually(std::time::Duration::from_secs(30), async || {
        let detail = send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
        if detail.body["summary"]["current_version"] == 2 {
            Ok(detail)
        } else {
            Err(detail.body)
        }
    })
    .await;
    assert_eq!(detail.body["update_run"], Value::Null);
}

#[tokio::test]
async fn approve_and_integrate_wait_while_a_reviewer_fix_runs_its_checks() {
    let fx = Fixture::new().await;
    let author = fx.agent("frontend").await;
    let review = fx.agent("review").await;
    give_role(&fx.handle.store, "review", Role::Reviewer, None).await;
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    let fix = fx.fixing_proposal(&author, &review, &go).await;
    let detail = send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
    assert_eq!(
        detail.body["update_run"],
        json!({"run": fix.to_string(), "purpose": "review_fix"})
    );
    let approve = send(
        &fx,
        Method::POST,
        "/proposals/P-1/reviews",
        Some(json!({"version": 1, "verdict": "approve", "body": "fine"})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (approve.status, approve.code()),
        (StatusCode::CONFLICT, "update_checks_running"),
        "{}",
        approve.body
    );
    assert_eq!(approve.body["error"]["details"]["purpose"], "review_fix");
    let integrate = send(
        &fx,
        Method::POST,
        "/proposals/P-1/integrate",
        Some(json!({"expected_version": 1})),
        Some(TANIM),
    )
    .await;
    assert_eq!(
        (integrate.status, integrate.code()),
        (StatusCode::CONFLICT, "update_checks_running")
    );
    std::fs::write(&go, "").unwrap();
    let detail = fold_store::testing::eventually(std::time::Duration::from_secs(30), async || {
        let detail = send(&fx, Method::GET, "/proposals/P-1", None, Some(TANIM)).await;
        if detail.body["summary"]["current_version"] == 2 {
            Ok(detail)
        } else {
            Err(detail.body)
        }
    })
    .await;
    assert_eq!(detail.body["update_run"], Value::Null);
}
