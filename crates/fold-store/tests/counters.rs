#[cfg(test)]
mod common;

use std::collections::BTreeSet;

use common::{count, exec, open};
use fold_core::IdKind;
use fold_store::{StoreError, counters};

#[tokio::test]
async fn concurrent_allocations_never_repeat() {
    let (_dir, _paths, store) = open().await;
    let mut handles = Vec::new();
    for _ in 0..64 {
        let store = store.clone();
        handles.push(tokio::spawn(async move {
            store
                .write(async |tx| counters::allocate(tx, IdKind::Task).await)
                .await
        }));
    }
    let mut numbers = BTreeSet::new();
    for handle in handles {
        assert!(numbers.insert(handle.await.unwrap().unwrap()));
    }
    assert_eq!(numbers, (1..=64).collect::<BTreeSet<u32>>());

    let rolled_back = store
        .write(async |tx| {
            let taken = counters::allocate(tx, IdKind::Task).await?;
            assert_eq!(taken, 65);
            Err::<(), _>(StoreError::SchemaTooNew { version: 0 })
        })
        .await;
    assert!(rolled_back.is_err());
    let next = store
        .write(async |tx| counters::allocate(tx, IdKind::Task).await)
        .await
        .unwrap();
    assert_eq!(next, 65, "a rolled-back number is not consumed");
}

#[tokio::test]
async fn kinds_count_independently() {
    let (_dir, _paths, store) = open().await;
    let numbers = store
        .write(async |tx| {
            let task = counters::allocate(tx, IdKind::Task).await?;
            let proposal = counters::allocate(tx, IdKind::Proposal).await?;
            let task_again = counters::allocate(tx, IdKind::Task).await?;
            Ok::<_, StoreError>((task, proposal, task_again))
        })
        .await
        .unwrap();
    assert_eq!(numbers, (1, 1, 2));
    assert_eq!(count(&store, "SELECT COUNT(*) FROM counters").await, 2);
}

#[tokio::test]
async fn a_counter_below_one_is_refused() {
    let (_dir, _paths, store) = open().await;
    let error = exec(
        &store,
        "INSERT INTO counters (kind, next) VALUES ('task', 0)",
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.constraint().as_deref(),
        Some("counters_next_positive")
    );
}

#[tokio::test]
async fn integration_numbers_come_from_their_own_counter() {
    let (_dir, _paths, store) = open().await;
    let numbers = store
        .write(async |tx| {
            let first = counters::allocate(tx, IdKind::Integration).await?;
            let revision = counters::allocate(tx, IdKind::Revision).await?;
            let second = counters::allocate(tx, IdKind::Integration).await?;
            Ok::<_, StoreError>((first, revision, second))
        })
        .await
        .unwrap();
    assert_eq!(numbers, (1, 1, 2));
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM counters WHERE kind = 'integration'"
        )
        .await,
        1
    );
}
