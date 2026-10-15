use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::requests::MAX_ATTACHMENT_BYTES;
use fold_daemon::http::router;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common::acceptance::Harness;

#[cfg(test)]
const BASE: &str = "/api/v1/projects/demo";
#[cfg(test)]
const BOUNDARY: &str = "fold-test-boundary";

/// One part of a multipart form: its name, its file name, its content type and its bytes.
#[cfg(test)]
type Part<'a> = (&'a str, Option<&'a str>, Option<&'a str>, &'a [u8]);

#[cfg(test)]
fn multipart(parts: &[Part<'_>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (name, file, media_type, content) in parts {
        bytes.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        let mut disposition = format!("Content-Disposition: form-data; name=\"{name}\"");
        if let Some(file) = file {
            disposition.push_str(&format!("; filename=\"{file}\""));
        }
        bytes.extend_from_slice(format!("{disposition}\r\n").as_bytes());
        if let Some(media_type) = media_type {
            bytes.extend_from_slice(format!("Content-Type: {media_type}\r\n").as_bytes());
        }
        bytes.extend_from_slice(b"\r\n");
        bytes.extend_from_slice(content);
        bytes.extend_from_slice(b"\r\n");
    }
    bytes.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    bytes
}

/// A raw request through the HTTP router, as `human` when one is named.
#[cfg(test)]
async fn send(
    h: &Harness,
    method: Method,
    path: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
    human: Option<&str>,
) -> axum::response::Response {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(human) = human {
        request = request.header("cookie", format!("fold_human={human}"));
    }
    if let Some(content_type) = content_type {
        request = request.header("content-type", content_type);
    }
    router(h.state.clone())
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap()
}

#[cfg(test)]
async fn json_of(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// `POST /requests` with the form `parts` as `human`.
#[cfg(test)]
async fn create(h: &Harness, parts: &[Part<'_>], human: Option<&str>) -> (StatusCode, Value) {
    let content_type = format!("multipart/form-data; boundary={BOUNDARY}");
    json_of(
        send(
            h,
            Method::POST,
            &format!("{BASE}/requests"),
            Some(&content_type),
            multipart(parts),
            human,
        )
        .await,
    )
    .await
}

#[cfg(test)]
fn draft() -> Value {
    json!([
        {"key": "c1", "title": "Index the board query", "description": "Add the index."},
        {"key": "c2", "title": "Show the faster board", "description": "Use it."},
    ])
}

#[cfg(test)]
async fn submit(h: &Harness, version: u64) {
    let submitted = h
        .call(
            "backend",
            "submit_request_draft",
            json!({"id": "RQ-1", "cards": draft(), "edges": [{"prerequisite": "c1", "dependent": "c2"}]}),
        )
        .await;
    assert_eq!(submitted["version"], version);
}

#[tokio::test]
async fn a_human_creates_asks_for_changes_edits_and_approves() {
    let h = Harness::start().await;
    let png: &[u8] = b"\x89PNG\r\n\x1a\nfake";
    let (status, created) = create(
        &h,
        &[
            ("body", None, None, b"Board is slow\nSee #T-1."),
            ("files", Some("shot.png"), Some("image/png"), png),
            ("files", Some("r\u{e9} port%.txt"), None, b"steps"),
        ],
        Some("ana"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(
        created["request"],
        json!({
            "id": "RQ-1",
            "kind": null,
            "title": null,
            "excerpt": "Board is slow",
            "state": "open",
            "requester": "human:ana",
            "drafter": null,
            "latest_version": null,
            "created_at": created["request"]["created_at"],
            "updated_at": created["request"]["updated_at"],
        })
    );
    assert_eq!(created["body"], "Board is slow\nSee #T-1.");
    assert_eq!(
        created["attachments"],
        json!([
            {"position": 1, "name": "shot.png", "media_type": "image/png", "size": png.len()},
            {"position": 2, "name": "r\u{e9} port%.txt", "media_type": "text/plain", "size": 5},
        ])
    );
    assert_eq!(created["thread"], Value::Null);

    let (status, listed) = h
        .http(
            Method::GET,
            &format!("{BASE}/requests?state=open"),
            None,
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["items"][0]["id"], "RQ-1");
    let (status, none) = h
        .http(
            Method::GET,
            &format!("{BASE}/requests?kind=bug"),
            None,
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(none["items"], json!([]));

    let response = send(
        &h,
        Method::GET,
        &format!("{BASE}/requests/RQ-1/attachments/1"),
        None,
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(headers["content-type"], "image/png");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(
        headers["content-disposition"],
        "attachment; filename=\"shot.png\"; filename*=UTF-8''shot.png"
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], png);
    let response = send(
        &h,
        Method::GET,
        &format!("{BASE}/requests/RQ-1/attachments/2"),
        None,
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"r_ port_.txt\"; filename*=UTF-8''r%C3%A9%20port%25.txt"
    );

    h.call(
        "backend",
        "claim_request",
        json!({"id": "RQ-1", "kind": "feature", "title": "Faster board"}),
    )
    .await;
    let (status, message) = h
        .http(
            Method::POST,
            &format!("{BASE}/feedback"),
            Some(&json!({"body": "It is the main board.", "request": "RQ-1"})),
            "tanim",
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{message}");
    assert_eq!(message["request"], "RQ-1");
    assert_eq!(message["recipients"][0]["name"], "backend");
    let (_, thread) = h
        .http(
            Method::GET,
            &format!("{BASE}/threads/{}", message["id"].as_str().unwrap()),
            None,
            "ana",
        )
        .await;
    assert_eq!(thread["request"], "RQ-1");
    let (status, reply) = h
        .http(
            Method::POST,
            &format!("{BASE}/feedback"),
            Some(&json!({"body": "Also the list.", "reply_to": message["id"]})),
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{reply}");
    assert_eq!(reply["request"], "RQ-1");
    let unrelated = h.send("review", &["frontend"], "other").await;
    let (status, refused) = h
        .http(
            Method::POST,
            &format!("{BASE}/feedback"),
            Some(&json!({"body": "x", "request": "RQ-1", "reply_to": unrelated})),
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(refused["error"]["details"]["reason"], "request_thread");

    submit(&h, 1).await;
    let (status, changed) = h
        .http(
            Method::POST,
            &format!("{BASE}/requests/RQ-1/changes"),
            Some(&json!({"version": 1, "comment": "Split c1."})),
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["request"]["state"], "changes_requested");
    submit(&h, 2).await;

    let (status, stale) = h
        .http(
            Method::POST,
            &format!("{BASE}/requests/RQ-1/approve"),
            Some(&json!({"expected_version": 1})),
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(stale["error"]["code"], "stale_version");

    let (status, edited) = h
        .http(
            Method::POST,
            &format!("{BASE}/requests/RQ-1/drafts"),
            Some(&json!({"cards": draft(), "edges": [{"prerequisite": "c1", "dependent": "c2"}], "note": "Mine."})),
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!(edited["request"]["latest_version"], 3);
    assert_eq!(edited["drafts"][2]["author"], "human:ana");

    let (status, approved) = h
        .http(
            Method::POST,
            &format!("{BASE}/requests/RQ-1/approve"),
            Some(&json!({"expected_version": 3})),
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["request"]["state"], "approved");
    assert_eq!(approved["request"]["kind"], "feature");
    assert_eq!(approved["request"]["title"], "Faster board");
    assert_eq!(approved["tasks"], json!(["T-1", "T-2"]));
    assert_eq!(approved["drafts"].as_array().unwrap().len(), 3);
    assert_eq!(
        approved["drafts"][0]["edges"],
        json!([{"prerequisite": "c1", "dependent": "c2"}])
    );
    assert_eq!(approved["comments"][0]["body"], "Split c1.");
    assert_eq!(approved["thread"]["id"], message["id"]);
    assert_eq!(approved["thread"]["messages"], 2);
    let linked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks t JOIN requests r ON r.id = t.request_id WHERE r.number = 1",
    )
    .fetch_one(h.handle.store.read())
    .await
    .unwrap();
    assert_eq!(linked, 2);
    let (status, read) = h
        .http(Method::GET, &format!("{BASE}/requests/RQ-1"), None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read, approved);
}

#[tokio::test]
async fn writes_need_a_named_human_who_is_the_requester() {
    let h = Harness::start().await;
    let (status, refused) = create(&h, &[("body", None, None, b"slow")], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(refused["error"]["code"], "human_name_required");
    let (status, _) = create(&h, &[("body", None, None, b"slow")], Some("ana")).await;
    assert_eq!(status, StatusCode::CREATED);

    let close = |human: Option<&str>| {
        let body = json!({"reason": "Not needed."}).to_string().into_bytes();
        let path = format!("{BASE}/requests/RQ-1/close");
        let human = human.map(str::to_owned);
        let h = &h;
        async move {
            json_of(
                send(
                    h,
                    Method::POST,
                    &path,
                    Some("application/json"),
                    body,
                    human.as_deref(),
                )
                .await,
            )
            .await
        }
    };
    let (status, refused) = close(None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(refused["error"]["code"], "human_name_required");
    let (status, refused) = close(Some("tanim")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(refused["error"]["code"], "requester_required");
    let (status, refused) = h
        .http(
            Method::POST,
            &format!("{BASE}/requests/RQ-1/changes"),
            Some(&json!({"version": 1, "comment": "more"})),
            "tanim",
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(refused["error"]["code"], "requester_required");
    let (status, closed) = close(Some("ana")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(closed["request"]["state"], "closed");
    assert_eq!(closed["close_reason"], "Not needed.");
    let (status, refused) = close(Some("ana")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(refused["error"]["code"], "request_not_open");
}

#[tokio::test]
async fn bad_input_is_refused_with_its_code() {
    let h = Harness::start().await;
    let (status, missing) = h
        .http(Method::GET, &format!("{BASE}/requests/RQ-9"), None, "ana")
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing["error"]["code"], "request_not_found");
    let (status, filter) = h
        .http(
            Method::GET,
            &format!("{BASE}/requests?state=nope"),
            None,
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(filter["error"]["code"], "invalid_request");

    let file: &[u8] = b"x";
    let many: Vec<Part<'_>> = std::iter::once(("body", None, None, &b"slow"[..]))
        .chain((0..11).map(|_| ("files", Some("a.txt"), Some("text/plain"), file)))
        .collect();
    let (status, refused) = create(&h, &many, Some("ana")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        refused["error"]["details"]["reason"],
        "too_many_attachments"
    );
    let large = vec![b'a'; MAX_ATTACHMENT_BYTES as usize + 1];
    let (status, refused) = create(
        &h,
        &[
            ("body", None, None, b"slow"),
            ("files", Some("small.txt"), None, file),
            ("files", Some("large.bin"), None, &large),
        ],
        Some("ana"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        refused["error"]["details"]["reason"],
        "attachment_too_large"
    );
    assert_eq!(refused["error"]["details"]["position"], 2);
    drop(large);
    let mut over = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"big.bin\"\r\n\r\n"
    )
    .into_bytes();
    over.resize(over.len() + 102 * 1024 * 1024, b'a');
    let response = send(
        &h,
        Method::POST,
        &format!("{BASE}/requests"),
        Some(&format!("multipart/form-data; boundary={BOUNDARY}")),
        over,
        Some("ana"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let (status, refused) = create(&h, &[("files", Some("a.txt"), None, file)], Some("ana")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(refused["error"]["code"], "invalid_request");
    let (status, refused) = create(&h, &[("body", None, None, b"   ")], Some("ana")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(refused["error"]["details"]["reason"], "empty");
    let (status, refused) = json_of(
        send(
            &h,
            Method::POST,
            &format!("{BASE}/requests"),
            Some("application/json"),
            b"{}".to_vec(),
            Some("ana"),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(refused["error"]["code"], "invalid_request");

    let (status, _) = create(&h, &[("body", None, None, b"slow")], Some("ana")).await;
    assert_eq!(status, StatusCode::CREATED);
    for (n, code, wanted) in [
        ("1", "not_found", StatusCode::NOT_FOUND),
        ("x", "invalid_request", StatusCode::BAD_REQUEST),
    ] {
        let (status, refused) = h
            .http(
                Method::GET,
                &format!("{BASE}/requests/RQ-1/attachments/{n}"),
                None,
                "ana",
            )
            .await;
        assert_eq!(status, wanted);
        assert_eq!(refused["error"]["code"], code);
    }
    let (status, refused) = h
        .http(
            Method::POST,
            &format!("{BASE}/requests/RQ-1/drafts"),
            Some(&json!({"cards": []})),
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(refused["error"]["code"], "invalid_request");
    assert_eq!(refused["error"]["details"]["reason"], "state");
}

#[test]
fn the_openapi_document_declares_every_request_route() {
    let doc = serde_json::to_value(fold_daemon::http::openapi()).unwrap();
    let base = "/api/v1/projects/{slug}/requests";
    for (path, method, operation) in [
        (base.to_owned(), "get", "list_requests"),
        (base.to_owned(), "post", "create_request"),
        (format!("{base}/{{id}}"), "get", "get_request"),
        (
            format!("{base}/{{id}}/attachments/{{n}}"),
            "get",
            "get_request_attachment",
        ),
        (
            format!("{base}/{{id}}/changes"),
            "post",
            "request_draft_changes",
        ),
        (
            format!("{base}/{{id}}/drafts"),
            "post",
            "edit_request_draft",
        ),
        (
            format!("{base}/{{id}}/approve"),
            "post",
            "approve_request_draft",
        ),
        (format!("{base}/{{id}}/close"), "post", "close_request"),
    ] {
        let declared = &doc["paths"][&path][method];
        assert_eq!(declared["operationId"], operation, "{method} {path}");
        let responses = declared["responses"].as_object().unwrap();
        let error = responses
            .iter()
            .find(|(status, _)| status.starts_with('4'))
            .map(|(_, response)| response.to_string())
            .unwrap_or_default();
        assert!(error.contains("ErrorEnvelope"), "{method} {path}");
    }
    let create = &doc["paths"][base]["post"]["requestBody"]["content"];
    assert!(create.get("multipart/form-data").is_some(), "{create}");
    for schema in [
        "RequestList",
        "RequestSummary",
        "RequestDetailView",
        "RequestKind",
        "RequestState",
        "CreateRequestForm",
        "ErrorEnvelope",
    ] {
        assert!(
            doc["components"]["schemas"][schema].is_object(),
            "components.schemas.{schema} is missing"
        );
    }
    let summary = &doc["components"]["schemas"]["RequestSummary"]["required"];
    assert!(summary.as_array().unwrap().contains(&json!("title")));
    assert!(summary.as_array().unwrap().contains(&json!("excerpt")));
    fold_daemon::http::openapi::check_operations(&fold_daemon::http::openapi()).unwrap();
}
