#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_core::WorkerState;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, WAIT};
use crate::worker_resume::{until_runs, until_state, worker};
use crate::workers_support::{FakeClaude, Run};

const MAIN: &str = "main";
const BASE: &str = "/api/v1/projects/demo";

async fn get(h: &Harness, path: &str) -> Value {
    let (status, body) = h
        .http(Method::GET, &format!("{BASE}{path}"), None, HUMAN)
        .await;
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    body
}

async fn post(h: &Harness, path: &str, body: Value) -> Value {
    let (status, answer) = h
        .http(Method::POST, &format!("{BASE}{path}"), Some(&body), HUMAN)
        .await;
    assert!(status.is_success(), "{path}: {status} {answer}");
    answer
}

/// The only run of the supervised main, once one of its user turns holds `text`.
async fn until_told(fake: &FakeClaude, text: &str) -> Run {
    eventually(WAIT, async || {
        let runs = fake.runs_of(MAIN);
        match runs.as_slice() {
            [run] if run.stdin.iter().any(|line| line.contains(text)) => Ok(run.clone()),
            other => Err(other
                .iter()
                .map(|run| run.stdin.clone())
                .collect::<Vec<_>>()),
        }
    })
    .await
}

fn question_ids(needs: &Value) -> Vec<String> {
    needs["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == "question")
        .map(|item| item["message"].as_str().unwrap().to_owned())
        .collect()
}

/// Acceptance 27, a request handled with no terminal: in a project whose setting has fold run the main agent, a human's request in the web app starts the supervised main, which claims it, asks the human in the request thread, and drafts the cards; the human's answer and approval reach the same idle process as its next turns, and no `fld claude` session ever runs.
#[tokio::test]
async fn acceptance_27_a_request_is_handled_end_to_end_by_the_supervised_main() {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    std::fs::write(
        std::path::Path::new(h.root()).join(".fold/config.toml"),
        "[main]\nsupervised = true\n",
    )
    .unwrap();

    let (status, created) = h
        .create_request_form(
            &[("body", None, None, b"The board needs a search box.")],
            Some(HUMAN),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let runs = until_runs(&fake, MAIN, 1).await;
    let first = &runs[0].stdin[1];
    assert!(
        first.contains("fold started you as main, the main agent"),
        "{first}"
    );
    assert!(
        first.contains("RQ-1 created. Call claim_request."),
        "{first}"
    );
    until_state(&h, MAIN, WorkerState::Idle).await;

    h.connect(MAIN).await;
    h.call(
        MAIN,
        "claim_request",
        json!({"id": "RQ-1", "kind": "feature", "title": "Search the board"}),
    )
    .await;
    let ask = h
        .send_with(
            MAIN,
            json!({
                "to": ["humans"],
                "request": "RQ-1",
                "body": "Search by title only, or by description too?",
                "choices": ["Title only", "Title and description"],
                "asks_human": true,
            }),
        )
        .await;
    assert_eq!(
        question_ids(&get(&h, "/needs-you").await),
        std::slice::from_ref(&ask)
    );

    let answer = post(
        &h,
        "/feedback",
        json!({"reply_to": ask, "body": "Title only"}),
    )
    .await;
    let answer = answer["id"].as_str().unwrap().to_owned();
    until_told(&fake, &answer).await;
    assert!(question_ids(&get(&h, "/needs-you").await).is_empty());

    let draft = h
        .call(
            MAIN,
            "submit_request_draft",
            json!({
                "id": "RQ-1",
                "cards": [
                    {"key": "c1", "title": "Add the title search query", "description": "Query by title.\n\n**Acceptance**: the query test passes."},
                    {"key": "c2", "title": "Show the board search box", "description": "Add the box.\n\n**Acceptance**: the box filters the cards."},
                ],
                "edges": [{"prerequisite": "c1", "dependent": "c2"}],
                "note": "The box needs the query.",
            }),
        )
        .await;
    assert_eq!(draft["state"], "draft_ready", "{draft}");
    let approved = post(&h, "/requests/RQ-1/approve", json!({"expected_version": 1})).await;
    assert_eq!(approved["tasks"], json!(["T-1", "T-2"]), "{approved}");
    until_told(&fake, "RQ-1 approved as T-1, T-2.").await;

    let done = h
        .send_with(
            MAIN,
            json!({"to": ["humans"], "request": "RQ-1", "body": "RQ-1 is planned as T-1 and T-2."}),
        )
        .await;
    let thread = get(&h, &format!("/threads/{ask}")).await;
    assert!(
        thread["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["id"] == done.as_str()),
        "{thread}"
    );
    assert!(question_ids(&get(&h, "/needs-you").await).is_empty());

    let row = worker(&h, MAIN).await;
    assert_eq!(row.role, fold_core::Role::Main);
    assert_eq!(
        fake.runs_of(MAIN).len(),
        1,
        "one process handled the whole request"
    );
}
