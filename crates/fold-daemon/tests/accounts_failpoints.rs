#![cfg(test)]

use std::path::Path;

use axum::http::StatusCode;
use fold_core::accounts::{AccountName, check_token};
use fold_core::{Clock, ManualClock, WorkerState, codes};
use fold_daemon::services::agents::accounts;
use tempfile::TempDir;

use crate::accounts::{TOKEN_A, TOKEN_B, accounts_with_two, choose};
use crate::project_pause::lead_running;
use crate::workers_support::FakeClaude;

fn name(text: &str) -> AccountName {
    AccountName::parse(text).unwrap()
}

/// The names of the files in `dir` that hold `token`.
fn holding(dir: &Path, token: &str) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .filter(|entry| {
            std::fs::read_to_string(entry.as_ref().unwrap().path())
                .is_ok_and(|text| text.contains(token))
        })
        .count()
}

/// A stop of an update or an add after the new token's file and before the index keeps the old account whole; the next change, or the daemon's start, removes the file the stop left, which holds a token.
#[test]
fn a_stop_before_the_index_keeps_the_old_account_and_leaves_no_token_for_long() {
    let scenario = fail::FailScenario::setup();
    let temp = TempDir::new().unwrap();
    let dir = temp.path().join("accounts");
    let clock = ManualClock::new();
    accounts::add(
        &dir,
        &name("work-a"),
        &check_token(TOKEN_A).unwrap(),
        clock.now(),
    )
    .unwrap();
    fail::cfg("accounts_after_token_file", "return").unwrap();
    let stopped = accounts::update(
        &dir,
        &name("work-a"),
        &check_token(TOKEN_B).unwrap(),
        clock.now(),
    );
    assert_eq!(stopped.unwrap_err().code(), codes::ACCOUNTS_IO);
    let new = accounts::add(
        &dir,
        &name("work-b"),
        &check_token(TOKEN_B).unwrap(),
        clock.now(),
    );
    assert!(new.is_err());
    fail::remove("accounts_after_token_file");
    let (_, token) = accounts::token(&dir, &name("work-a")).unwrap().unwrap();
    assert!(token.expose() == TOKEN_A, "the old token stays");
    assert_eq!(
        accounts::token(&dir, &name("work-b")).unwrap(),
        None,
        "nothing of the new account"
    );
    assert_eq!(
        holding(&dir, TOKEN_B),
        1,
        "each change removes what an earlier stop left, and the last stop left its file"
    );
    accounts::remove_leftovers(&dir).unwrap();
    assert_eq!(holding(&dir, TOKEN_B), 0);
    assert_eq!(holding(&dir, TOKEN_A), 1);
    scenario.teardown();
}

/// A stop of an update after the index names the new token, before the old token's file goes, gives the account its new token whole; the old file goes with the next change.
#[test]
fn a_stop_after_the_index_gives_the_new_account_and_the_old_file_goes_next() {
    let scenario = fail::FailScenario::setup();
    let temp = TempDir::new().unwrap();
    let dir = temp.path().join("accounts");
    let clock = ManualClock::new();
    accounts::add(
        &dir,
        &name("work-a"),
        &check_token(TOKEN_A).unwrap(),
        clock.now(),
    )
    .unwrap();
    fail::cfg("accounts_after_index", "return").unwrap();
    assert!(
        accounts::update(
            &dir,
            &name("work-a"),
            &check_token(TOKEN_B).unwrap(),
            clock.now()
        )
        .is_err()
    );
    let (_, token) = accounts::token(&dir, &name("work-a")).unwrap().unwrap();
    assert!(token.expose() == TOKEN_B, "the new token is the account's");
    assert_eq!(holding(&dir, TOKEN_A), 1, "the old file is still there");
    assert!(accounts::remove(&dir, &name("work-a")).is_err());
    assert_eq!(
        accounts::list(&dir).unwrap(),
        Vec::new(),
        "the removal is made"
    );
    fail::remove("accounts_after_index");
    accounts::add(
        &dir,
        &name("work-c"),
        &check_token(TOKEN_A).unwrap(),
        clock.now(),
    )
    .unwrap();
    assert_eq!(
        holding(&dir, TOKEN_B),
        0,
        "the next change removed the files the stops left"
    );
    assert_eq!(holding(&dir, TOKEN_A), 1);
    scenario.teardown();
}

/// A choice whose commit fails after the config's rename puts the old config back and stores no event, so no worker switches.
#[tokio::test]
async fn a_choice_that_cannot_commit_leaves_the_config_as_it_was() {
    let scenario = fail::FailScenario::setup();
    let clock = ManualClock::new();
    let (_accounts, dir) = accounts_with_two(&clock);
    let mut fake = FakeClaude::new("result");
    crate::accounts::host_accounts(&mut fake, &dir, true);
    let harness = lead_running(&fake, WorkerState::Idle).await;
    let config = Path::new(harness.root()).join(".fold/config.toml");
    let before = std::fs::read_to_string(&config).ok();
    fail::cfg("account_before_commit", "return").unwrap();
    let (status, body) = choose(&harness, Some("work-a")).await;
    fail::remove("account_before_commit");
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(std::fs::read_to_string(&config).ok(), before);
    assert!(
        !harness
            .events()
            .await
            .iter()
            .any(|event| event.kind == "workers.account_changed")
    );
    assert_eq!(fake.runs_of(crate::project_pause::LEAD).len(), 1);
    scenario.teardown();
}
