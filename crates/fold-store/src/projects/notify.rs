use std::collections::{BTreeMap, BTreeSet};

use fold_core::{NotifyTargetId, Timestamp};
use sqlx::SqliteConnection;

use crate::columns::{optional_time, time};
use crate::error::StoreError;
use crate::store::WriteTx;

/// What the notifier of one project knows: the Needs you items it found open, each target that applies with its last send, and the items each target got.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotifyState {
    /// Each open item's key with the time the notifier first found it.
    pub items: BTreeMap<String, Timestamp>,
    /// Each applying target with its last send, `None` before its first.
    pub targets: BTreeMap<NotifyTargetId, Option<Timestamp>>,
    pub sent: BTreeSet<(NotifyTargetId, String)>,
}

fn target_id(value: i64) -> Result<NotifyTargetId, StoreError> {
    NotifyTargetId::try_from_db(value).ok_or(StoreError::Corrupt("notify_targets_seen.target"))
}

pub async fn notify_state(conn: &mut SqliteConnection) -> Result<NotifyState, StoreError> {
    let mut state = NotifyState::default();
    for row in sqlx::query!(r#"SELECT key AS "key!", seen_at FROM notify_items"#)
        .fetch_all(&mut *conn)
        .await?
    {
        state
            .items
            .insert(row.key, time(row.seen_at, "notify_items.seen_at")?);
    }
    for row in
        sqlx::query!(r#"SELECT target AS "target!: i64", last_sent_at FROM notify_targets_seen"#)
            .fetch_all(&mut *conn)
            .await?
    {
        state.targets.insert(
            target_id(row.target)?,
            optional_time(row.last_sent_at, "notify_targets_seen.last_sent_at")?,
        );
    }
    for row in sqlx::query!(r#"SELECT target AS "target!: i64", key AS "key!" FROM notify_sent"#)
        .fetch_all(&mut *conn)
        .await?
    {
        state.sent.insert((target_id(row.target)?, row.key));
    }
    Ok(state)
}

/// Records `open`, the keys of the items in Needs you now: a new key gets `now` as its time, and a key that is gone loses its row with every send of it.
pub async fn sync_notify_items(
    tx: &mut WriteTx,
    open: &BTreeSet<String>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let known: BTreeSet<String> = sqlx::query_scalar!(r#"SELECT key AS "key!" FROM notify_items"#)
        .fetch_all(tx.conn())
        .await?
        .into_iter()
        .collect();
    let now = now.as_millis();
    for key in open.difference(&known) {
        sqlx::query!(
            "INSERT INTO notify_items (key, seen_at) VALUES (?1, ?2)",
            key,
            now
        )
        .execute(tx.conn())
        .await?;
    }
    for key in known.difference(open) {
        sqlx::query!("DELETE FROM notify_items WHERE key = ?1", key)
            .execute(tx.conn())
            .await?;
    }
    Ok(())
}

/// Makes `applying` the targets that apply to the project. A target that stops applying loses its rows; one that starts applying gets a row without a send and counts every open item as sent, so it hears only of items that come after.
pub async fn sync_notify_targets(
    tx: &mut WriteTx,
    applying: &BTreeSet<NotifyTargetId>,
) -> Result<(), StoreError> {
    let known = sqlx::query_scalar!(r#"SELECT target AS "target!: i64" FROM notify_targets_seen"#)
        .fetch_all(tx.conn())
        .await?
        .into_iter()
        .map(target_id)
        .collect::<Result<BTreeSet<_>, _>>()?;
    for target in known.difference(applying) {
        let number = target.to_db();
        sqlx::query!("DELETE FROM notify_targets_seen WHERE target = ?1", number)
            .execute(tx.conn())
            .await?;
    }
    for target in applying.difference(&known) {
        let number = target.to_db();
        sqlx::query!(
            "INSERT INTO notify_targets_seen (target, last_sent_at) VALUES (?1, NULL)",
            number
        )
        .execute(tx.conn())
        .await?;
        sqlx::query!(
            "INSERT INTO notify_sent (target, key) SELECT ?1, key FROM notify_items",
            number
        )
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

/// Records a send to `target` at `now`. With `delivered`, the items of `keys` that are still open count as sent; a failed send leaves them waiting for the next one, which the interval holds back.
pub async fn record_notify_send(
    tx: &mut WriteTx,
    target: NotifyTargetId,
    keys: &[String],
    delivered: bool,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = target.to_db();
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE notify_targets_seen SET last_sent_at = ?2 WHERE target = ?1",
        number,
        now
    )
    .execute(tx.conn())
    .await?;
    if delivered {
        for key in keys {
            sqlx::query!(
                "INSERT OR IGNORE INTO notify_sent (target, key)
                 SELECT ?1, key FROM notify_items WHERE key = ?2
                 AND EXISTS (SELECT 1 FROM notify_targets_seen WHERE target = ?1)",
                number,
                key
            )
            .execute(tx.conn())
            .await?;
        }
    }
    Ok(())
}
