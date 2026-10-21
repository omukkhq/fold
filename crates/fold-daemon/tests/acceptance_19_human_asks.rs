#![cfg(test)]

use std::collections::BTreeSet;

use axum::http::{Method, StatusCode};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, WAIT};
use crate::workers_support::FakeClaude;

const MAIN: &str = "frontend";

/// What waits for the human, read the three ways a person sees it: the tasks whose board card shows the question mark, the question items of Needs you as `(message, task)`, and the Messages listing's marked threads with the tab badge's count.
#[derive(Debug, PartialEq, Eq)]
struct Waiting {
    board: BTreeSet<String>,
    needs_you: Vec<(String, Option<String>)>,
    marked: Vec<String>,
    count: u64,
}

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

async fn waiting(h: &Harness) -> Waiting {
    let board = get(h, "/board").await;
    let board = board["columns"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|column| column["cards"].as_array().unwrap().iter())
        .filter(|card| {
            card["attention"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["kind"] == "question")
        })
        .map(|card| card["task"].as_str().unwrap().to_owned())
        .collect();
    let needs_you = get(h, "/needs-you").await["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == "question")
        .map(|item| {
            (
                item["message"].as_str().unwrap().to_owned(),
                item["task"].as_str().map(str::to_owned),
            )
        })
        .collect();
    let listing = get(h, "/messages?mine=true").await;
    let marked = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|thread| thread["awaiting_human"] == true)
        .map(|thread| thread["id"].as_str().unwrap().to_owned())
        .collect();
    Waiting {
        board,
        needs_you,
        marked,
        count: listing["total_awaiting_human"].as_u64().unwrap(),
    }
}

/// Checks that the board, Needs you and the Messages count name exactly `asks`, each `(message, task)` the first message of its thread.
async fn agree(h: &Harness, asks: &[(&str, &str)]) {
    let seen = waiting(h).await;
    let expected = Waiting {
        board: asks.iter().map(|(_, task)| (*task).to_owned()).collect(),
        needs_you: asks
            .iter()
            .map(|(message, task)| ((*message).to_owned(), Some((*task).to_owned())))
            .collect(),
        marked: asks
            .iter()
            .rev()
            .map(|(message, _)| (*message).to_owned())
            .collect(),
        count: asks.len() as u64,
    };
    assert_eq!(seen, expected);
}

async fn ask(h: &Harness, from: &str, task: &str, text: &str) -> String {
    h.send_with(
        from,
        json!({"to": [format!("human:{HUMAN}")], "body": text, "task": task, "asks_human": true}),
    )
    .await
}

async fn refused_code(h: &Harness, agent: &str, tool: &str, arguments: Value) -> String {
    h.refused(agent, tool, arguments).await["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Acceptance 19, asks to the human: only a real ask waits for the human, and the board's question mark, Needs you and the Messages count always agree on it. A report waits for no one. An ask closes when the human replies in its thread, when the asking agent or the main agent records that the user answered elsewhere, or when the human marks it answered. A worker's AskUserQuestion closes only with the human's reply, because the worker waits for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_19_asks_wait_for_the_human_until_answered_and_every_view_agrees() {
    let fake = FakeClaude::new("ask_question");
    let h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main(MAIN).await;
    let first = h.create_task(MAIN, "Pick the chart library").await;
    let second = h.create_task(MAIN, "Pick the port").await;
    let third = h.create_task(MAIN, "Pick the colour").await;
    agree(&h, &[]).await;

    // A report to the human and a thank-you wait for no one.
    h.send_with(
        "backend",
        json!({"to": ["humans", MAIN], "body": "The build passes.", "task": first}),
    )
    .await;
    h.send_with(
        "backend",
        json!({"to": [format!("human:{HUMAN}")], "body": "Thanks.", "task": first}),
    )
    .await;
    agree(&h, &[]).await;

    // An ask waits until the human replies in its thread.
    let chart = ask(&h, "backend", &first, "Recharts or plain SVG?").await;
    agree(&h, &[(&chart, &first)]).await;
    let (status, reply) = h
        .http(
            Method::POST,
            "/api/v1/projects/demo/feedback",
            Some(&json!({"reply_to": chart, "body": "Plain SVG."})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{reply}");
    agree(&h, &[]).await;

    // The user answered in the main agent's session: the asker records it, then the main agent for another agent's ask.
    let port = ask(&h, "backend", &second, "Port 8080 or 3000?").await;
    let size = ask(&h, "review", &second, "Is 200 lines too long?").await;
    agree(&h, &[(&port, &second), (&size, &second)]).await;
    assert_eq!(
        refused_code(&h, "backend", "mark_answered", json!({"message": size})).await,
        "ask_close_denied"
    );
    let answered = h
        .call("backend", "mark_answered", json!({"message": port}))
        .await;
    assert_eq!(answered, json!({"answered": port}));
    agree(&h, &[(&size, &second)]).await;
    h.call(MAIN, "mark_answered", json!({"message": size}))
        .await;
    agree(&h, &[]).await;
    assert_eq!(
        refused_code(&h, MAIN, "mark_answered", json!({"message": size})).await,
        "ask_not_open"
    );

    // The human marks a task's ask answered on the board card.
    let colour = ask(&h, "review", &third, "Red or blue?").await;
    agree(&h, &[(&colour, &third)]).await;
    let (status, marked) = h
        .http(
            Method::POST,
            "/api/v1/projects/demo/asks/answered",
            Some(&json!({"task": third})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{marked}");
    assert_eq!(marked, json!({"answered": [colour]}));
    agree(&h, &[]).await;
    let closed: Vec<Value> = h
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "message.ask_closed")
        .map(|event| json!([event.actor, event.data["message"], event.data["reason"]]))
        .collect();
    assert_eq!(
        closed,
        [
            json!(["backend", port, "answered_elsewhere"]),
            json!([MAIN, size, "answered_elsewhere"]),
            json!([format!("human:{HUMAN}"), colour, "dismissed"]),
        ]
    );

    // A worker's AskUserQuestion waits in Needs you and the Messages count; only the human's reply ends it.
    h.call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    h.call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    h.call(
        MAIN,
        "start_worker",
        json!({"role": "area_lead", "area": "web"}),
    )
    .await;
    let question = eventually(WAIT, async || {
        let seen = waiting(&h).await;
        match seen.needs_you.as_slice() {
            [(message, None)] => Ok(message.clone()),
            _ => Err(seen),
        }
    })
    .await;
    let seen = waiting(&h).await;
    assert!(seen.board.is_empty(), "the question is linked to no task");
    assert_eq!((seen.marked.len(), seen.count), (1, 1));
    assert_eq!(
        refused_code(&h, MAIN, "mark_answered", json!({"message": question})).await,
        "ask_not_open"
    );
    let (status, marked) = h
        .http(
            Method::POST,
            "/api/v1/projects/demo/asks/answered",
            Some(&json!({"thread": question})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{marked}");
    assert_eq!(marked, json!({"answered": []}));
    assert_eq!(
        waiting(&h).await.count,
        1,
        "the worker's question still waits"
    );
    let (status, reply) = h
        .http(
            Method::POST,
            "/api/v1/projects/demo/feedback",
            Some(&json!({"reply_to": question, "body": "Red"})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{reply}");
    let after = waiting(&h).await;
    assert!(after.needs_you.is_empty(), "{after:?}");
    assert_eq!(after.count, 0);
}
