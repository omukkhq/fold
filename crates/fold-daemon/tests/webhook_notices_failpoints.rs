#![cfg(test)]

use std::time::Duration;

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
