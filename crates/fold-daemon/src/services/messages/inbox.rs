use std::collections::BTreeSet;

use fold_core::carry::{CarriedLines, CarriedMessage, carry};
use fold_core::events::types;
use fold_core::{
    Actor, AddressedAs, AgentActor, AgentName, Choice, FoldError, MessageId, NewEvent, ProposalId,
    RequestId, Sender, TaskId, Timestamp, WorkerName, policy,
};
use fold_store::agents::WorkerRow;
use fold_store::messages::{self, InboxRow, MessageLines};
use fold_store::{SqliteConnection, Store, StoreError, WriteTx, agents};
use serde_json::json;

use super::{MessageError, MessageService, message_not_found};
use crate::services::agents::{actor_worker, worker_not_found};
use crate::services::events::EventBuffer;
#[cfg(feature = "failpoints")]
use crate::services::fired;

/// One unacknowledged message of a worker's inbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxItem {
    pub id: MessageId,
    pub thread: MessageId,
    pub reply_to: Option<MessageId>,
    pub sender: Sender,
    /// The worker whose row this is.
    pub to: WorkerName,
    pub addressed_as: AddressedAs,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
    pub request: Option<RequestId>,
    pub body: String,
    pub sent_at: Timestamp,
    /// The first time an inbox returned the row.
    pub delivered_at: Timestamp,
    /// The options of a multiple-choice question; empty for any other message.
    pub choices: Vec<Choice>,
    /// The short header of a multiple-choice question.
    pub header: Option<String>,
    /// The sender marked the message as asking a human for an answer.
    pub asks_human: bool,
    /// The lines of a version of the message's proposal that the message asks about.
    pub lines: Option<MessageLines>,
}

fn item(row: InboxRow, now: Timestamp) -> InboxItem {
    let message = row.message;
    InboxItem {
        id: message.id,
        thread: message.thread,
        reply_to: message.reply_to,
        sender: message.sender,
        to: row.worker,
        addressed_as: row.addressed_as,
        task: message.task,
        proposal: message.proposal,
        request: message.request,
        body: message.body,
        sent_at: message.created_at,
        delivered_at: row.delivered_at.unwrap_or(now),
        choices: message.choices,
        header: message.header,
        asks_human: message.asks_human,
        lines: message.lines,
    }
}

impl MessageService {
    /// The acting worker's unacknowledged rows in message number order. The same write transaction sets `delivered_at` on the rows that had none, so a returned row is always marked delivered and a marked row was always returned. Reading never acknowledges: the rows come back on every call until `acknowledge`.
    pub async fn inbox(&self, actor: &Actor) -> Result<Vec<InboxItem>, MessageError> {
        let agent = policy::can_use_inbox(actor).map_err(FoldError::from)?;
        let now = self.clock.now();
        self.store
            .write(async |tx| {
                let worker = acting_worker(tx, agent).await?;
                let rows = messages::inbox(tx.conn(), &[worker.id]).await?;
                let undelivered: Vec<i64> = rows
                    .iter()
                    .filter(|row| row.delivered_at.is_none())
                    .map(|row| row.row)
                    .collect();
                messages::mark_delivered(tx, &undelivered, now).await?;
                #[cfg(feature = "failpoints")]
                fail::fail_point!("inbox_after_delivery", |_| Err(MessageError::from(fired(
                    "inbox_after_delivery"
                ))));
                Ok(rows.into_iter().map(|row| item(row, now)).collect())
            })
            .await
    }

    /// Acknowledges the acting worker's row of `message`, or with `up_to` every unacknowledged row of that worker up to it, and returns the ids this call changed. A row acknowledged before keeps its time, so a repeat changes nothing and appends no event. A message the worker did not receive is `message_not_found`.
    pub async fn acknowledge(
        &self,
        actor: &Actor,
        message: MessageId,
        up_to: bool,
    ) -> Result<Vec<MessageId>, MessageError> {
        self.acknowledge_all(actor, &[message], up_to).await
    }

    /// [`Self::acknowledge`] for several messages in one transaction: a message the worker did not receive refuses the whole call and acknowledges nothing. The ids come back in message number order.
    pub async fn acknowledge_all(
        &self,
        actor: &Actor,
        targets: &[MessageId],
        up_to: bool,
    ) -> Result<Vec<MessageId>, MessageError> {
        let agent = policy::can_use_inbox(actor).map_err(FoldError::from)?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let acknowledged = self
            .store
            .write(async |tx| {
                let worker = acting_worker(tx, agent).await?;
                let mut ids = BTreeSet::new();
                for message in targets {
                    let changed = messages::acknowledge(tx, worker.id, *message, up_to, now)
                        .await?
                        .ok_or_else(|| message_not_found(*message))?;
                    ids.extend(changed);
                }
                let ids: Vec<MessageId> = ids.into_iter().collect();
                append_acknowledged(tx, &mut buffer, actor, &ids, now).await?;
                Ok::<_, MessageError>(ids)
            })
            .await?;
        self.bus.publish(buffer);
        Ok(acknowledged)
    }

    /// The Stop hook's text for `agent`: the messages to its worker that no inbox, notice or wait has shown yet, as many as fit, with the ids it carries, or `None` when there is none. Nothing is marked: the hook may not get the answer to its session in time, so it reports the ids it used with [`MessageService::mark_carried`].
    pub async fn carry_for_stop(
        &self,
        agent: &AgentName,
    ) -> Result<Option<StopCarry>, MessageError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let Some(worker) = agent_worker(&mut conn, agent).await? else {
            return Ok(None);
        };
        let rows = undelivered(
            messages::inbox(&mut conn, &[worker.id]).await?,
            &worker.name,
        );
        let Some(head) = new_messages_head(&rows) else {
            return Ok(None);
        };
        let carried = carry_rows(&head, &rows);
        Ok(Some(StopCarry {
            text: carried.text,
            messages: carried.messages.into_iter().map(|(id, _)| id).collect(),
        }))
    }

    /// Marks `agent`'s rows of `messages` delivered, as an inbox read does, once the Stop hook has given its text to the session. Rows read or acknowledged since keep their times; an id the agent has no unacknowledged row of is skipped. Returns how many rows it marked.
    pub async fn mark_carried(
        &self,
        agent: &AgentName,
        carried: &[MessageId],
    ) -> Result<u64, MessageError> {
        let now = self.clock.now();
        self.store
            .write(async |tx| {
                let Some(worker) = agent_worker(tx.conn(), agent).await? else {
                    return Ok(0);
                };
                let rows: Vec<i64> = messages::inbox(tx.conn(), &[worker.id])
                    .await?
                    .into_iter()
                    .filter(|row| row.delivered_at.is_none() && carried.contains(&row.message.id))
                    .map(|row| row.row)
                    .collect();
                messages::mark_delivered(tx, &rows, now).await?;
                Ok::<_, MessageError>(rows.len() as u64)
            })
            .await
    }

    /// The text of a wait that a message ended: the messages to `worker` that nothing has shown yet, or `None` when there is none. The carried rows are marked delivered in the same transaction.
    pub async fn carry_for_wait(&self, worker: &WorkerRow) -> Result<Option<String>, MessageError> {
        let now = self.clock.now();
        self.store
            .write(async |tx| carry_in_tx(tx, worker, now).await)
            .await
    }

    /// The time on this service's clock, which a wait records as its start.
    pub fn now(&self) -> Timestamp {
        self.clock.now()
    }

    /// [`unread_count`] for this service's project.
    pub async fn unread_count(&self, agent_id: i64) -> Result<u64, MessageError> {
        unread_count(&self.store, agent_id).await
    }
}

/// The unacknowledged rows of every worker of the agent `agent_id`, read from the read pool: the unread part of the footer.
pub async fn unread_count(store: &Store, agent_id: i64) -> Result<u64, MessageError> {
    Ok(messages::unread_count(store.read(), agent_id).await?)
}

/// Records that a notice reached a session or the worker process of `agent` at `now`: `pushed_at` on the agent's rows of each of `pushed`, the message the notice is about and the messages it carried, where they have none, and `delivered_at` on the rows `carried`, as an inbox read marks them. Returns the messages whose rows got `pushed_at` here. It appends no stored event, as reading an inbox appends none; the caller sends a live `message.pushed` for each after the commit.
pub(crate) async fn record_pushed(
    store: &Store,
    agent: &AgentName,
    pushed: &[MessageId],
    carried: &[i64],
    now: Timestamp,
) -> Result<Vec<MessageId>, MessageError> {
    Ok(store
        .write(async |tx| {
            messages::mark_delivered(tx, carried, now).await?;
            let mut first = Vec::new();
            for message in pushed {
                if messages::mark_pushed(tx, *message, agent, now).await? > 0 {
                    first.push(*message);
                }
            }
            Ok::<_, StoreError>(first)
        })
        .await?)
}

/// The Stop hook's text and the messages it carries, which count as delivered once the hook reports that it used them.
pub struct StopCarry {
    pub text: String,
    pub messages: Vec<MessageId>,
}

/// What a wake carries: its text, the recipient rows of the messages in it, which count as delivered once the text reaches the agent, and each message with its thread.
pub(crate) struct CarriedInbox {
    pub text: String,
    pub rows: Vec<i64>,
    pub messages: Vec<(MessageId, MessageId)>,
}

/// The push notice `head` with the messages to the worker of `agent` that nothing has shown yet, as many as fit after it, read from the read pool. Nothing is marked: the push marks the rows once the notice gets through. An unknown agent has none.
pub(crate) async fn carry_for_push(
    store: &Store,
    agent: &AgentName,
    head: &str,
) -> Result<CarriedInbox, MessageError> {
    let mut conn = store.read().acquire().await.map_err(StoreError::from)?;
    let Some(worker) = agent_worker(&mut conn, agent).await? else {
        return Ok(carry_rows(head, &[]));
    };
    let rows = undelivered(
        messages::inbox(&mut conn, &[worker.id]).await?,
        &worker.name,
    );
    Ok(carry_rows(head, &rows))
}

/// The one worker of `agent`, or `None` for an agent the project does not know.
async fn agent_worker(
    conn: &mut SqliteConnection,
    agent: &AgentName,
) -> Result<Option<WorkerRow>, StoreError> {
    let Some(row) = agents::find_agent(&mut *conn, agent).await? else {
        return Ok(None);
    };
    agents::main_worker(&mut *conn, row.id).await
}

async fn carry_in_tx(
    tx: &mut WriteTx,
    worker: &WorkerRow,
    now: Timestamp,
) -> Result<Option<String>, MessageError> {
    let rows = undelivered(
        messages::inbox(tx.conn(), &[worker.id]).await?,
        &worker.name,
    );
    let Some(head) = new_messages_head(&rows) else {
        return Ok(None);
    };
    let carried = carry_rows(&head, &rows);
    messages::mark_delivered(tx, &carried.rows, now).await?;
    Ok(Some(carried.text))
}

/// The first sentence of a wake that has no event of its own, or `None` without a message.
fn new_messages_head(rows: &[InboxRow]) -> Option<String> {
    match rows.len() {
        0 => None,
        1 => Some("fold: 1 new message.".to_owned()),
        count => Some(format!("fold: {count} new messages.")),
    }
}

/// A wake carries only the rows no inbox, notice or wait has shown: a delivered message that waits for its acknowledgement, such as fold's note of an integration, would fill the text before the message that caused the wake. A message `reader` sent itself is never carried back to it.
fn undelivered(rows: Vec<InboxRow>, reader: &WorkerName) -> Vec<InboxRow> {
    rows.into_iter()
        .filter(|row| {
            row.delivered_at.is_none()
                && !matches!(&row.message.sender, Sender::Worker(sender) if sender == reader)
        })
        .collect()
}

/// One entry per row of one worker's inbox, in its order.
fn carry_rows(head: &str, rows: &[InboxRow]) -> CarriedInbox {
    let entries: Vec<CarriedMessage<'_>> = rows
        .iter()
        .map(|row| {
            let message = &row.message;
            CarriedMessage {
                id: message.id,
                sender: message.sender.to_string(),
                reply_to: message.reply_to,
                task: message.task,
                proposal: message.proposal,
                request: message.request,
                body: &message.body,
                lines: message
                    .lines
                    .as_ref()
                    .zip(message.proposal)
                    .map(|(lines, proposal)| CarriedLines {
                        proposal,
                        version: lines.version,
                        path: &lines.path,
                        side: lines.side.as_str(),
                        start_line: lines.start_line,
                        end_line: lines.end_line,
                        excerpt: &lines.excerpt,
                        excerpt_cut: lines.excerpt_cut,
                    }),
                choices: message
                    .choices
                    .iter()
                    .map(|choice| choice.label.as_str())
                    .collect(),
            }
        })
        .collect();
    let carried = carry(head, &entries);
    let rows = &rows[..carried.count];
    CarriedInbox {
        text: carried.text,
        rows: rows.iter().map(|row| row.row).collect(),
        messages: rows
            .iter()
            .map(|row| (row.message.id, row.message.thread))
            .collect(),
    }
}

async fn acting_worker(tx: &mut WriteTx, agent: &AgentActor) -> Result<WorkerRow, MessageError> {
    Ok(actor_worker(tx.conn(), agent)
        .await?
        .ok_or_else(worker_not_found)?)
}

/// Appends `message.acknowledged` for the ids a call changed; nothing when none changed. The event names the newest message it covers; `wait_for_events` returns it only to the acknowledging worker's agent, whose inbox it changed.
pub(super) async fn append_acknowledged(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    ids: &[MessageId],
    now: Timestamp,
) -> Result<(), MessageError> {
    if let Some(newest) = ids.last() {
        let event = NewEvent::new(types::MESSAGE_ACKNOWLEDGED, actor.event_actor())?
            .message(*newest)
            .data(json!({
                "ids": ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
            }))?;
        buffer.append(tx, &event, now).await?;
    }
    Ok(())
}
