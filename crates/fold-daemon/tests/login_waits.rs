#![cfg(test)]

use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::accounts::{AccountName, check_token};
use fold_core::turn_waits::{LOGIN_FIRST_WAIT, LOGIN_RERUN_TURN, WaitReason};
use fold_core::{ManualClock, WorkerState};
use fold_daemon::services::agents::accounts;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::accounts::{
    TOKEN_A, TOKEN_B, accounts_with_two, choose, host_accounts, ran_with, until_started,
};
use crate::common::acceptance::{HUMAN, Harness};
use crate::project_pause::{LEAD, lead_running, until_not_running};
use crate::turn_waits::{
    advance, areas, events_of, fake_with_turn, needs_you, read_fixture, reruns_of, set_turn,
    start_lead, until_idle, until_turns,
};
use crate::worker_resume::{MAIN, until_state, worker};
use crate::workers_support::{FakeClaude, Run};

/// The text of Claude Code's own message in the recorded turn that its login was refused.
const RECORDED: &str = "Not logged in · Please run /login";

/// The recorded turn whose login was refused, with Claude Code's message replaced by `text`.
pub fn login_turn(text: &str) -> String {
    let quoted = serde_json::to_string(text).unwrap();
    read_fixture("synthetic_error").replace(&format!("\"{RECORDED}\""), &quoted)
}

fn login_items(items: &[Value]) -> Vec<&Value> {
    items
        .iter()
        .filter(|item| item["kind"] == "login_refused")
        .collect()
}

/// The run of [`LEAD`] that took the `count`th re-run of its waiting turn.
async fn the_rerun(fake: &FakeClaude, count: usize) -> Run {
    until_turns(fake, LEAD, LOGIN_RERUN_TURN, count)
        .await
        .into_iter()
        .rfind(|run| run.stdin.iter().any(|line| line.contains(LOGIN_RERUN_TURN)))
        .unwrap()
}

#[tokio::test]
async fn each_refused_login_waits_with_one_item_for_the_project_and_no_failed_turn() {
    for text in [
        RECORDED,
        "Invalid API key · Please run /login",
        "API Error: 401 {\"type\":\"error\",\"error\":{\"type\":\"authentication_error\",\"message\":\"Invalid bearer token\"}}",
        "Failed to authenticate: OAuth token revoked",
        "OAuth token has expired. Please obtain a new token or refresh your existing token.",
    ] {
        let dir = TempDir::new().unwrap();
        let (fake, _turn) = fake_with_turn(&dir, &login_turn(text));
        let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
        harness.make_main("frontend").await;
        areas(&harness).await;
        let web = start_lead(&mut harness, "web").await;
        let api = start_lead(&mut harness, "api").await;
        for lead in [&web, &api] {
            let row = until_idle(&harness, lead, |row| row.wait.is_some()).await;
            let wait = row.wait.unwrap();
            assert_eq!(
                (wait.reason, wait.limit),
                (WaitReason::Login, None),
                "{text}"
            );
            assert_eq!(
                row.failed_turns, 0,
                "{text}: a refused login is no failed turn"
            );
            assert_eq!(row.last_error.as_deref(), Some(text));
        }
        let items = needs_you(&harness).await;
        let refused = login_items(&items);
        assert_eq!(refused.len(), 1, "{text}: {items:?}");
        let item = refused[0];
        assert_eq!(item["agent"], Value::Null, "one item for the project");
        assert_eq!(item["agents"], json!(["api-lead", "web-lead"]));
        assert_eq!(item["account"], "default");
        assert_eq!(item["title"], text);
        assert_eq!(item["screen"], "worker");
        assert!(
            item["reason"].as_str().unwrap().contains("`claude login`"),
            "{item}"
        );
        assert_eq!(item["actions"], json!([{"action": "pause"}]));
        assert!(
            !items
                .iter()
                .any(|item| item["kind"] == "worker_model_unreachable"),
            "{text}"
        );
        assert!(
            events_of(&harness, "worker.model_unreachable")
                .await
                .is_empty()
        );
        assert_eq!(
            events_of(&harness, "worker.waiting").await[0],
            json!({"agent": "web-lead", "reason": "login", "limit": null})
        );
        let (_, workers) = harness
            .http(Method::GET, "/api/v1/projects/demo/workers", None, "ana")
            .await;
        for entry in workers["workers"].as_array().unwrap() {
            assert_eq!(entry["waits_for"], "login", "{text}");
            assert_eq!(entry["model_unreachable"], false, "{text}");
        }
        let listed = harness.call("frontend", "list_workers", json!({})).await;
        assert_eq!(listed["workers"][0]["waits_for"], "login");
    }
}

#[tokio::test]
async fn a_refused_login_runs_again_only_after_its_long_backoff_when_nothing_changes() {
    let dir = TempDir::new().unwrap();
    let (fake, turn) = fake_with_turn(&dir, &login_turn(RECORDED));
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main("frontend").await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let first = until_idle(&harness, &lead, |row| row.wait.is_some()).await;
    let since = first.wait.as_ref().unwrap().since;
    advance(&harness, LOGIN_FIRST_WAIT - Duration::from_secs(1));
    advance(&harness, Duration::from_secs(1));
    // The refused re-run ends at the project's time; one that ran before the backoff ended would end a second early.
    let again = until_idle(&harness, &lead, |row| {
        row.wait.as_ref().is_some_and(|wait| wait.tries == 1)
    })
    .await;
    assert_eq!(
        again.wait.unwrap().tried_at,
        since.checked_add(LOGIN_FIRST_WAIT).unwrap(),
        "the first re-run comes after LOGIN_FIRST_WAIT, not before"
    );
    set_turn(&turn, &read_fixture("result"));
    advance(&harness, 2 * LOGIN_FIRST_WAIT);
    let runs = until_turns(&fake, &lead, LOGIN_RERUN_TURN, 2).await;
    assert_eq!(runs.len(), 1, "the turn runs again in the same process");
    assert_eq!(
        reruns_of(&fake, &lead, LOGIN_RERUN_TURN),
        2,
        "the backoff doubles"
    );
    let done = until_idle(&harness, &lead, |row| row.wait.is_none()).await;
    assert_eq!(done.session_id, first.session_id);
    assert_eq!(done.last_error, None);
    assert!(login_items(&needs_you(&harness).await).is_empty());
}

/// A lead on `work-a` whose turn waits for a working login, with the next turn set to end well.
async fn lead_refused() -> (
    TempDir,
    std::path::PathBuf,
    TempDir,
    FakeClaude,
    Harness,
    String,
) {
    let clock = ManualClock::new();
    let (accounts, dir) = accounts_with_two(&clock);
    let turns = TempDir::new().unwrap();
    let (mut fake, turn) = fake_with_turn(&turns, &read_fixture("result"));
    host_accounts(&mut fake, &dir, true);
    let harness = lead_running(&fake, WorkerState::Idle).await;
    assert_eq!(choose(&harness, Some("work-a")).await.0, StatusCode::OK);
    until_started(&fake, LEAD, 2).await;
    until_state(&harness, LEAD, WorkerState::Idle).await;
    let session = worker(&harness, LEAD).await.session_id;
    set_turn(&turn, &login_turn("Invalid API key · Please run /login"));
    harness
        .call(
            MAIN,
            "send_message",
            json!({"to": [LEAD], "body": "look at T-1"}),
        )
        .await;
    until_idle(&harness, LEAD, |row| row.wait.is_some()).await;
    set_turn(&turn, &read_fixture("result"));
    (accounts, dir, turns, fake, harness, session)
}

#[tokio::test]
async fn the_item_of_a_named_account_names_its_token_its_update_and_the_other_accounts() {
    let (_accounts, _dir, _turns, _fake, harness, _session) = lead_refused().await;
    let items = needs_you(&harness).await;
    let refused = login_items(&items);
    assert_eq!(refused.len(), 1, "{items:?}");
    let item = refused[0];
    assert_eq!(item["account"], "work-a");
    assert!(
        item["reason"]
            .as_str()
            .unwrap()
            .contains("`fld account update work-a`"),
        "{item}"
    );
    assert_eq!(
        item["actions"],
        json!([
            {"action": "pause"},
            {"action": "switch_account", "account": "default"},
            {"action": "switch_account", "account": "work-b"}
        ])
    );
    let text = serde_json::to_string(&items).unwrap();
    assert!(!text.contains(TOKEN_A) && !text.contains(TOKEN_B));
}

#[tokio::test]
async fn the_item_keeps_naming_the_refused_account_through_a_pause_and_a_switch() {
    let (_accounts, _dir, _turns, fake, harness, session) = lead_refused().await;
    let paused = harness.call(MAIN, "pause_project", json!({})).await;
    assert_eq!(paused["paused"], true, "{paused}");
    until_not_running(&harness, LEAD).await;
    let (status, body) = choose(&harness, Some("work-b")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = needs_you(&harness).await;
    let refused = login_items(&items);
    assert_eq!(refused.len(), 1, "{items:?}");
    let item = refused[0];
    assert_eq!(
        item["account"], "work-a",
        "the refused account, not the project's"
    );
    assert!(
        item["reason"]
            .as_str()
            .unwrap()
            .contains("`fld account update work-a`"),
        "{item}"
    );
    assert_eq!(
        item["actions"],
        json!([
            {"action": "pause"},
            {"action": "switch_account", "account": "default"},
            {"action": "switch_account", "account": "work-a"}
        ]),
        "the switches go to every account other than the project's"
    );

    let resumed = harness.call(MAIN, "resume_project", json!({})).await;
    assert_eq!(resumed["paused"], false, "{resumed}");
    until_idle(&harness, LEAD, |row| row.wait.is_none()).await;
    let resumed = fake.runs_of(LEAD).pop().unwrap();
    assert!(
        ran_with(&resumed, Some(TOKEN_B)),
        "the resume runs on work-b"
    );
    assert_eq!(resumed.flag("--resume"), Some(session.as_str()));
    assert!(login_items(&needs_you(&harness).await).is_empty());
}

#[tokio::test]
async fn a_new_token_of_the_account_runs_the_refused_turn_again_at_once_in_its_session() {
    use fold_core::Clock as _;
    let (_accounts, dir, _turns, fake, harness, session) = lead_refused().await;
    let later = ManualClock::new();
    later.advance(Duration::from_secs(60)).unwrap();
    accounts::update(
        &dir,
        &AccountName::parse("work-a").unwrap(),
        &check_token(TOKEN_B).unwrap(),
        later.now(),
    )
    .unwrap();
    let rerun = the_rerun(&fake, 1).await;
    assert!(
        ran_with(&rerun, Some(TOKEN_B)),
        "the turn runs on the new token"
    );
    assert_eq!(rerun.flag("--resume"), Some(session.as_str()));
    let done = until_idle(&harness, LEAD, |row| row.wait.is_none()).await;
    assert_eq!(done.session_id, session);
    assert!(login_items(&needs_you(&harness).await).is_empty());
    assert_eq!(
        events_of(&harness, "worker.wait_ended").await,
        [json!({"agent": LEAD, "reason": "login", "limit": null})]
    );
}

#[tokio::test]
async fn a_switch_of_the_account_runs_the_refused_turn_again_at_once_on_the_new_one() {
    let (_accounts, _dir, _turns, fake, harness, session) = lead_refused().await;
    let (status, body) = choose(&harness, Some("work-b")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rerun = the_rerun(&fake, 1).await;
    assert!(ran_with(&rerun, Some(TOKEN_B)), "the turn runs on work-b");
    assert_eq!(rerun.flag("--resume"), Some(session.as_str()));
    until_idle(&harness, LEAD, |row| row.wait.is_none()).await;
    assert!(login_items(&needs_you(&harness).await).is_empty());
}

#[tokio::test]
async fn a_new_login_of_claude_code_restarts_the_refused_turn_in_its_session() {
    let clock = ManualClock::new();
    let (_accounts, dir) = accounts_with_two(&clock);
    let turns = TempDir::new().unwrap();
    let (mut fake, turn) = fake_with_turn(&turns, &read_fixture("result"));
    host_accounts(&mut fake, &dir, true);
    let harness = lead_running(&fake, WorkerState::Idle).await;
    let session = worker(&harness, LEAD).await.session_id;
    set_turn(&turn, &login_turn("OAuth token has expired"));
    harness
        .call(
            MAIN,
            "send_message",
            json!({"to": [LEAD], "body": "look at T-1"}),
        )
        .await;
    until_idle(&harness, LEAD, |row| row.wait.is_some()).await;
    let items = needs_you(&harness).await;
    let item = login_items(&items)[0];
    assert_eq!(item["account"], "default");
    assert!(item["reason"].as_str().unwrap().contains("`claude login`"));
    assert_eq!(
        item["actions"],
        json!([
            {"action": "pause"},
            {"action": "switch_account", "account": "work-a"},
            {"action": "switch_account", "account": "work-b"}
        ])
    );
    let before = fake.runs_of(LEAD).len();
    assert_eq!(reruns_of(&fake, LEAD, LOGIN_RERUN_TURN), 0);

    set_turn(&turn, &read_fixture("result"));
    // `claude login` writes a new credentials file and renames it into place.
    let config = dir.parent().unwrap().join("claude-config");
    let fresh = config.join("credentials.new");
    std::fs::write(&fresh, "{\"login\": 2}\n").unwrap();
    std::fs::rename(&fresh, config.join(".credentials.json")).unwrap();
    let rerun = the_rerun(&fake, 1).await;
    assert_eq!(
        fake.runs_of(LEAD).len(),
        before + 1,
        "the process starts again to read the new login"
    );
    assert_eq!(rerun.flag("--resume"), Some(session.as_str()));
    assert!(ran_with(&rerun, None), "default passes no token");
    until_idle(&harness, LEAD, |row| row.wait.is_none()).await;
    assert!(login_items(&needs_you(&harness).await).is_empty());
    let (_, workers) = harness
        .http(Method::GET, "/api/v1/projects/demo/workers", None, HUMAN)
        .await;
    assert_eq!(workers["workers"][0]["waits_for"], Value::Null);
}

#[tokio::test]
async fn a_daemon_start_runs_a_refused_turn_again_once_its_process_is_up() {
    let clock = ManualClock::new();
    let (_accounts, dir) = accounts_with_two(&clock);
    let turns = TempDir::new().unwrap();
    let (mut fake, turn) = fake_with_turn(&turns, &read_fixture("result"));
    host_accounts(&mut fake, &dir, true);
    let mut harness = lead_running(&fake, WorkerState::Idle).await;
    let session = worker(&harness, LEAD).await.session_id;
    set_turn(&turn, &login_turn(RECORDED));
    harness
        .call(
            MAIN,
            "send_message",
            json!({"to": [LEAD], "body": "look at T-1"}),
        )
        .await;
    until_idle(&harness, LEAD, |row| row.wait.is_some()).await;

    // The person signs Claude Code in while the daemon is down.
    set_turn(&turn, &read_fixture("result"));
    harness.restart().await;
    let rerun = the_rerun(&fake, 1).await;
    assert_eq!(rerun.flag("--resume"), Some(session.as_str()));
    until_idle(&harness, LEAD, |row| row.wait.is_none()).await;
}

#[tokio::test]
async fn a_worker_that_waits_is_never_one_that_cannot_reach_the_model() {
    let dir = TempDir::new().unwrap();
    let (fake, turn) = fake_with_turn(&dir, &read_fixture("result"));
    let harness = lead_running(&fake, WorkerState::Idle).await;
    set_turn(&turn, &login_turn("API Error: 529 Overloaded"));
    for count in 1..=3 {
        harness
            .call(
                MAIN,
                "send_message",
                json!({"to": [LEAD], "body": format!("look at T-{count}")}),
            )
            .await;
        until_idle(&harness, LEAD, |row| row.failed_turns == count).await;
    }
    let failing = |items: &[Value]| {
        items
            .iter()
            .any(|item| item["kind"] == "worker_model_unreachable" && item["agent"] == LEAD)
    };
    assert!(failing(&needs_you(&harness).await));

    set_turn(&turn, &login_turn(RECORDED));
    harness
        .call(
            MAIN,
            "send_message",
            json!({"to": [LEAD], "body": "look at T-4"}),
        )
        .await;
    let row = until_idle(&harness, LEAD, |row| row.wait.is_some()).await;
    assert_eq!(row.failed_turns, 0, "a wait ends the run of failed turns");
    let items = needs_you(&harness).await;
    assert!(!failing(&items), "{items:?}");
    assert_eq!(login_items(&items).len(), 1);
    let (_, workers) = harness
        .http(Method::GET, "/api/v1/projects/demo/workers", None, HUMAN)
        .await;
    let entry = workers["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == LEAD)
        .unwrap();
    assert_eq!(entry["model_unreachable"], false, "{entry}");
    assert_eq!(entry["waits_for"], "login");
    let (_, overview) = harness
        .http(Method::GET, "/api/v1/projects/demo/agents", None, HUMAN)
        .await;
    assert!(
        !serde_json::to_string(&overview)
            .unwrap()
            .contains("\"model_unreachable\":true"),
        "{overview}"
    );
}

#[tokio::test]
async fn a_reviewer_whose_login_is_refused_waits_and_runs_its_turn_again_in_its_session() {
    use crate::worker_resume::{REVIEWER, proposed_by_a_task_worker};
    let dir = TempDir::new().unwrap();
    let (fake, turn) = fake_with_turn(&dir, &read_fixture("result"));
    let (mut harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    until_state(&harness, "review-lead", WorkerState::Idle).await;
    harness.connect("review-lead").await;
    set_turn(&turn, &login_turn("OAuth token has expired"));
    let started = harness
        .call(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-1"]}),
        )
        .await;
    assert_eq!(started["worker"]["name"], REVIEWER, "{started}");
    let row = until_idle(&harness, REVIEWER, |row| row.wait.is_some()).await;
    assert_eq!(row.wait.unwrap().reason, WaitReason::Login);
    assert_eq!(row.failed_turns, 0);
    let items = needs_you(&harness).await;
    let refused = login_items(&items);
    assert_eq!(refused.len(), 1, "{items:?}");
    assert!(
        refused[0]["agents"]
            .as_array()
            .unwrap()
            .contains(&json!(REVIEWER)),
        "{items:?}"
    );

    set_turn(&turn, &read_fixture("result"));
    advance(&harness, LOGIN_FIRST_WAIT);
    let runs = until_turns(&fake, REVIEWER, LOGIN_RERUN_TURN, 1).await;
    assert_eq!(
        runs.len(),
        1,
        "the reviewer's turn runs again in its process"
    );
    let done = until_idle(&harness, REVIEWER, |row| row.wait.is_none()).await;
    assert_eq!(done.session_id, row.session_id);
}
