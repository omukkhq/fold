#![cfg(test)]

use std::time::Duration;

use axum::http::StatusCode;
use fold_core::accounts::{AccountName, check_token};
use fold_core::turn_waits::LOGIN_RERUN_TURN;
use fold_core::{Clock as _, ManualClock, WorkerState};
use fold_daemon::live::ActivityState;
use fold_daemon::services::agents::accounts;
use fold_daemon::services::tasks::sweep_leases;
use fold_daemon::services::workspaces::check_stalls;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::accounts::{TOKEN_A, TOKEN_B, accounts_with_two, choose, host_accounts, ran_with};
use crate::common::acceptance::{Harness, Work};
use crate::login_waits::login_turn;
use crate::turn_waits::{
    events_of, fake_with_turn, needs_you, read_fixture, set_turn, until_idle, until_turns,
};
use crate::worker_resume::{TASK_WORKER, task_worker_for};
use crate::workers_support::{FakeClaude, Run};

const LEAD: &str = "web-lead";
const LEASE: Duration = Duration::from_secs(30 * 60);

async fn activity(harness: &Harness, work: &Work) -> Option<ActivityState> {
    check_stalls(&harness.handle, harness.project.now())
        .await
        .unwrap();
    harness
        .handle
        .live
        .get(work.workspace.parse().unwrap())
        .unwrap()
        .view
        .activity
        .as_ref()
        .map(|activity| activity.state)
}

fn refused(items: &[Value]) -> Vec<&Value> {
    items
        .iter()
        .filter(|item| item["kind"] == "login_refused")
        .collect()
}

/// The newest run of `agent` that took the `count`th re-run of its turn.
async fn rerun_of(fake: &FakeClaude, agent: &str, count: usize) -> Run {
    until_turns(fake, agent, LOGIN_RERUN_TURN, count)
        .await
        .into_iter()
        .rfind(|run| run.stdin.iter().any(|line| line.contains(LOGIN_RERUN_TURN)))
        .unwrap()
}

/// The API refuses the login of a lead and its task worker: their turns wait with neutral chips, the person sees one project item that names the login, the claims stay fresh, and nothing fails. A switch to an account in Needs you runs both turns again at once in their sessions. Later that account's token is refused too; `fld account update` gives it a new token, and the turns run again at once on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_44_a_refused_login_waits_and_a_new_login_runs_the_turns_again_in_their_sessions()
 {
    let clock = ManualClock::new();
    let (_accounts, dir) = accounts_with_two(&clock);
    let turns = TempDir::new().unwrap();
    let (mut fake, turn) = fake_with_turn(&turns, &login_turn("Not logged in · Please run /login"));
    host_accounts(&mut fake, &dir, true);
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    let lead_session = until_idle(&harness, LEAD, |row| row.wait.is_some())
        .await
        .session_id;
    let worker_session = until_idle(&harness, TASK_WORKER, |row| row.wait.is_some())
        .await
        .session_id;

    assert_eq!(
        activity(&harness, &work).await,
        Some(ActivityState::WaitsForLogin)
    );
    let items = needs_you(&harness).await;
    let item = refused(&items);
    assert_eq!(item.len(), 1, "{items:?}");
    assert_eq!(item[0]["account"], "default");
    assert_eq!(item[0]["agents"], json!([LEAD, TASK_WORKER]));
    assert!(
        item[0]["reason"]
            .as_str()
            .unwrap()
            .contains("`claude login`")
    );
    assert_eq!(
        item[0]["actions"],
        json!([
            {"action": "pause"},
            {"action": "switch_account", "account": "work-a"},
            {"action": "switch_account", "account": "work-b"}
        ])
    );
    assert!(
        !items
            .iter()
            .any(|item| item["kind"] == "worker_model_unreachable")
    );
    for _ in 0..3 {
        harness.project.clock().advance(LEASE).unwrap();
        assert_eq!(
            sweep_leases(&harness.handle, harness.project.now())
                .await
                .unwrap()
                .len(),
            0,
            "the claim of a turn that waits for a login stays fresh"
        );
    }
    assert!(events_of(&harness, "worker.stalled").await.is_empty());
    assert!(
        events_of(&harness, "worker.model_unreachable")
            .await
            .is_empty()
    );

    // The person switches to work-a from the item.
    set_turn(&turn, &read_fixture("result"));
    let (status, body) = choose(&harness, Some("work-a")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for (agent, session) in [(LEAD, &lead_session), (TASK_WORKER, &worker_session)] {
        let rerun = rerun_of(&fake, agent, 1).await;
        assert!(ran_with(&rerun, Some(TOKEN_A)), "{agent} runs on work-a");
        assert_eq!(rerun.flag("--resume"), Some(session.as_str()), "{agent}");
        until_idle(&harness, agent, |row| row.wait.is_none()).await;
    }
    assert!(refused(&needs_you(&harness).await).is_empty());
    assert_ne!(
        activity(&harness, &work).await,
        Some(ActivityState::WaitsForLogin)
    );

    // The token of work-a is revoked later.
    set_turn(
        &turn,
        &login_turn("Failed to authenticate: OAuth token revoked"),
    );
    harness
        .call(
            LEAD,
            "send_message",
            json!({"to": [TASK_WORKER], "body": "Add a footer too."}),
        )
        .await;
    until_idle(&harness, TASK_WORKER, |row| row.wait.is_some()).await;
    let items = needs_you(&harness).await;
    let item = refused(&items);
    assert_eq!(item[0]["account"], "work-a");
    assert!(
        item[0]["reason"]
            .as_str()
            .unwrap()
            .contains("`fld account update work-a`")
    );
    let text = serde_json::to_string(&items).unwrap();
    assert!(!text.contains(TOKEN_A) && !text.contains(TOKEN_B));

    set_turn(&turn, &read_fixture("result"));
    let later = ManualClock::new();
    later.advance(Duration::from_secs(60)).unwrap();
    accounts::update(
        &dir,
        &AccountName::parse("work-a").unwrap(),
        &check_token(TOKEN_B).unwrap(),
        later.now(),
    )
    .unwrap();
    let rerun = rerun_of(&fake, TASK_WORKER, 2).await;
    assert!(
        ran_with(&rerun, Some(TOKEN_B)),
        "the turn runs again on the new token"
    );
    assert_eq!(rerun.flag("--resume"), Some(worker_session.as_str()));
    until_idle(&harness, TASK_WORKER, |row| row.wait.is_none()).await;
    assert!(refused(&needs_you(&harness).await).is_empty());
}
