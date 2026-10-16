pub mod acceptance;
pub mod invariants;
pub mod roles;
pub mod sse;

/// The questions to humans that no human has answered in their thread yet, in number order, each with its task, as the needs-you list reads them.
pub async fn awaiting_human(store: &fold_store::Store) -> Vec<fold_store::messages::MessageRow> {
    let mut conn = store.read().acquire().await.unwrap();
    fold_store::messages::awaiting_human(&mut conn)
        .await
        .unwrap()
}
