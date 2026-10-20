#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_core::{AgentName, Role, codes};
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, SLUG};
use crate::common::roles::give_role;

const MAIN: &str = "planner";
const LEAD: &str = "frontend";
const AREA: &str = "frontend-work";
const WORKER: &str = "frontend-worker";
const REVIEWER: &str = "reviewer-1";

fn notes_path(query: &str) -> String {
    format!("/api/v1/projects/{SLUG}/notes{query}")
}

fn ids(page: &Value) -> Vec<&str> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note["id"].as_str().unwrap())
        .collect()
}

async fn start_context(h: &Harness, agent: &str) -> String {
    h.handle
        .agents()
        .session_context(SLUG, &AgentName::parse(agent).unwrap(), &h.handle.tasks)
        .await
        .unwrap()
}

#[tokio::test]
async fn acceptance_20_the_leads_record_their_decisions_and_the_human_reads_them_later() {
    let mut h = Harness::start().await;
    h.make_main(MAIN).await;
    h.connect(MAIN).await;
    give_role(&h.handle.store, WORKER, Role::TaskWorker, Some(AREA)).await;
    h.connect(WORKER).await;
    give_role(&h.handle.store, REVIEWER, Role::Reviewer, None).await;
    h.connect(REVIEWER).await;
    let task = h.create_task(LEAD, "Sort the board").await;
    let graph = h.call(MAIN, "get_task_graph", json!({})).await;

    let refused = h
        .refused(
            WORKER,
            "add_note",
            json!({"title": "Due date first", "decision": "Cards sort by due date.", "reason": "The task names no order."}),
        )
        .await;
    assert_eq!(refused["code"], codes::NOTE_DENIED, "{refused}");
    assert!(
        refused["hint"].as_str().unwrap().contains("your lead"),
        "{refused}"
    );
    h.send(
        WORKER,
        &[LEAD],
        "I sort the cards by due date, because the task names no order.",
    )
    .await;

    let lead_note = h
        .call(
            LEAD,
            "add_note",
            json!({
                "title": "Cards sort by due date",
                "decision": "The board sorts the cards of a column by due date.",
                "reason": "The task names no order, and the due date is what a human scans for.",
                "links": [format!("#{task}"), "@web/src/board.ts"],
            }),
        )
        .await;
    assert_eq!(lead_note["id"], "n-1");
    assert_eq!(lead_note["area"], AREA);
    let review_note = h
        .call(
            "review",
            "add_note",
            json!({"title": "Screenshots for UI work", "decision": "A UI proposal needs a screenshot.", "reason": "The checks cannot see the screen."}),
        )
        .await;
    assert_eq!(review_note, json!({"id": "n-2"}));
    let main_note = h
        .call(
            MAIN,
            "add_note",
            json!({"title": "bun for scripts", "decision": "Scripts run with bun.", "reason": "The project already uses bun."}),
        )
        .await;
    assert_eq!(main_note, json!({"id": "n-3"}));
    let reviewer = h
        .refused(
            REVIEWER,
            "add_note",
            json!({"title": "t", "decision": "d", "reason": "r"}),
        )
        .await;
    assert_eq!(reviewer["code"], codes::NOTE_DENIED);
    let malformed = h
        .refused(
            LEAD,
            "add_note",
            json!({"title": "t", "decision": "d", "reason": "r", "links": ["T-1"]}),
        )
        .await;
    assert_eq!(malformed["code"], codes::INVALID_REQUEST);

    let added: Vec<Value> = h
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "note.added")
        .map(|event| event.data)
        .collect();
    assert_eq!(
        added,
        [
            json!({"note": "n-1", "agent": LEAD, "area": AREA}),
            json!({"note": "n-2", "agent": "review", "area": null}),
            json!({"note": "n-3", "agent": MAIN, "area": null}),
        ]
    );

    let (status, page) = h.http(Method::GET, &notes_path(""), None, HUMAN).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(ids(&page), ["n-3", "n-2", "n-1"]);
    assert_eq!(page["unread"], 3);
    assert_eq!(page["next"], Value::Null);
    let first = &page["items"][2];
    assert_eq!(first["author"], LEAD);
    assert_eq!(first["agent"], LEAD);
    assert_eq!(first["area"], AREA);
    assert_eq!(first["title"], "Cards sort by due date");
    assert_eq!(
        first["links"],
        json!([format!("#{task}"), "@web/src/board.ts"])
    );
    let (_, of_area) = h
        .http(
            Method::GET,
            &notes_path(&format!("?area={AREA}")),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(ids(&of_area), ["n-1"]);
    let (_, of_main) = h
        .http(
            Method::GET,
            &notes_path(&format!("?agent={MAIN}&limit=1")),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(ids(&of_main), ["n-3"]);
    let (_, older) = h
        .http(Method::GET, &notes_path("?limit=2"), None, HUMAN)
        .await;
    assert_eq!(ids(&older), ["n-3", "n-2"]);
    assert_eq!(older["next"], "n-2");

    let listed = h.call(WORKER, "list_notes", json!({"area": AREA})).await;
    assert_eq!(listed["notes"][0]["id"], "n-1");
    assert_eq!(
        listed["notes"][0]["reason"],
        "The task names no order, and the due date is what a human scans for."
    );

    let worker_context = start_context(&h, WORKER).await;
    assert!(
        worker_context.contains("- n-1 Cards sort by due date"),
        "{worker_context}"
    );
    assert!(!worker_context.contains("n-2"), "{worker_context}");
    let reviewer_context = start_context(&h, REVIEWER).await;
    assert!(
        reviewer_context.contains("- n-3 bun for scripts\n- n-2 Screenshots for UI work"),
        "{reviewer_context}"
    );
    assert!(!reviewer_context.contains("n-1"), "{reviewer_context}");
    let main_context = start_context(&h, MAIN).await;
    for note in ["n-1", "n-2", "n-3"] {
        assert!(main_context.contains(note), "{main_context}");
    }

    let (status, read) = h
        .http(
            Method::POST,
            &notes_path("/read"),
            Some(&json!({"through": "n-2"})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read, json!({"unread": 1}));
    let (_, read) = h
        .http(
            Method::POST,
            &notes_path("/read"),
            Some(&json!({"through": "n-99"})),
            HUMAN,
        )
        .await;
    assert_eq!(read, json!({"unread": 0}));
    let (_, back) = h
        .http(
            Method::POST,
            &notes_path("/read"),
            Some(&json!({"through": "n-1"})),
            HUMAN,
        )
        .await;
    assert_eq!(back, json!({"unread": 0}), "a read mark never moves back");
    let (_, other) = h.http(Method::GET, &notes_path(""), None, "bo").await;
    assert_eq!(other["unread"], 3, "each human reads on their own");

    let (_, needs_you) = h
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/needs-you"),
            None,
            HUMAN,
        )
        .await;
    assert!(!needs_you.to_string().contains("n-1"), "{needs_you}");
    assert_eq!(h.call(MAIN, "get_task_graph", json!({})).await, graph);
}
