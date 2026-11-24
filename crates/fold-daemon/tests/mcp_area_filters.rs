#![cfg(test)]

use fold_core::events::types;
use fold_core::{EventActor, NewEvent, codes};
use fold_daemon::services::events::EventBuffer;
use fold_store::StoreError;
use serde_json::{Value, json};

use crate::common::acceptance::Harness;
use crate::workers_support::FakeClaude;

const MAIN: &str = "frontend";

fn ids(list: &Value, field: &str) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|item| item[field].as_str().unwrap().to_owned())
        .collect()
}

/// Area `web` over label `ui` and area `api` over label `server`, each with a running lead; T-1 and T-3 carry `ui`, T-2 carries `server`, and T-3 needs T-2.
async fn two_areas(fake: &FakeClaude) -> Harness {
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    for (label, color, area) in [("ui", "sage", "web"), ("server", "slate", "api")] {
        harness
            .call(MAIN, "create_label", json!({"name": label, "color": color}))
            .await;
        harness
            .call(MAIN, "define_area", json!({"name": area, "label": label}))
            .await;
    }
    for (title, labels, needs) in [
        ("Build the settings screen", json!(["ui"]), json!([])),
        ("Serve the settings", json!(["server"]), json!([])),
        ("Show the saved settings", json!(["ui"]), json!(["T-2"])),
    ] {
        harness
            .call(
                MAIN,
                "create_task",
                json!({"title": title, "labels": labels, "needs": needs}),
            )
            .await;
    }
    for area in ["web", "api"] {
        harness
            .call(
                MAIN,
                "start_worker",
                json!({"role": "area_lead", "area": area}),
            )
            .await;
    }
    harness
}

#[tokio::test]
async fn get_ready_tasks_and_get_task_graph_with_an_area_give_only_its_tasks() {
    let fake = FakeClaude::new("result");
    let harness = two_areas(&fake).await;
    let every = harness.call(MAIN, "get_ready_tasks", json!({})).await;
    assert_eq!(ids(&every["tasks"], "id"), ["T-2", "T-1"], "T-2 frees T-3");
    let web = harness
        .call(MAIN, "get_ready_tasks", json!({"area": "web"}))
        .await;
    assert_eq!(ids(&web["tasks"], "id"), ["T-1"]);
    let api = harness
        .call(MAIN, "get_ready_tasks", json!({"area": "api"}))
        .await;
    assert_eq!(ids(&api["tasks"], "id"), ["T-2"]);

    let graph = harness
        .call(MAIN, "get_task_graph", json!({"area": "web"}))
        .await;
    assert_eq!(ids(&graph["tasks"], "id"), ["T-1", "T-3"], "{graph}");
    assert_eq!(
        graph["tasks"][1]["needs"],
        json!(["T-2"]),
        "a need outside the area keeps its id"
    );
    assert_eq!(graph["done_count"], 0);
    let graph = harness
        .call(MAIN, "get_task_graph", json!({"area": "api"}))
        .await;
    assert_eq!(ids(&graph["tasks"], "id"), ["T-2"], "{graph}");
}

#[tokio::test]
async fn list_workers_with_an_area_gives_only_that_area_and_its_workers() {
    let fake = FakeClaude::new("result");
    let harness = two_areas(&fake).await;
    let every = harness.call(MAIN, "list_workers", json!({})).await;
    assert_eq!(ids(&every["areas"], "name"), ["api", "web"], "{every}");
    let web = harness
        .call(MAIN, "list_workers", json!({"area": "web"}))
        .await;
    assert_eq!(ids(&web["areas"], "name"), ["web"], "{web}");
    let workers = web["workers"].as_array().unwrap();
    assert_eq!(workers.len(), 1, "{web}");
    assert_eq!(workers[0]["area"], "web");
    assert_eq!(workers[0]["role"], "area_lead");
    assert_eq!(web["stopped_count"], 0);
}

#[tokio::test]
async fn wait_for_events_with_an_area_wakes_only_for_the_tasks_of_that_area() {
    let fake = FakeClaude::new("result");
    let harness = two_areas(&fake).await;
    let web = harness
        .call(
            MAIN,
            "wait_for_events",
            json!({"types": ["task.created"], "area": "web", "timeout_s": 1}),
        )
        .await;
    let tasks: Vec<&str> = web["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["task"].as_str().unwrap())
        .collect();
    assert_eq!(tasks, ["T-1", "T-3"], "{web}");

    for (title, label) in [("Store the settings", "server"), ("Style the form", "ui")] {
        harness
            .call(
                MAIN,
                "create_task",
                json!({"title": title, "labels": [label]}),
            )
            .await;
    }
    let api = harness
        .call(
            MAIN,
            "wait_for_events",
            json!({"types": ["task.created"], "area": "api", "timeout_s": 1}),
        )
        .await;
    let tasks: Vec<&str> = api["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["task"].as_str().unwrap())
        .collect();
    assert_eq!(
        tasks,
        ["T-4"],
        "the cursor moved past the web events: {api}"
    );
    let after = harness
        .call(
            MAIN,
            "wait_for_events",
            json!({"types": ["task.created"], "timeout_s": 1}),
        )
        .await;
    assert_eq!(
        after,
        json!({"timed_out": true}),
        "the area wait moved the cursor past T-5 too"
    );
}

#[tokio::test]
async fn an_area_wait_still_wakes_for_its_own_messages_and_told_stalls_only() {
    let fake = FakeClaude::new("result");
    let harness = two_areas(&fake).await;
    harness.drain_events(MAIN).await;
    harness
        .send("backend", &["review"], "not for the waiter")
        .await;
    let own = harness.send("backend", &[MAIN], "for the waiter").await;
    let stalled = |agent: &str, told: &str| {
        NewEvent::new(types::WORKER_STALLED, EventActor::parse("fold").unwrap())
            .unwrap()
            .data(json!({"agent": agent, "reason": "silent", "call": null, "minutes": 5, "told": [told]}))
            .unwrap()
    };
    let batch = [
        stalled("api-worker", "backend"),
        stalled("web-worker", MAIN),
    ];
    let now = harness.handle.clock.now();
    let mut buffer = EventBuffer::new();
    harness
        .handle
        .store
        .write(async |tx| {
            for event in &batch {
                buffer.append(tx, event, now).await?;
            }
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    harness.handle.bus.publish(buffer);

    let woken = harness
        .call(
            MAIN,
            "wait_for_events",
            json!({"area": "web", "timeout_s": 1}),
        )
        .await;
    let events = woken["events"].as_array().unwrap();
    let kinds: Vec<&str> = events
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["message.sent", "worker.stalled"], "{woken}");
    assert_eq!(events[0]["data"]["message"], own.as_str(), "{woken}");
    assert_eq!(events[1]["data"]["agent"], "web-worker", "{woken}");
}

#[tokio::test]
async fn an_unknown_area_is_refused_with_area_not_found_by_each_tool() {
    let fake = FakeClaude::new("result");
    let harness = two_areas(&fake).await;
    for tool in [
        "wait_for_events",
        "get_ready_tasks",
        "list_workers",
        "get_task_graph",
    ] {
        let refused = harness.refused(MAIN, tool, json!({"area": "mobile"})).await;
        assert_eq!(refused["code"], codes::AREA_NOT_FOUND, "{tool}: {refused}");
        assert_eq!(refused["details"]["area"], "mobile", "{tool}");
        assert!(
            refused["hint"].as_str().unwrap().contains("list_workers"),
            "{tool}: {refused}"
        );
    }
}
