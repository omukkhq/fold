#![cfg(test)]

use fold_core::codes;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, WAIT};
use crate::workers_support::FakeClaude;

const MAIN: &str = "frontend";
const WEB_LEAD: &str = "web-lead";
const API_LEAD: &str = "api-lead";

/// Two areas: `web` over label `ui` with `T-1` to `T-4`, and `api` over label `srv` with `T-5` to `T-7`, each with its lead, and at most two task workers at once.
async fn two_areas(harness: &mut Harness) {
    harness.make_main(MAIN).await;
    let config = std::path::Path::new(harness.root()).join(".fold/config.toml");
    std::fs::write(&config, "[workers]\ntask_worker = 2\n").unwrap();
    for (area, label, count) in [("web", "ui", 4), ("api", "srv", 3)] {
        harness
            .call(
                MAIN,
                "create_label",
                json!({"name": label, "color": "sage"}),
            )
            .await;
        harness
            .call(MAIN, "define_area", json!({"name": area, "label": label}))
            .await;
        for page in 1..=count {
            harness
                .call(
                    MAIN,
                    "create_task",
                    json!({"title": format!("{area} part {page}"), "labels": [label]}),
                )
                .await;
        }
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
    harness.connect(WEB_LEAD).await;
    harness.connect(API_LEAD).await;
}

fn task_worker(task: &str, queue: bool) -> Value {
    json!({"role": "task_worker", "tasks": [task], "queue": queue})
}

async fn start(harness: &Harness, lead: &str, task: &str) -> String {
    let started = harness
        .call(lead, "start_worker", task_worker(task, false))
        .await;
    started["worker"]["name"].as_str().unwrap().to_owned()
}

/// The tasks of each queued `worker.started`, in the order fold ran them, once there are `count`.
async fn until_queued_started(harness: &Harness, count: usize) -> Vec<Value> {
    eventually(WAIT, async || {
        let started: Vec<Value> = harness
            .events()
            .await
            .into_iter()
            .filter(|event| event.kind == "worker.started" && event.data["queued"] == true)
            .map(|event| event.data)
            .collect();
        if started.len() == count {
            Ok(started)
        } else {
            Err(started.len())
        }
    })
    .await
}

/// The stored `worker.queued_start_dropped` of the start named `name`, `null` for one without a name, once there is one.
async fn until_dropped(harness: &Harness, name: &Value) -> crate::common::acceptance::StoredEvent {
    eventually(WAIT, async || {
        harness
            .events()
            .await
            .into_iter()
            .find(|event| {
                event.kind == "worker.queued_start_dropped" && event.data["name"] == *name
            })
            .ok_or(())
    })
    .await
}

/// The bodies of the messages from fold in `agent`'s inbox.
async fn fold_notices(harness: &Harness, agent: &str) -> Vec<String> {
    let inbox = harness.call(agent, "get_inbox", json!({})).await;
    inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["from"] == "fold")
        .map(|message| message["body"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn queued_starts_of_two_areas_take_the_free_places_in_turn_and_tell_their_lead() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    two_areas(&mut harness).await;
    let first_web = start(&harness, WEB_LEAD, "T-1").await;
    let first_api = start(&harness, API_LEAD, "T-5").await;

    let full = harness
        .refused(WEB_LEAD, "start_worker", task_worker("T-2", false))
        .await;
    assert_eq!(full["code"], codes::WORKER_LIMIT_REACHED);
    assert!(
        full["hint"].as_str().unwrap().contains("queue: true"),
        "{full}"
    );

    let queued = harness
        .call(WEB_LEAD, "start_worker", task_worker("T-2", true))
        .await;
    assert_eq!(queued["queued"], true);
    assert_eq!(queued["position"], 1);
    assert_eq!(queued["area"], "web");
    let second = harness
        .call(WEB_LEAD, "start_worker", task_worker("T-3", true))
        .await;
    assert_eq!(second["position"], 2);
    let api = harness
        .call(API_LEAD, "start_worker", task_worker("T-6", true))
        .await;
    assert_eq!(api["position"], 2, "one place for each area in a round");

    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    let queue: Vec<(Value, Value)> = listed["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| (entry["area"].clone(), entry["tasks"][0].clone()))
        .collect();
    assert_eq!(
        queue,
        [
            (json!("web"), json!("T-2")),
            (json!("api"), json!("T-6")),
            (json!("web"), json!("T-3")),
        ]
    );

    harness
        .call(MAIN, "stop_worker", json!({"name": first_web}))
        .await;
    let started = until_queued_started(&harness, 1).await;
    assert_eq!(started[0]["tasks"], json!(["T-2"]));
    harness
        .call(MAIN, "stop_worker", json!({"name": first_api}))
        .await;
    let started = until_queued_started(&harness, 2).await;
    assert_eq!(
        started[1]["tasks"],
        json!(["T-6"]),
        "the api area gets the next place"
    );
    let second_web = started[0]["agent"].as_str().unwrap().to_owned();
    harness
        .call(MAIN, "stop_worker", json!({"name": second_web}))
        .await;
    let started = until_queued_started(&harness, 3).await;
    assert_eq!(started[2]["tasks"], json!(["T-3"]));

    let stopped = harness
        .events()
        .await
        .into_iter()
        .find(|event| event.kind == "worker.stopped")
        .unwrap();
    assert_eq!(stopped.data["role"], "task_worker");
    assert_eq!(stopped.data["area"], "web");

    let notices = fold_notices(&harness, WEB_LEAD).await;
    for agent in [&second_web, started[2]["agent"].as_str().unwrap()] {
        assert!(
            notices
                .iter()
                .any(|body| body.contains(&format!("fold started @{agent}"))),
            "{notices:?}"
        );
    }
    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    assert_eq!(listed["queue"], json!([]));
}

#[tokio::test]
async fn a_queued_start_whose_task_was_archived_drops_with_a_notice() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    two_areas(&mut harness).await;
    start(&harness, WEB_LEAD, "T-1").await;
    start(&harness, API_LEAD, "T-5").await;
    harness
        .call(WEB_LEAD, "start_worker", task_worker("T-2", true))
        .await;
    harness
        .call(MAIN, "archive_task", json!({"id": "T-2"}))
        .await;
    let dropped = eventually(WAIT, async || {
        harness
            .events()
            .await
            .into_iter()
            .find(|event| event.kind == "worker.queued_start_dropped")
            .ok_or(())
    })
    .await;
    assert_eq!(dropped.data["reason"], "task_archived");
    assert_eq!(dropped.data["tasks"], json!(["T-2"]));
    let notices = fold_notices(&harness, WEB_LEAD).await;
    assert!(
        notices
            .iter()
            .any(|body| body.contains("fold dropped the worker start you queued")),
        "{notices:?}"
    );
    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    assert_eq!(listed["queue"], json!([]));
}

#[tokio::test]
async fn a_queue_refuses_scopes_a_taken_name_and_work_that_is_taken() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    two_areas(&mut harness).await;
    start(&harness, WEB_LEAD, "T-1").await;
    start(&harness, API_LEAD, "T-5").await;
    let named = harness
        .refused(
            WEB_LEAD,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-2"], "name": API_LEAD, "queue": true}),
        )
        .await;
    assert_eq!(named["code"], codes::WORKER_EXISTS);
    let claimed = harness
        .refused(WEB_LEAD, "start_worker", task_worker("T-1", true))
        .await;
    assert_eq!(claimed["code"], codes::INVALID_REQUEST);
    assert_eq!(claimed["details"]["reason"], "task_claimed");
    let scoped = harness
        .refused(
            WEB_LEAD,
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-1"], "queue": true, "scopes": [{"proposal": "P-1", "scope": "full", "reason": "new"}]}),
        )
        .await;
    assert_eq!(scoped["code"], codes::INVALID_REQUEST);
    assert!(
        scoped["hint"]
            .as_str()
            .unwrap()
            .contains("set_review_scope")
    );
    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    assert_eq!(listed["queue"], json!([]));
}

#[tokio::test]
async fn a_queued_restart_runs_in_a_new_session_or_drops_once_its_worker_runs_again() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    two_areas(&mut harness).await;
    let web = start(&harness, WEB_LEAD, "T-1").await;
    harness
        .call(MAIN, "stop_worker", json!({"name": web}))
        .await;
    let busy = start(&harness, WEB_LEAD, "T-2").await;
    start(&harness, API_LEAD, "T-5").await;

    let queued = harness
        .call(
            WEB_LEAD,
            "restart_worker",
            json!({"name": web, "queue": true}),
        )
        .await;
    assert_eq!(queued["queued"], true, "{queued}");
    assert_eq!(queued["name"], web.as_str());
    harness
        .call(MAIN, "stop_worker", json!({"name": busy}))
        .await;
    let refreshed = eventually(WAIT, async || {
        harness
            .events()
            .await
            .into_iter()
            .find(|event| event.kind == "worker.refreshed" && event.data["queued"] == true)
            .ok_or(())
    })
    .await;
    assert_eq!(refreshed.data["agent"], web.as_str());
    assert!(
        fold_notices(&harness, WEB_LEAD)
            .await
            .iter()
            .any(|body| body.contains(&format!("fold started @{web}"))),
    );

    // Once the restarted worker stops again and its queued restart waits, a restart outside the queue runs it first.
    harness
        .call(MAIN, "stop_worker", json!({"name": web}))
        .await;
    start(&harness, WEB_LEAD, "T-4").await;
    harness
        .call(
            WEB_LEAD,
            "restart_worker",
            json!({"name": web, "queue": true}),
        )
        .await;
    // The follower runs the queue for one event after the other, so once the start queued behind the restart drops, every earlier run is over and the restart waited in each.
    harness
        .call(WEB_LEAD, "start_worker", task_worker("T-3", true))
        .await;
    harness
        .call(MAIN, "archive_task", json!({"id": "T-3"}))
        .await;
    until_dropped(&harness, &Value::Null).await;
    let config = std::path::Path::new(harness.root()).join(".fold/config.toml");
    std::fs::write(&config, "[workers]\ntask_worker = 3\n").unwrap();
    harness
        .call(WEB_LEAD, "restart_worker", json!({"name": web}))
        .await;
    let dropped = until_dropped(&harness, &json!(web)).await;
    assert_eq!(dropped.data["reason"], "cancelled");
    assert_eq!(dropped.data["name"], web.as_str());
    assert!(
        fold_notices(&harness, WEB_LEAD)
            .await
            .iter()
            .any(|body| body.contains("fold dropped the worker start you queued")),
    );
}

#[tokio::test]
async fn a_queued_start_takes_over_an_ended_workers_task_and_the_main_agent_queues_a_restart() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    two_areas(&mut harness).await;
    let ended = start(&harness, WEB_LEAD, "T-1").await;
    harness
        .call(MAIN, "stop_worker", json!({"name": ended}))
        .await;
    let api_ended = start(&harness, API_LEAD, "T-5").await;
    harness
        .call(MAIN, "stop_worker", json!({"name": api_ended}))
        .await;
    let web = start(&harness, WEB_LEAD, "T-2").await;
    let api = start(&harness, API_LEAD, "T-6").await;

    let queued = harness
        .call(WEB_LEAD, "start_worker", task_worker("T-1", true))
        .await;
    assert_eq!(queued["queued"], true, "{queued}");
    let restart = harness
        .call(
            MAIN,
            "restart_worker",
            json!({"name": api_ended, "queue": true}),
        )
        .await;
    assert_eq!(restart["queued"], true, "{restart}");

    harness
        .call(MAIN, "stop_worker", json!({"name": web}))
        .await;
    let started = until_queued_started(&harness, 1).await;
    assert_eq!(started[0]["tasks"], json!(["T-1"]));
    harness
        .call(MAIN, "stop_worker", json!({"name": api}))
        .await;
    let refreshed = eventually(WAIT, async || {
        harness
            .events()
            .await
            .into_iter()
            .find(|event| event.kind == "worker.refreshed" && event.data["queued"] == true)
            .ok_or(())
    })
    .await;
    assert_eq!(refreshed.data["agent"], api_ended.as_str());
    assert!(
        !harness
            .events()
            .await
            .iter()
            .any(|event| event.kind == "worker.queued_start_dropped")
    );
}
