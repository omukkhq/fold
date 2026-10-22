#![cfg(test)]

use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::{Actor, AgentName, Clock, HumanName, WorkerState};
use fold_store::agents::WorkerProcessRow;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, SLUG, WAIT};
use crate::workers_support::FakeClaude;

const MAIN: &str = "frontend";

fn name(text: &str) -> AgentName {
    AgentName::parse(text).unwrap()
}

async fn worker(harness: &Harness, agent: &str) -> WorkerProcessRow {
    harness
        .handle
        .agents()
        .get_worker(&name(agent))
        .await
        .unwrap()
}

async fn until_idle_after(harness: &Harness, agent: &str, turns: u64) -> WorkerProcessRow {
    eventually(WAIT, async || {
        let row = worker(harness, agent).await;
        if row.state == WorkerState::Idle && row.turns >= turns {
            Ok(row)
        } else {
            Err((row.state, row.turns))
        }
    })
    .await
}

async fn get(harness: &Harness, path: &str) -> Value {
    let (status, body) = harness.http(Method::GET, path, None, "ana").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn amount(list: &Value, key: &str, value: &str) -> u64 {
    list.as_array()
        .unwrap()
        .iter()
        .find(|entry| entry[key] == value)
        .unwrap_or_else(|| panic!("no {key} {value} in {list}"))["cost_micro_usd"]
        .as_u64()
        .unwrap()
}

/// The area `web` over label `ui`, with task `T-1` in it, and its lead `web-lead` running.
async fn web_area(harness: &mut Harness) {
    harness.make_main(MAIN).await;
    harness
        .call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    harness
        .call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Draw the page", "labels": ["ui"]}),
        )
        .await;
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    harness.connect("web-lead").await;
}

#[tokio::test]
async fn a_stopped_and_resumed_task_worker_keeps_every_turn_in_the_project_and_task_cost() {
    let fake = FakeClaude::new("result").resuming_with("resumed");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    web_area(&mut harness).await;
    until_idle_after(&harness, "web-lead", 1).await;
    harness
        .call(
            "web-lead",
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    let first = until_idle_after(&harness, "web-worker", 1).await;
    assert!(first.cost_micro_usd > 0);

    let clock = harness.project.clock().clone();
    let stopped = harness
        .handle
        .workers
        .sweep_idle(clock.advance(Duration::from_secs(15 * 60)).unwrap())
        .await
        .unwrap();
    assert_eq!(stopped, [name("web-worker")]);
    let resumed_at = clock.now();
    harness
        .handle
        .agents()
        .resume_worker(
            &Actor::human(HumanName::parse("ana").unwrap()),
            &name("web-worker"),
            "Go on.".to_owned(),
        )
        .await
        .unwrap();
    let second = until_idle_after(&harness, "web-worker", 2).await;
    assert!(second.cost_micro_usd > first.cost_micro_usd);

    // The lead can take more turns meanwhile, so its own numbers are read from the answer.
    let all = get(&harness, &format!("/api/v1/projects/{SLUG}/cost")).await;
    let lead = amount(&all["workers"], "name", "web-lead");
    assert!(lead > 0);
    let total = lead + second.cost_micro_usd;
    assert_eq!(all["total"]["cost_micro_usd"], total);
    assert_eq!(all["total"]["workers"], 2);
    assert_eq!(amount(&all["by_role"], "role", "area_lead"), lead);
    assert_eq!(
        amount(&all["by_role"], "role", "task_worker"),
        second.cost_micro_usd
    );
    assert_eq!(amount(&all["by_role"], "role", "reviewer"), 0);
    assert_eq!(amount(&all["by_area"], "area", "web"), total);
    assert_eq!(
        amount(&all["workers"], "name", "web-worker"),
        second.cost_micro_usd,
        "both processes of the worker count"
    );
    let task = &all["tasks"][0];
    assert_eq!(task["task"], "T-1");
    assert_eq!(task["title"], "Draw the page");
    assert_eq!(task["area"], "web");
    assert_eq!(task["build_micro_usd"], second.cost_micro_usd);
    assert_eq!(task["review_micro_usd"], 0);
    assert_eq!(all["slot_minutes"], 60, "all time has hour slots");
    let slots: u64 = all["slots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|slot| slot["cost_micro_usd"].as_u64().unwrap())
        .sum();
    assert_eq!(slots, total);

    let recent = get(
        &harness,
        &format!(
            "/api/v1/projects/{SLUG}/cost?since={}",
            resumed_at.as_millis()
        ),
    )
    .await;
    let task_workers = recent["by_role"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["role"] == "task_worker")
        .unwrap();
    assert_eq!(
        (&task_workers["cost_micro_usd"], &task_workers["turns"]),
        (
            &json!(second.cost_micro_usd - first.cost_micro_usd),
            &json!(1)
        ),
        "only the turn after the resume ended since then"
    );
    assert_eq!(recent["since"], resumed_at.as_millis());
    assert_eq!(
        recent["slot_minutes"], 15,
        "a range of minutes has quarter hours"
    );

    let of_task = get(&harness, &format!("/api/v1/projects/{SLUG}/cost/tasks/T-1")).await;
    assert_eq!(of_task["build_micro_usd"], second.cost_micro_usd);
    assert_eq!(of_task["review_micro_usd"], 0);
    assert_eq!(of_task["total_micro_usd"], second.cost_micro_usd);
    assert_eq!(
        of_task["workers"],
        json!([{
            "name": "web-worker",
            "role": "task_worker",
            "part": "build",
            "model": "claude-haiku-5-5",
            "proposals": [],
            "turns": 2,
            "cost_micro_usd": second.cost_micro_usd,
            "tokens": {
                "input": 4 + 2,
                "output": 86 + 134,
                "cache_read": 37675 + 26871,
                "cache_write": 12751 + 210,
                "total": 77733,
            },
        }])
    );

    let (status, missing) = harness
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/cost/tasks/T-9"),
            None,
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(missing["error"]["code"], "task_not_found");
    let (status, refused) = harness
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/cost?since=soon"),
            None,
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(refused["error"]["code"], "invalid_request");

    let listed = get(
        &harness,
        &format!("/api/v1/projects?today={}&week=0", resumed_at.as_millis()),
    )
    .await;
    let cost = &listed["projects"][0]["overview"]["cost"];
    let all_time = cost["all_time_micro_usd"].as_u64().unwrap();
    assert!(all_time >= total, "{cost}");
    assert_eq!(cost["week_micro_usd"].as_u64(), Some(all_time));
    let today = cost["today_micro_usd"].as_u64().unwrap();
    assert!(
        today >= second.cost_micro_usd - first.cost_micro_usd && today < all_time,
        "{cost}"
    );

    let shown = get(
        &harness,
        &format!(
            "/api/v1/projects/{SLUG}?today={}&week=0",
            resumed_at.as_millis()
        ),
    )
    .await;
    let cost = &shown["overview"]["cost"];
    assert_eq!(
        cost["week_micro_usd"], cost["all_time_micro_usd"],
        "the project shows the viewer's days as the list does"
    );
    assert!(
        cost["today_micro_usd"].as_u64().unwrap() < cost["all_time_micro_usd"].as_u64().unwrap()
    );
    let (status, refused) = harness
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}?today=soon"),
            None,
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(refused["error"]["code"], "invalid_request");
}

/// Claude Code's `--resume` goes on from the session's running total: the recorded session ends its interrupted turn at $0.00613623 and its resumed turn at $0.00651414, so the resumed turn cost only what the total grew by.
#[tokio::test]
async fn a_resumed_session_counts_only_what_its_running_total_grew_by() {
    let fake = FakeClaude::new("interrupt").resuming_with("resumed");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    web_area(&mut harness).await;
    until_idle_after(&harness, "web-lead", 1).await;
    harness
        .call(
            "web-lead",
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    let first = until_idle_after(&harness, "web-worker", 1).await;
    assert_eq!(first.cost_micro_usd, 6136);

    let clock = harness.project.clock().clone();
    harness
        .handle
        .workers
        .sweep_idle(clock.advance(Duration::from_secs(15 * 60)).unwrap())
        .await
        .unwrap();
    let resumed_at = clock.now();
    harness
        .handle
        .agents()
        .resume_worker(
            &Actor::human(HumanName::parse("ana").unwrap()),
            &name("web-worker"),
            "Go on.".to_owned(),
        )
        .await
        .unwrap();
    let second = until_idle_after(&harness, "web-worker", 2).await;
    assert_eq!(
        second.cost_micro_usd, 6514,
        "6136 and the 378 of the resumed turn"
    );

    let recent = get(
        &harness,
        &format!(
            "/api/v1/projects/{SLUG}/cost?since={}",
            resumed_at.as_millis()
        ),
    )
    .await;
    assert_eq!(amount(&recent["workers"], "name", "web-worker"), 378);
    let of_task = get(&harness, &format!("/api/v1/projects/{SLUG}/cost/tasks/T-1")).await;
    assert_eq!(of_task["build_micro_usd"], 6514);
    // The resumed turn's own usage, not the session's running `modelUsage` of 321934 cache reads.
    let tokens = json!({
        "input": 2,
        "output": 134,
        "cache_read": 26871,
        "cache_write": 210,
        "total": 27217,
    });
    assert_eq!(of_task["tokens"], tokens);
    assert_eq!(of_task["workers"][0]["tokens"], tokens);
    let worker = recent["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "web-worker")
        .unwrap();
    assert_eq!(worker["tokens"], tokens);
}
