#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_core::{AgentName, WorkerState};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{
    FakeInbox, HUMAN, Harness, SLUG, TOKEN, WAIT, message_notice, wire,
};
use crate::workers_support::FakeClaude;

const MAIN: &str = "frontend";
const LEAD: &str = "design-lead";

async fn until_state(h: &Harness, agent: &str, state: WorkerState) {
    eventually(WAIT, async || {
        let row = h
            .handle
            .agents()
            .get_worker(&AgentName::parse(agent).unwrap())
            .await
            .unwrap();
        if row.state == state {
            Ok(())
        } else {
            Err(row.state)
        }
    })
    .await;
}

/// Waits until the worker process of `agent` has read a stdin line that holds `text`.
async fn until_stdin(fake: &FakeClaude, agent: &str, text: &str) {
    eventually(WAIT, async || {
        let lines: Vec<String> = fake
            .runs_of(agent)
            .into_iter()
            .flat_map(|run| run.stdin)
            .collect();
        if lines.iter().any(|line| line.contains(text)) {
            Ok(())
        } else {
            Err(lines.len())
        }
    })
    .await;
}

/// Reads the session's notices until the one for message `message`; notices about tasks may come before it.
async fn until_notice(inbox: &mut FakeInbox, message: &str) {
    let wanted = wire(TOKEN, &message_notice(message));
    while inbox.next().await != wanted {}
}

/// The area `design` over label `design` with two tasks, and its lead, started, idle and connected.
async fn design_area(h: &mut Harness) {
    h.call(
        MAIN,
        "create_label",
        json!({"name": "design", "color": "sage"}),
    )
    .await;
    h.call(
        MAIN,
        "define_area",
        json!({"name": "design", "label": "design", "instructions": "Draw screens first."}),
    )
    .await;
    for title in ["Draw the sign-in screen", "Draw the board screen"] {
        h.call(
            MAIN,
            "create_task",
            json!({"title": title, "labels": ["design"]}),
        )
        .await;
    }
    h.call(
        MAIN,
        "start_worker",
        json!({"role": "area_lead", "area": "design"}),
    )
    .await;
    until_state(h, LEAD, WorkerState::Idle).await;
    h.connect(LEAD).await;
}

fn chat_path(agent: &str) -> String {
    format!("/api/v1/projects/{SLUG}/agents/{agent}/chat")
}

async fn chat(h: &Harness, agent: &str, body: &str) -> Value {
    let (status, sent) = h
        .http(
            Method::POST,
            &chat_path(agent),
            Some(&json!({"body": body})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    sent
}

fn ids(page: &Value) -> Vec<&str> {
    page["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["id"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn acceptance_19_a_human_chats_with_a_lead_and_the_main_agent_outside_any_task() {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main(MAIN).await;
    let mut main_inbox = h.register_inbox(MAIN).await;
    design_area(&mut h).await;
    let statuses = h.call(MAIN, "get_task_graph", json!({})).await;

    let sent = chat(&h, LEAD, "Use the new design tokens from now on.").await;
    let first = sent["id"].as_str().unwrap().to_owned();
    assert_eq!(sent["thread"], first.as_str());
    assert_eq!(sent["task"], Value::Null);
    assert_eq!(sent["recipients"][0]["name"], LEAD);
    until_stdin(&fake, LEAD, &message_notice(&first)).await;
    let inbox = h.call(LEAD, "get_inbox", json!({})).await;
    assert!(
        inbox.to_string().contains("Use the new design tokens"),
        "{inbox}"
    );
    assert!(
        inbox.to_string().contains(&format!("human:{HUMAN}")),
        "{inbox}"
    );

    let answer = h
        .send_with(
            LEAD,
            json!({"to": [format!("human:{HUMAN}")], "reply_to": first, "body": "I give the tokens to each task worker."}),
        )
        .await;
    let forwarded = h
        .send(
            LEAD,
            &[MAIN],
            "The human asks for a dark theme, which changes the approved scope.",
        )
        .await;
    until_notice(&mut main_inbox, &forwarded).await;

    let (status, page) = h.http(Method::GET, &chat_path(LEAD), None, HUMAN).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["agent"], LEAD);
    assert_eq!(ids(&page), [first.as_str(), answer.as_str()]);
    assert_eq!(page["messages"][1]["thread"], first.as_str());
    assert_eq!(page["next"], Value::Null);
    let again = chat(&h, LEAD, "Thanks.").await;
    assert_eq!(again["thread"], first.as_str());

    let to_main = chat(&h, MAIN, "Plan the dark theme with me later.").await;
    let to_main = to_main["id"].as_str().unwrap();
    until_notice(&mut main_inbox, to_main).await;
    let (_, main_chat) = h.http(Method::GET, &chat_path(MAIN), None, HUMAN).await;
    assert_eq!(ids(&main_chat), [to_main]);

    let (status, area) = h
        .http(
            Method::PATCH,
            &format!("/api/v1/projects/{SLUG}/areas/design"),
            Some(&json!({"add_instructions": "Use the new design tokens."})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{area}");
    assert_eq!(
        area["instructions"],
        "Draw screens first.\n\nUse the new design tokens."
    );
    let told = eventually(WAIT, async || {
        let inbox = h.call(LEAD, "get_inbox", json!({})).await;
        let items = inbox["messages"].as_array().cloned().unwrap_or_default();
        items
            .into_iter()
            .find(|item| item["from"] == "fold")
            .ok_or(inbox)
    })
    .await;
    assert!(
        told["body"]
            .as_str()
            .unwrap()
            .ends_with("Draw screens first.\n\nUse the new design tokens."),
        "{told}"
    );
    until_stdin(&fake, LEAD, &message_notice(told["id"].as_str().unwrap())).await;
    let (_, page) = h.http(Method::GET, &chat_path(LEAD), None, HUMAN).await;
    assert_eq!(ids(&page).len(), 3, "fold's message stays out of the chat");

    assert_eq!(h.call(MAIN, "get_task_graph", json!({})).await, statuses);
}
