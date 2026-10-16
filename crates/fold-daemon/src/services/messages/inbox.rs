use fold_core::events::types;
use fold_core::{
    Actor, AddressedAs, FoldError, MessageId, NewEvent, ProposalId, RequestId, Sender, TaskId,
    Timestamp, WorkerName, codes, policy,
};
use fold_store::agents::WorkerRow;
use fold_store::messages::{self, InboxRow};
use fold_store::{Store, WriteTx, agents};
use serde_json::json;

use super::{MessageError, MessageService, message_not_found};
use crate::services::events::EventBuffer;

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
    pub choices: Vec<String>,
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
    }
}

impl MessageService {
    /// The acting worker's unacknowledged rows in message number order, or with `include_workers` those of every worker of its agent. The same write transaction sets `delivered_at` on the rows that had none, so a returned row is always marked delivered and a marked row was always returned. Reading never acknowledges: the rows come back on every call until `acknowledge`.
    pub async fn inbox(
        &self,
        actor: &Actor,
        include_workers: bool,
    ) -> Result<Vec<InboxItem>, MessageError> {
        let now = self.clock.now();
        self.store
            .write(async |tx| {
                let worker = acting_worker(tx, actor).await?;
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
                fail::fail_point!("inbox_after_delivery", |_| {
                    Err(MessageError::from(FoldError::new(
                        codes::FAILPOINT,
                        "inbox_after_delivery fired",
                    )))
                });
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
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let acknowledged = self
            .store
            .write(async |tx| {
                let worker = acting_worker(tx, actor).await?;
                let ids = messages::acknowledge(tx, worker.id, message, up_to, now)
                    .await?
                    .ok_or_else(|| message_not_found(message))?;
                // The event names the newest message it covers, whose recipients include the acknowledging worker, so `wait_for_events` reaches that worker's agent.
                if let Some(newest) = ids.last() {
                    let event = NewEvent::new(types::MESSAGE_ACKNOWLEDGED, actor.event_actor())?
                        .message(*newest)
                        .data(json!({
                            "ids": ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
                        }))?;
                    buffer.append(tx, &event, now).await?;
                }
                Ok::<_, MessageError>(ids)
            })
            .await?;
        self.bus.publish(buffer);
        Ok(acknowledged)
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

async fn acting_worker(tx: &mut WriteTx, actor: &Actor) -> Result<WorkerRow, MessageError> {
    let agent = policy::can_use_inbox(actor).map_err(FoldError::from)?;
    let worker = match agents::find_agent(tx.conn(), &agent.agent).await? {
        Some(row) => agents::find_worker(tx.conn(), row.id, &agent.worker).await?,
        None => None,
    };
    worker.ok_or_else(|| {
        FoldError::new(codes::WORKER_NOT_FOUND, "the worker does not exist")
            .with_hint("call register_worker")
            .into()
    })
}
