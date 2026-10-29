#![cfg(test)]

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness};

const FILE: &str = "web/src/board.ts";
const FIRST: &str = "export const a = 1;\nexport const b = 2;\nexport const c = 3;\n";
const SECOND: &str =
    "export const a = 1;\nexport const b = 20;\nexport const c = 3;\nexport const d = 4;\n";

async fn post(h: &Harness, body: &Value) -> (StatusCode, Value) {
    h.http(
        Method::POST,
        "/api/v1/projects/demo/requests/from-code",
        Some(body),
        HUMAN,
    )
    .await
}

/// `frontend` proposes `content` at `FILE` on a new task; returns the proposal id.
async fn proposal_with(h: &Harness, title: &str, content: &str) -> String {
    let task = h.create_task("frontend", title).await;
    let work = h.start_work("frontend", &task).await;
    work.write(&[(FILE, content)]);
    h.propose("frontend", &work, title).await
}

/// Main at r2 holds `FIRST`, and P-2 changes it to `SECOND` as its v1.
async fn main_and_a_change() -> (Harness, String) {
    let h = Harness::start().await;
    let first = proposal_with(&h, "board numbers", FIRST).await;
    h.give_review("review", &first).await;
    h.approve(&first).await;
    let integrated = h
        .call(
            "review",
            "integrate_proposal",
            json!({"id": first, "expected_version": 1}),
        )
        .await;
    assert_eq!(integrated["revision"], "r2", "{integrated}");
    let second = proposal_with(&h, "more board numbers", SECOND).await;
    (h, second)
}

/// Acceptance 31: a person selects lines of main in the repository and of a proposal's diff, writes a comment, and each comment becomes an ordinary request that keeps where the lines are and what they said. The main agent and the leads hear of it as of any request, and the request read gives them the lines, so they plan with the code in view.
#[tokio::test]
async fn acceptance_31_a_comment_on_lines_of_main_or_of_a_proposal_becomes_a_request() {
    let (h, proposal) = main_and_a_change().await;

    let (status, created) = post(
        &h,
        &json!({"body": "Name these constants after the columns.", "path": FILE,
                "start_line": 2, "end_line": 3, "revision": "r2"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["request"]["id"], "RQ-1");
    assert_eq!(created["request"]["state"], "open");
    assert_eq!(created["request"]["requester"], format!("human:{HUMAN}"));
    assert_eq!(created["body"], "Name these constants after the columns.");
    let on_main = json!({"path": FILE, "start_line": 2, "end_line": 3,
        "excerpt": "export const b = 2;\nexport const c = 3;", "excerpt_cut": false,
        "revision": "r2", "proposal": null, "version": null, "side": null});
    assert_eq!(created["request"]["code"], on_main);

    let (status, created) = post(
        &h,
        &json!({"body": "Why does d exist?", "path": FILE, "start_line": 4, "end_line": 4,
                "proposal": proposal, "version": 1, "side": "new"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let on_change = json!({"path": FILE, "start_line": 4, "end_line": 4,
        "excerpt": "export const d = 4;", "excerpt_cut": false,
        "revision": null, "proposal": proposal, "version": 1, "side": "new"});
    assert_eq!(created["request"]["code"], on_change);

    // The old side of a version is the main it was made on.
    let (status, created) = post(
        &h,
        &json!({"body": "Keep b as it was.", "path": FILE, "start_line": 2, "end_line": 2,
                "proposal": proposal, "version": 1, "side": "old"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["request"]["code"]["excerpt"], "export const b = 2;");
    assert_eq!(created["request"]["code"]["side"], "old");

    // A request written without lines has no code; the list carries each request's lines for the screens that mark them.
    let (status, listed) = h
        .http(Method::GET, "/api/v1/projects/demo/requests", None, HUMAN)
        .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[2]["code"], on_main);
    assert_eq!(items[1]["code"], on_change);

    // The request is the ordinary kind: the orchestrators hear of it, read the lines with it and claim it.
    let read = h
        .call("frontend", "get_request", json!({"id": "RQ-1"}))
        .await;
    assert_eq!(read["body"], "Name these constants after the columns.");
    assert_eq!(
        read["code"],
        json!({"path": FILE, "start_line": 2, "end_line": 3,
               "excerpt": "export const b = 2;\nexport const c = 3;", "excerpt_cut": false,
               "revision": "r2"})
    );
    let read = h
        .call("frontend", "get_request", json!({"id": "RQ-2"}))
        .await;
    assert_eq!(
        read["code"],
        json!({"path": FILE, "start_line": 4, "end_line": 4,
               "excerpt": "export const d = 4;", "excerpt_cut": false,
               "proposal": proposal, "version": 1, "side": "new"})
    );
    let claimed = h
        .call(
            "frontend",
            "claim_request",
            json!({"id": "RQ-1", "kind": "feature", "title": "Name the board constants"}),
        )
        .await;
    assert_eq!(claimed["state"], "drafting", "{claimed}");

    // The event names the request and the anchor by id; the comment and the lines stay out of it.
    let events = h.events().await;
    let created: Vec<&Value> = events
        .iter()
        .filter(|event| event.kind == "request.created")
        .map(|event| &event.data)
        .collect();
    assert_eq!(
        created,
        [
            &json!({"request": "RQ-1", "attachments": 0, "code_revision": "r2"}),
            &json!({"request": "RQ-2", "attachments": 0, "code_proposal": proposal, "code_version": 1}),
            &json!({"request": "RQ-3", "attachments": 0, "code_proposal": proposal, "code_version": 1}),
        ]
    );
}

#[tokio::test]
async fn acceptance_31_a_request_on_code_names_lines_that_exist() {
    let (h, proposal) = main_and_a_change().await;
    let on_main = |path: &str, start: u32, end: u32| json!({"body": "Look here.", "path": path, "start_line": start, "end_line": end, "revision": "r2"});
    for (body, status, code) in [
        (
            on_main("web/../secret", 1, 1),
            StatusCode::BAD_REQUEST,
            "invalid_path",
        ),
        (on_main("", 1, 1), StatusCode::BAD_REQUEST, "invalid_path"),
        (
            on_main("web/src/none.ts", 1, 1),
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown_comment_path",
        ),
        (
            on_main("web/src", 1, 1),
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown_comment_path",
        ),
        (
            on_main(FILE, 3, 4),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_comment_range",
        ),
        (
            on_main(FILE, 0, 1),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_comment_range",
        ),
        (
            on_main(FILE, 3, 2),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_comment_range",
        ),
        (
            json!({"body": "Look here.", "path": FILE, "start_line": 1, "end_line": 1, "revision": "r9"}),
            StatusCode::NOT_FOUND,
            "revision_not_found",
        ),
        (
            json!({"body": "Look here.", "path": FILE, "start_line": 1, "end_line": 1,
                   "proposal": proposal, "version": 7, "side": "new"}),
            StatusCode::NOT_FOUND,
            "proposal_not_found",
        ),
        (
            json!({"body": "Look here.", "path": FILE, "start_line": 1, "end_line": 1,
                   "proposal": "P-9", "version": 1, "side": "new"}),
            StatusCode::NOT_FOUND,
            "proposal_not_found",
        ),
        (
            json!({"body": "Look here.", "path": FILE, "start_line": 1, "end_line": 1,
                   "revision": "r2", "proposal": proposal, "version": 1, "side": "new"}),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            json!({"body": "Look here.", "path": FILE, "start_line": 1, "end_line": 1,
                   "proposal": proposal, "version": 1}),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            json!({"body": "  ", "path": FILE, "start_line": 1, "end_line": 1, "revision": "r2"}),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
    ] {
        let (answer, refused) = post(&h, &body).await;
        assert_eq!(answer, status, "{body}: {refused}");
        assert_eq!(refused["error"]["code"], code, "{body}: {refused}");
        assert!(
            !refused.to_string().contains("export const"),
            "a refusal quotes no code: {refused}"
        );
    }

    // The range refusal says how long the file is; no refusal made a request.
    let (_, refused) = post(&h, &on_main(FILE, 3, 4)).await;
    assert_eq!(refused["error"]["details"]["line_count"], 3, "{refused}");
    let (_, listed) = h
        .http(Method::GET, "/api/v1/projects/demo/requests", None, HUMAN)
        .await;
    assert_eq!(listed["items"], json!([]));
}
