use std::collections::{BTreeMap, BTreeSet};

use fold_core::TaskId;
use fold_store::SqliteConnection;
use fold_store::tasks::claims;
use fold_store::{Store, StoreError, messages};

use crate::live::LiveState;
use crate::services::reviews::awaiting_human_review_in;

/// Why a task needs a person. The daemon decides every kind; no client derives one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttentionKind {
    /// An unanswered fold message from an agent to a human, linked to the task.
    Question,
    /// A proposal of the task waits for a human approval.
    HumanReview,
    /// The task's active workspace is stalled in the live state.
    Stalled,
    /// The lease sweeper marked the task's implementation claim, or the review claim of its open proposal.
    StaleClaim,
}

impl AttentionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AttentionKind::Question => "question",
            AttentionKind::HumanReview => "human_review",
            AttentionKind::Stalled => "stalled",
            AttentionKind::StaleClaim => "stale_claim",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttentionItem {
    pub kind: AttentionKind,
    /// For a `Question`: one of the task's open asks is closable, so a human can mark it answered, because it is no worker's `AskUserQuestion`, which only a reply in its thread ends. False for every other kind.
    pub closable: bool,
}

impl AttentionItem {
    /// An item of `kind` that marks nothing closable.
    pub fn of(kind: AttentionKind) -> Self {
        Self {
            kind,
            closable: false,
        }
    }
}

/// The attention items of each task in `task_ids`, read in one read transaction of `read`; `stalled` holds the tasks whose workspace is stalled, from [`stalled_tasks`]. No item depends on the time of the read: the lease sweeper marks a stale claim and the stall job sets the stalled flag. Every id gets an entry; an empty list means nothing needs a person.
pub async fn attention(
    read: &Store,
    task_ids: &[TaskId],
    stalled: &BTreeSet<TaskId>,
) -> Result<BTreeMap<TaskId, Vec<AttentionItem>>, StoreError> {
    let mut tx = read.read().begin().await?;
    let items = attention_in(&mut tx, task_ids, stalled).await?;
    tx.commit().await?;
    Ok(items)
}

/// [`attention`] on `conn`, so a caller reads it in the same snapshot as the rest of its view.
pub async fn attention_in(
    conn: &mut SqliteConnection,
    task_ids: &[TaskId],
    stalled: &BTreeSet<TaskId>,
) -> Result<BTreeMap<TaskId, Vec<AttentionItem>>, StoreError> {
    let questions = messages::tasks_awaiting_human(&mut *conn).await?;
    let human_review: BTreeSet<TaskId> = awaiting_human_review_in(&mut *conn)
        .await?
        .into_iter()
        .map(|waiting| waiting.task)
        .collect();
    let stale_claims = claims::tasks_with_stale_claims(&mut *conn).await?;
    Ok(task_ids
        .iter()
        .map(|id| {
            let mut items = Vec::new();
            if let Some(&closable) = questions.get(id) {
                items.push(AttentionItem {
                    kind: AttentionKind::Question,
                    closable,
                });
            }
            if human_review.contains(id) {
                items.push(AttentionItem::of(AttentionKind::HumanReview));
            }
            if stalled.contains(id) {
                items.push(AttentionItem::of(AttentionKind::Stalled));
            }
            if stale_claims.contains(id) {
                items.push(AttentionItem::of(AttentionKind::StaleClaim));
            }
            (*id, items)
        })
        .collect())
}

/// The tasks whose workspace has the stalled flag in `live`.
pub fn stalled_tasks(live: Option<&LiveState>) -> BTreeSet<TaskId> {
    live.map(LiveState::all)
        .unwrap_or_default()
        .iter()
        .filter(|state| state.view.stalled)
        .filter_map(|state| state.view.task)
        .collect()
}
