#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, WAIT};

const MAIN: &str = "frontend";

/// The items of the web app's Needs you as `(kind, id)`, in its order: the ask's message, the proposal of a review, the request of a draft, else the task.
async fn web_items(h: &Harness) -> Vec<(String, Option<String>)> {
    let (status, list) = h
        .http(Method::GET, "/api/v1/projects/demo/needs-you", None, HUMAN)
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            let id = ["message", "proposal", "request", "task"]
                .iter()
                .find_map(|key| item[*key].as_str())
                .map(str::to_owned);
            (item["kind"].as_str().unwrap().to_owned(), id)
        })
        .collect()
}

/// The main agent's `get_needs_you` items as `(kind, id)`, in its order.
fn tool_items(listed: &Value) -> Vec<(String, Option<String>)> {
    listed["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["kind"].as_str().unwrap().to_owned(),
                item["id"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

/// The footer line of a call of the main agent.
async fn footer(h: &Harness) -> String {
    let result = h.call_raw(MAIN, "list_agents", json!({})).await;
    result
        .content
        .last()
        .and_then(|content| content.as_text())
        .map(|text| text.text.clone())
        .unwrap_or_default()
}

/// Acceptance 51, the main agent reads what waits for the user: two agents ask the humans and a task waits for a person. The main agent's `get_needs_you` lists the same items as the web app's Needs you, each with its id, agent, age and one line, and its footer counts them; another agent is refused. A human answers one ask in the web app, and fold tells the main agent, whose list and count drop that ask.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_51_the_main_agent_lists_what_waits_for_the_user_and_hears_a_human_answer() {
    let h = Harness::start_without_leads().await;
    h.make_main(MAIN).await;
    let mut inbox = h.register_inbox(MAIN).await;
    let color = h
        .send_with(
            "backend",
            json!({"to": ["humans"], "body": "Which colour for the chart?\nIt shows the load.", "choices": ["Red", "Blue"]}),
        )
        .await;
    let font = h
        .send_with(
            "review",
            json!({"to": ["humans"], "body": "Is the font licence fine?", "asks_human": true}),
        )
        .await;
    let created = h
        .call(
            MAIN,
            "create_task",
            json!({"title": "Sign in to the registry", "kind": "human", "description": "How to do it:\n1. Sign in."}),
        )
        .await;
    let human_task = created["id"].as_str().unwrap().to_owned();

    let web = eventually(WAIT, async || {
        let items = web_items(&h).await;
        if items.len() == 3 {
            Ok(items)
        } else {
            Err(items)
        }
    })
    .await;
    let listed = h.call(MAIN, "get_needs_you", json!({})).await;
    assert_eq!(tool_items(&listed), web, "{listed}");
    let items = listed["items"].as_array().unwrap();
    let ask = items
        .iter()
        .find(|item| item["id"] == color.as_str())
        .unwrap();
    assert_eq!(ask["kind"], "question");
    assert_eq!(ask["agent"], "backend");
    assert_eq!(ask["line"], "Which colour for the chart?");
    assert!(ask["age"].as_str().unwrap().ends_with('s'), "{ask}");
    let task = items
        .iter()
        .find(|item| item["id"] == human_task.as_str())
        .unwrap();
    assert_eq!(task["kind"], "human_task");
    assert_eq!(task["line"], "Sign in to the registry");
    assert!(
        footer(&h)
            .await
            .ends_with("2 questions for the user · 1 wait for the user")
    );

    let refused = h.refused("backend", "get_needs_you", json!({})).await;
    assert_eq!(refused["code"], "main_agent_required");
    assert!(
        !footer_of(&h, "backend").await.contains("for the user"),
        "only the main agent's footer counts the items"
    );

    // A human answers the colour question in the web app; fold tells the main agent that it no longer waits.
    let (status, reply) = h
        .http(
            Method::POST,
            "/api/v1/projects/demo/feedback",
            Some(&json!({"reply_to": color, "body": "Red"})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{reply}");
    let reply = reply["id"].as_str().unwrap().to_owned();
    let expected = format!(
        "fold: human:{HUMAN} answered {color} in {reply} outside your session; it does not wait for the user now. Call get_thread."
    );
    let notice = tokio::time::timeout(WAIT, async {
        loop {
            let post = inbox.next().await;
            if post.contains("outside your session") {
                return post;
            }
        }
    })
    .await
    .expect("the notice of the human's answer within the wait");
    assert!(notice.contains(&expected), "{notice}");
    assert!(!notice.contains(&font));
    let after = tool_items(&h.call(MAIN, "get_needs_you", json!({})).await);
    assert_eq!(after, web_items(&h).await);
    assert_eq!(after.len(), 2, "{after:?}");
    assert!(
        footer(&h)
            .await
            .ends_with("1 questions for the user · 1 wait for the user")
    );
}

async fn footer_of(h: &Harness, agent: &str) -> String {
    let result = h.call_raw(agent, "list_agents", json!({})).await;
    result
        .content
        .last()
        .and_then(|content| content.as_text())
        .map(|text| text.text.clone())
        .unwrap_or_default()
}

/// Acceptance 51, a held task and an open question: the human task the main agent put on hold shows once, as its hold, though it is also a ready human task; the question comes first in `get_needs_you` although the hold is older, and the footer counts the question and the hold apart, the question first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_51_a_held_task_and_an_open_question_are_counted_apart_with_the_question_first()
{
    let h = Harness::start_without_leads().await;
    h.make_main(MAIN).await;
    let created = h
        .call(
            MAIN,
            "create_task",
            json!({"title": "Sign in to the registry", "kind": "human", "description": "How to do it:\n1. Sign in."}),
        )
        .await;
    let held = created["id"].as_str().unwrap().to_owned();
    h.call(
        MAIN,
        "set_hold",
        json!({"id": held, "reason": "Wait until the registry account exists"}),
    )
    .await;
    let asked = h
        .send_with(
            "backend",
            json!({"to": ["humans"], "body": "Which colour for the chart?", "choices": ["Red", "Blue"]}),
        )
        .await;

    let listed = eventually(WAIT, async || {
        let listed = tool_items(&h.call(MAIN, "get_needs_you", json!({})).await);
        if listed.len() >= 2 {
            Ok(listed)
        } else {
            Err(listed)
        }
    })
    .await;
    assert_eq!(
        listed,
        [
            ("question".to_owned(), Some(asked)),
            ("blocker".to_owned(), Some(held)),
        ],
        "the question first, and one row for the held human task"
    );
    let line = footer(&h).await;
    assert!(
        line.ends_with("1 questions for the user · 1 held"),
        "{line}"
    );
    assert!(!line.contains("wait for the user"), "{line}");
}
