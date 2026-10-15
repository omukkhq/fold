use std::sync::Arc;

use fold_core::events::types;
use fold_core::messages::resolve;
use fold_core::{
    Actor, Address, AddressedAs, Clock, EventError, FoldError, IdKind, MessageBody, MessageId,
    NewEvent, ProposalId, Recipient, Sender, Target, TaskId, Timestamp, codes, policy,
};
use fold_store::messages::{self, MessageHead, MessageRow, NewMessage, RecipientRow};
use fold_store::tasks::claims;
use fold_store::{Store, StoreError, WriteTx, agents, counters, projects, reviews, tasks};
use serde_json::json;
use thiserror::Error;

use crate::services::events::{EventBuffer, EventBus};

mod inbox;
mod threads;

pub use inbox::{InboxItem, unread_count};
pub use threads::{
    DEFAULT_THREAD_LIMIT, Feedback, MAX_THREAD_LIMIT, ThreadPage, ThreadQuery, ThreadView,
    tasks_awaiting_human,
};

#[derive(Debug, Error)]
pub enum MessageError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A command built an event the declared types refuse: a defect in the command, not in the input.
    #[error(transparent)]
    Event(#[from] EventError),
}

impl MessageError {
    /// The stable code of a refusal, `None` for an internal failure.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            MessageError::Fold(error) => Some(error.code()),
            MessageError::Store(error) => error.code(),
            MessageError::Event(_) => None,
        }
    }
}

/// A message to send. `task` and `proposal` left `None` on a reply take the parent's.
#[derive(Debug, Clone)]
pub struct SendMessage {
    pub to: Vec<Address>,
    pub body: MessageBody,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
    pub reply_to: Option<MessageId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipientView {
    /// The worker name, or `human:<name>`.
    pub name: Target,
    pub addressed_as: AddressedAs,
    pub delivered_at: Option<Timestamp>,
    pub acknowledged_at: Option<Timestamp>,
}

/// A message with public ids only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageView {
    pub id: MessageId,
    /// The id of the thread's first message.
    pub thread: MessageId,
    pub reply_to: Option<MessageId>,
    pub sender: Sender,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
    pub body: String,
    pub sent_at: Timestamp,
    pub recipients: Vec<RecipientView>,
}

fn recipient_view(row: RecipientRow) -> RecipientView {
    RecipientView {
        name: row.target,
        addressed_as: row.addressed_as,
        delivered_at: row.delivered_at,
        acknowledged_at: row.acked_at,
    }
}

fn message_view(row: MessageRow) -> MessageView {
    MessageView {
        id: row.id,
        thread: row.thread,
        reply_to: row.reply_to,
        sender: row.sender,
        task: row.task,
        proposal: row.proposal,
        body: row.body,
        sent_at: row.created_at,
        recipients: row.recipients.into_iter().map(recipient_view).collect(),
    }
}

/// The messages of one project. Humans live in the host registry, so the service reads `registry` before each send to resolve `human:<name>` and `humans`. A message never writes a task, claim or status row.
#[derive(Clone)]
pub struct MessageService {
    store: Store,
    registry: Store,
    bus: EventBus,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for MessageService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageService").finish_non_exhaustive()
    }
}

impl MessageService {
    pub fn new(store: Store, registry: Store, bus: EventBus, clock: Arc<dyn Clock>) -> Self {
        Self {
            store,
            registry,
            bus,
            clock,
        }
    }

    /// Sends `m-<n>` from the actor to every recipient `to` resolves to. A reply joins its parent's thread; any other message starts a thread whose id is its own number.
    pub async fn send(
        &self,
        actor: &Actor,
        input: SendMessage,
    ) -> Result<MessageView, MessageError> {
        let humans = projects::list_humans(self.registry.read()).await?;
        let sender = Sender::of(actor);
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let view = self
            .store
            .write(async |tx| {
                require_sender(tx, actor).await?;
                let parent = match input.reply_to {
                    Some(parent) => Some(
                        messages::find(tx.conn(), parent)
                            .await?
                            .ok_or_else(|| message_not_found(parent))?,
                    ),
                    None => None,
                };
                if let Some(task) = input.task {
                    require_task(tx, task).await?;
                }
                if let Some(proposal) = input.proposal
                    && !reviews::proposal_exists(tx.conn(), proposal).await?
                {
                    return Err(FoldError::new(
                        codes::PROPOSAL_NOT_FOUND,
                        format!("{proposal} does not exist"),
                    )
                    .with_hint("call list_proposals to see the proposals")
                    .into());
                }
                let directory = messages::directory(tx.conn(), humans).await?;
                let recipients =
                    resolve(&input.to, &directory, &sender).map_err(FoldError::from)?;
                let draft = Draft {
                    actor,
                    sender: &sender,
                    task: input.task.or(parent.and_then(|p| p.task)),
                    proposal: input.proposal.or(parent.and_then(|p| p.proposal)),
                    parent,
                    body: &input.body,
                    recipients,
                    now,
                };
                write_message(tx, &mut buffer, draft).await
            })
            .await?;
        self.bus.publish(buffer);
        Ok(view)
    }

    /// A human's feedback on `task`, linked to it. It goes to `to` when given, otherwise to the worker holding the task's implementation claim.
    pub async fn send_feedback(
        &self,
        actor: &Actor,
        task: TaskId,
        body: MessageBody,
        to: Option<Vec<Address>>,
    ) -> Result<MessageView, MessageError> {
        policy::can_send_feedback(actor).map_err(FoldError::from)?;
        let humans = projects::list_humans(self.registry.read()).await?;
        let sender = Sender::of(actor);
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let view = self
            .store
            .write(async |tx| {
                require_task(tx, task).await?;
                let to = match to {
                    Some(to) => to,
                    None => {
                        let holder = claims::unreleased_for_task(tx.conn(), task)
                            .await?
                            .ok_or_else(|| no_claim_holder(task))?;
                        vec![Address::of_worker(&holder.worker)]
                    }
                };
                let directory = messages::directory(tx.conn(), humans).await?;
                let recipients = resolve(&to, &directory, &sender).map_err(FoldError::from)?;
                let draft = Draft {
                    actor,
                    sender: &sender,
                    task: Some(task),
                    proposal: None,
                    parent: None,
                    body: &body,
                    recipients,
                    now,
                };
                write_message(tx, &mut buffer, draft).await
            })
            .await?;
        self.bus.publish(buffer);
        Ok(view)
    }

    /// Every message of the thread that `message` belongs to, in number order. Any message of the thread names it, its first message included.
    pub async fn thread(&self, message: MessageId) -> Result<Vec<MessageView>, MessageError> {
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        let head = messages::find(&mut tx, message)
            .await?
            .ok_or_else(|| message_not_found(message))?;
        let rows = messages::thread(&mut tx, head.thread).await?;
        tx.commit().await.map_err(StoreError::from)?;
        Ok(rows.into_iter().map(message_view).collect())
    }

    /// Questions from workers to humans that no human has answered in their thread yet, in number order, each with its task.
    pub async fn awaiting_human(&self) -> Result<Vec<MessageView>, MessageError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let rows = messages::awaiting_human(&mut conn).await?;
        Ok(rows.into_iter().map(message_view).collect())
    }
}

struct Draft<'a> {
    actor: &'a Actor,
    sender: &'a Sender,
    task: Option<TaskId>,
    proposal: Option<ProposalId>,
    parent: Option<MessageHead>,
    body: &'a MessageBody,
    recipients: Vec<Recipient>,
    now: Timestamp,
}

/// Allocates the number, writes the message and its recipient rows, and appends `message.sent`, whose data names the recipients and never holds the body.
async fn write_message(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    draft: Draft<'_>,
) -> Result<MessageView, MessageError> {
    let number = counters::allocate(tx, IdKind::Message).await?;
    let id = MessageId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
    let thread = draft.parent.map_or(id, |parent| parent.thread);
    let reply_to = draft.parent.map(|parent| parent.id);
    messages::insert(
        tx,
        &NewMessage {
            number: id,
            thread,
            reply_to,
            sender: draft.sender,
            task: draft.task,
            proposal: draft.proposal,
            body: draft.body.as_str(),
            now: draft.now,
        },
    )
    .await?;
    for recipient in &draft.recipients {
        messages::insert_recipient(tx, id, recipient).await?;
    }
    let names: Vec<String> = draft
        .recipients
        .iter()
        .map(|recipient| recipient.target.to_string())
        .collect();
    let mut event = NewEvent::new(types::MESSAGE_SENT, draft.actor.event_actor())?
        .message(id)
        .data(json!({
            "message": id.to_string(),
            "thread": thread.to_string(),
            "reply_to": reply_to.map(|parent| parent.to_string()),
            "recipients": names,
        }))?;
    if let Some(task) = draft.task {
        event = event.task(task);
    }
    if let Some(proposal) = draft.proposal {
        event = event.proposal(proposal);
    }
    buffer.append(tx, &event, draft.now).await?;
    Ok(MessageView {
        id,
        thread,
        reply_to,
        sender: draft.sender.clone(),
        task: draft.task,
        proposal: draft.proposal,
        body: draft.body.as_str().to_owned(),
        sent_at: draft.now,
        recipients: draft
            .recipients
            .into_iter()
            .map(|recipient| RecipientView {
                name: recipient.target,
                addressed_as: recipient.addressed_as,
                delivered_at: None,
                acknowledged_at: None,
            })
            .collect(),
    })
}

/// Sends `body` from `fold` to `recipients` inside the caller's transaction, linked to `task` and `proposal`, so a command can tell the agents concerned about what it just did in the same commit. The caller knows each recipient worker exists and publishes the buffer after the commit.
pub(crate) async fn send_from_fold_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    recipients: Vec<Recipient>,
    links: (TaskId, ProposalId),
    body: &MessageBody,
    now: Timestamp,
) -> Result<MessageView, MessageError> {
    let draft = Draft {
        actor: &Actor::System,
        sender: &Sender::Fold,
        task: Some(links.0),
        proposal: Some(links.1),
        parent: None,
        body,
        recipients,
        now,
    };
    write_message(tx, buffer, draft).await
}

/// An agent sends through a registered worker; a human or the system needs no row.
async fn require_sender(tx: &mut WriteTx, actor: &Actor) -> Result<(), MessageError> {
    let Actor::Agent(agent) = actor else {
        return Ok(());
    };
    let worker = match agents::find_agent(tx.conn(), &agent.agent).await? {
        Some(row) => agents::find_worker(tx.conn(), row.id, &agent.worker).await?,
        None => None,
    };
    if worker.is_none() {
        return Err(
            FoldError::new(codes::WORKER_NOT_FOUND, "the worker does not exist")
                .with_hint("call register_worker")
                .into(),
        );
    }
    Ok(())
}

async fn require_task(tx: &mut WriteTx, task: TaskId) -> Result<(), MessageError> {
    if tasks::fetch(tx.conn(), task).await?.is_none() {
        return Err(
            FoldError::new(codes::TASK_NOT_FOUND, format!("{task} does not exist"))
                .with_hint("call get_task_graph to see the tasks")
                .into(),
        );
    }
    Ok(())
}

fn message_not_found(message: MessageId) -> FoldError {
    FoldError::new(
        codes::MESSAGE_NOT_FOUND,
        format!("{message} does not exist"),
    )
    .with_hint("call get_inbox to see your messages")
}

fn no_claim_holder(task: TaskId) -> FoldError {
    FoldError::new(
        codes::RECIPIENT_NOT_FOUND,
        format!("{task} has no claim holder to receive the feedback"),
    )
    .with_details([("task", json!(task.to_string()))])
    .with_hint("choose a recipient for the feedback")
}
