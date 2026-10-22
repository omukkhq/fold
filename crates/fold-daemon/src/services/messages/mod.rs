use std::collections::BTreeSet;
use std::sync::Arc;

use fold_core::events::types;
use fold_core::messages::resolve;
use fold_core::tasks::areas::feedback_to;
use fold_core::{
    Actor, Address, AddressedAs, AgentActor, AgentName, Choice, Clock, EventError, FoldError,
    HumanName, IdKind, MessageBody, MessageChoices, MessageId, NewEvent, ProposalId, Recipient,
    RequestId, Sender, Target, TaskId, Timestamp, WorkerName, codes, policy,
};
use fold_store::messages::{self, MessageHead, MessageLines, MessageRow, NewMessage, RecipientRow};
use fold_store::{Store, StoreError, WriteTx, agents, counters, projects, reviews, tasks};
use serde_json::json;
use thiserror::Error;

use crate::push::PushTargets;
use crate::services::agents::{actor_worker, worker_not_found};
use crate::services::events::{EventBuffer, EventBus};
#[cfg(feature = "failpoints")]
use crate::services::fired;

mod asks;
mod forward;
mod inbox;
mod threads;

pub use asks::DismissScope;
pub(crate) use forward::{forward_in_tx, tell_lead_in_tx};
pub(crate) use inbox::record_pushed;
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
    /// The first time a notice about the message reached a session or the worker process of the recipient's agent; `None` for a human and while no notice got through.
    pub pushed_at: Option<Timestamp>,
    /// Whether a notice to the recipient's agent would reach one of its sessions now; `None` for a human, for an agent without a connected session, and for every recipient of a service without the project's push targets.
    pub reachable: Option<bool>,
    /// The lead fold sent the message on to because the recipient, a stopped worker, could not start to answer it; the sender itself when the sender is that lead.
    pub forwarded_to: Option<WorkerName>,
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
    /// The sender marked the message as asking a human for an answer.
    pub asks_human: bool,
    /// The lines of a version of the message's proposal that the message asks about.
    pub lines: Option<MessageLines>,
}

fn recipient_view(row: RecipientRow) -> RecipientView {
    RecipientView {
        name: row.target,
        addressed_as: row.addressed_as,
        delivered_at: row.delivered_at,
        acknowledged_at: row.acked_at,
        pushed_at: row.pushed_at,
        reachable: None,
        forwarded_to: row.forwarded_to,
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
        asks_human: row.asks_human,
        lines: row.lines,
    }
}

/// The messages of one project. Humans live in the host registry, so the service reads `registry` before each send to resolve `human:<name>` and `humans`. A message never writes a task, claim or status row.
#[derive(Clone)]
pub struct MessageService {
    store: Store,
    registry: Store,
    bus: EventBus,
    clock: Arc<dyn Clock>,
    push: Option<PushTargets>,
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
            push: None,
        }
    }

    /// Lets the thread views tell whether each agent recipient can be reached by a notice now.
    #[must_use]
    pub fn with_push_targets(mut self, push: PushTargets) -> Self {
        self.push = Some(push);
        self
    }

    /// The agents with a connected session, read only when the service has push targets to judge them by.
    async fn connected(
        &self,
        conn: &mut fold_store::SqliteConnection,
    ) -> Result<BTreeSet<AgentName>, MessageError> {
        if self.push.is_none() {
            return Ok(BTreeSet::new());
        }
        Ok(agents::open_bound_sessions(conn)
            .await?
            .into_iter()
            .map(|session| session.agent)
            .collect())
    }

    /// Sets `reachable` of every agent recipient with a connected session from the push targets as they are now. An agent without one, such as a supervised worker between its turns, is left `None`: no session of it could take a notice, and nothing is wrong with that.
    fn mark_reach(&self, messages: &mut [MessageView], connected: &BTreeSet<AgentName>) {
        let Some(push) = &self.push else {
            return;
        };
        for recipient in messages
            .iter_mut()
            .flat_map(|message| message.recipients.iter_mut())
        {
            if let Target::Worker(worker) = &recipient.name
                && connected.contains(&worker.agent())
            {
                recipient.reachable = Some(push.reach(&worker.agent()).reachable());
            }
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
        self.send_inner(actor, input, request, choices, SendOptions::default())
            .await
    }

    /// [`MessageService::send_with_choices`] for a message that `asks_human` marks as asking a human for an answer: it waits for a human, as a question with choices does, until a human writes in its thread or the ask is closed. A mark without a human recipient is `invalid_request`.
    pub async fn send_marked(
        &self,
        actor: &Actor,
        input: SendMessage,
        request: Option<RequestId>,
        choices: Option<MessageChoices>,
        asks_human: bool,
    ) -> Result<MessageView, MessageError> {
        let send = SendOptions {
            asks_human,
            ..SendOptions::default()
        };
        self.send_inner(actor, input, request, choices, send).await
    }

    /// The send of [`MessageService::send_marked`]. With `send.to_drafter` the message goes to the drafter of `request`, read in the send's own transaction, instead of to `input.to`.
    pub(super) async fn send_inner(
        &self,
        actor: &Actor,
        input: SendMessage,
        request: Option<RequestId>,
        choices: Option<MessageChoices>,
        send: SendOptions,
    ) -> Result<MessageView, MessageError> {
        policy::can_send_message(actor).map_err(FoldError::from)?;
        let humans = projects::list_humans(self.registry.read()).await?;
        let sender = Sender::of(actor);
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let view = self
            .store
            .write(async |tx| {
                let sender_worker = require_sender(tx, actor).await?;
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
                    Some(request) if send.to_drafter => {
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
                if send.asks_human && !reaches_human(&recipients) {
                    return Err(ask_without_human().into());
                }
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
                    asks_human: send.asks_human,
                    lines: None,
                    recipients,
                    now,
                };
                let view = write_message(tx, &mut buffer, draft).await?;
                // A reply also acknowledges the message it answers for a sender that received it.
                if let (Some(worker), Some(parent)) = (&sender_worker, parent) {
                    let changed =
                        fold_store::messages::acknowledge(tx, worker.id, parent.id, false, now)
                            .await?
                            .unwrap_or_default();
                    inbox::append_acknowledged(tx, &mut buffer, actor, &changed, now).await?;
                }
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

    /// A human's feedback on `task`, linked to it. It goes to the task's readers only: to `to` when every entry names one, otherwise to the first reader, the claim holder when there is one.
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
                let readers = crate::services::tasks::readers_in(tx.conn(), task).await?;
                let to = feedback_to(task, to, &readers)?;
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
                    asks_human: false,
                    lines: None,
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
    asks_human: bool,
    /// The lines of a version of `proposal` the message asks about.
    lines: Option<MessageLines>,
    recipients: Vec<Recipient>,
    now: Timestamp,
}

/// How [`MessageService::send_inner`] sends beyond its input.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct SendOptions {
    /// The message goes to the drafter of its request instead of to its `to`.
    pub to_drafter: bool,
    /// The sender marks the message as asking a human for an answer.
    pub asks_human: bool,
}

fn reaches_human(recipients: &[Recipient]) -> bool {
    recipients
        .iter()
        .any(|recipient| matches!(recipient.target, Target::Human(_)))
}

fn ask_without_human() -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        "only a message to a human asks a human for an answer",
    )
    .with_hint("send the ask to humans or human:<name>, or leave out asks_human")
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
            asks_human: draft.asks_human,
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
    if let (Some(lines), Some(proposal)) = (&draft.lines, draft.proposal) {
        messages::insert_lines(tx, id, proposal, lines).await?;
    }
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
                pushed_at: None,
                reachable: None,
                forwarded_to: None,
            })
            .collect(),
        choices,
        header,
        asks_human: draft.asks_human,
        lines: draft.lines,
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
        asks_human: false,
        lines: None,
        recipients,
        now,
    };
    write_message(tx, buffer, draft).await
}

/// A human's question to the worker `to` about `lines` of a version of `links.1`, linked to the proposal and its task `links.0`.
pub(crate) struct LineQuestion<'a> {
    pub actor: &'a Actor,
    pub to: WorkerName,
    pub links: (TaskId, ProposalId),
    pub body: &'a MessageBody,
    pub lines: MessageLines,
    pub now: Timestamp,
}

/// Sends a question about lines of a proposal version inside the caller's transaction, as one message that starts its own thread. The worker is resolved as any other send resolves it. The caller publishes the buffer after the commit.
pub(crate) async fn ask_about_lines_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    question: LineQuestion<'_>,
) -> Result<MessageView, MessageError> {
    let sender = Sender::of(question.actor);
    let directory = messages::directory(tx.conn(), Vec::new()).await?;
    let recipients = resolve(&[Address::of_worker(&question.to)], &directory, &sender)
        .map_err(FoldError::from)?;
    let draft = Draft {
        actor: question.actor,
        sender: &sender,
        task: Some(question.links.0),
        proposal: Some(question.links.1),
        request: None,
        join: None,
        parent: None,
        body: question.body,
        choices: None,
        asks_human: false,
        lines: Some(question.lines),
        recipients,
        now: question.now,
    };
    write_message(tx, buffer, draft).await
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
        asks_human: false,
        lines: None,
        recipients,
        now: question.now,
    };
    write_message(tx, buffer, draft).await
}

/// The registered worker an agent sends through; a human or the system needs no row and gets `None`.
async fn require_sender(
    tx: &mut WriteTx,
    actor: &Actor,
) -> Result<Option<agents::WorkerRow>, MessageError> {
    let Actor::Agent(agent) = actor else {
        return Ok(None);
    };
    match actor_worker(tx.conn(), agent).await? {
        Some(worker) => Ok(Some(worker)),
        None => Err(worker_not_found().into()),
    }
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
