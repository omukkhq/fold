use fold_core::{NotifySendId, NotifyTargetId, Timestamp};
use sqlx::SqliteExecutor;

use crate::columns::{count, optional_time, time};
use crate::error::StoreError;
use crate::store::WriteTx;

fn target_id(value: i64, column: &'static str) -> Result<NotifyTargetId, StoreError> {
    NotifyTargetId::try_from_db(value).ok_or(StoreError::Corrupt(column))
}

/// The `seq` of the newest event the notifier passed; `None` before its first pass with a target.
pub async fn cursor(executor: impl SqliteExecutor<'_>) -> Result<Option<u64>, StoreError> {
    let seq = sqlx::query_scalar!("SELECT last_seq FROM notify_state WHERE id = 1")
        .fetch_optional(executor)
        .await?;
    seq.map(|seq| count(seq, "notify_state.last_seq"))
        .transpose()
}

/// Moves the cursor to `seq`; it never moves back.
pub async fn set_cursor(tx: &mut WriteTx, seq: u64) -> Result<(), StoreError> {
    let seq = i64::try_from(seq).map_err(|_| StoreError::Corrupt("notify_state.last_seq"))?;
    sqlx::query!(
        "INSERT INTO notify_state (id, last_seq) VALUES (1, ?1)
         ON CONFLICT (id) DO UPDATE SET last_seq = MAX(last_seq, excluded.last_seq)",
        seq
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Starts the cursor at `seq` when it never moved; a cursor that moved stays where it is.
pub async fn start_cursor(tx: &mut WriteTx, seq: u64) -> Result<(), StoreError> {
    let seq = i64::try_from(seq).map_err(|_| StoreError::Corrupt("notify_state.last_seq"))?;
    sqlx::query!(
        "INSERT INTO notify_state (id, last_seq) VALUES (1, ?1) ON CONFLICT (id) DO NOTHING",
        seq
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The first event at or after `since`, where a cursor that never moved starts: a target hears only of what came after it was added.
pub async fn first_seq_since(
    executor: impl SqliteExecutor<'_>,
    since: Timestamp,
) -> Result<Option<u64>, StoreError> {
    let since = since.as_millis();
    let seq = sqlx::query_scalar!(
        r#"SELECT MIN(seq) AS "seq: i64" FROM events WHERE created_at >= ?1"#,
        since
    )
    .fetch_one(executor)
    .await?;
    seq.map(|seq| count(seq, "events.seq")).transpose()
}

/// A Needs you item the notifier found open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenItem {
    pub key: String,
    pub kind: String,
    /// The public id the item names, such as `m-31`.
    pub item: Option<String>,
    /// The item's screen below the project in the web app.
    pub path: String,
    pub seen_at: Timestamp,
}

pub async fn seen_items(executor: impl SqliteExecutor<'_>) -> Result<Vec<SeenItem>, StoreError> {
    sqlx::query!(
        r#"SELECT key AS "key!", kind, item, path, seen_at FROM notify_needs_you ORDER BY seen_at, key"#
    )
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|row| {
        Ok(SeenItem {
            key: row.key,
            kind: row.kind,
            item: row.item,
            path: row.path,
            seen_at: time(row.seen_at, "notify_needs_you.seen_at")?,
        })
    })
    .collect()
}

pub async fn add_seen(tx: &mut WriteTx, item: &SeenItem) -> Result<(), StoreError> {
    let seen_at = item.seen_at.as_millis();
    sqlx::query!(
        "INSERT INTO notify_needs_you (key, kind, item, path, seen_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (key) DO NOTHING",
        item.key,
        item.kind,
        item.item,
        item.path,
        seen_at
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

pub async fn remove_seen(tx: &mut WriteTx, key: &str) -> Result<(), StoreError> {
    sqlx::query!("DELETE FROM notify_needs_you WHERE key = ?1", key)
        .execute(tx.conn())
        .await?;
    Ok(())
}

/// One notice that waits for its target's next send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRow {
    pub id: i64,
    pub target: NotifyTargetId,
    pub seq: u64,
    /// The notice as JSON.
    pub notice: String,
    pub created_at: Timestamp,
}

/// Queues `notice` for `target`; `opened` is the Needs you key of a `needs_you.opened`.
pub async fn queue(
    tx: &mut WriteTx,
    target: NotifyTargetId,
    seq: u64,
    notice: &str,
    opened: Option<&str>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let target = target.to_db();
    let seq = i64::try_from(seq).map_err(|_| StoreError::Corrupt("notify_outbox.seq"))?;
    let now = now.as_millis();
    sqlx::query!(
        "INSERT INTO notify_outbox (target, seq, notice, opened, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        target,
        seq,
        notice,
        opened,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Takes back the `needs_you.opened` of the item `key` that still waits for `target`. Returns whether one waited: then the item came and went inside one window, and its close needs no notice either.
pub async fn take_back_opened(
    tx: &mut WriteTx,
    target: NotifyTargetId,
    key: &str,
) -> Result<bool, StoreError> {
    let target = target.to_db();
    let taken = sqlx::query!(
        "DELETE FROM notify_outbox WHERE target = ?1 AND opened = ?2",
        target,
        key
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(taken > 0)
}

/// Each target with waiting notices and the time its first one came.
pub async fn waiting(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<(NotifyTargetId, Timestamp)>, StoreError> {
    sqlx::query!(
        r#"SELECT target AS "target!: i64", MIN(created_at) AS "first!: i64"
           FROM notify_outbox GROUP BY target ORDER BY target"#
    )
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|row| {
        Ok((
            target_id(row.target, "notify_outbox.target")?,
            time(row.first, "notify_outbox.created_at")?,
        ))
    })
    .collect()
}

/// The oldest `limit` notices that wait for `target`, in event order.
pub async fn outbox_of(
    executor: impl SqliteExecutor<'_>,
    target: NotifyTargetId,
    limit: usize,
) -> Result<Vec<OutboxRow>, StoreError> {
    let number = target.to_db();
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    sqlx::query!(
        r#"SELECT id AS "id!", seq, notice, created_at FROM notify_outbox
           WHERE target = ?1 ORDER BY id LIMIT ?2"#,
        number,
        limit
    )
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(|row| {
        Ok(OutboxRow {
            id: row.id,
            target,
            seq: count(row.seq, "notify_outbox.seq")?,
            notice: row.notice,
            created_at: time(row.created_at, "notify_outbox.created_at")?,
        })
    })
    .collect()
}

/// Deletes the waiting notices and the unfinished sends of every target outside `active`: a target that was removed or turned off gets none of what queued for it.
pub async fn drop_inactive(tx: &mut WriteTx, active: &[NotifyTargetId]) -> Result<u64, StoreError> {
    let keep = serde_json::to_string(
        &active
            .iter()
            .map(|target| target.to_db())
            .collect::<Vec<_>>(),
    )
    .map_err(|_| StoreError::Corrupt("notify targets"))?;
    let outbox = sqlx::query!(
        "DELETE FROM notify_outbox WHERE target NOT IN (SELECT value FROM json_each(?1))",
        keep
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    let sends = sqlx::query!(
        "DELETE FROM notify_sends WHERE state IN ('pending', 'sending')
         AND target NOT IN (SELECT value FROM json_each(?1))",
        keep
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(outbox + sends)
}

/// Whether a waiting notice or an unfinished send belongs to a target outside `active`.
pub async fn has_inactive(
    executor: impl SqliteExecutor<'_> + Copy,
    active: &[NotifyTargetId],
) -> Result<bool, StoreError> {
    let keep = serde_json::to_string(
        &active
            .iter()
            .map(|target| target.to_db())
            .collect::<Vec<_>>(),
    )
    .map_err(|_| StoreError::Corrupt("notify targets"))?;
    let found = sqlx::query_scalar!(
        r#"SELECT EXISTS (
             SELECT 1 FROM notify_outbox WHERE target NOT IN (SELECT value FROM json_each(?1))
             UNION ALL
             SELECT 1 FROM notify_sends WHERE state IN ('pending', 'sending')
               AND target NOT IN (SELECT value FROM json_each(?1))
           ) AS "found!: bool""#,
        keep
    )
    .fetch_one(executor)
    .await?;
    Ok(found)
}

/// Where a send stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendState {
    /// Waits for its next try at `next_try_at`.
    Pending,
    /// A try is in flight.
    Sending,
    Delivered,
    GivenUp,
}

impl SendState {
    pub fn as_str(self) -> &'static str {
        match self {
            SendState::Pending => "pending",
            SendState::Sending => "sending",
            SendState::Delivered => "delivered",
            SendState::GivenUp => "given_up",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        Ok(match text {
            "pending" => SendState::Pending,
            "sending" => SendState::Sending,
            "delivered" => SendState::Delivered,
            "given_up" => SendState::GivenUp,
            _ => return Err(StoreError::Corrupt("notify_sends.state")),
        })
    }
}

/// One send and its newest try.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRow {
    pub number: NotifySendId,
    pub target: NotifyTargetId,
    pub webhook_id: String,
    pub state: SendState,
    pub tries: u32,
    pub next_try_at: Option<Timestamp>,
    pub body: Vec<u8>,
    /// A JSON list of the short forms of the notices, such as `task.moved T-18`.
    pub summary: String,
    pub notices: u32,
    pub test: bool,
    pub created_at: Timestamp,
    pub last_try_at: Option<Timestamp>,
    pub status: Option<u16>,
    pub error: Option<String>,
    pub duration_ms: Option<u64>,
}

/// A send before it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSend<'a> {
    pub target: NotifyTargetId,
    /// Unique across sends; the number of the send follows it, so the id names the send in every project.
    pub webhook_id_prefix: &'a str,
    pub body: &'a [u8],
    pub summary: &'a str,
    pub notices: u32,
    pub test: bool,
    /// The waiting notices the send carries, which leave the outbox with it.
    pub outbox: &'a [i64],
}

/// Stores a send that waits for its first try now, and takes its notices out of the outbox, in one write, so a notice is in the outbox or in exactly one send. A send before the cursor's first move starts the cursor at the newest event.
pub async fn create_send(
    tx: &mut WriteTx,
    new: &NewSend<'_>,
    now: Timestamp,
) -> Result<SendRow, StoreError> {
    let number = sqlx::query_scalar!(
        r#"INSERT INTO notify_state (id, last_seq, next_send)
           VALUES (1, (SELECT COALESCE(MAX(seq), 0) FROM events), 2)
           ON CONFLICT (id) DO UPDATE SET next_send = next_send + 1
           RETURNING next_send - 1 AS "number!: i64""#
    )
    .fetch_one(tx.conn())
    .await?;
    let send = u32::try_from(number)
        .ok()
        .and_then(NotifySendId::from_number)
        .ok_or(StoreError::Corrupt("notify_state.next_send"))?;
    let webhook_id = format!("{}-{}", new.webhook_id_prefix, send.number());
    let target = new.target.to_db();
    let notices = i64::from(new.notices);
    let test = i64::from(new.test);
    let at = now.as_millis();
    sqlx::query!(
        "INSERT INTO notify_sends (number, target, webhook_id, state, next_try_at, body, summary, notices, test, created_at)
         VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7, ?8, ?4)",
        number,
        target,
        webhook_id,
        at,
        new.body,
        new.summary,
        notices,
        test
    )
    .execute(tx.conn())
    .await?;
    for id in new.outbox {
        sqlx::query!("DELETE FROM notify_outbox WHERE id = ?1", id)
            .execute(tx.conn())
            .await?;
    }
    Ok(SendRow {
        number: send,
        target: new.target,
        webhook_id,
        state: SendState::Pending,
        tries: 0,
        next_try_at: Some(now),
        body: new.body.to_vec(),
        summary: new.summary.to_owned(),
        notices: new.notices,
        test: new.test,
        created_at: now,
        last_try_at: None,
        status: None,
        error: None,
        duration_ms: None,
    })
}

struct SendRecord {
    number: i64,
    target: i64,
    webhook_id: String,
    state: String,
    tries: i64,
    next_try_at: Option<i64>,
    body: Vec<u8>,
    summary: String,
    notices: i64,
    test: i64,
    created_at: i64,
    last_try_at: Option<i64>,
    status: Option<i64>,
    error: Option<String>,
    duration_ms: Option<i64>,
}

impl SendRecord {
    fn into_row(self) -> Result<SendRow, StoreError> {
        Ok(SendRow {
            number: u32::try_from(self.number)
                .ok()
                .and_then(NotifySendId::from_number)
                .ok_or(StoreError::Corrupt("notify_sends.number"))?,
            target: target_id(self.target, "notify_sends.target")?,
            webhook_id: self.webhook_id,
            state: SendState::parse(&self.state)?,
            tries: count(self.tries, "notify_sends.tries")?,
            next_try_at: optional_time(self.next_try_at, "notify_sends.next_try_at")?,
            body: self.body,
            summary: self.summary,
            notices: count(self.notices, "notify_sends.notices")?,
            test: self.test != 0,
            created_at: time(self.created_at, "notify_sends.created_at")?,
            last_try_at: optional_time(self.last_try_at, "notify_sends.last_try_at")?,
            status: self
                .status
                .map(|status| count(status, "notify_sends.status"))
                .transpose()?,
            error: self.error,
            duration_ms: self
                .duration_ms
                .map(|duration| count(duration, "notify_sends.duration_ms"))
                .transpose()?,
        })
    }
}

/// The sends that wait for a try or have one in flight, oldest first.
pub async fn open_sends(executor: impl SqliteExecutor<'_>) -> Result<Vec<SendRow>, StoreError> {
    sqlx::query_as!(
        SendRecord,
        r#"SELECT number, target, webhook_id, state, tries, next_try_at, body, summary, notices, test,
                  created_at, last_try_at, status, error, duration_ms
           FROM notify_sends WHERE state IN ('pending', 'sending') ORDER BY id"#
    )
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(SendRecord::into_row)
    .collect()
}

/// The newest `limit` sends of `target`, newest first: the send log.
pub async fn send_log(
    executor: impl SqliteExecutor<'_>,
    target: NotifyTargetId,
    limit: i64,
) -> Result<Vec<SendRow>, StoreError> {
    let target = target.to_db();
    sqlx::query_as!(
        SendRecord,
        r#"SELECT number, target, webhook_id, state, tries, next_try_at, body, summary, notices, test,
                  created_at, last_try_at, status, error, duration_ms
           FROM notify_sends WHERE target = ?1 ORDER BY id DESC LIMIT ?2"#,
        target,
        limit
    )
    .fetch_all(executor)
    .await?
    .into_iter()
    .map(SendRecord::into_row)
    .collect()
}

pub async fn get_send(
    executor: impl SqliteExecutor<'_>,
    number: NotifySendId,
) -> Result<Option<SendRow>, StoreError> {
    let number = number.to_db();
    sqlx::query_as!(
        SendRecord,
        r#"SELECT number, target, webhook_id, state, tries, next_try_at, body, summary, notices, test,
                  created_at, last_try_at, status, error, duration_ms
           FROM notify_sends WHERE number = ?1"#,
        number
    )
    .fetch_optional(executor)
    .await?
    .map(SendRecord::into_row)
    .transpose()
}

/// Marks a try of `number` in flight. Returns false when the send is no longer open.
pub async fn start_try(
    tx: &mut WriteTx,
    number: NotifySendId,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let number = number.to_db();
    let now = now.as_millis();
    let changed = sqlx::query!(
        "UPDATE notify_sends SET state = 'sending', last_try_at = ?2
         WHERE number = ?1 AND state IN ('pending', 'sending')",
        number,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(changed == 1)
}

/// What one try ended in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TryResult<'a> {
    pub state: SendState,
    pub tries: u32,
    pub next_try_at: Option<Timestamp>,
    pub status: Option<u16>,
    /// A word of fold's own, such as `timeout`; never a receiver's text.
    pub error: Option<&'a str>,
    pub duration_ms: u64,
}

pub async fn finish_try(
    tx: &mut WriteTx,
    number: NotifySendId,
    result: &TryResult<'_>,
) -> Result<(), StoreError> {
    let number = number.to_db();
    let state = result.state.as_str();
    let tries = i64::from(result.tries);
    let next = result.next_try_at.map(Timestamp::as_millis);
    let status = result.status.map(i64::from);
    let duration = i64::try_from(result.duration_ms).unwrap_or(i64::MAX);
    sqlx::query!(
        "UPDATE notify_sends SET state = ?2, tries = ?3, next_try_at = ?4, status = ?5, error = ?6, duration_ms = ?7
         WHERE number = ?1",
        number,
        state,
        tries,
        next,
        status,
        result.error,
        duration
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Puts every send whose try started at or before `started_by` and is still in flight back to wait for a try now, with the same webhook id: at a start, the tries a stopped daemon left; later, a try whose record never came.
pub async fn reset_sending(
    tx: &mut WriteTx,
    started_by: Timestamp,
    now: Timestamp,
) -> Result<u64, StoreError> {
    let started_by = started_by.as_millis();
    let now = now.as_millis();
    Ok(sqlx::query!(
        "UPDATE notify_sends SET state = 'pending', next_try_at = ?2
         WHERE state = 'sending' AND last_try_at <= ?1",
        started_by,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected())
}

/// Sends a finished send again with its webhook id, its body and a fresh count of tries. Returns false when the send is still open.
pub async fn send_again(
    tx: &mut WriteTx,
    number: NotifySendId,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let number = number.to_db();
    let now = now.as_millis();
    let changed = sqlx::query!(
        "UPDATE notify_sends SET state = 'pending', tries = 0, next_try_at = ?2
         WHERE number = ?1 AND state IN ('delivered', 'given_up')",
        number,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(changed == 1)
}

/// Keeps the newest `keep` finished sends of `target` and deletes the older ones.
pub async fn prune_log(
    tx: &mut WriteTx,
    target: NotifyTargetId,
    keep: i64,
) -> Result<(), StoreError> {
    let target = target.to_db();
    sqlx::query!(
        "DELETE FROM notify_sends WHERE target = ?1 AND state IN ('delivered', 'given_up')
         AND id NOT IN (SELECT id FROM notify_sends WHERE target = ?1 ORDER BY id DESC LIMIT ?2)",
        target,
        keep
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Deletes every waiting notice and every send of `target`, as the target is removed.
pub async fn forget_target(tx: &mut WriteTx, target: NotifyTargetId) -> Result<(), StoreError> {
    let target = target.to_db();
    sqlx::query!("DELETE FROM notify_outbox WHERE target = ?1", target)
        .execute(tx.conn())
        .await?;
    sqlx::query!("DELETE FROM notify_sends WHERE target = ?1", target)
        .execute(tx.conn())
        .await?;
    Ok(())
}
