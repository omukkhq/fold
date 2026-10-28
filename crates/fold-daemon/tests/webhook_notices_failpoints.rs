#![cfg(test)]

use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_daemon::services::projects::notify::STALE_TRY;
use serde_json::json;

use crate::webhook_notices::{Fixture, Receiver};

/// The daemon stops after a post reached its receiver and before fold wrote that it was delivered: the restarted daemon sends it once more with the same webhook id, so the receiver drops the repeat, and nothing else.
#[tokio::test]
async fn a_stop_in_the_middle_of_a_send_sends_it_again_with_the_same_id_and_nothing_more() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    let mut receiver = Receiver::start(&[]).await;
    fx.target(
        "webhook",
        &receiver.url,
        false,
        json!({"window_seconds": 0}),
    )
    .await;
    fx.moved("in_progress", "review").await;
    fail::cfg("notify_after_post", "return").unwrap();
    assert!(fx.try_pass().await.is_err());
    let first = receiver.next().await;
    fail::remove("notify_after_post");
    fx.restart().await;
    fx.moved("review", "done").await;
    assert_eq!(fx.pass().await, 2);
    let again = receiver.next().await;
    assert_eq!(
        again.header("webhook-id"),
        first.header("webhook-id"),
        "the send in flight goes again with its id"
    );
    assert_eq!(again.body, first.body);
    assert_eq!(
        receiver.next().await.json()["data"]["notices"][0]["to"],
        "done"
    );
    fx.advance(Duration::from_secs(3600));
    assert_eq!(fx.pass().await, 0);
    assert!(receiver.none_waiting());
    scenario.teardown();
}

/// A try whose record never came, as after a failed write, goes again with the same webhook id once [`STALE_TRY`] passed, without a restart.
#[tokio::test]
async fn a_try_whose_record_never_came_goes_again_without_a_restart() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    let mut receiver = Receiver::start(&[]).await;
    fx.target(
        "webhook",
        &receiver.url,
        false,
        json!({"window_seconds": 0}),
    )
    .await;
    fx.moved("in_progress", "review").await;
    fail::cfg("notify_after_post", "return").unwrap();
    assert!(fx.try_pass().await.is_err());
    let first = receiver.next().await;
    fail::remove("notify_after_post");
    assert_eq!(fx.pass().await, 0, "the try may still be in flight");
    assert!(receiver.none_waiting());
    fx.advance(STALE_TRY + Duration::from_secs(1));
    assert_eq!(fx.pass().await, 1);
    assert_eq!(
        receiver.next().await.header("webhook-id"),
        first.header("webhook-id")
    );
    scenario.teardown();
}

/// A test send in flight when the daemon stopped gets one more try after the restart, and a failed one gives the send up.
#[tokio::test]
async fn a_test_in_flight_at_a_stop_gets_one_more_try() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    let mut receiver = Receiver::start(&[200, 503, 500]).await;
    let (target, _) = fx.target("webhook", &receiver.url, false, json!({})).await;
    fail::cfg("notify_after_post", "return").unwrap();
    let (status, _) = fx
        .call(
            Method::POST,
            &format!("/api/v1/projects/demo/notify/targets/{target}/test"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    receiver.next().await;
    fail::remove("notify_after_post");
    fx.restart().await;
    assert_eq!(fx.pass().await, 0);
    assert_eq!(receiver.next().await.types(), ["webhook.test"]);
    let log = fx.send_log(&target).await;
    assert_eq!(
        (
            log["sends"][0]["state"].as_str(),
            log["sends"][0]["tries"].as_u64()
        ),
        (Some("given_up"), Some(1))
    );
    fx.advance(Duration::from_secs(3600));
    assert_eq!(fx.pass().await, 0);
    assert!(receiver.none_waiting());
    scenario.teardown();
}
