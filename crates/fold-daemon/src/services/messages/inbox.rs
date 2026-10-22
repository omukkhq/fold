use std::collections::BTreeSet;

use fold_core::carry::{CarriedLines, CarriedMessage, carry};
use fold_core::events::types;
use fold_core::{
    Actor, AddressedAs, AgentActor, AgentName, Choice, FoldError, MessageId, NewEvent, ProposalId,
    RequestId, Sender, TaskId, Timestamp, WorkerName, policy,
};
use fold_store::agents::WorkerRow;
use fold_store::messages::{self, InboxRow, MessageLines};
use fold_store::{Store, StoreError, WriteTx, agents};
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
    /// The worker whose row this is. With `include_workers` one message can appear once per worker.
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
    /// The acting worker's unacknowledged rows in message number order, or with `include_workers` those of every worker of its agent. The same write transaction sets `delivered_at` on the rows that had none, so a returned row is always marked delivered and a marked row was always returned. Reading never acknowledges: the rows come back on every call until `acknowledge`.
    pub async fn inbox(
        &self,
        actor: &Actor,
        include_workers: bool,
    ) -> Result<Vec<InboxItem>, MessageError> {
        let agent = policy::can_use_inbox(actor).map_err(FoldError::from)?;
        let now = self.clock.now();
        self.store
            .write(async |tx| {
                let worker = acting_worker(tx, agent).await?;
                let workers = if include_workers {
                    agents::worker_ids(tx.conn(), worker.agent_id).await?
                } else {
                    vec![worker.id]
                };
                let rows = messages::inbox(tx.conn(), &workers).await?;
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

    /// The Stop hook's text for `agent`: the messages to its workers that no inbox, notice or wait has shown yet, as many as fit, or `None` when there is none. The same write transaction marks the carried rows delivered, as [`MessageService::inbox`] marks the rows it returns.
    pub async fn carry_for_stop(&self, agent: &AgentName) -> Result<Option<String>, MessageError> {
        let now = self.clock.now();
        self.store
            .write(async |tx| {
                let Some(row) = agents::find_agent(tx.conn(), agent).await? else {
                    return Ok(None);
                };
                let workers = agents::worker_ids(tx.conn(), row.id).await?;
                carry_in_tx(tx, &workers, &WorkerName::main(agent), now).await
            })
            .await
    }

    /// The text of a wait that a message ended: the messages to `worker`, or to every worker of its agent for a main worker, which also wakes for them, that nothing has shown yet; `None` when there is none. The carried rows are marked delivered in the same transaction.
    pub async fn carry_for_wait(&self, worker: &WorkerRow) -> Result<Option<String>, MessageError> {
        let now = self.clock.now();
        self.store
            .write(async |tx| {
                let workers = if worker.name.is_main() {
                    agents::worker_ids(tx.conn(), worker.agent_id).await?
                } else {
                    vec![worker.id]
                };
                carry_in_tx(tx, &workers, &worker.name, now).await
            })
            .await
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

/// Records that a notice reached a session or the worker process of `agent` at `now`: `pushed_at` on the agent's rows of `about`, the message the notice is about, where they have none, and `delivered_at` on the rows `carried` whose messages the notice held, as an inbox read marks them. Returns how many rows got `pushed_at`. It appends no stored event, as reading an inbox appends none; the caller sends the live `message.pushed` after the commit.
pub(crate) async fn record_pushed(
    store: &Store,
    agent: &AgentName,
    about: Option<MessageId>,
    carried: &[i64],
    now: Timestamp,
) -> Result<u64, MessageError> {
    Ok(store
        .write(async |tx| {
            messages::mark_delivered(tx, carried, now).await?;
            match about {
                Some(message) => messages::mark_pushed(tx, message, agent, now).await,
                None => Ok(0),
            }
        })
        .await?)
}

/// What a wake carries: its text and the recipient rows of the messages in it, which count as delivered once the text reaches the agent.
pub(crate) struct CarriedInbox {
    pub text: String,
    pub rows: Vec<i64>,
}

/// The push notice `head` with the messages to every worker of `agent` that nothing has shown yet, as many as fit after it, read from the read pool. Nothing is marked: the push marks the rows once the notice gets through. An unknown agent has none.
pub(crate) async fn carry_for_push(
    store: &Store,
    agent: &AgentName,
    head: &str,
) -> Result<CarriedInbox, MessageError> {
    let mut conn = store.read().acquire().await.map_err(StoreError::from)?;
    let Some(row) = agents::find_agent(&mut *conn, agent).await? else {
        return Ok(carry_rows(head, &[], &WorkerName::main(agent)));
    };
    let workers = agents::worker_ids(&mut *conn, row.id).await?;
    let reader = WorkerName::main(agent);
    let rows = undelivered(messages::inbox(&mut conn, &workers).await?, &reader);
    Ok(carry_rows(head, &rows, &reader))
}

async fn carry_in_tx(
    tx: &mut WriteTx,
    workers: &[i64],
    reader: &WorkerName,
    now: Timestamp,
) -> Result<Option<String>, MessageError> {
    let rows = undelivered(messages::inbox(tx.conn(), workers).await?, reader);
    let count = distinct_messages(&rows).len();
    if count == 0 {
        return Ok(None);
    }
    let head = match count {
        1 => "fold: 1 new message.".to_owned(),
        _ => format!("fold: {count} new messages."),
    };
    let carried = carry_rows(&head, &rows, reader);
    messages::mark_delivered(tx, &carried.rows, now).await?;
    Ok(Some(carried.text))
}

/// A wake carries only the rows no inbox, notice or wait has shown: a delivered message that waits for its acknowledgement, such as fold's note of an integration, would fill the text before the message that caused the wake. A message `reader` sent, such as a main worker's message to its child, is never carried back to it.
fn undelivered(rows: Vec<InboxRow>, reader: &WorkerName) -> Vec<InboxRow> {
    rows.into_iter()
        .filter(|row| {
            row.delivered_at.is_none()
                && !matches!(&row.message.sender, Sender::Worker(sender) if sender == reader)
        })
        .collect()
}

/// The rows grouped by message, in the inbox's order: one message to several workers of the agent has a row for each.
fn distinct_messages(rows: &[InboxRow]) -> Vec<&[InboxRow]> {
    rows.chunk_by(|a, b| a.message.id == b.message.id).collect()
}

/// One entry per message, which names its worker when it is not to `reader`; the rest is read with `include_workers` when any message is to another worker.
fn carry_rows(head: &str, rows: &[InboxRow], reader: &WorkerName) -> CarriedInbox {
    let groups = distinct_messages(rows);
    let entries: Vec<CarriedMessage<'_>> = groups
        .iter()
        .map(|group| {
            let message = &group[0].message;
            CarriedMessage {
                id: message.id,
                to: (!group.iter().any(|row| &row.worker == reader))
                    .then(|| group[0].worker.as_str()),
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
    let tool = if entries.iter().any(|entry| entry.to.is_some()) {
        "get_inbox with include_workers"
    } else {
        "get_inbox"
    };
    let carried = carry(head, &entries, tool);
    CarriedInbox {
        text: carried.text,
        rows: groups[..carried.count]
            .iter()
            .flat_map(|group| group.iter().map(|row| row.row))
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
