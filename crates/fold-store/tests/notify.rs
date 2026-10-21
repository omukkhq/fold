use std::collections::{BTreeMap, BTreeSet};

use crate::common::open;
use fold_core::{NotifyTargetId, Timestamp};
use fold_store::projects::notify::{
    notify_state, record_notify_send, sync_notify_items, sync_notify_targets,
};

fn keys(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|key| (*key).to_owned()).collect()
}

#[tokio::test]
async fn an_unsure_kind_keeps_its_row_and_every_other_missing_item_loses_it() {
    let (_dir, _paths, store) = open().await;
    let target = NotifyTargetId::from_number(1).unwrap();
    let now = Timestamp::from_millis(5).unwrap();
    let open_now = keys(&["head_moved:ab", "folder_held", "question:m-1"]);
    store
        .write(async |tx| {
            sync_notify_items(tx, &open_now, &[], now).await?;
            sync_notify_targets(tx, &BTreeMap::from([(target, BTreeSet::new())])).await?;
            record_notify_send(
                tx,
                target,
                &open_now.iter().cloned().collect::<Vec<_>>(),
                true,
                now,
            )
            .await
        })
        .await
        .unwrap();

    let recorded = store
        .write(async |tx| {
            sync_notify_items(tx, &BTreeSet::new(), &["head_moved", "folder_held"], now).await
        })
        .await
        .unwrap();
    assert_eq!(recorded, open_now);
    let mut conn = store.read().acquire().await.unwrap();
    let state = notify_state(&mut conn).await.unwrap();
    drop(conn);
    assert_eq!(
        state.items.keys().cloned().collect::<BTreeSet<_>>(),
        keys(&["folder_held", "head_moved:ab"]),
        "a folder item's absence says nothing yet, a question's ends it"
    );
    assert!(state.sent.contains(&(target, "head_moved:ab".to_owned())));

    store
        .write(async |tx| sync_notify_items(tx, &BTreeSet::new(), &[], now).await)
        .await
        .unwrap();
    let mut conn = store.read().acquire().await.unwrap();
    let state = notify_state(&mut conn).await.unwrap();
    assert!(state.items.is_empty() && state.sent.is_empty(), "{state:?}");
}
