#![cfg(test)]

use std::collections::BTreeMap;

use fold_store::Store;
use fold_store::agents;

use crate::common::{count, exec, open};

fn pairs(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(tool, hash)| ((*tool).to_owned(), (*hash).to_owned()))
        .collect()
}

fn map(rows: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs(rows).into_iter().collect()
}

async fn record(store: &Store, fingerprint: &str, rows: &[(&str, &str)]) {
    let rows = pairs(rows);
    store
        .write(async |tx| agents::record_tool_schema_hashes(tx, fingerprint, &rows).await)
        .await
        .unwrap();
}

async fn read(store: &Store, fingerprint: &str) -> BTreeMap<String, String> {
    agents::tool_schema_hashes(store.read(), fingerprint)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_rows_of_two_fingerprints_round_trip() {
    let (_dir, _paths, store) = open().await;
    record(&store, "fp-a", &[("claim_task", "h1"), ("get_task", "h2")]).await;
    record(&store, "fp-b", &[("claim_task", "h9")]).await;
    assert_eq!(
        read(&store, "fp-a").await,
        map(&[("claim_task", "h1"), ("get_task", "h2")])
    );
    assert_eq!(read(&store, "fp-b").await, map(&[("claim_task", "h9")]));
}

#[tokio::test]
async fn a_second_record_of_the_same_rows_is_ignored() {
    let (_dir, _paths, store) = open().await;
    record(&store, "fp-a", &[("claim_task", "h1")]).await;
    record(&store, "fp-a", &[("claim_task", "h1"), ("get_task", "h2")]).await;
    assert_eq!(
        read(&store, "fp-a").await,
        map(&[("claim_task", "h1"), ("get_task", "h2")])
    );
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM tool_schema_hashes").await,
        2
    );
}

#[tokio::test]
async fn a_second_record_with_another_hash_keeps_the_first() {
    let (_dir, _paths, store) = open().await;
    record(&store, "fp-a", &[("claim_task", "h1")]).await;
    record(&store, "fp-a", &[("claim_task", "other")]).await;
    assert_eq!(read(&store, "fp-a").await, map(&[("claim_task", "h1")]));
}

#[tokio::test]
async fn an_unknown_fingerprint_gives_an_empty_map() {
    let (_dir, _paths, store) = open().await;
    record(&store, "fp-a", &[("claim_task", "h1")]).await;
    assert!(read(&store, "fp-none").await.is_empty());
}

#[tokio::test]
async fn an_update_is_refused() {
    let (_dir, _paths, store) = open().await;
    record(&store, "fp-a", &[("claim_task", "h1")]).await;
    let refused = exec(&store, "UPDATE tool_schema_hashes SET hash = 'x'").await;
    assert!(refused.is_err());
    assert_eq!(read(&store, "fp-a").await, map(&[("claim_task", "h1")]));
}

#[tokio::test]
async fn a_delete_is_refused() {
    let (_dir, _paths, store) = open().await;
    record(&store, "fp-a", &[("claim_task", "h1")]).await;
    let refused = exec(&store, "DELETE FROM tool_schema_hashes").await;
    assert!(refused.is_err());
    assert_eq!(read(&store, "fp-a").await, map(&[("claim_task", "h1")]));
}
