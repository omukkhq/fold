#![cfg(test)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::turn_waits::{
    LIMIT_FIRST_WAIT, LIMIT_RERUN_TURN, LIMIT_WAIT_HINT, LimitKind, NETWORK_FIRST_WAIT,
    NETWORK_RERUN_TURN, NETWORK_WAIT_HINT, RESET_MARGIN, WaitReason,
};
use fold_core::{AgentName, WorkerState};
use fold_store::agents::WorkerProcessRow;
use fold_store::testing::eventually;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::common::acceptance::{Harness, WAIT};
use crate::workers_support::{FakeClaude, Run, fixture};

const MAIN: &str = "frontend";

pub const NETWORK_TEXT: &str =
    "API Error: Can't reach the API server — check your internet connection";

/// A fake whose every user turn replays the file it returns, which a test rewrites to change what the next turn does.
pub fn fake_with_turn(dir: &TempDir, content: &str) -> (FakeClaude, PathBuf) {
    let mut fake = FakeClaude::new("result");
    let turn = dir.path().join("turn.jsonl");
    std::fs::write(&turn, content).unwrap();
    for (key, value) in &mut fake.launcher.env {
        if key == "FOLD_FAKE_CLAUDE_TURN" {
            *value = turn.to_string_lossy().into_owned();
        }
    }
    (fake, turn)
}

/// Replaces the turn file in one rename, so a turn under way never reads half of it.
pub fn set_turn(turn: &Path, content: &str) {
    let partial = turn.with_extension("partial");
    std::fs::write(&partial, content).unwrap();
    std::fs::rename(partial, turn).unwrap();
}

pub fn read_fixture(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).unwrap()
}

/// The recorded usage-limit turn with Claude Code's message replaced by `text`.
pub fn limit_turn(text: &str) -> String {
    read_fixture("usage_limit").replace(
        "You've hit your session limit · resets 1:50am (Asia/Tokyo)",
        text,
    )
}

/// Labels `ui` and `server` and the areas `web` and `api` over them.
async fn areas(harness: &Harness) {
    for label in ["ui", "server"] {
        harness
            .call(
                MAIN,
                "create_label",
                json!({"name": label, "color": "sage"}),
            )
            .await;
    }
    for (area, label) in [("web", "ui"), ("api", "server")] {
        harness
            .call(MAIN, "define_area", json!({"name": area, "label": label}))
            .await;
    }
}

async fn start_lead(harness: &mut Harness, area: &str) -> String {
    let started = harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": area}),
        )
        .await;
    let lead = started["worker"]["name"].as_str().unwrap().to_owned();
    harness.connect(&lead).await;
    lead
}

pub async fn worker(harness: &Harness, agent: &str) -> WorkerProcessRow {
    harness
        .handle
        .agents()
        .get_worker(&AgentName::parse(agent).unwrap())
        .await
        .unwrap()
}

/// The row of `agent` once it is idle and `check` holds for it.
pub async fn until_idle(
    harness: &Harness,
    agent: &str,
    check: impl Fn(&WorkerProcessRow) -> bool,
) -> WorkerProcessRow {
    eventually(WAIT, async || {
        let row = worker(harness, agent).await;
        if row.state == WorkerState::Idle && check(&row) {
            Ok(row)
        } else {
            Err((row.state, row.wait))
        }
    })
    .await
}

/// The runs of `agent` once its newest run has read a user turn that holds `text` for the `count`th time.
pub async fn until_turns(fake: &FakeClaude, agent: &str, text: &str, count: usize) -> Vec<Run> {
    eventually(WAIT, async || {
        let runs = fake.runs_of(agent);
        let seen = runs
            .iter()
            .flat_map(|run| run.stdin.iter())
            .filter(|line| line.contains(text))
            .count();
        if seen >= count { Ok(runs) } else { Err(seen) }
    })
    .await
}

pub fn reruns_of(fake: &FakeClaude, agent: &str, text: &str) -> usize {
    fake.runs_of(agent)
        .iter()
        .flat_map(|run| run.stdin.iter())
        .filter(|line| line.contains(text))
        .count()
}

pub async fn events_of(harness: &Harness, kind: &str) -> Vec<Value> {
    harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == kind)
        .map(|event| event.data)
        .collect()
}

pub async fn needs_you(harness: &Harness) -> Vec<Value> {
    let (status, list) = harness
        .http(Method::GET, "/api/v1/projects/demo/needs-you", None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["items"].as_array().unwrap().clone()
}

pub fn advance(harness: &Harness, by: Duration) {
    harness.project.clock().advance(by).unwrap();
}

#[tokio::test]
async fn a_turn_that_cannot_reach_the_model_waits_and_runs_again_in_its_session_once_the_model_answers()
 {
    let dir = TempDir::new().unwrap();
    let (fake, turn) = fake_with_turn(&dir, &read_fixture("network_error"));
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let first = until_idle(&harness, &lead, |row| row.wait.is_some()).await;
    let wait = first.wait.clone().unwrap();
    assert_eq!(
        (wait.reason, wait.limit, wait.tries),
        (WaitReason::Network, None, 0)
    );
    assert_eq!(first.last_error.as_deref(), Some(NETWORK_TEXT));
    assert_eq!(first.failed_turns, 0, "a turn that waits is no failed turn");
    assert_eq!(
        events_of(&harness, "worker.waiting").await,
        [json!({"agent": "web-lead", "reason": "network", "limit": null})]
    );
    assert!(
        events_of(&harness, "worker.model_unreachable")
            .await
            .is_empty()
    );

    let items = needs_you(&harness).await;
    let unreachable: Vec<&Value> = items
        .iter()
        .filter(|item| item["kind"] == "worker_model_unreachable")
        .collect();
    assert_eq!(unreachable.len(), 1, "{items:?}");
    assert_eq!(
        unreachable[0]["agent"],
        Value::Null,
        "one item for the project"
    );
    assert_eq!(unreachable[0]["agents"], json!(["web-lead"]));
    assert_eq!(unreachable[0]["title"], NETWORK_TEXT);
    assert_eq!(unreachable[0]["reason"], NETWORK_WAIT_HINT);
    let (_, workers) = harness
        .http(Method::GET, "/api/v1/projects/demo/workers", None, "ana")
        .await;
    assert_eq!(workers["workers"][0]["waits_for"], "network");
    assert_eq!(workers["workers"][0]["model_unreachable"], false);
    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    assert_eq!(listed["workers"][0]["waits_for"], "network");
    assert_eq!(reruns_of(&fake, &lead, NETWORK_RERUN_TURN), 0);

    // The model still cannot be reached: the re-run fails the same way and the wait keeps its start.
    advance(&harness, NETWORK_FIRST_WAIT);
    until_turns(&fake, &lead, NETWORK_RERUN_TURN, 1).await;
    let again = until_idle(&harness, &lead, |row| {
        row.wait.as_ref().is_some_and(|wait| wait.tries == 1)
    })
    .await;
    assert_eq!(again.wait.unwrap().since, wait.since);
    assert_eq!(events_of(&harness, "worker.waiting").await.len(), 1);
    assert_eq!(again.failed_turns, 0);

    set_turn(&turn, &read_fixture("result"));
    advance(&harness, NETWORK_FIRST_WAIT);
    assert_eq!(
        reruns_of(&fake, &lead, NETWORK_RERUN_TURN),
        1,
        "the second re-run waits twice as long"
    );
    advance(&harness, NETWORK_FIRST_WAIT);
    let runs = until_turns(&fake, &lead, NETWORK_RERUN_TURN, 2).await;
    let done = until_idle(&harness, &lead, |row| row.wait.is_none()).await;
    assert_eq!(runs.len(), 1, "the turn runs again in the same process");
    assert_eq!(done.session_id, first.session_id);
    assert_eq!(done.last_error, None);
    assert_eq!(
        events_of(&harness, "worker.wait_ended").await,
        [json!({"agent": "web-lead", "reason": "network", "limit": null})]
    );
    assert!(
        !needs_you(&harness)
            .await
            .iter()
            .any(|item| item["kind"] == "worker_model_unreachable")
    );
}

#[tokio::test]
async fn a_turn_of_any_worker_that_reaches_the_model_runs_every_turn_that_waits_for_the_network() {
    let dir = TempDir::new().unwrap();
    let (fake, turn) = fake_with_turn(&dir, &read_fixture("network_error"));
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let web = start_lead(&mut harness, "web").await;
    until_idle(&harness, &web, |row| row.wait.is_some()).await;
    let api = start_lead(&mut harness, "api").await;
    until_idle(&harness, &api, |row| row.wait.is_some()).await;
    let items = needs_you(&harness).await;
    let unreachable: Vec<&Value> = items
        .iter()
        .filter(|item| item["kind"] == "worker_model_unreachable")
        .collect();
    assert_eq!(unreachable.len(), 1, "{items:?}");
    assert_eq!(unreachable[0]["agents"], json!(["api-lead", "web-lead"]));

    set_turn(&turn, &read_fixture("result"));
    harness
        .call(MAIN, "send_message", json!({"to": [&web], "body": "go on"}))
        .await;
    until_idle(&harness, &web, |row| row.wait.is_none()).await;
    until_turns(&fake, &api, NETWORK_RERUN_TURN, 1).await;
    until_idle(&harness, &api, |row| row.wait.is_none()).await;
    assert_eq!(
        reruns_of(&fake, &web, NETWORK_RERUN_TURN),
        0,
        "the message's turn ran the waiting turn of its own worker"
    );
}

#[tokio::test]
async fn each_usage_limit_text_waits_with_one_item_that_names_the_limit_and_its_reset() {
    for (text, kind, reset) in [
        (
            "You've hit your session limit · resets 1:50am (Asia/Tokyo)",
            "session",
            Some("1:50am (Asia/Tokyo)"),
        ),
        (
            "You've hit your weekly limit · resets Mon 12:00am",
            "weekly",
            Some("Mon 12:00am"),
        ),
        (
            "You've hit your Opus limit · resets 3:45pm",
            "opus",
            Some("3:45pm"),
        ),
        ("You've hit your Sonnet limit", "sonnet", None),
        ("Claude AI usage limit reached|1789999200", "usage", None),
    ] {
        let dir = TempDir::new().unwrap();
        let (fake, _turn) = fake_with_turn(&dir, &limit_turn(text));
        let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
        harness.make_main(MAIN).await;
        areas(&harness).await;
        let web = start_lead(&mut harness, "web").await;
        let api = start_lead(&mut harness, "api").await;
        for lead in [&web, &api] {
            let row = until_idle(&harness, lead, |row| row.wait.is_some()).await;
            let wait = row.wait.unwrap();
            assert_eq!(wait.reason, WaitReason::UsageLimit, "{text}");
            assert_eq!(wait.limit.map(LimitKind::as_str), Some(kind));
            assert_eq!(row.failed_turns, 0, "{text}");
        }
        let items = needs_you(&harness).await;
        let limits: Vec<&Value> = items
            .iter()
            .filter(|item| item["kind"] == "usage_limit")
            .collect();
        assert_eq!(limits.len(), 1, "{text}: {items:?}");
        let item = limits[0];
        assert_eq!(item["limit"], kind);
        assert_eq!(item["reset"], json!(reset));
        assert_eq!(item["agents"], json!(["api-lead", "web-lead"]));
        assert_eq!(item["agent"], Value::Null);
        assert_eq!(item["screen"], "worker");
        assert_eq!(item["reason"], LIMIT_WAIT_HINT);
        assert_eq!(item["actions"], json!([{"action": "pause"}]));
        assert_eq!(
            item["resets_at"],
            if kind == "usage" {
                json!(1_789_999_200_000_i64)
            } else {
                Value::Null
            }
        );
        let (_, workers) = harness
            .http(Method::GET, "/api/v1/projects/demo/workers", None, "ana")
            .await;
        for entry in workers["workers"].as_array().unwrap() {
            assert_eq!(entry["waits_for"], "usage_limit", "{text}");
        }
        assert!(
            !items
                .iter()
                .any(|item| item["kind"] == "worker_model_unreachable"),
            "{text}"
        );
        assert_eq!(
            events_of(&harness, "worker.waiting").await.len(),
            2,
            "{text}"
        );
    }
}

#[tokio::test]
async fn a_known_reset_runs_every_turn_at_the_limit_again_and_an_unknown_one_tries_one_first() {
    let dir = TempDir::new().unwrap();
    let (fake, turn) = fake_with_turn(
        &dir,
        &limit_turn("Claude AI usage limit reached|1789999200"),
    );
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let web = start_lead(&mut harness, "web").await;
    until_idle(&harness, &web, |row| row.wait.is_some()).await;
    let api = start_lead(&mut harness, "api").await;
    until_idle(&harness, &api, |row| row.wait.is_some()).await;

    set_turn(
        &turn,
        &limit_turn("You've hit your weekly limit · resets Mon 12:00am"),
    );
    let reset = fold_core::Timestamp::from_millis(1_789_999_200_000).unwrap();
    harness
        .project
        .clock()
        .set(reset.checked_add(RESET_MARGIN).unwrap());
    until_turns(&fake, &web, LIMIT_RERUN_TURN, 1).await;
    until_turns(&fake, &api, LIMIT_RERUN_TURN, 1).await;
    // Both turns reach the weekly limit now, whose reset has no instant: only the oldest wait tries again.
    for lead in [&web, &api] {
        until_idle(&harness, lead, |row| {
            row.wait
                .as_ref()
                .is_some_and(|wait| wait.limit == Some(LimitKind::Weekly))
        })
        .await;
    }
    assert_eq!(
        events_of(&harness, "worker.wait_ended").await.len(),
        0,
        "a wait for another limit is still a wait"
    );
    // Both waits started at the same time, so the first name tries first.
    advance(&harness, LIMIT_FIRST_WAIT);
    until_turns(&fake, &api, LIMIT_RERUN_TURN, 2).await;
    until_idle(&harness, &api, |row| {
        row.wait.as_ref().is_some_and(|wait| wait.tries == 1)
    })
    .await;
    assert_eq!(reruns_of(&fake, &web, LIMIT_RERUN_TURN), 1);

    set_turn(&turn, &read_fixture("result"));
    advance(&harness, LIMIT_FIRST_WAIT.saturating_mul(2));
    until_idle(&harness, &api, |row| row.wait.is_none()).await;
    until_turns(&fake, &web, LIMIT_RERUN_TURN, 2).await;
    until_idle(&harness, &web, |row| row.wait.is_none()).await;
    assert!(
        !needs_you(&harness)
            .await
            .iter()
            .any(|item| item["kind"] == "usage_limit")
    );
    assert_eq!(fake.runs_of(&web).len(), 1);
    assert_eq!(fake.runs_of(&api).len(), 1);
}

#[tokio::test]
async fn a_daemon_start_resumes_a_waiting_worker_in_its_session_and_runs_its_turn_again() {
    let dir = TempDir::new().unwrap();
    let (fake, turn) = fake_with_turn(&dir, &read_fixture("network_error"));
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let waiting = until_idle(&harness, &lead, |row| row.wait.is_some()).await;

    set_turn(&turn, &read_fixture("result"));
    harness.restart().await;
    eventually(WAIT, async || {
        if fake.runs_of(&lead).len() == 2 {
            Ok(())
        } else {
            Err(())
        }
    })
    .await;
    let row = worker(&harness, &lead).await;
    assert!(row.wait.is_some(), "the wait outlives the daemon");
    advance(&harness, NETWORK_FIRST_WAIT);
    let runs = until_turns(&fake, &lead, NETWORK_RERUN_TURN, 1).await;
    assert_eq!(
        runs[1].flag("--resume"),
        Some(waiting.session_id.as_str()),
        "the turn runs again in the same session"
    );
    let done = until_idle(&harness, &lead, |row| row.wait.is_none()).await;
    assert_eq!(done.session_id, waiting.session_id);
}

#[tokio::test]
async fn a_pause_holds_the_waiting_turns_and_its_resume_runs_them() {
    let dir = TempDir::new().unwrap();
    let (fake, turn) = fake_with_turn(&dir, &read_fixture("network_error"));
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    until_idle(&harness, &lead, |row| row.wait.is_some()).await;
    let (status, paused) = harness
        .http(Method::POST, "/api/v1/projects/demo/pause", None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK, "{paused}");
    set_turn(&turn, &read_fixture("result"));
    let (status, resumed) = harness
        .http(Method::POST, "/api/v1/projects/demo/resume", None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    until_idle(&harness, &lead, |row| row.wait.is_none()).await;
    let runs = fake.runs_of(&lead);
    assert_eq!(runs.len(), 2);
    assert!(runs[1].has("--resume"));
}

/// The re-run of a waiting turn ends the wait and its error as soon as the model answers, while the turn still works, and the turns that waited count as none of the worker's turns, so they never use up a lead's turns for its hand-off note.
#[tokio::test]
async fn the_first_answer_of_the_model_ends_the_wait_and_waiting_turns_count_as_no_turns() {
    let dir = TempDir::new().unwrap();
    let (fake, turn) = fake_with_turn(&dir, &read_fixture("network_error"));
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let first = until_idle(&harness, &lead, |row| row.wait.is_some()).await;
    assert_eq!(first.turns, 0, "a turn that waits is no turn of the worker");
    // A turn that the model answers and that has not ended yet.
    let answering: String = read_fixture("result")
        .lines()
        .filter(|line| !line.contains(r#""type":"result""#))
        .map(|line| format!("{line}\n"))
        .collect();
    set_turn(&turn, &answering);
    advance(&harness, NETWORK_FIRST_WAIT);
    let working = eventually(WAIT, async || {
        let row = worker(&harness, &lead).await;
        if row.state == WorkerState::Working && row.wait.is_none() {
            Ok(row)
        } else {
            Err((row.state, row.wait))
        }
    })
    .await;
    assert_eq!(
        working.last_error, None,
        "the wait's error goes with the wait"
    );
    assert_eq!(working.turns, 0);
    assert_eq!(
        events_of(&harness, "worker.wait_ended").await,
        [json!({"agent": "web-lead", "reason": "network", "limit": null})]
    );
    let (_, workers) = harness
        .http(Method::GET, "/api/v1/projects/demo/workers", None, "ana")
        .await;
    assert_eq!(workers["workers"][0]["waits_for"], Value::Null);
    assert_eq!(workers["workers"][0]["last_error"], Value::Null);
}
