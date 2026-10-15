use std::collections::{BTreeSet, HashSet};

use fold_core::messages::reply_addresses;
use fold_core::{
    Actor, Address, AgentName, FoldError, MessageBody, MessageId, ProposalId, Sender, TaskId,
    Timestamp, WorkerName, codes, policy,
};
use fold_store::messages::{self, MessageRow, ThreadFilter};
use fold_store::{Store, StoreError};

use super::{
    MessageError, MessageService, MessageView, SendMessage, message_not_found, message_view,
};

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

/// A human's feedback on a task or reply to a message. It needs `task` or `reply_to`.
#[derive(Debug, Clone)]
pub struct Feedback {
    pub body: MessageBody,
    pub task: Option<TaskId>,
    pub reply_to: Option<MessageId>,
    /// Left out, feedback goes to the task's claim holder and a reply to the parent's sender.
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
        let awaiting = messages::threads_awaiting_human(&mut *tx).await?;
        tx.commit().await.map_err(StoreError::from)?;
        Ok(thread_view(head.thread, rows, &awaiting))
    }

    /// A human's feedback (`task`) or reply (`reply_to`). A reply joins the parent's thread and inherits its task unless `task` names another; it answers a question to humans, which clears the task's `question` attention.
    pub async fn feedback(
        &self,
        actor: &Actor,
        input: Feedback,
    ) -> Result<MessageView, MessageError> {
        policy::can_send_feedback(actor).map_err(FoldError::from)?;
        let Some(parent) = input.reply_to else {
            let task = input.task.ok_or_else(|| {
                FoldError::new(codes::INVALID_REQUEST, "feedback needs a task or reply_to")
            })?;
            return self.send_feedback(actor, task, input.body, input.to).await;
        };
        let to = match input.to {
            Some(to) => to,
            None => self.reply_default(actor, parent).await?,
        };
        self.send(
            actor,
            SendMessage {
                to,
                body: input.body,
                task: input.task,
                proposal: None,
                reply_to: Some(parent),
            },
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

/// The tasks with an unanswered question to a human, the source of the `question` attention kind. A human reply in the thread removes the task.
pub async fn tasks_awaiting_human(store: &Store) -> Result<BTreeSet<TaskId>, StoreError> {
    messages::tasks_awaiting_human(store.read()).await
}

fn parse_query(query: &ThreadQuery) -> Result<(ThreadFilter, u32), FoldError> {
    let (agent, worker) = match query.agent.as_deref() {
        None => (None, None),
        Some(name) if name.contains('/') => (None, Some(WorkerName::parse(name)?)),
        Some(name) => (Some(AgentName::parse(name)?), None),
    };
    let limit = match query.limit {
        Some(0) => {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                "limit must be at least 1",
            ));
        }
        Some(limit) => limit.min(MAX_THREAD_LIMIT),
        None => DEFAULT_THREAD_LIMIT,
    };
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
        participants,
        awaiting_human: awaiting.contains(&id),
        last_at: messages
            .last()
            .map_or(Timestamp::EPOCH, |message| message.sent_at),
        messages,
    }
}
