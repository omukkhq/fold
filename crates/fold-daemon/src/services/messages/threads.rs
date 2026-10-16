use std::collections::{BTreeSet, HashSet};

use fold_core::messages::reply_addresses;
use fold_core::{
    Actor, Address, AgentName, FoldError, MessageBody, MessageId, ProposalId, RequestId, Sender,
    TaskId, Timestamp, WorkerName, codes, policy,
};
use fold_store::SqliteConnection;
use fold_store::StoreError;
use fold_store::messages::{self, MessageRow, ThreadFilter};
use fold_store::tasks::requests;
use serde_json::json;

use super::{
    MessageError, MessageService, MessageView, SendMessage, message_not_found, message_view,
};
use crate::services::page_limit;

/// The page size when a listing names no `limit`.
pub const DEFAULT_THREAD_LIMIT: u32 = 50;

/// The largest page one listing returns; a larger `limit` is lowered to it.
pub const MAX_THREAD_LIMIT: u32 = 200;

/// A thread listing as a transport receives it. The service parses it, so every transport refuses the same input with the same code.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadQuery {
    /// An agent name, which matches every worker of the agent, or a full worker name.
    pub agent: Option<String>,
    pub task: Option<String>,
    pub proposal: Option<String>,
    pub awaiting_human: bool,
    /// The `next` of the previous page.
    pub before: Option<String>,
    pub limit: Option<u32>,
}

/// One thread with every message in number order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadView {
    /// The id of the thread's first message.
    pub id: MessageId,
    /// The first task a message of the thread is linked to.
    pub task: Option<TaskId>,
    /// The first proposal a message of the thread is linked to.
    pub proposal: Option<ProposalId>,
    /// The request whose clarification thread this is.
    pub request: Option<RequestId>,
    /// Every sender and recipient, in the order they first appear.
    pub participants: Vec<String>,
    /// The thread holds a question to a human that no human has answered.
    pub awaiting_human: bool,
    pub last_at: Timestamp,
    pub messages: Vec<MessageView>,
}

/// Threads with the newest message first. `next` is the `before` of the following page, `None` on the last page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadPage {
    pub items: Vec<ThreadView>,
    pub next: Option<MessageId>,
    /// Threads with a question to a human that no human has answered, in the whole project whatever the filter.
    pub total_awaiting_human: u64,
}

/// A human's feedback on a task, message in a request's clarification thread, or reply to a message. It needs `task`, `request` or `reply_to`.
#[derive(Debug, Clone)]
pub struct Feedback {
    pub body: MessageBody,
    pub task: Option<TaskId>,
    /// The request whose clarification thread the message joins. A reply takes its parent's request, and naming another is refused.
    pub request: Option<RequestId>,
    pub reply_to: Option<MessageId>,
    /// Left out, feedback goes to the task's claim holder, a request message to the request's drafter and a reply to the parent's sender.
    pub to: Option<Vec<Address>>,
}

impl MessageService {
    /// One page of threads, newest message first, read in one read transaction.
    pub async fn threads(&self, query: &ThreadQuery) -> Result<ThreadPage, MessageError> {
        let (filter, limit) = parse_query(query)?;
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        let mut heads = messages::thread_page(&mut tx, &filter, limit + 1).await?;
        let more = heads.len() > limit as usize;
        heads.truncate(limit as usize);
        let ids: Vec<MessageId> = heads.iter().map(|head| head.thread).collect();
        let rows = messages::threads(&mut tx, &ids).await?;
        let awaiting = messages::threads_awaiting_human(&mut *tx).await?;
        tx.commit().await.map_err(StoreError::from)?;
        let mut items: Vec<ThreadView> = group(rows)
            .into_iter()
            .map(|(thread, rows)| thread_view(thread, rows, &awaiting))
            .collect();
        items.sort_by_key(|thread| std::cmp::Reverse(last_number(thread)));
        let next = more.then(|| heads.last().map(|head| head.last)).flatten();
        Ok(ThreadPage {
            items,
            next,
            total_awaiting_human: awaiting.len() as u64,
        })
    }

    /// The thread that `message` belongs to; any message of the thread names it.
    pub async fn thread_view(&self, message: MessageId) -> Result<ThreadView, MessageError> {
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        let head = messages::find(&mut tx, message)
            .await?
            .ok_or_else(|| message_not_found(message))?;
        let rows = messages::thread(&mut tx, head.thread).await?;
        let awaiting = awaiting_set(&mut tx, head.thread).await?;
        tx.commit().await.map_err(StoreError::from)?;
        Ok(thread_view(head.thread, rows, &awaiting))
    }

    /// A human's feedback (`task`), message in a request's clarification thread (`request`, linked to `task` when given) or reply (`reply_to`). A reply joins the parent's thread and inherits its task unless `task` names another, and its request; it answers a question to humans, which clears the task's `question` attention.
    pub async fn feedback(
        &self,
        actor: &Actor,
        input: Feedback,
    ) -> Result<MessageView, MessageError> {
        let Some(parent) = input.reply_to else {
            if let Some(request) = input.request {
                policy::can_send_feedback(actor).map_err(FoldError::from)?;
                return self
                    .request_feedback(actor, request, input.task, input.body, input.to)
                    .await;
            }
            let task = input.task.ok_or_else(|| {
                FoldError::new(
                    codes::INVALID_REQUEST,
                    "feedback needs a task, a request or reply_to",
                )
                .with_hint("name the task, the request or the message the feedback answers")
            })?;
            // `send_feedback` checks the policy itself.
            return self.send_feedback(actor, task, input.body, input.to).await;
        };
        policy::can_send_feedback(actor).map_err(FoldError::from)?;
        let to = match input.to {
            Some(to) => to,
            None => self.reply_default(actor, parent).await?,
        };
        self.send_linked(
            actor,
            SendMessage {
                to,
                body: input.body,
                task: input.task,
                proposal: None,
                reply_to: Some(parent),
            },
            input.request,
        )
        .await
    }

    /// The request's clarification thread, `None` before anyone wrote about it; refused with `request_not_found` when the request does not exist.
    pub async fn request_thread(
        &self,
        request: RequestId,
    ) -> Result<Option<ThreadView>, MessageError> {
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        if requests::fetch(&mut tx, request).await?.is_none() {
            return Err(crate::services::tasks::request_missing(request).into());
        }
        let Some(thread) = messages::request_thread(&mut tx, request).await? else {
            return Ok(None);
        };
        let rows = messages::thread(&mut tx, thread).await?;
        let awaiting = awaiting_set(&mut tx, thread).await?;
        tx.commit().await.map_err(StoreError::from)?;
        Ok(Some(thread_view(thread, rows, &awaiting)))
    }

    /// A named human's message in the request's clarification thread, linked to `task` when given. It goes to `to` when given, otherwise to the request's drafter; with neither it is `recipient_not_found`.
    async fn request_feedback(
        &self,
        actor: &Actor,
        request: RequestId,
        task: Option<TaskId>,
        body: MessageBody,
        to: Option<Vec<Address>>,
    ) -> Result<MessageView, MessageError> {
        let to_drafter = to.is_none();
        self.send_inner(
            actor,
            SendMessage {
                to: to.unwrap_or_default(),
                body,
                task,
                proposal: None,
                reply_to: None,
            },
            Some(request),
            None,
            to_drafter,
        )
        .await
    }

    async fn reply_default(
        &self,
        actor: &Actor,
        parent: MessageId,
    ) -> Result<Vec<Address>, MessageError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let row = messages::get(&mut conn, parent)
            .await?
            .ok_or_else(|| message_not_found(parent))?;
        let recipients: Vec<_> = row
            .recipients
            .into_iter()
            .map(|recipient| recipient.target)
            .collect();
        Ok(reply_addresses(
            &row.sender,
            &recipients,
            &Sender::of(actor),
        ))
    }
}

pub(super) fn no_drafter(request: RequestId) -> FoldError {
    FoldError::new(
        codes::RECIPIENT_NOT_FOUND,
        format!("{request} has no drafter to receive the message"),
    )
    .with_details([("request", json!(request.to_string()))])
    .with_hint("choose a recipient for the message")
}

fn parse_query(query: &ThreadQuery) -> Result<(ThreadFilter, u32), FoldError> {
    let (agent, worker) = match query.agent.as_deref() {
        None => (None, None),
        Some(name) if name.contains('/') => (None, Some(WorkerName::parse(name)?)),
        Some(name) => (Some(AgentName::parse(name)?), None),
    };
    let limit = page_limit(query.limit, MAX_THREAD_LIMIT)?.unwrap_or(DEFAULT_THREAD_LIMIT);
    Ok((
        ThreadFilter {
            agent,
            worker,
            task: query
                .task
                .as_deref()
                .map(str::parse::<TaskId>)
                .transpose()?,
            proposal: query
                .proposal
                .as_deref()
                .map(str::parse::<ProposalId>)
                .transpose()?,
            awaiting_human: query.awaiting_human,
            before: query
                .before
                .as_deref()
                .map(str::parse::<MessageId>)
                .transpose()?,
        },
        limit,
    ))
}

/// Splits rows ordered by thread into one list per thread.
fn group(rows: Vec<MessageRow>) -> Vec<(MessageId, Vec<MessageRow>)> {
    let mut groups: Vec<(MessageId, Vec<MessageRow>)> = Vec::new();
    for row in rows {
        match groups.last_mut() {
            Some((thread, group)) if *thread == row.thread => group.push(row),
            _ => groups.push((row.thread, vec![row])),
        }
    }
    groups
}

fn last_number(thread: &ThreadView) -> Option<MessageId> {
    thread.messages.last().map(|message| message.id)
}

/// `rows` are the messages of thread `id` in number order.
/// The one thread a single-thread read shows, as the set [`thread_view`] takes: holding `thread` when it waits for a human.
async fn awaiting_set(
    conn: &mut SqliteConnection,
    thread: MessageId,
) -> Result<BTreeSet<MessageId>, StoreError> {
    Ok(
        if messages::thread_awaits_human(&mut *conn, thread).await? {
            BTreeSet::from([thread])
        } else {
            BTreeSet::new()
        },
    )
}

fn thread_view(id: MessageId, rows: Vec<MessageRow>, awaiting: &BTreeSet<MessageId>) -> ThreadView {
    let mut seen = HashSet::new();
    let mut participants = Vec::new();
    for row in &rows {
        let names = std::iter::once(row.sender.to_string()).chain(
            row.recipients
                .iter()
                .map(|recipient| recipient.target.to_string()),
        );
        for name in names {
            if seen.insert(name.clone()) {
                participants.push(name);
            }
        }
    }
    let messages: Vec<MessageView> = rows.into_iter().map(message_view).collect();
    ThreadView {
        id,
        task: messages.iter().find_map(|message| message.task),
        proposal: messages.iter().find_map(|message| message.proposal),
        request: messages.iter().find_map(|message| message.request),
        participants,
        awaiting_human: awaiting.contains(&id),
        last_at: messages
            .last()
            .map_or(Timestamp::EPOCH, |message| message.sent_at),
        messages,
    }
}
