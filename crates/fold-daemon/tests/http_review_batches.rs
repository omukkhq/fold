#![cfg(test)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fold_core::ProposalId;
use fold_daemon::http::router;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use crate::proposal_tools_support::{Fixture, t};

async fn get(fx: &Fixture, path: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .uri(format!("/api/v1/projects/demo{path}"))
        .body(Body::empty())
        .unwrap();
    let response = router(fx.state.clone()).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// `frontend` and `backend` each add a file as P-1 and P-2, and `review` batches both and checks them out as `rb-1`.
async fn batched() -> Fixture {
    let fx = Fixture::new().await;
    for (task, name, path) in [(1, "frontend", "src/one.rs"), (2, "backend", "src/two.rs")] {
        let author = fx.agent(name).await;
        fx.workspaces()
            .create_workspace(&author, t(task))
            .await
            .unwrap();
        fx.write(task, path, "one line\n");
        fx.reviews()
            .create_proposal(&author, t(task), "Add a file", None)
            .await
            .unwrap();
    }
    let review = fx.agent("review").await;
    let proposals = [1, 2].map(|n| ProposalId::from_number(n).unwrap()).to_vec();
    let batch = fx
        .reviews()
        .create_review_batch(&review, proposals)
        .await
        .unwrap();
    fx.reviews()
        .create_batch_checkout(&review, batch.id)
        .await
        .unwrap();
    fx
}

#[tokio::test]
async fn a_batch_reads_with_its_members_and_checkout() {
    let fx = batched().await;

    let (status, body) = get(&fx, "/reviews/batches/rb-1").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], "rb-1");
    assert_eq!(body["holder"], "review");
    assert_eq!(body["state"], "open");
    assert_eq!(body["submitted_at"], Value::Null);
    let members = body["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert_eq!(members[0]["proposal"], "P-1");
    assert_eq!(members[0]["task"], "T-1");
    assert_eq!(members[0]["position"], 1);
    assert_eq!(members[0]["version"], 1);
    assert_eq!(members[0]["current_version"], 1);
    assert_eq!(members[0]["proposal_state"], "open");
    assert_eq!(members[0]["in_batch"], true);
    assert_eq!(body["checkout"]["path"], "ws/review-rb-1");
    assert_eq!(body["checkout"]["base"], "r1");
    assert_eq!(body["verdicts"], serde_json::json!([]));
}

#[tokio::test]
async fn the_list_filters_open_batches() {
    let fx = batched().await;

    let (status, body) = get(&fx, "/reviews/batches?open=true").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["batches"].as_array().unwrap().len(), 1);
    let (status, body) = get(&fx, "/reviews/batches").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["batches"][0]["id"], "rb-1");
}

#[tokio::test]
async fn an_unknown_or_malformed_batch_is_refused() {
    let fx = batched().await;

    let (status, body) = get(&fx, "/reviews/batches/rb-7").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "review_batch_not_found");
    let (status, body) = get(&fx, "/reviews/batches/P-1").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_id");
}
