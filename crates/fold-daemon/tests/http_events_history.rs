#![cfg(test)]

use crate::daemon_support::Opened;
use axum::http::StatusCode;
use fold_core::EventFilter;
use fold_store::events;
use serde_json::Value;

fn seqs(body: &Value) -> Vec<u64> {
    body["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["seq"].as_u64().unwrap())
        .collect()
}

async fn newest(opened: &Opened) -> u64 {
    events::head_seq(opened.handle.store.read()).await.unwrap()
}

#[tokio::test]
async fn history_lists_every_stored_event_newest_first() {
    let opened = Opened::new().await;
    let newest = newest(&opened).await;
    assert!(newest >= 3, "the fixture stores events");

    let (status, body) = opened.get("/events/history").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(seqs(&body), (1..=newest).rev().collect::<Vec<_>>());
    assert_eq!(body["before"], Value::Null);
    let stored = events::read_after(
        opened.handle.store.read(),
        newest - 1,
        &EventFilter::default(),
        1,
    )
    .await
    .unwrap();
    assert_eq!(body["events"][0], serde_json::to_value(&stored[0]).unwrap());
}

#[tokio::test]
async fn history_pages_backwards_with_before() {
    let opened = Opened::new().await;
    let newest = newest(&opened).await;

    let (status, page) = opened.get("/events/history?limit=2").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(seqs(&page), vec![newest, newest - 1]);
    assert_eq!(page["before"].as_u64(), Some(newest - 1));
    let mut seen = seqs(&page);
    let mut before = page["before"].as_u64();
    while let Some(cursor) = before {
        let (status, page) = opened
            .get(&format!("/events/history?limit=2&before={cursor}"))
            .await;
        assert_eq!(status, StatusCode::OK);
        let listed = seqs(&page);
        assert!(!listed.is_empty() && listed.len() <= 2);
        assert!(listed.iter().all(|seq| *seq < cursor));
        seen.extend(listed);
        before = page["before"].as_u64();
    }
    assert_eq!(seen, (1..=newest).rev().collect::<Vec<_>>());
}

#[tokio::test]
async fn history_before_past_the_newest_event_lists_the_newest_events() {
    let opened = Opened::new().await;
    let newest = newest(&opened).await;

    let (status, page) = opened
        .get(&format!("/events/history?limit=2&before={}", newest + 1000))
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(seqs(&page), vec![newest, newest - 1]);
    assert_eq!(page["before"].as_u64(), Some(newest - 1));
}

#[tokio::test]
async fn history_before_the_first_event_is_empty() {
    let opened = Opened::new().await;

    let (status, body) = opened.get("/events/history?before=1").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(seqs(&body), Vec::<u64>::new());
    assert_eq!(body["before"], Value::Null);
}

#[tokio::test]
async fn history_refuses_a_zero_limit_and_a_query_that_is_not_a_number() {
    let opened = Opened::new().await;

    let (status, body) = opened.get("/events/history?limit=0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_event_filter");

    let (status, body) = opened.get("/events/history?before=r7").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_request");
}
