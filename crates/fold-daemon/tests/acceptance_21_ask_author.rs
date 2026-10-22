#![cfg(test)]

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness};

const FILE: &str = "web/src/board.ts";
const CONTENT: &str = "export const a = 1;\nexport const b = 2;\nexport const c = 3;\n";

async fn http(
    h: &Harness,
    method: Method,
    path: &str,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    h.http(method, &format!("/api/v1/projects/demo{path}"), body, HUMAN)
        .await
}

async fn get(h: &Harness, path: &str) -> Value {
    let (status, body) = http(h, Method::GET, path, None).await;
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    body
}

/// What a question must leave unchanged: the proposal's turn, verdict, review claim and reviews, its comments, and the task's status.
async fn review_state(h: &Harness, proposal: &str, task: &str) -> Value {
    let detail = get(h, &format!("/proposals/{proposal}")).await;
    let comments = get(h, &format!("/proposals/{proposal}/comments")).await;
    let task = get(h, &format!("/tasks/{task}")).await;
    json!({
        "summary": detail["summary"],
        "reviews": detail["reviews"],
        "comments": comments["comments"],
        "status": task["status"],
    })
}

#[tokio::test]
async fn acceptance_21_a_human_asks_the_author_about_lines_of_the_diff() {
    let h = Harness::start().await;
    let task = h.create_task("frontend", "board numbers").await;
    let work = h.start_work("frontend", &task).await;
    work.write(&[(FILE, CONTENT)]);
    let proposal = h.propose("frontend", &work, "Board numbers").await;
    h.call("frontend", "get_inbox", json!({})).await;
    let before = review_state(&h, &proposal, &task).await;
    let reviews_before = h
        .events()
        .await
        .iter()
        .filter(|event| event.kind.starts_with("review."))
        .count();

    let ask = json!({"version": 1, "path": FILE, "side": "new", "start_line": 2, "end_line": 3, "body": "Why two constants here?"});
    let (status, sent) = http(
        &h,
        Method::POST,
        &format!("/proposals/{proposal}/questions"),
        Some(&ask),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let question = sent["id"].as_str().unwrap().to_owned();
    assert_eq!(sent["sender"], format!("human:{HUMAN}"));
    assert_eq!(sent["task"], task.as_str());
    assert_eq!(sent["proposal"], proposal.as_str());
    assert_eq!(sent["recipients"][0]["name"], "frontend");
    assert_eq!(
        sent["lines"],
        json!({"version": 1, "path": FILE, "side": "new", "start_line": 2, "end_line": 3,
               "excerpt": "export const b = 2;\nexport const c = 3;", "excerpt_cut": false})
    );

    // The author reads the question with the lines in its inbox and in the thread.
    let inbox = h.call("frontend", "get_inbox", json!({})).await;
    let item = inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == question.as_str())
        .unwrap()
        .clone();
    assert_eq!(item["proposal"], proposal.as_str());
    assert_eq!(
        item["lines"],
        json!({"version": 1, "path": FILE, "side": "new", "start_line": 2, "end_line": 3,
               "excerpt": "export const b = 2;\nexport const c = 3;"})
    );
    let thread = h
        .call("frontend", "get_thread", json!({"thread": question}))
        .await;
    assert_eq!(thread["messages"][0]["lines"], item["lines"]);

    // The question writes no review, verdict or comment and leaves the turn and the status as they were.
    assert_eq!(review_state(&h, &proposal, &task).await, before);
    let reviews_after = h
        .events()
        .await
        .iter()
        .filter(|event| event.kind.starts_with("review."))
        .count();
    assert_eq!(reviews_after, reviews_before);

    // The author answers in the thread, which the proposal's listing shows with the lines.
    let answer = h
        .send_with(
            "frontend",
            json!({"to": [format!("human:{HUMAN}")], "body": "One per column.", "reply_to": question}),
        )
        .await;
    let listing = get(&h, &format!("/messages?proposal={proposal}")).await;
    let threads = listing["items"].as_array().unwrap();
    let asked = threads
        .iter()
        .find(|thread| thread["id"] == question.as_str())
        .unwrap();
    let ids: Vec<&str> = asked["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [question.as_str(), answer.as_str()]);
    assert_eq!(asked["messages"][0]["lines"]["start_line"], 2);
    assert_eq!(asked["messages"][1]["lines"], Value::Null);
    assert_eq!(review_state(&h, &proposal, &task).await, before);
}

#[tokio::test]
async fn acceptance_21_a_human_asks_the_author_about_the_whole_proposal() {
    let h = Harness::start().await;
    let task = h.create_task("frontend", "board numbers").await;
    let work = h.start_work("frontend", &task).await;
    work.write(&[(FILE, CONTENT)]);
    let proposal = h.propose("frontend", &work, "Board numbers").await;
    let before = review_state(&h, &proposal, &task).await;

    let (status, sent) = http(
        &h,
        Method::POST,
        &format!("/proposals/{proposal}/questions"),
        Some(&json!({"version": 1, "body": "Why constants and not an enum?"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let question = sent["id"].as_str().unwrap().to_owned();
    assert_eq!(sent["sender"], format!("human:{HUMAN}"));
    assert_eq!(sent["task"], task.as_str());
    assert_eq!(sent["proposal"], proposal.as_str());
    assert_eq!(sent["recipients"][0]["name"], "frontend");
    assert_eq!(sent["lines"], Value::Null);
    assert!(h.inbox("frontend").await.contains(&question));

    let listing = get(&h, &format!("/messages?proposal={proposal}")).await;
    assert!(
        listing["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|thread| thread["id"] == question.as_str())
    );
    assert_eq!(review_state(&h, &proposal, &task).await, before);
}

#[tokio::test]
async fn acceptance_21_a_question_names_lines_that_exist() {
    let h = Harness::start().await;
    let task = h.create_task("frontend", "board numbers").await;
    let work = h.start_work("frontend", &task).await;
    work.write(&[(FILE, CONTENT)]);
    let proposal = h.propose("frontend", &work, "Board numbers").await;
    let path = format!("/proposals/{proposal}/questions");
    for (ask, status, code) in [
        (
            json!({"version": 1, "path": FILE, "side": "new", "start_line": 3, "end_line": 2, "body": "why"}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_comment_range",
        ),
        (
            json!({"version": 1, "path": FILE, "side": "new", "start_line": 3, "end_line": 4, "body": "why"}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_comment_range",
        ),
        (
            json!({"version": 1, "path": FILE, "side": "old", "start_line": 1, "end_line": 1, "body": "why"}),
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown_comment_path",
        ),
        (
            json!({"version": 2, "path": FILE, "side": "new", "start_line": 1, "end_line": 1, "body": "why"}),
            StatusCode::NOT_FOUND,
            "proposal_not_found",
        ),
        (
            json!({"version": 1, "path": "../x", "side": "new", "start_line": 1, "end_line": 1, "body": "why"}),
            StatusCode::BAD_REQUEST,
            "invalid_path",
        ),
        (
            json!({"version": 1, "path": FILE, "side": "new", "start_line": 1, "end_line": 1, "body": "  "}),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            json!({"version": 1, "path": FILE, "body": "why"}),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            json!({"version": 2, "body": "why"}),
            StatusCode::NOT_FOUND,
            "proposal_not_found",
        ),
    ] {
        let (answer, body) = http(&h, Method::POST, &path, Some(&ask)).await;
        assert_eq!(
            (answer, body["error"]["code"].as_str().unwrap()),
            (status, code),
            "{ask}"
        );
    }
    let (status, body) = http(&h, Method::POST, "/proposals/P-9/questions", Some(&json!({"version": 1, "path": FILE, "side": "new", "start_line": 1, "end_line": 1, "body": "why"}))).await;
    assert_eq!(
        (status, body["error"]["code"].as_str().unwrap()),
        (StatusCode::NOT_FOUND, "proposal_not_found")
    );
    let listing = get(&h, &format!("/messages?proposal={proposal}")).await;
    let from_human = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|thread| thread["messages"].as_array().unwrap())
        .filter(|message| message["sender"] == format!("human:{HUMAN}"))
        .count();
    assert_eq!(from_human, 0);
}

#[tokio::test]
async fn acceptance_21_a_question_about_an_integrated_proposal_goes_to_its_author() {
    let h = Harness::start().await;
    let task = h.create_task("frontend", "board numbers").await;
    let work = h.start_work("frontend", &task).await;
    work.write(&[(FILE, CONTENT)]);
    let proposal = h.propose("frontend", &work, "Board numbers").await;
    h.give_review("review", &proposal).await;
    h.approve(&proposal).await;
    let integrated = h
        .call(
            "review",
            "integrate_proposal",
            json!({"id": proposal, "expected_version": 1}),
        )
        .await;
    assert_eq!(integrated["result"], "succeeded");

    // The integration released the claim, so the question goes to the proposal's author.
    let ask = json!({"version": 1, "path": FILE, "side": "new", "start_line": 1, "end_line": 1, "body": "Why a constant?"});
    let (status, sent) = http(
        &h,
        Method::POST,
        &format!("/proposals/{proposal}/questions"),
        Some(&ask),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    assert_eq!(sent["recipients"][0]["name"], "frontend");
    assert_eq!(sent["lines"]["excerpt"], "export const a = 1;");
}

#[tokio::test]
async fn acceptance_21_a_question_goes_to_the_worker_holding_the_task() {
    // `frontend` leads its area and writes the proposal; `backend` is the main agent, which claims a task of any area.
    let mut h = Harness::start_without_leads().await;
    h.lead_area("frontend").await;
    h.make_main("backend").await;
    let task = h.create_task("frontend", "board numbers").await;
    let work = h.start_work("frontend", &task).await;
    work.write(&[(FILE, CONTENT)]);
    let proposal = h.propose("frontend", &work, "Board numbers").await;
    h.call("frontend", "release_task", json!({"id": task}))
        .await;
    let claimed = h.call("backend", "claim_task", json!({"id": task})).await;
    assert_eq!(claimed["holder"], "backend");

    let ask = json!({"version": 1, "path": FILE, "side": "new", "start_line": 1, "end_line": 1, "body": "Why a constant?"});
    let (status, sent) = http(
        &h,
        Method::POST,
        &format!("/proposals/{proposal}/questions"),
        Some(&ask),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    let recipients: Vec<&str> = sent["recipients"]
        .as_array()
        .unwrap()
        .iter()
        .map(|recipient| recipient["name"].as_str().unwrap())
        .collect();
    assert_eq!(recipients, ["backend"]);
    let question = sent["id"].as_str().unwrap();
    assert!(h.inbox("backend").await.iter().any(|id| id == question));
    assert!(!h.inbox("frontend").await.iter().any(|id| id == question));
}

#[tokio::test]
async fn acceptance_21_a_question_to_a_stopped_author_starts_it_again_to_answer() {
    use crate::worker_resume::{TASK_WORKER, proposed_by_a_task_worker, until_runs, worker};
    use crate::workers_support::FakeClaude;

    let fake = FakeClaude::new("result");
    let (mut harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    harness.disconnect(TASK_WORKER).await;
    let stopped = worker(&harness, TASK_WORKER).await;

    let ask = json!({"version": 1, "path": "page.html", "side": "new", "start_line": 1, "end_line": 1, "body": "Why a page?"});
    let (status, sent) = harness
        .http(
            Method::POST,
            "/api/v1/projects/demo/proposals/P-1/questions",
            Some(&ask),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    assert_eq!(sent["recipients"][0]["name"], TASK_WORKER);
    let question = sent["id"].as_str().unwrap().to_owned();

    // The author's own session starts again with a turn that names the question.
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(stopped.session_id.as_str()));
    let turn = &runs[1].stdin[1];
    assert!(
        turn.contains(&format!("- {question} from human:{HUMAN} about T-1")),
        "{turn}"
    );

    harness.connect(TASK_WORKER).await;
    let inbox = harness.call(TASK_WORKER, "get_inbox", json!({})).await;
    let item = inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == question.as_str())
        .cloned()
        .unwrap();
    assert_eq!(item["lines"]["excerpt"], "<p>page</p>");
}
