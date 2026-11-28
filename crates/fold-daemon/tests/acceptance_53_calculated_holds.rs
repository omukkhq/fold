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

/// The board card of `task`, from whichever column holds it.
async fn card(h: &Harness, task: &str) -> Value {
    let board = get(h, "/board").await;
    board["columns"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|column| column["cards"].as_array().unwrap().iter())
        .find(|card| card["task"] == task)
        .cloned()
        .unwrap_or_else(|| panic!("no card for {task}: {board}"))
}

/// The short `get_task_graph` node of each task, by id.
async fn graph_nodes(h: &Harness) -> Vec<Value> {
    h.call(MAIN, "get_task_graph", json!({})).await["tasks"]
        .as_array()
        .unwrap()
        .clone()
}

fn node<'a>(nodes: &'a [Value], task: &str) -> &'a Value {
    nodes
        .iter()
        .find(|node| node["id"] == task)
        .unwrap_or_else(|| panic!("no node for {task}"))
}

/// The task ids of the web app's Needs you.
async fn needs_you_tasks(h: &Harness) -> Vec<String> {
    get(h, "/needs-you").await["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["task"].as_str().map(str::to_owned))
        .collect()
}

async fn ready(h: &Harness) -> Vec<String> {
    h.call(MAIN, "get_ready_tasks", json!({})).await["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["id"].as_str().unwrap().to_owned())
        .collect()
}

/// Acceptance 53, a hold holds back the tasks downstream of it: the main agent plans a chain in which a schema task leads to an API task and that to a screen task, a docs task that needs the schema with a hold of its own, and a free task. A hold on the schema task shows as `held_by` on the API, screen and docs tasks in `get_task`, `get_task_graph` and the HTTP task and board card, and the status reason names it; the docs task shows its own hold too. Needs you lists only the manual holds and `get_ready_tasks` leaves out only the held task. A human takes the hold off and every `held_by` is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_53_a_hold_shows_on_every_task_downstream_until_it_is_cleared() {
    let h = Harness::start_without_leads().await;
    h.make_main(MAIN).await;
    let created = h
        .call(
            MAIN,
            "create_task_plan",
            json!({"tasks": [
                {"key": "schema", "title": "Write the schema", "description": "Check the schema"},
                {"key": "api", "title": "Serve the API", "description": "Check the API", "needs": ["schema"]},
                {"key": "screen", "title": "Draw the screen", "description": "Check the screen", "needs": ["api"]},
                {"key": "docs", "title": "Write the docs", "description": "Check the docs", "needs": ["schema"], "hold": {"reason": "Wait for the style guide"}},
                {"key": "free", "title": "Tidy the logs", "description": "Check the logs"}
            ]}),
        )
        .await;
    let id = |key: &str| -> String {
        created["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["key"] == key)
            .and_then(|task| task["id"].as_str())
            .unwrap()
            .to_owned()
    };
    let (schema, api, screen, docs, free) = (
        id("schema"),
        id("api"),
        id("screen"),
        id("docs"),
        id("free"),
    );
    let held = h
        .call(
            MAIN,
            "set_hold",
            json!({"id": schema, "reason": "Wait for the data owner"}),
        )
        .await;
    assert_eq!(held, json!({"id": schema, "hold": true}));

    for task in [&api, &screen, &docs] {
        let view = h.call(MAIN, "get_task", json!({"id": task})).await;
        assert_eq!(view["held_by"], json!([schema]), "{view}");
        assert!(
            view["status_reason"]
                .as_str()
                .unwrap()
                .ends_with(&format!(", held through {schema}")),
            "{view}"
        );
        let http_view = get(&h, &format!("/tasks/{task}")).await;
        assert_eq!(http_view["held_by"], json!([schema]), "{http_view}");
        assert_eq!(http_view["status_reason"], view["status_reason"]);
        assert_eq!(card(&h, task).await["held_by"], json!([schema]));
    }
    let docs_view = h.call(MAIN, "get_task", json!({"id": docs})).await;
    assert_eq!(docs_view["hold"]["reason"], "Wait for the style guide");
    let schema_view = h.call(MAIN, "get_task", json!({"id": schema})).await;
    assert_eq!(schema_view["held_by"], json!([]), "{schema_view}");
    assert!(
        !schema_view["status_reason"]
            .as_str()
            .unwrap()
            .contains("held through")
    );
    assert!(card(&h, &schema).await.get("held_by").is_none());

    let nodes = graph_nodes(&h).await;
    for task in [&api, &screen, &docs] {
        assert_eq!(node(&nodes, task)["held_by"], json!([schema]));
    }
    assert!(node(&nodes, &schema).get("held_by").is_none());
    assert!(node(&nodes, &free).get("held_by").is_none());
    let full = h
        .call(MAIN, "get_task_graph", json!({"detail": "full"}))
        .await;
    let full_nodes = full["tasks"].as_array().unwrap();
    assert_eq!(node(full_nodes, &free)["held_by"], json!([]));

    let mut waiting = needs_you_tasks(&h).await;
    waiting.sort();
    let mut manual = vec![schema.clone(), docs.clone()];
    manual.sort();
    assert_eq!(waiting, manual, "only the manual holds wait for a person");
    assert_eq!(ready(&h).await, std::slice::from_ref(&free));

    let (status, cleared) = h
        .http(
            Method::DELETE,
            &format!("/api/v1/projects/demo/tasks/{schema}/hold"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    for task in [&api, &screen, &docs] {
        let view = h.call(MAIN, "get_task", json!({"id": task})).await;
        assert_eq!(view["held_by"], json!([]), "{view}");
        assert!(
            !view["status_reason"]
                .as_str()
                .unwrap()
                .contains("held through"),
            "{view}"
        );
        assert!(
            get(&h, &format!("/tasks/{task}"))
                .await
                .get("held_by")
                .is_none()
        );
        assert!(card(&h, task).await.get("held_by").is_none());
    }
    let nodes = graph_nodes(&h).await;
    for task in [&api, &screen, &docs] {
        assert!(node(&nodes, task).get("held_by").is_none());
    }
    let docs_view = h.call(MAIN, "get_task", json!({"id": docs})).await;
    assert_eq!(docs_view["hold"]["reason"], "Wait for the style guide");
}
