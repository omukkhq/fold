use std::sync::Arc;

use fold_core::events::types;
use fold_core::messages::resolve;
use fold_core::{
    Actor, Address, AddressedAs, AgentActor, AgentName, Choice, Clock, EventError, FoldError, HumanName,
    IdKind, MessageBody, MessageChoices, MessageId, NewEvent, ProposalId, Recipient, RequestId,
    Sender, Target, TaskId, Timestamp, WorkerName, codes, policy,
};
use fold_store::messages::{self, MessageHead, MessageRow, NewMessage, RecipientRow};
use fold_store::tasks::claims;
use fold_store::{Store, StoreError, WriteTx, counters, projects, reviews, tasks};
use serde_json::json;
use thiserror::Error;

use crate::services::agents::{actor_worker, worker_not_found};
use crate::services::events::{EventBuffer, EventBus};
#[cfg(feature = "failpoints")]
use crate::services::fired;

mod chat;
mod inbox;
mod threads;

pub use chat::{ChatPage, DEFAULT_CHAT_LIMIT, MAX_CHAT_LIMIT};
pub use inbox::{InboxItem, unread_count};
pub use threads::{
    DEFAULT_THREAD_LIMIT, Feedback, MAX_THREAD_LIMIT, ThreadPage, ThreadQuery, ThreadView,
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

    /// The code a log line names: the stable code, or `internal_error` for an internal failure.
    pub fn log_code(&self) -> &'static str {
        self.code().unwrap_or(codes::INTERNAL_ERROR)
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
    /// The request whose clarification thread holds the message.
    pub request: Option<RequestId>,
    pub body: String,
    pub sent_at: Timestamp,
    pub recipients: Vec<RecipientView>,
    /// The options of a multiple-choice question, in order; empty for any other message.
    pub choices: Vec<Choice>,
    /// The short header of a multiple-choice question.
    pub header: Option<String>,
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
        request: row.request,
        body: row.body,
        sent_at: row.created_at,
        recipients: row.recipients.into_iter().map(recipient_view).collect(),
        choices: row.choices,
        header: row.header,
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
        self.send_linked(actor, input, None).await
    }

    /// [`MessageService::send`] with the message linked to `request`, so it joins the request's clarification thread: the thread its first message started, or a new one. Anyone may write there, and the message never changes the request. A reply takes its parent's request; a reply that names another request, or a request for a parent outside its thread, is `invalid_request` with `details.reason = "request_thread"`.
    pub async fn send_linked(
        &self,
        actor: &Actor,
        input: SendMessage,
        request: Option<RequestId>,
    ) -> Result<MessageView, MessageError> {
        self.send_with_choices(actor, input, request, None).await
    }

    /// [`MessageService::send_linked`] for a question with `choices`: the recipients answer with a normal reply that names one choice, or with free text. Every send passes `policy::can_send_message`, which admits every registered agent, every named human and fold itself.
    pub async fn send_with_choices(
        &self,
        actor: &Actor,
        input: SendMessage,
        request: Option<RequestId>,
        choices: Option<MessageChoices>,
    ) -> Result<MessageView, MessageError> {
        self.send_inner(actor, input, request, choices, false).await
    }

    /// The send of [`MessageService::send_with_choices`]. With `to_drafter` the message goes to the drafter of `request`, read in the send's own transaction, instead of to `input.to`.
    pub(super) async fn send_inner(
        &self,
        actor: &Actor,
        input: SendMessage,
        request: Option<RequestId>,
        choices: Option<MessageChoices>,
        to_drafter: bool,
    ) -> Result<MessageView, MessageError> {
        policy::can_send_message(actor).map_err(FoldError::from)?;
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
                if let (Some(request), Some(parent)) = (request, parent)
                    && parent.request != Some(request)
                {
                    return Err(other_thread(request, parent.id).into());
                }
                let request = request.or(parent.and_then(|p| p.request));
                let join = match (request, parent) {
                    (Some(request), None) => require_request(tx, request).await?,
                    _ => None,
                };
                if let Some(task) = input.task {
                    require_task(tx, task).await?;
                }
                if let Some(proposal) = input.proposal
                    && !reviews::proposal_exists(tx.conn(), proposal).await?
                {
                    return Err(crate::services::reviews::proposal_missing(proposal).into());
                }
                let to = match request {
                    Some(request) if to_drafter => {
                        let row = tasks::requests::fetch(tx.conn(), request)
                            .await?
                            .ok_or_else(|| crate::services::tasks::request_missing(request))?;
                        let drafter = row.drafter.ok_or_else(|| threads::no_drafter(request))?;
                        vec![Address::of_worker(&drafter.worker)]
                    }
                    _ => input.to.clone(),
                };
                let directory = messages::directory(tx.conn(), humans).await?;
                let recipients = resolve(&to, &directory, &sender).map_err(FoldError::from)?;
                let draft = Draft {
                    actor,
                    sender: &sender,
                    task: input.task.or(parent.and_then(|p| p.task)),
                    proposal: input.proposal.or(parent.and_then(|p| p.proposal)),
                    request,
                    join,
                    parent,
                    body: &input.body,
                    choices: choices.as_ref(),
                    recipients,
                    now,
                };
                let view = write_message(tx, &mut buffer, draft).await?;
                #[cfg(feature = "failpoints")]
                fail::fail_point!("send_message_in_transaction", |_| Err(fired(
                    "send_message_in_transaction"
                )
                .into()));
                Ok::<_, MessageError>(view)
            })
            .await?;
        #[cfg(feature = "failpoints")]
        fail::fail_point!("send_message_after_commit", |_| Err(fired(
            "send_message_after_commit"
        )
        .into()));
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
                    request: None,
                    join: None,
                    parent: None,
                    body: &body,
                    choices: None,
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
}

struct Draft<'a> {
    actor: &'a Actor,
    sender: &'a Sender,
    task: Option<TaskId>,
    proposal: Option<ProposalId>,
    request: Option<RequestId>,
    /// The thread a message without a parent joins: its request's thread.
    join: Option<MessageId>,
    parent: Option<MessageHead>,
    body: &'a MessageBody,
    choices: Option<&'a MessageChoices>,
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
    let thread = draft
        .parent
        .map(|parent| parent.thread)
        .or(draft.join)
        .unwrap_or(id);
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
            request: draft.request,
            body: draft.body.as_str(),
            now: draft.now,
        },
    )
    .await?;
    for recipient in &draft.recipients {
        messages::insert_recipient(tx, id, recipient).await?;
    }
    let choices: Vec<Choice> = draft
        .choices
        .map(|choices| choices.options().to_vec())
        .unwrap_or_default();
    let header = draft
        .choices
        .and_then(MessageChoices::header)
        .map(str::to_owned);
    messages::insert_choices(tx, id, &choices, header.as_deref()).await?;
    let names: Vec<String> = draft
        .recipients
        .iter()
        .map(|recipient| recipient.target.to_string())
        .collect();
    let mut data = json!({
        "message": id.to_string(),
        "thread": thread.to_string(),
        "reply_to": reply_to.map(|parent| parent.to_string()),
        "recipients": names,
    });
    if let Some(request) = draft.request {
        data["request"] = json!(request.to_string());
    }
    let mut event = NewEvent::new(types::MESSAGE_SENT, draft.actor.event_actor())?
        .message(id)
        .data(data)?;
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
        request: draft.request,
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
        choices,
        header,
    })
}

/// Sends `body` from `fold` to `recipients` inside the caller's transaction, linked to `task` and, when it has one, `proposal`, so a command can tell the agents concerned about what it just did in the same commit. The caller knows each recipient worker exists and publishes the buffer after the commit.
pub(crate) async fn send_from_fold_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    recipients: Vec<Recipient>,
    links: (TaskId, Option<ProposalId>),
    body: &MessageBody,
    now: Timestamp,
) -> Result<MessageView, MessageError> {
    let draft = Draft {
        actor: &Actor::System,
        sender: &Sender::Fold,
        task: Some(links.0),
        proposal: links.1,
        request: None,
        join: None,
        parent: None,
        body,
        choices: None,
        recipients,
        now,
    };
    write_message(tx, buffer, draft).await
}

/// Sends `body` from `fold` to the main worker of each of `agents` inside the caller's transaction, linked to nothing, so a command can tell agents about a change that no task carries. Sends nothing when `agents` is empty. The caller publishes the buffer after the commit.
pub(crate) async fn tell_agents_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    agents: &[AgentName],
    body: &MessageBody,
    now: Timestamp,
) -> Result<Option<MessageView>, MessageError> {
    if agents.is_empty() {
        return Ok(None);
    }
    let to: Vec<Address> = agents.iter().cloned().map(Address::Agent).collect();
    let directory = messages::directory(tx.conn(), Vec::new()).await?;
    let recipients = resolve(&to, &directory, &Sender::Fold).map_err(FoldError::from)?;
    let draft = Draft {
        actor: &Actor::System,
        sender: &Sender::Fold,
        task: None,
        proposal: None,
        request: None,
        join: None,
        parent: None,
        body,
        choices: None,
        recipients,
        now,
    };
    Ok(Some(write_message(tx, buffer, draft).await?))
}

/// A worker's question to every human in `humans`, linked to `task`, with `choices` when the question offers them.
pub(crate) struct WorkerQuestion<'a> {
    pub humans: Vec<HumanName>,
    pub worker: &'a WorkerName,
    pub task: Option<TaskId>,
    pub body: &'a MessageBody,
    pub choices: Option<&'a MessageChoices>,
    pub now: Timestamp,
}

/// Sends a worker's question inside the caller's transaction. It stands in for the `AskUserQuestion` call that a headless worker cannot show to anyone. The caller publishes the buffer after the commit.
pub(crate) async fn ask_humans_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    question: WorkerQuestion<'_>,
) -> Result<MessageView, MessageError> {
    let sender = Sender::Worker(question.worker.clone());
    let directory = messages::directory(tx.conn(), question.humans).await?;
    let recipients = resolve(&[Address::Humans], &directory, &sender).map_err(FoldError::from)?;
    let actor = Actor::Agent(AgentActor {
        agent: question.worker.agent(),
        worker: question.worker.clone(),
        tags: Vec::new(),
    });
    let draft = Draft {
        actor: &actor,
        sender: &sender,
        task: question.task,
        proposal: None,
        request: None,
        join: None,
        parent: None,
        body: question.body,
        choices: question.choices,
        recipients,
        now: question.now,
    };
    write_message(tx, buffer, draft).await
}

/// An agent sends through a registered worker; a human or the system needs no row.
async fn require_sender(tx: &mut WriteTx, actor: &Actor) -> Result<(), MessageError> {
    let Actor::Agent(agent) = actor else {
        return Ok(());
    };
    if actor_worker(tx.conn(), agent).await?.is_none() {
        return Err(worker_not_found().into());
    }
    Ok(())
}

async fn require_task(tx: &mut WriteTx, task: TaskId) -> Result<(), MessageError> {
    if tasks::fetch(tx.conn(), task).await?.is_none() {
        return Err(crate::services::tasks::task_missing(task).into());
    }
    Ok(())
}

/// The request's thread, which a new message about it joins; refused with `request_not_found` when the request does not exist.
async fn require_request(
    tx: &mut WriteTx,
    request: RequestId,
) -> Result<Option<MessageId>, MessageError> {
    if fold_store::tasks::requests::fetch(tx.conn(), request)
        .await?
        .is_none()
    {
        return Err(crate::services::tasks::request_missing(request).into());
    }
    Ok(messages::request_thread(tx.conn(), request).await?)
}

fn other_thread(request: RequestId, parent: MessageId) -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!("{parent} is not in the thread of {request}"),
    )
    .with_details([
        ("reason", json!("request_thread")),
        ("request", json!(request.to_string())),
        ("reply_to", json!(parent.to_string())),
    ])
    .with_hint("call get_thread with the request to see its messages, then leave out reply_to to join the request's thread, or leave out request to reply")
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
