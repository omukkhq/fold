#![cfg(test)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fold_core::reviews::Verdict;
use fold_core::{ProposalId, ReviewBatchId};
use fold_daemon::http::router;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::proposal_tools_support::{Fixture, t};
use fold_daemon::services::reviews::BatchVerdict;

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
    assert_eq!(body["checkout"]["path"], ".fold/ws/review-rb-1");
    assert_eq!(body["checkout"]["base"], "r1");
    assert_eq!(body["verdicts"], serde_json::json!([]));
}

/// A member that came back with a new version after the batch asked for changes shows the change since the version the batch reviewed.
#[tokio::test]
async fn a_member_with_a_new_version_shows_the_change_since_the_reviewed_one() {
    let fx = batched().await;
    let review = fx.agent("review").await;
    let verdict = |number: u32, verdict: Verdict| BatchVerdict {
        proposal: ProposalId::from_number(number).unwrap(),
        verdict,
        body: "see the file".to_owned(),
    };
    fx.reviews()
        .submit_batch_review(
            &review,
            ReviewBatchId::from_number(1).unwrap(),
            vec![
                verdict(1, Verdict::RequestChanges),
                verdict(2, Verdict::Comment),
            ],
            Vec::new(),
        )
        .await
        .unwrap();
    fx.write(1, "src/one.rs", "one line\ntwo lines\n");
    let frontend = fx.agent("frontend").await;
    fx.reviews()
        .update_proposal(&frontend, ProposalId::from_number(1).unwrap(), None)
        .await
        .unwrap();

    let (status, body) = get(&fx, "/reviews/batches/rb-1").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    let members = body["members"].as_array().unwrap();
    assert_eq!(members[0]["current_version"], 2);
    assert_eq!(
        members[0]["since"],
        json!({"version": 1, "totals": {"files": 1, "added_lines": 1, "removed_lines": 0}})
    );
    assert_eq!(members[1]["since"], Value::Null);
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
