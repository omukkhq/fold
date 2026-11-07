#![cfg(test)]

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, TOKEN, wire};

/// The session-start context the hook gives a new session of `agent`.
async fn start_context(h: &Harness, agent: &str) -> String {
    let (status, answer) = h
        .socket(
            Method::POST,
            "/api/v1/hooks/session-start",
            Some(&json!({"cwd": h.root(), "agent": agent})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    answer["context"].as_str().unwrap().to_owned()
}

/// Acceptance 39, a request reaches the orchestrators that wait or start later: the area lead `backend` waits for task status changes only, and a human's request ends that wait at once with the request's id and the next tools, while the reviewer's wait runs on. The main agent `frontend` starts after the request was made, so no push reached it; its session-start context names the request, it claims it, and the next start names none.
#[tokio::test]
async fn acceptance_39_a_new_request_ends_a_leads_wait_and_reaches_a_main_agent_that_starts_later()
{
    let mut h = Harness::start_without_leads().await;
    h.lead_area("backend").await;
    let mut lead_inbox = h.register_inbox("backend").await;
    h.drain_events("backend").await;
    h.drain_events("review").await;

    let body = "The board needs a search.";
    let (lead, reviewer, (status, created)) = tokio::join!(
        h.call(
            "backend",
            "wait_for_events",
            json!({"types": ["task.status_changed"], "timeout_s": 1500}),
        ),
        h.call(
            "review",
            "wait_for_events",
            json!({"types": ["task.status_changed"], "timeout_s": 3}),
        ),
        async {
            h.until_waiting(2).await;
            h.create_request_form(&[("body", None, None, body.as_bytes())], Some(HUMAN))
                .await
        },
    );
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["request"]["id"], "RQ-1");
    assert_eq!(lead["events"].as_array().unwrap().len(), 1, "{lead}");
    assert_eq!(lead["events"][0]["type"], "request.created");
    assert_eq!(lead["requests"], json!(["RQ-1"]));
    let hint = lead["hint"].as_str().unwrap();
    assert!(
        hint.contains("get_request") && hint.contains("claim_request"),
        "{hint}"
    );
    assert!(!lead.to_string().contains(body), "ids, never the text");
    assert_eq!(
        lead_inbox.next().await,
        wire(TOKEN, "fold: RQ-1 created. Call claim_request.")
    );
    assert_eq!(
        reviewer,
        json!({"timed_out": true}),
        "a reviewer claims no request, so its wait runs on"
    );

    h.make_main("frontend").await;
    let context = start_context(&h, "frontend").await;
    assert!(
        context
            .contains("Requests to claim: RQ-1. Call get_request for each, then claim_request.\n"),
        "{context}"
    );
    assert!(!context.contains(body), "ids, never the text: {context}");
    let read = h
        .call("frontend", "get_request", json!({"id": "RQ-1"}))
        .await;
    assert_eq!(read["body"], body);
    h.call(
        "frontend",
        "claim_request",
        json!({"id": "RQ-1", "kind": "feature", "title": "Search the board"}),
    )
    .await;

    let context = start_context(&h, "frontend").await;
    assert!(context.contains("Requests to claim: none.\n"), "{context}");
    let lead = start_context(&h, "backend").await;
    assert!(lead.contains("Requests to claim: none.\n"), "{lead}");
    let waited: Value = h
        .call(
            "backend",
            "wait_for_events",
            json!({"types": ["task.status_changed"], "timeout_s": 1}),
        )
        .await;
    assert_eq!(
        waited,
        json!({"timed_out": true}),
        "a claimed request wakes no lead"
    );
}
