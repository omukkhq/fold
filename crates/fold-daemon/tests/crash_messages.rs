#![cfg(test)]

use std::sync::Arc;

use crate::common::invariants::assert_consistent;
use crate::reviews_support::{Fixture, by, names};
use fold_core::events::types;
use fold_core::{Address, MessageBody};
use fold_daemon::services::messages::{MessageService, SendMessage};

fn messages(fx: &Fixture) -> MessageService {
    MessageService::new(
        fx.project.store().clone(),
        fx.project.registry().clone(),
        fx.bus.clone(),
        Arc::new(fx.project.clock().clone()),
    )
}

fn to_backend(text: &str) -> SendMessage {
    SendMessage {
        to: Address::parse_all(&["backend"]).unwrap(),
        body: MessageBody::parse(text).unwrap(),
        task: None,
        proposal: None,
        reply_to: None,
    }
}

/// Sends `frontend`'s message to `backend` with `failpoint` set to return, checks the command reports `failpoint`, and restarts the project as a crash there would.
async fn send_stopped_at(fx: &mut Fixture, failpoint: &str) {
    fail::cfg(failpoint, "return").unwrap();
    let result = messages(fx)
        .send(&by(&fx.frontend.worker), to_backend("the schema changed"))
        .await;
    fail::remove(failpoint);
    assert_eq!(result.unwrap_err().code(), Some("failpoint"));
    fx.restart().await;
}

/// A crash before the commit leaves no message, recipient or `message.sent`; a retry sends it.
#[tokio::test]
async fn a_crash_inside_the_send_transaction_leaves_no_message() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    let seq = fx.last_seq().await;

    send_stopped_at(&mut fx, "send_message_in_transaction").await;

    assert_eq!(fx.count("SELECT COUNT(*) FROM messages").await, 0);
    assert_eq!(fx.count("SELECT COUNT(*) FROM message_recipients").await, 0);
    assert_eq!(fx.last_seq().await, seq);
    let backend = by(&fx.backend.worker);
    assert!(
        messages(&fx)
            .inbox(&backend, false)
            .await
            .unwrap()
            .is_empty()
    );
    assert_consistent(&fx.project).await;

    let sent = messages(&fx)
        .send(&by(&fx.frontend.worker), to_backend("the schema changed"))
        .await
        .unwrap();
    assert_eq!(sent.id.to_string(), "m-1");
    assert_eq!(
        names(&fx.events_after(seq).await),
        [types::MESSAGE_SENT.as_str()]
    );
    assert_consistent(&fx.project).await;
    scenario.teardown();
}

/// A crash after the commit, before the push and the publish, keeps the message, its recipients and its event, and the recipient's inbox returns it.
#[tokio::test]
async fn a_crash_after_the_send_commit_keeps_the_message() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    let seq = fx.last_seq().await;

    send_stopped_at(&mut fx, "send_message_after_commit").await;

    assert_eq!(fx.count("SELECT COUNT(*) FROM messages").await, 1);
    assert_eq!(fx.count("SELECT COUNT(*) FROM message_recipients").await, 1);
    assert_eq!(
        names(&fx.events_after(seq).await),
        [types::MESSAGE_SENT.as_str()]
    );
    let inbox = messages(&fx)
        .inbox(&by(&fx.backend.worker), false)
        .await
        .unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].body, "the schema changed");
    assert_consistent(&fx.project).await;
    scenario.teardown();
}
