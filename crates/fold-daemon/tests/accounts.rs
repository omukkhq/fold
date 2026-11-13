#![cfg(test)]

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use fold_core::accounts::{AccountName, check_token};
use fold_core::{ManualClock, WorkerState, codes};
use fold_daemon::services::agents::accounts;
use fold_store::testing::eventually;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::common::acceptance::{HUMAN, Harness, WAIT};
use crate::project_pause::{LEAD, lead_running};
use crate::worker_resume::{MAIN, until_state, worker};
use crate::workers_support::{FakeClaude, Run};

pub(crate) const TOKEN_A: &str = "sk-ant-oat01-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub(crate) const TOKEN_B: &str = "sk-ant-oat01-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const ACCOUNT: &str = "/api/v1/projects/demo/account";
const TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// The host's accounts folder with `work-a` holding [`TOKEN_A`] and `work-b` holding [`TOKEN_B`].
pub(crate) fn accounts_with_two(clock: &ManualClock) -> (TempDir, PathBuf) {
    use fold_core::Clock as _;
    let temp = TempDir::new().unwrap();
    let dir = temp.path().join("accounts");
    for (name, token) in [("work-a", TOKEN_A), ("work-b", TOKEN_B)] {
        accounts::add(
            &dir,
            &AccountName::parse(name).unwrap(),
            &check_token(token).unwrap(),
            clock.now(),
        )
        .unwrap();
    }
    (temp, dir)
}

/// Whether `run` got `token` as its account token. The values are compared here and never printed.
pub(crate) fn ran_with(run: &Run, token: Option<&str>) -> bool {
    run.env.get(TOKEN_ENV).map(String::as_str) == token
}

pub(crate) async fn choose(harness: &Harness, account: Option<&str>) -> (StatusCode, Value) {
    harness
        .http(
            Method::PUT,
            ACCOUNT,
            Some(&json!({ "account": account })),
            HUMAN,
        )
        .await
}

/// The runs of `agent` once there are `count` of them and the newest has read its `initialize` line.
pub(crate) async fn until_started(fake: &FakeClaude, agent: &str, count: usize) -> Vec<Run> {
    eventually(WAIT, async || {
        let runs = fake.runs_of(agent);
        match runs.last() {
            Some(last) if runs.len() == count && !last.stdin.is_empty() => Ok(runs),
            _ => Err(runs.len()),
        }
    })
    .await
}

fn account_events(harness: &[crate::common::acceptance::StoredEvent]) -> Vec<Value> {
    harness
        .iter()
        .filter(|event| event.kind == "workers.account_changed")
        .map(|event| event.data.clone())
        .collect()
}

/// Every file below `dir` whose bytes hold `needle`, without following a link.
fn files_holding(dir: &Path, needle: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            found.extend(files_holding(&path, needle));
        } else if kind.is_file()
            && std::fs::read(&path)
                .is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle.as_bytes()))
        {
            found.push(path);
        }
    }
    found
}

#[tokio::test]
async fn a_change_of_the_account_resumes_each_idle_worker_in_its_own_session_with_the_new_token() {
    let clock = ManualClock::new();
    let (_accounts, dir) = accounts_with_two(&clock);
    let mut fake = FakeClaude::new("result");
    fake.launcher.accounts = Some(dir);
    let harness = lead_running(&fake, WorkerState::Idle).await;
    let first = &fake.runs_of(LEAD)[0];
    assert!(
        ran_with(first, None),
        "without an account the login stays as it is"
    );
    assert!(!first.env.contains_key("FOLD_ACCOUNT"));
    let session = worker(&harness, LEAD).await.session_id;

    let (status, body) = choose(&harness, Some("work-a")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({"account": "work-a", "accounts": ["work-a", "work-b"]})
    );
    let runs = until_started(&fake, LEAD, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(session.as_str()));
    assert!(
        ran_with(&runs[1], Some(TOKEN_A)),
        "the resume has the token of work-a"
    );
    assert_eq!(
        runs[1].env.get("FOLD_ACCOUNT").map(String::as_str),
        Some("work-a")
    );
    assert!(
        !runs[1].args.iter().any(|arg| arg.contains(TOKEN_A)),
        "no argument holds the token"
    );
    assert_eq!(
        runs[1].stdin.len(),
        1,
        "an idle lead with nothing waiting gets no turn"
    );
    until_state(&harness, LEAD, WorkerState::Idle).await;

    let (status, body) = choose(&harness, Some("work-b")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let runs = until_started(&fake, LEAD, 3).await;
    assert_eq!(runs[2].flag("--resume"), Some(session.as_str()));
    assert!(
        ran_with(&runs[2], Some(TOKEN_B)),
        "the resume has the token of work-b"
    );
    let row = until_state(&harness, LEAD, WorkerState::Idle).await;
    assert_eq!(row.session_id, session, "no session is lost");

    let (status, _) = choose(&harness, Some("work-b")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = choose(&harness, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["account"], Value::Null);
    let runs = until_started(&fake, LEAD, 4).await;
    assert!(ran_with(&runs[3], None), "Claude Code's own login again");
    assert_eq!(runs[3].flag("--resume"), Some(session.as_str()));
    let events = harness.events().await;
    assert_eq!(
        account_events(&events),
        [
            json!({"old": null, "new": "work-a"}),
            json!({"old": "work-a", "new": "work-b"}),
            json!({"old": "work-b", "new": null}),
        ],
        "a choice of the same account stores nothing"
    );
    assert!(
        !events.iter().any(|event| event.kind == "worker.stopped"),
        "a switch stops no worker"
    );
    for event in &events {
        let text = event.data.to_string();
        assert!(
            !text.contains(TOKEN_A) && !text.contains(TOKEN_B),
            "an event holds a token"
        );
    }
    let config =
        std::fs::read_to_string(Path::new(harness.root()).join(".fold/config.toml")).unwrap();
    assert!(!config.contains("account"), "{config}");
}

#[tokio::test]
async fn a_working_worker_keeps_its_process_until_its_turn_ends() {
    let clock = ManualClock::new();
    let (_accounts, dir) = accounts_with_two(&clock);
    let mut fake = FakeClaude::new("result").holding_resumed_turns();
    fake.launcher.accounts = Some(dir);
    let harness = lead_running(&fake, WorkerState::Idle).await;
    assert_eq!(choose(&harness, Some("work-a")).await.0, StatusCode::OK);
    until_started(&fake, LEAD, 2).await;
    until_state(&harness, LEAD, WorkerState::Idle).await;

    // A message gives the resumed lead a turn, which the stand-in holds, so the lead works.
    harness
        .call(
            MAIN,
            "send_message",
            json!({"to": [LEAD], "body": "look at T-1"}),
        )
        .await;
    until_state(&harness, LEAD, WorkerState::Working).await;
    assert_eq!(choose(&harness, Some("work-b")).await.0, StatusCode::OK);
    eventually(WAIT, async || {
        let events = harness.events().await;
        (account_events(&events).len() == 2).then_some(()).ok_or(())
    })
    .await;
    tokio::task::yield_now().await;
    let runs = fake.runs_of(LEAD);
    assert_eq!(runs.len(), 2, "the working lead keeps its process");
    assert_eq!(worker(&harness, LEAD).await.state, WorkerState::Working);

    let turns = worker(&harness, LEAD).await.turns;
    fake.release(1, None);
    let runs = until_started(&fake, LEAD, 3).await;
    assert!(ran_with(&runs[2], Some(TOKEN_B)));
    assert_eq!(runs[2].flag("--resume"), runs[1].flag("--resume"));
    assert!(
        !runs[1]
            .stdin
            .iter()
            .any(|line| line.contains("\"interrupt\"")),
        "the turn was never interrupted"
    );
    let row = until_state(&harness, LEAD, WorkerState::Idle).await;
    assert!(row.turns > turns, "the held turn ran to its end");
}

#[tokio::test]
async fn a_notice_that_comes_while_the_process_changes_reaches_the_new_one() {
    let clock = ManualClock::new();
    let (_accounts, dir) = accounts_with_two(&clock);
    let mut fake = FakeClaude::new("result").stubborn();
    fake.launcher.accounts = Some(dir);
    let harness = lead_running(&fake, WorkerState::Idle).await;
    assert_eq!(choose(&harness, Some("work-a")).await.0, StatusCode::OK);
    // The stand-in ignores the closed stdin and SIGINT, so its process lives on until the project's clock reaches SIGTERM.
    let first = fake.runs_of(LEAD).remove(0);
    eventually(WAIT, async || fake.saw_eof(&first).then_some(()).ok_or(())).await;
    harness
        .call(
            MAIN,
            "send_message",
            json!({"to": [LEAD], "body": "look at T-1"}),
        )
        .await;
    let project_clock = harness.project.clock().clone();
    let runs = eventually(WAIT, async || {
        let runs = fake.runs_of(LEAD);
        if runs.len() == 2 && runs[1].stdin.len() >= 2 {
            Ok(runs)
        } else {
            project_clock
                .advance(std::time::Duration::from_secs(6))
                .unwrap();
            Err(runs.len())
        }
    })
    .await;
    assert!(ran_with(&runs[1], Some(TOKEN_A)));
    assert!(
        runs[1].stdin[1].contains("\"type\":\"user\""),
        "the notice is the new process's first turn"
    );
    assert_eq!(
        runs[0].stdin.len(),
        first.stdin.len(),
        "the old process never got it"
    );
}

#[tokio::test]
async fn a_new_token_of_the_same_account_reaches_each_worker_after_its_next_turn() {
    use fold_core::Clock as _;
    let clock = ManualClock::new();
    let (_accounts, dir) = accounts_with_two(&clock);
    let mut fake = FakeClaude::new("result");
    fake.launcher.accounts = Some(dir.clone());
    let harness = lead_running(&fake, WorkerState::Idle).await;
    assert_eq!(choose(&harness, Some("work-a")).await.0, StatusCode::OK);
    until_started(&fake, LEAD, 2).await;
    until_state(&harness, LEAD, WorkerState::Idle).await;
    clock.advance(std::time::Duration::from_secs(60)).unwrap();
    accounts::add(
        &dir,
        &AccountName::parse("work-a").unwrap(),
        &check_token(TOKEN_B).unwrap(),
        clock.now(),
    )
    .unwrap();
    harness
        .call(
            MAIN,
            "send_message",
            json!({"to": [LEAD], "body": "look at T-1"}),
        )
        .await;
    let runs = until_started(&fake, LEAD, 3).await;
    assert!(
        ran_with(&runs[1], Some(TOKEN_A)),
        "the turn ran with the old token"
    );
    assert!(
        ran_with(&runs[2], Some(TOKEN_B)),
        "then the lead continues with the new one"
    );
    assert_eq!(runs[2].flag("--resume"), runs[1].flag("--resume"));
}

#[tokio::test]
async fn the_route_names_accounts_only_and_refuses_an_unknown_one() {
    let clock = ManualClock::new();
    let (_accounts, dir) = accounts_with_two(&clock);
    let mut fake = FakeClaude::new("result");
    fake.launcher.accounts = Some(dir);
    let harness = lead_running(&fake, WorkerState::Idle).await;
    let (status, listed) = harness
        .http(Method::GET, "/api/v1/projects/demo/workers", None, HUMAN)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        listed["account"],
        json!({"account": null, "accounts": ["work-a", "work-b"]})
    );
    let (status, unknown) = choose(&harness, Some("work-c")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{unknown}");
    assert_eq!(unknown["error"]["code"], codes::ACCOUNT_NOT_FOUND);
    assert_eq!(unknown["error"]["details"]["account"], "work-c");
    assert!(
        unknown["error"]["details"]["hint"]
            .as_str()
            .unwrap()
            .contains("fld account add work-c")
    );
    let (status, bad) = choose(&harness, Some("Work C")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    assert_eq!(bad["error"]["code"], codes::INVALID_ACCOUNT_NAME);
    let (status, chosen) = choose(&harness, Some("work-a")).await;
    assert_eq!(status, StatusCode::OK);
    let (_, listed) = harness
        .http(Method::GET, "/api/v1/projects/demo/workers", None, HUMAN)
        .await;
    assert_eq!(listed["account"]["account"], "work-a");
    for body in [&chosen, &listed, &unknown] {
        let text = body.to_string();
        assert!(
            !text.contains(TOKEN_A) && !text.contains(TOKEN_B),
            "an answer holds a token"
        );
    }
    until_started(&fake, LEAD, 2).await;
    let root = Path::new(harness.root());
    assert!(
        files_holding(root, TOKEN_A).is_empty(),
        "a file of the project folder holds the token"
    );
}

#[tokio::test]
async fn a_worker_of_a_project_that_names_an_unknown_account_fails_with_its_reason() {
    let clock = ManualClock::new();
    let (_accounts, dir) = accounts_with_two(&clock);
    let mut fake = FakeClaude::new("result");
    fake.launcher.accounts = Some(dir);
    let harness = lead_running(&fake, WorkerState::Idle).await;
    let config = Path::new(harness.root()).join(".fold/config.toml");
    let text = std::fs::read_to_string(&config).unwrap_or_default();
    std::fs::write(&config, format!("account = \"gone\"\n{text}")).unwrap();
    // The idle lead keeps its process: a start would fail, so no switch happens.
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "review_lead", "name": "reviews"}),
        )
        .await;
    let row = until_state(&harness, "reviews", WorkerState::Failed).await;
    assert_eq!(row.failed_reason.as_deref(), Some(codes::ACCOUNT_NOT_FOUND));
    assert!(
        fake.runs_of("reviews").is_empty(),
        "no Claude Code ran without its account"
    );
    assert_eq!(worker(&harness, LEAD).await.state, WorkerState::Idle);
}
