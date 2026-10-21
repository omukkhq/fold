use fold_core::events::types;
use fold_core::{Actor, FoldError, MessageId, NewEvent, TaskId, Timestamp, codes, policy};
use fold_store::messages::{self, AskClosure, AskScope, OpenAsk};
use fold_store::{WriteTx, agents};
use serde_json::json;

use super::{MessageError, MessageService, message_not_found, require_task};
use crate::services::events::EventBuffer;

/// The open asks a human marks answered at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DismissScope {
    /// The thread that holds this message.
    Thread(MessageId),
    /// Every ask linked to this task.
    Task(TaskId),
}

impl MessageService {
    /// Records that the user answered the open ask `message` outside its thread, with `message.ask_closed` in the same transaction, so it stops waiting for a human. Only the asking agent or the main agent records it. A message that is no open ask is `ask_not_open`, and so is a worker's `AskUserQuestion` message, which ends only with a human's reply because its worker waits for that reply.
    pub async fn mark_answered(
        &self,
        actor: &Actor,
        message: MessageId,
    ) -> Result<MessageId, MessageError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let ask = messages::open_asks(tx.conn(), AskScope::Message(message))
                    .await?
                    .pop();
                let Some(ask) = ask else {
                    if messages::find(tx.conn(), message).await?.is_none() {
                        return Err(message_not_found(message).into());
                    }
                    return Err(not_open(message).into());
                };
                if ask.worker_question {
                    return Err(not_open(message)
                        .with_hint("the worker's question ends when a human replies in its thread")
                        .into());
                }
                let standing = agents::standing_of(tx.conn(), actor).await?;
                policy::can_close_ask(actor, standing, &ask.sender).map_err(FoldError::from)?;
                close(
                    tx,
                    &mut buffer,
                    actor,
                    &ask,
                    AskClosure::AnsweredElsewhere,
                    now,
                )
                .await
            })
            .await?;
        self.bus.publish(buffer);
        Ok(message)
    }

    /// A human marks the open asks of `scope` answered, each with `message.ask_closed` in one transaction, and gets their ids in number order; none when nothing waits. A worker's `AskUserQuestion` message stays open, because its worker waits for the human's reply in the thread.
    pub async fn dismiss_asks(
        &self,
        actor: &Actor,
        scope: DismissScope,
    ) -> Result<Vec<MessageId>, MessageError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let closed = self
            .store
            .write(async |tx| {
                let scope = match scope {
                    DismissScope::Thread(message) => {
                        let head = messages::find(tx.conn(), message)
                            .await?
                            .ok_or_else(|| message_not_found(message))?;
                        AskScope::Thread(head.thread)
                    }
                    DismissScope::Task(task) => {
                        require_task(tx, task).await?;
                        AskScope::Task(task)
                    }
                };
                let mut closed = Vec::new();
                for ask in messages::open_asks(tx.conn(), scope).await? {
                    if ask.worker_question {
                        continue;
                    }
                    policy::can_close_ask(
                        actor,
                        agents::standing_of(tx.conn(), actor).await?,
                        &ask.sender,
                    )
                    .map_err(FoldError::from)?;
                    close(tx, &mut buffer, actor, &ask, AskClosure::Dismissed, now).await?;
                    closed.push(ask.id);
                }
                Ok::<_, MessageError>(closed)
            })
            .await?;
        self.bus.publish(buffer);
        Ok(closed)
    }
}

async fn close(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    ask: &OpenAsk,
    reason: AskClosure,
    now: Timestamp,
) -> Result<(), MessageError> {
    let by = actor.event_actor();
    messages::close_ask(tx, ask.id, reason, by.as_str(), now).await?;
    let event = NewEvent::new(types::MESSAGE_ASK_CLOSED, by)?
        .message(ask.id)
        .task(ask.task)
        .data(json!({
            "message": ask.id.to_string(),
            "thread": ask.thread.to_string(),
            "reason": reason.as_str(),
        }))?;
    buffer.append(tx, &event, now).await?;
    Ok(())
}

fn not_open(message: MessageId) -> FoldError {
    FoldError::new(
        codes::ASK_NOT_OPEN,
        format!("{message} is not an open ask to a human"),
    )
    .with_details([("message", json!(message.to_string()))])
    .with_hint("call get_thread to see the thread; a human's reply or an earlier close already ended the ask")
}
