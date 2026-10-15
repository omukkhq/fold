#![cfg(test)]

use std::time::Duration;

use crate::common::acceptance::{AGENTS, Harness, SLUG};
use axum::http::{Method, StatusCode};
use fold_core::{AgentName, Clock, Timestamp};
use serde_json::Value;

/// The `session.ended` events after `seq`: agent, reason and time, in agent order.
async fn ended_after(harness: &Harness, seq: i64) -> Vec<(String, Option<String>, i64)> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT data, created_at FROM events WHERE type = 'session.ended' AND seq > ?1 ORDER BY seq",
    )
    .bind(seq)
    .fetch_all(harness.handle.store.read())
    .await
    .unwrap();
    let mut ended: Vec<_> = rows
        .into_iter()
        .map(|(data, at)| {
            let data: Value = serde_json::from_str(&data).unwrap();
            (
                data["agent"].as_str().unwrap().to_owned(),
                data["reason"].as_str().map(str::to_owned),
                at,
            )
        })
        .collect();
    ended.sort();
    ended
}

async fn last_seq(harness: &Harness) -> i64 {
    sqlx::query_scalar("SELECT COALESCE(MAX(seq), 0) FROM events")
        .fetch_one(harness.handle.store.read())
        .await
        .unwrap()
}

/// The ids of the sessions with no `ended_at`, and of those that ended at `at`.
async fn sessions(harness: &Harness, at: Timestamp) -> (Vec<i64>, Vec<i64>) {
    let open = sqlx::query_scalar("SELECT id FROM sessions WHERE ended_at IS NULL ORDER BY id")
        .fetch_all(harness.handle.store.read())
        .await
        .unwrap();
    let ended = sqlx::query_scalar("SELECT id FROM sessions WHERE ended_at = ?1 ORDER BY id")
        .bind(at.as_millis())
        .fetch_all(harness.handle.store.read())
        .await
        .unwrap();
    (open, ended)
}

/// Every agent of the project with its presence and connected session count, from the agents route.
async fn presence(harness: &Harness) -> Vec<(String, String, u64)> {
    let (status, body) = harness
        .socket(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/agents"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| {
            (
                agent["name"].as_str().unwrap().to_owned(),
                agent["presence"].as_str().unwrap().to_owned(),
                agent["sessions"].as_u64().unwrap(),
            )
        })
        .collect()
}

/// Every agent of [`AGENTS`] offline with no session, in registration order as the route lists them.
fn offline() -> Vec<(String, String, u64)> {
    AGENTS
        .iter()
        .map(|agent| ((*agent).to_owned(), "offline".to_owned(), 0))
        .collect()
}

/// A daemon dropped with three sessions open: the restarted project ends each of them at its start time with one `session.ended` of reason `daemon_restart`, before any request reaches it, and every agent shows offline with no push target.
#[tokio::test]
async fn sessions_open_at_a_drop_end_at_the_start_time_with_session_ended() {
    let mut harness = Harness::start().await;
    let _inbox = harness.register_inbox("frontend").await;
    let clock = harness.project.clock().clone();
    let (open, _) = sessions(&harness, clock.now()).await;
    assert_eq!(open.len(), AGENTS.len());
    let before = last_seq(&harness).await;

    harness.drop_daemon().await;
    clock.advance(Duration::from_secs(5)).unwrap();
    let start = clock.now();
    harness.start_again().await;

    let (still_open, ended) = sessions(&harness, start).await;
    assert_eq!(still_open, Vec::<i64>::new());
    assert_eq!(ended, open);
    let events = ended_after(&harness, before).await;
    let mut expected: Vec<(String, Option<String>, i64)> = AGENTS
        .iter()
        .map(|agent| {
            (
                (*agent).to_owned(),
                Some("daemon_restart".to_owned()),
                start.as_millis(),
            )
        })
        .collect();
    expected.sort();
    assert_eq!(events, expected);
    assert_eq!(presence(&harness).await, offline());
    let frontend = AgentName::parse("frontend").unwrap();
    assert!(harness.handle.push.sockets(&frontend).is_empty());

    // A second start finds nothing left to end.
    let after = last_seq(&harness).await;
    harness.drop_daemon().await;
    harness.start_again().await;
    assert_eq!(ended_after(&harness, after).await, []);
}

/// A graceful stop ends the sessions at the stop time, so the restarted project ends none and records no `session.ended`; every agent still shows offline.
#[tokio::test]
async fn sessions_ended_by_a_graceful_stop_get_no_second_end_at_start() {
    let mut harness = Harness::start().await;
    let clock = harness.project.clock().clone();
    let (open, _) = sessions(&harness, clock.now()).await;
    let stopped = clock.now();
    harness.stop().await;

    clock.advance(Duration::from_secs(5)).unwrap();
    harness.start_again().await;

    let (still_open, ended) = sessions(&harness, stopped).await;
    assert_eq!(still_open, Vec::<i64>::new());
    assert_eq!(ended, open);
    let mut expected: Vec<(String, Option<String>, i64)> = AGENTS
        .iter()
        .map(|agent| ((*agent).to_owned(), None, stopped.as_millis()))
        .collect();
    expected.sort();
    assert_eq!(ended_after(&harness, 0).await, expected);
    assert_eq!(presence(&harness).await, offline());
}
