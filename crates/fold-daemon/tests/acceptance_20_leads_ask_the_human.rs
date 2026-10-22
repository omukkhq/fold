#![cfg(test)]

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness};

const MAIN: &str = "frontend";
const LEAD: &str = "backend";

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

/// The questions of Needs you as `(message, task, agent)`.
async fn questions(h: &Harness) -> Vec<(String, Option<String>, Option<String>)> {
    get(h, "/needs-you").await["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == "question")
        .map(|item| {
            (
                item["message"].as_str().unwrap().to_owned(),
                item["task"].as_str().map(str::to_owned),
                item["agent"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

/// Acceptance 20, a lead asks the human directly: an area lead's ask to the humans waits in Needs you, the human's reply in the web app goes back to the lead in the same thread, and the main agent receives neither the ask nor the answer.
#[tokio::test]
async fn acceptance_20_a_leads_ask_reaches_the_human_and_its_answer_skips_the_main_agent() {
    let mut h = Harness::start_without_leads().await;
    h.lead_area(LEAD).await;
    h.make_main(MAIN).await;
    let task = h.create_task_for(MAIN, LEAD, "Draw the usage chart").await;
    let main_before = h.inbox(MAIN).await;

    let ask = h
        .send_with(
            LEAD,
            json!({
                "to": ["humans"],
                "body": "Which chart library does the usage page use?",
                "task": task,
                "choices": ["Recharts", "Plain SVG"],
                "asks_human": true,
            }),
        )
        .await;
    assert_eq!(
        questions(&h).await,
        [(ask.clone(), Some(task.clone()), Some(LEAD.to_owned()))]
    );
    assert!(!h.inbox(MAIN).await.contains(&ask), "main gets no copy");

    let (status, reply) = h
        .http(
            Method::POST,
            "/api/v1/projects/demo/feedback",
            Some(&json!({"reply_to": ask, "body": "Plain SVG"})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{reply}");
    let reply = reply["id"].as_str().unwrap().to_owned();
    assert!(questions(&h).await.is_empty(), "the reply ends the ask");
    assert!(
        h.inbox(LEAD).await.contains(&reply),
        "the lead gets the answer"
    );
    assert_eq!(h.inbox(MAIN).await, main_before, "main receives nothing");

    let thread = get(&h, &format!("/threads/{ask}")).await;
    let messages: Vec<&str> = thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["id"].as_str().unwrap())
        .collect();
    assert_eq!(messages, [ask.as_str(), reply.as_str()]);
    let participants: Vec<&str> = thread["participants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect();
    assert!(!participants.contains(&MAIN), "{participants:?}");
}
