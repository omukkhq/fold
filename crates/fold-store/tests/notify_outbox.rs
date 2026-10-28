#![cfg(test)]

use fold_core::{NotifySendId, NotifyTargetId, Timestamp};
use fold_store::projects::notify::{self, NewSend, SeenItem, SendState, TryResult};

use crate::common::{count, open};

fn nt(number: u32) -> NotifyTargetId {
    NotifyTargetId::from_number(number).unwrap()
}

fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

#[tokio::test]
async fn a_notice_waits_in_the_outbox_until_one_send_takes_it() {
    let (_dir, _paths, store) = open().await;
    assert_eq!(notify::cursor(store.read()).await.unwrap(), None);
    store
        .write(async |tx| {
            notify::queue(tx, nt(1), 4, "{\"a\":1}", at(10)).await?;
            notify::queue(tx, nt(1), 5, "{\"a\":2}", at(20)).await?;
            notify::queue(tx, nt(2), 5, "{\"a\":3}", at(30)).await?;
            notify::set_cursor(tx, 5).await?;
            notify::set_cursor(tx, 3).await
        })
        .await
        .unwrap();
    assert_eq!(
        notify::cursor(store.read()).await.unwrap(),
        Some(5),
        "the cursor never moves back"
    );
    assert_eq!(
        notify::waiting(store.read()).await.unwrap(),
        vec![(nt(1), at(10)), (nt(2), at(30))]
    );
    let rows = notify::outbox_of(store.read(), nt(1), 1).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].seq, 4);
    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let send = store
        .write(async |tx| {
            notify::create_send(
                tx,
                &NewSend {
                    target: nt(1),
                    webhook_id_prefix: "demo-x",
                    body: b"{}",
                    summary: "[\"task.moved T-1\"]",
                    notices: 1,
                    test: false,
                    outbox: &ids,
                },
                at(40),
            )
            .await
        })
        .await
        .unwrap();
    assert_eq!(send.number, NotifySendId::from_number(1).unwrap());
    assert_eq!(send.webhook_id, "demo-x-1");
    assert_eq!(count(&store, "SELECT COUNT(*) FROM notify_outbox").await, 2);
    assert_eq!(
        notify::open_sends(store.read()).await.unwrap(),
        vec![send.clone()]
    );
    assert_eq!(notify::cursor(store.read()).await.unwrap(), Some(5));

    assert!(
        store
            .write(async |tx| notify::start_try(tx, send.number, at(41)).await)
            .await
            .unwrap()
    );
    store
        .write(async |tx| notify::reset_sending(tx, at(50)).await)
        .await
        .unwrap();
    let reset = notify::get_send(store.read(), send.number)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (reset.state, reset.next_try_at),
        (SendState::Pending, Some(at(50)))
    );
    assert!(
        !store
            .write(async |tx| notify::send_again(tx, send.number, at(51)).await)
            .await
            .unwrap(),
        "an open send is not sent again"
    );
    store
        .write(async |tx| {
            notify::finish_try(
                tx,
                send.number,
                &TryResult {
                    state: SendState::Delivered,
                    tries: 1,
                    next_try_at: None,
                    status: Some(200),
                    error: None,
                    duration_ms: 12,
                },
            )
            .await
        })
        .await
        .unwrap();
    assert!(notify::open_sends(store.read()).await.unwrap().is_empty());
    assert!(
        !store
            .write(async |tx| notify::start_try(tx, send.number, at(52)).await)
            .await
            .unwrap()
    );
    assert!(
        store
            .write(async |tx| notify::send_again(tx, send.number, at(60)).await)
            .await
            .unwrap()
    );
    let again = notify::get_send(store.read(), send.number)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (again.state, again.tries, again.webhook_id.as_str()),
        (SendState::Pending, 0, "demo-x-1")
    );
    assert_eq!(again.status, Some(200));
}

#[tokio::test]
async fn the_log_keeps_the_newest_sends_and_a_gone_target_leaves_nothing() {
    let (_dir, _paths, store) = open().await;
    for n in 0..5 {
        let send = store
            .write(async |tx| {
                notify::create_send(
                    tx,
                    &NewSend {
                        target: nt(1),
                        webhook_id_prefix: "p",
                        body: b"{}",
                        summary: "[]",
                        notices: 0,
                        test: true,
                        outbox: &[],
                    },
                    at(n),
                )
                .await
            })
            .await
            .unwrap();
        store
            .write(async |tx| {
                notify::finish_try(
                    tx,
                    send.number,
                    &TryResult {
                        state: SendState::GivenUp,
                        tries: 7,
                        next_try_at: None,
                        status: None,
                        error: Some("timeout"),
                        duration_ms: 15_000,
                    },
                )
                .await
            })
            .await
            .unwrap();
    }
    store
        .write(async |tx| notify::prune_log(tx, nt(1), 3).await)
        .await
        .unwrap();
    let log = notify::send_log(store.read(), nt(1), 50).await.unwrap();
    assert_eq!(
        log.iter()
            .map(|send| send.number.number())
            .collect::<Vec<_>>(),
        [5, 4, 3]
    );
    assert_eq!(log[0].error.as_deref(), Some("timeout"));
    store
        .write(async |tx| {
            notify::queue(tx, nt(1), 1, "{}", at(1)).await?;
            notify::queue(tx, nt(2), 1, "{}", at(1)).await
        })
        .await
        .unwrap();
    assert!(notify::has_inactive(store.read(), &[nt(1)]).await.unwrap());
    assert_eq!(
        store
            .write(async |tx| notify::drop_inactive(tx, &[nt(1)]).await)
            .await
            .unwrap(),
        1
    );
    assert!(!notify::has_inactive(store.read(), &[nt(1)]).await.unwrap());
    store
        .write(async |tx| notify::forget_target(tx, nt(1)).await)
        .await
        .unwrap();
    assert_eq!(count(&store, "SELECT COUNT(*) FROM notify_sends").await, 0);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM notify_outbox").await, 0);
}

#[tokio::test]
async fn seen_items_keep_their_first_time() {
    let (_dir, _paths, store) = open().await;
    let item = SeenItem {
        key: "question:m-3".to_owned(),
        kind: "question".to_owned(),
        item: Some("m-3".to_owned()),
        path: "messages?thread=m-2".to_owned(),
        seen_at: at(5),
    };
    store
        .write(async |tx| {
            notify::add_seen(tx, &item).await?;
            notify::add_seen(
                tx,
                &SeenItem {
                    seen_at: at(9),
                    ..item.clone()
                },
            )
            .await
        })
        .await
        .unwrap();
    assert_eq!(notify::seen_items(store.read()).await.unwrap(), vec![item]);
    store
        .write(async |tx| notify::remove_seen(tx, "question:m-3").await)
        .await
        .unwrap();
    assert!(notify::seen_items(store.read()).await.unwrap().is_empty());
    assert_eq!(
        notify::first_seq_since(store.read(), at(0)).await.unwrap(),
        None
    );
}
