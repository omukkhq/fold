#![cfg(test)]

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness};

const MAIN: &str = "frontend";

async fn get(h: &Harness, path: &str) -> Value {
    let (status, body) = h
        .http(
            Method::GET,
            &format!("/api/v1/projects/demo{path}"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    body
}

/// The question items of Needs you as `(message, questions)`: one item per open set.
async fn needs_you(h: &Harness) -> Vec<(String, u64)> {
    get(h, "/needs-you").await["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == "question")
        .map(|item| {
            (
                item["message"].as_str().unwrap().to_owned(),
                item["questions"].as_u64().unwrap(),
            )
        })
        .collect()
}

/// The messages of `agent`'s inbox as `(id, reply_to, body)`.
async fn inbox(h: &Harness, agent: &str) -> Vec<(String, Option<String>, String)> {
    h.call(agent, "get_inbox", json!({})).await["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| {
            (
                message["id"].as_str().unwrap().to_owned(),
                message["reply_to"].as_str().map(str::to_owned),
                message["body"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// Acceptance 32, question sets: each worker asks the humans everything it needs as one set and waits. Two workers wait for the same thing, the MCP server to connect again. The user fixes it and tells the main agent, which finds both open sets, replies to each worker in its thread and marks each answered, so both continue without the user answering in the web app. A set that the user answers in the web app goes back as one reply that holds each question with its answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_32_two_workers_wait_the_user_tells_main_and_both_continue() {
    let h = Harness::start_without_leads().await;
    h.make_main(MAIN).await;
    let checkout = h.create_task(MAIN, "Build the checkout").await;
    let search = h.create_task(MAIN, "Build the search").await;

    // Each worker asks one set and waits for its answer.
    let review = h
        .send_with(
            "review",
            json!({
                "to": ["humans"],
                "body": "The Bifrost MCP server is gone from my session.",
                "task": checkout,
                "questions": [
                    {"text": "Is the Bifrost MCP server connected again?", "choices": ["Yes, it is reconnected", "Not yet"]},
                    {"text": "Which keys must I use for the Mistral calls?", "choices": ["Production keys", "Mock the calls"]},
                ],
            }),
        )
        .await;
    let backend = h
        .send_with(
            "backend",
            json!({
                "to": ["humans"],
                "body": "I wait for the Bifrost MCP server.",
                "task": search,
                "questions": [{"text": "Is the Bifrost MCP server connected again?", "choices": ["Yes", "Not yet"]}],
            }),
        )
        .await;
    assert_eq!(
        needs_you(&h).await,
        [(review.clone(), 2), (backend.clone(), 1)],
        "one item per set, whatever its number of questions"
    );

    // The user reconnects the server and tells the main agent; it finds every set this answers.
    let listed = h.call(MAIN, "list_open_asks", json!({})).await;
    let asks = listed["asks"].as_array().unwrap();
    let found: Vec<(&str, &str, u64)> = asks
        .iter()
        .map(|ask| {
            (
                ask["id"].as_str().unwrap(),
                ask["agent"].as_str().unwrap(),
                ask["questions"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (review.as_str(), "review", 2),
            (backend.as_str(), "backend", 1)
        ]
    );
    assert_eq!(
        asks[1]["first_line"], "I wait for the Bifrost MCP server.",
        "the first line says what the ask is about"
    );
    let refused = h.refused("backend", "list_open_asks", json!({})).await;
    assert_eq!(refused["code"], "main_agent_required");

    for (ask, agent) in [(&review, "review"), (&backend, "backend")] {
        h.send_with(
            MAIN,
            json!({
                "to": [agent],
                "reply_to": ask,
                "body": "The user reconnected the Bifrost MCP server. Continue; mock the Mistral calls.",
            }),
        )
        .await;
        let answered = h.call(MAIN, "mark_answered", json!({"message": ask})).await;
        assert_eq!(answered, json!({"answered": ask}));
    }
    assert!(needs_you(&h).await.is_empty());
    assert_eq!(
        h.call(MAIN, "list_open_asks", json!({})).await,
        json!({"asks": []})
    );

    // Both workers continue: each finds the main agent's reply in the thread of its own set.
    for (ask, agent) in [(&review, "review"), (&backend, "backend")] {
        let messages = inbox(&h, agent).await;
        assert!(
            messages.iter().any(
                |(_, reply_to, body)| reply_to.as_deref() == Some(ask.as_str())
                    && body.starts_with("The user reconnected")
            ),
            "{agent}: {messages:?}"
        );
    }

    // A set the user answers in the web app goes back as one reply with each question and its answer.
    let next = h
        .send_with(
            "review",
            json!({
                "to": ["humans"],
                "body": "One more before the proposal.",
                "task": checkout,
                "questions": [
                    {"text": "Do I run the e2e suite first?", "choices": ["Yes, the full suite", "Skip it"]},
                    {"text": "Which journey matters most?"},
                ],
            }),
        )
        .await;
    let (status, reply) = h
        .http(
            Method::POST,
            &format!("/api/v1/projects/demo/asks/{next}/answer"),
            Some(&json!({"answers": ["yes, the full suite", "The sign-in journey"]})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{reply}");
    let messages = inbox(&h, "review").await;
    let (_, _, body) = messages
        .iter()
        .find(|(id, _, _)| *id == reply["id"])
        .expect("the reply reaches the asker");
    assert_eq!(
        body,
        "1. Do I run the e2e suite first?\nYes, the full suite\n\n2. Which journey matters most?\nThe sign-in journey"
    );
    let thread = h
        .call("review", "get_thread", json!({"thread": next}))
        .await;
    assert_eq!(
        thread["messages"][1]["answers"],
        json!(["Yes, the full suite", "The sign-in journey"])
    );
    assert!(needs_you(&h).await.is_empty());
}
