#![cfg(test)]

use std::collections::BTreeSet;

use axum::http::{Method, StatusCode};
use fold_core::{LabelName, TaskId, codes};
use fold_store::{StoreError, tasks};
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, SLUG};

const MAIN: &str = "frontend";

/// The main agent `frontend`, the labels `ui`, `server`, `chore`, `bug` and `docs`, and the areas `web` over `ui` and `api` over `server`.
async fn two_areas() -> Harness {
    let h = Harness::start_without_leads().await;
    h.make_main(MAIN).await;
    for label in ["ui", "server", "chore", "bug", "docs"] {
        h.call(
            MAIN,
            "create_label",
            json!({"name": label, "color": "sage"}),
        )
        .await;
    }
    h.call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    h.call(
        MAIN,
        "define_area",
        json!({"name": "api", "label": "server"}),
    )
    .await;
    h
}

fn path(rest: &str) -> String {
    format!("/api/v1/projects/{SLUG}{rest}")
}

fn assert_two_areas(error: &Value) {
    assert_eq!(error["code"], codes::INVALID_REQUEST, "{error}");
    assert_eq!(error["details"]["reason"], "two_areas", "{error}");
    let areas: BTreeSet<&str> = error["details"]["areas"]
        .as_array()
        .unwrap()
        .iter()
        .map(|area| area.as_str().unwrap())
        .collect();
    assert_eq!(areas, BTreeSet::from(["api", "web"]), "{error}");
}

async fn http_refused(h: &Harness, method: Method, rest: &str, body: Option<&Value>) {
    let (status, body) = h.http(method, &path(rest), body, HUMAN).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_two_areas(&body["error"]);
}

async fn labels_of(h: &Harness, task: &str) -> Value {
    h.call(MAIN, "get_task", json!({"id": task})).await["labels"].clone()
}

async fn task_ids(h: &Harness) -> Vec<Value> {
    h.call(MAIN, "get_task_graph", json!({})).await["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["id"].clone())
        .collect()
}

/// Stores the labels of `task` without the services, which check the one-area rule, so the task can carry two area labels as the database allows.
async fn store_labels(h: &Harness, task: u32, labels: &[&str]) {
    let task = TaskId::from_number(task).unwrap();
    let labels: Vec<LabelName> = labels
        .iter()
        .map(|label| LabelName::parse(label).unwrap())
        .collect();
    h.handle
        .store
        .write(async |tx| {
            tasks::replace_labels(tx, task, &labels).await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn every_create_path_refuses_a_second_area_label_and_creates_nothing() {
    let h = two_areas().await;

    let refused = h
        .refused(
            MAIN,
            "create_task",
            json!({"title": "Both", "labels": ["ui", "chore", "server"]}),
        )
        .await;
    assert_two_areas(&refused);
    assert!(refused["hint"].as_str().unwrap().contains("ui for web"));
    assert!(refused["hint"].as_str().unwrap().contains("server for api"));

    let refused = h
        .refused(
            MAIN,
            "create_task_plan",
            json!({"tasks": [
                {"key": "one", "title": "One", "description": "## body", "labels": ["ui"]},
                {"key": "two", "title": "Two", "description": "## body", "labels": ["ui", "server"], "needs": ["one"]},
            ]}),
        )
        .await;
    assert_two_areas(&refused);

    http_refused(
        &h,
        Method::POST,
        "/tasks",
        Some(&json!({"title": "Both", "labels": ["server", "ui"]})),
    )
    .await;
    assert!(task_ids(&h).await.is_empty());

    let created = h
        .call(
            MAIN,
            "create_task",
            json!({"title": "Many labels", "labels": ["ui", "chore", "bug", "docs"]}),
        )
        .await;
    assert_eq!(
        labels_of(&h, created["id"].as_str().unwrap()).await,
        json!(["bug", "chore", "docs", "ui"])
    );
    let (status, body) = h
        .http(
            Method::POST,
            &path("/tasks"),
            Some(&json!({"title": "No area", "labels": ["chore", "bug", "docs"]})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["areas_conflict"], false);
}

#[tokio::test]
async fn every_update_path_refuses_a_second_area_label_and_changes_nothing() {
    let h = two_areas().await;
    h.call(
        MAIN,
        "create_task",
        json!({"title": "Board query", "labels": ["ui", "chore"]}),
    )
    .await;

    let refused = h
        .refused(
            MAIN,
            "update_task",
            json!({"id": "T-1", "title": "Renamed", "labels": ["ui", "server", "chore"]}),
        )
        .await;
    assert_two_areas(&refused);
    http_refused(
        &h,
        Method::PATCH,
        "/tasks/T-1",
        Some(&json!({"labels": ["server", "ui"]})),
    )
    .await;
    http_refused(&h, Method::PUT, "/tasks/T-1/labels/server", None).await;
    let task = h.call(MAIN, "get_task", json!({"id": "T-1"})).await;
    assert_eq!(task["title"], "Board query");
    assert_eq!(task["labels"], json!(["chore", "ui"]));
    let updates = h
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "task.updated")
        .count();
    assert_eq!(updates, 0, "a refused change appends no task.updated");

    h.call(
        MAIN,
        "update_task",
        json!({"id": "T-1", "labels": ["server", "chore"]}),
    )
    .await;
    let (status, body) = h
        .http(Method::PUT, &path("/tasks/T-1/labels/bug"), None, HUMAN)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        labels_of(&h, "T-1").await,
        json!(["bug", "chore", "server"])
    );
}

#[tokio::test]
async fn a_task_stored_with_two_areas_is_flagged_and_only_a_label_change_must_leave_one_area() {
    let h = two_areas().await;
    h.call(MAIN, "create_task", json!({"title": "Legacy"}))
        .await;
    h.call(
        MAIN,
        "create_task",
        json!({"title": "Plain", "labels": ["ui"]}),
    )
    .await;
    store_labels(&h, 1, &["chore", "server", "ui"]).await;

    let legacy = h.call(MAIN, "get_task", json!({"id": "T-1"})).await;
    assert_eq!(legacy["areas_conflict"], true, "{legacy}");
    let plain = h.call(MAIN, "get_task", json!({"id": "T-2"})).await;
    assert!(plain.get("areas_conflict").is_none(), "{plain}");

    let (_, view) = h.http(Method::GET, &path("/tasks/T-1"), None, HUMAN).await;
    assert_eq!(view["areas_conflict"], true, "{view}");
    let (_, view) = h.http(Method::GET, &path("/tasks/T-2"), None, HUMAN).await;
    assert_eq!(view["areas_conflict"], false, "{view}");
    let (_, list) = h.http(Method::GET, &path("/tasks"), None, HUMAN).await;
    let flags: Vec<(Value, Value)> = list["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| (task["task"].clone(), task["areas_conflict"].clone()))
        .collect();
    assert_eq!(
        flags,
        [(json!("T-1"), json!(true)), (json!("T-2"), json!(false))]
    );
    let (_, board) = h.http(Method::GET, &path("/board"), None, HUMAN).await;
    let cards: Vec<(Value, Value)> = board["columns"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|column| column["cards"].as_array().unwrap().iter())
        .map(|card| (card["task"].clone(), card["areas_conflict"].clone()))
        .collect();
    assert_eq!(
        cards,
        [(json!("T-1"), json!(true)), (json!("T-2"), json!(false))]
    );

    h.call(
        MAIN,
        "update_task",
        json!({"id": "T-1", "title": "Renamed"}),
    )
    .await;
    let (status, view) = h
        .http(
            Method::PATCH,
            &path("/tasks/T-1"),
            Some(&json!({"description": "Both areas still"})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["areas_conflict"], true, "{view}");
    let edited = h.call(MAIN, "get_task", json!({"id": "T-1"})).await;
    assert_eq!(edited["title"], "Renamed");
    assert_eq!(edited["description"], "Both areas still");
    assert_eq!(edited["areas_conflict"], true, "{edited}");

    let refused = h
        .refused(
            MAIN,
            "update_task",
            json!({"id": "T-1", "title": "Renamed again", "labels": ["chore", "server", "ui"]}),
        )
        .await;
    assert_two_areas(&refused);
    assert!(refused["hint"].as_str().unwrap().contains("update_task"));
    http_refused(&h, Method::DELETE, "/tasks/T-1/labels/chore", None).await;
    http_refused(&h, Method::PUT, "/tasks/T-1/labels/bug", None).await;
    let unchanged = h.call(MAIN, "get_task", json!({"id": "T-1"})).await;
    assert_eq!(unchanged["title"], "Renamed");
    assert_eq!(unchanged["labels"], json!(["chore", "server", "ui"]));

    let (status, view) = h
        .http(
            Method::DELETE,
            &path("/tasks/T-1/labels/server"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["areas_conflict"], false, "{view}");
    assert_eq!(labels_of(&h, "T-1").await, json!(["chore", "ui"]));
}

#[tokio::test]
async fn define_area_refuses_a_label_that_shares_a_live_task_with_another_area() {
    let h = two_areas().await;
    for (title, labels) in [
        ("Guide for the board", json!(["ui", "docs"])),
        ("Guide alone", json!(["docs", "chore"])),
        ("Guide for the API", json!(["server", "docs"])),
    ] {
        h.call(
            MAIN,
            "create_task",
            json!({"title": title, "labels": labels}),
        )
        .await;
    }

    let refused = h
        .refused(
            MAIN,
            "define_area",
            json!({"name": "guides", "label": "docs"}),
        )
        .await;
    assert_two_areas(&refused);
    assert_eq!(refused["details"]["tasks"], json!(["T-1", "T-3"]));
    assert!(refused["hint"].as_str().unwrap().contains("update_task"));
    let (status, body) = h
        .http(
            Method::POST,
            &path("/areas"),
            Some(&json!({"name": "guides", "label": "docs"})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["details"]["tasks"], json!(["T-1", "T-3"]));

    h.call(
        MAIN,
        "update_task",
        json!({"id": "T-1", "labels": ["docs"]}),
    )
    .await;
    h.call(MAIN, "archive_task", json!({"id": "T-3"})).await;
    h.call(
        MAIN,
        "define_area",
        json!({"name": "guides", "label": "docs"}),
    )
    .await;
    let areas = h.call(MAIN, "list_workers", json!({})).await["areas"].clone();
    let names: BTreeSet<&str> = areas
        .as_array()
        .unwrap()
        .iter()
        .map(|area| area["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, BTreeSet::from(["api", "guides", "web"]));

    h.call(
        MAIN,
        "update_area",
        json!({"name": "web", "instructions": "Own web/."}),
    )
    .await;
}
