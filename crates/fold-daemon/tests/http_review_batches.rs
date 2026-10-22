#![cfg(test)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fold_core::reviews::Verdict;
use fold_core::{ProposalId, ReviewBatchId};
use fold_daemon::http::router;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common::roles::give_role;
use crate::proposal_tools_support::{Fixture, t};
use fold_core::roles::Role;
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

/// `frontend` and `backend` each write `files` of their own as P-1 and P-2, and `review` batches both as `rb-1`.
async fn batched_with(files: [(&str, &str); 2]) -> Fixture {
    let fx = Fixture::new().await;
    for ((task, name), (path, content)) in [(1, "frontend"), (2, "backend")].into_iter().zip(files)
    {
        let author = fx.agent(name).await;
        fx.workspaces()
            .create_workspace(&author, t(task))
            .await
            .unwrap();
        fx.write(task, path, content);
        fx.reviews()
            .create_proposal(&author, t(task), "Add a file", None)
            .await
            .unwrap()
            .detail()
            .unwrap();
    }
    fx.agent("review").await;
    give_role(&fx.handle.store, "review", Role::Reviewer, None).await;
    let proposals = [1, 2].map(|n| ProposalId::from_number(n).unwrap()).to_vec();
    crate::common::roles::give_batch(
        &fx.handle.store,
        "review",
        &proposals,
        fx.handle.clock.now(),
    )
    .await;
    fx
}

const TWO_FILES: [(&str, &str); 2] = [("src/one.rs", "one line\n"), ("src/two.rs", "one line\n")];

/// [`batched_with`] two new files, and `review` checks the batch out.
async fn batched() -> Fixture {
    let fx = batched_with(TWO_FILES).await;
    let review = fx.agent("review").await;
    fx.reviews()
        .create_batch_checkout(&review, ReviewBatchId::from_number(1).unwrap())
        .await
        .unwrap();
    fx
}

/// Every file under the object store, so a test sees whether a read wrote one.
fn stored_objects(fx: &Fixture) -> usize {
    fn count(dir: &std::path::Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                if path.is_dir() { count(&path) } else { 1 }
            })
            .sum()
    }
    count(&fx.project.paths().objects())
}

fn changed(body: &Value) -> Vec<(String, String)> {
    body["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap().to_owned(),
                file["change"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn pair(path: &str, change: &str) -> (String, String) {
    (path.to_owned(), change.to_owned())
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
        .unwrap()
        .detail()
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

#[tokio::test]
async fn the_merged_change_holds_every_member_on_main_and_writes_nothing() {
    let fx = batched_with(TWO_FILES).await;
    let before = stored_objects(&fx);

    let (status, body) = get(&fx, "/reviews/batches/rb-1/diff").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["batch"], "rb-1");
    assert_eq!(body["basis"], "merged");
    assert_eq!(body["base"], "r1");
    assert_eq!(body["head"], Value::Null);
    assert_eq!(body["conflict"], Value::Null);
    assert_eq!(
        body["members"],
        json!([{"proposal": "P-1", "version": 1}, {"proposal": "P-2", "version": 1}])
    );
    assert_eq!(
        changed(&body),
        [pair("src/one.rs", "added"), pair("src/two.rs", "added")]
    );
    assert_eq!(
        body["totals"],
        json!({"files": 2, "added_lines": 2, "removed_lines": 0})
    );
    let (status, file) = get(&fx, "/reviews/batches/rb-1/diff/file?path=src/two.rs").await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file["path"], "src/two.rs");
    assert_eq!(file["source"], "recorded");
    assert_eq!(file["hunks"][0]["lines"][0]["kind"], "added");
    assert_eq!(file["hunks"][0]["lines"][0]["text"], "one line\n");
    assert_eq!(
        stored_objects(&fx),
        before,
        "the merged tree stays in memory"
    );
    let (status, missing) = get(&fx, "/reviews/batches/rb-1/diff/file?path=README.md").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing["error"]["code"], "path_not_in_comparison");

    let review = fx.agent("review").await;
    fx.reviews()
        .create_batch_checkout(&review, ReviewBatchId::from_number(1).unwrap())
        .await
        .unwrap();
    let (_, again) = get(&fx, "/reviews/batches/rb-1/diff").await;
    assert_eq!(again, body, "the checkout holds the same tree");
}

#[tokio::test]
async fn a_conflicting_member_is_named_and_the_change_holds_the_members_before_it() {
    let fx = batched_with([
        ("README.md", "# demo\nfrom one\n"),
        ("README.md", "# demo\nfrom two\n"),
    ])
    .await;

    let (status, body) = get(&fx, "/reviews/batches/rb-1/diff").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["basis"], "merged");
    assert_eq!(
        body["conflict"],
        json!({"proposal": "P-2", "version": 1, "with": "P-1", "paths": ["README.md"]})
    );
    assert_eq!(body["members"], json!([{"proposal": "P-1", "version": 1}]));
    assert_eq!(changed(&body), [pair("README.md", "modified")]);
    let (status, file) = get(&fx, "/reviews/batches/rb-1/diff/file?path=README.md").await;
    assert_eq!(status, StatusCode::OK, "{file}");
    let added: Vec<&str> = file["hunks"][0]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|line| line["kind"] == "added")
        .map(|line| line["text"].as_str().unwrap())
        .collect();
    assert_eq!(added, ["from one\n"]);
}

#[tokio::test]
async fn an_integrated_batch_shows_the_change_of_main_across_its_revisions() {
    let fx = batched_with(TWO_FILES).await;
    let review = fx.agent("review").await;
    let batch = ReviewBatchId::from_number(1).unwrap();
    let approve = |number: u32| BatchVerdict {
        proposal: ProposalId::from_number(number).unwrap(),
        verdict: Verdict::Approve,
        body: String::new(),
    };
    fx.reviews()
        .submit_batch_review(&review, batch, vec![approve(1), approve(2)], Vec::new())
        .await
        .unwrap();
    let (status, submitted) = get(&fx, "/reviews/batches/rb-1/diff").await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    assert_eq!(
        submitted["basis"], "merged",
        "the members the review covered"
    );
    assert_eq!(submitted["members"].as_array().unwrap().len(), 2);
    fx.reviews()
        .integrate_review_batch(&crate::proposal_tools_support::human("tanim"), batch)
        .await
        .unwrap();

    let (status, body) = get(&fx, "/reviews/batches/rb-1/diff").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["basis"], "integrated");
    assert_eq!(body["base"], "r1");
    assert_eq!(body["head"], "r3");
    assert_eq!(body["conflict"], Value::Null);
    assert_eq!(
        body["members"],
        json!([{"proposal": "P-1", "version": 1}, {"proposal": "P-2", "version": 1}])
    );
    assert_eq!(
        changed(&body),
        [pair("src/one.rs", "added"), pair("src/two.rs", "added")]
    );
    let (status, file) = get(&fx, "/reviews/batches/rb-1/diff/file?path=src/one.rs").await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file["hunks"][0]["lines"][0]["text"], "one line\n");
}

#[tokio::test]
async fn the_merged_change_of_an_unknown_batch_is_refused() {
    let fx = batched_with(TWO_FILES).await;

    let (status, body) = get(&fx, "/reviews/batches/rb-7/diff").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "review_batch_not_found");
    let (status, body) = get(&fx, "/reviews/batches/rb-1/diff/file?path=..%2Fx").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["code"], "invalid_path");
}
