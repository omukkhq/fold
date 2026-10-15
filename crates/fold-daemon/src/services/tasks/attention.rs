use std::collections::{BTreeMap, BTreeSet};

use fold_core::{TaskId, Timestamp};
use fold_store::tasks::claims;
use fold_store::{Store, StoreError, messages};

use crate::live::LiveState;
use crate::services::reviews::awaiting_human_review;

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
}

/// The attention items of each task in `task_ids`, read from the read pool of `read` and decided at `now`; `stalled` holds the tasks whose workspace is stalled, from [`stalled_tasks`]. Every id gets an entry; an empty list means nothing needs a person.
pub async fn attention(
    read: &Store,
    task_ids: &[TaskId],
    stalled: &BTreeSet<TaskId>,
    _now: Timestamp,
) -> Result<BTreeMap<TaskId, Vec<AttentionItem>>, StoreError> {
    let questions = messages::tasks_awaiting_human(read.read()).await?;
    let human_review: BTreeSet<TaskId> = awaiting_human_review(read)
        .await?
        .into_iter()
        .map(|waiting| waiting.task)
        .collect();
    let stale_claims = claims::tasks_with_stale_claims(read.read()).await?;
    Ok(task_ids
        .iter()
        .map(|id| {
            let mut items = Vec::new();
            if questions.contains(id) {
                items.push(AttentionItem {
                    kind: AttentionKind::Question,
                });
            }
            if human_review.contains(id) {
                items.push(AttentionItem {
                    kind: AttentionKind::HumanReview,
                });
            }
            if stalled.contains(id) {
                items.push(AttentionItem {
                    kind: AttentionKind::Stalled,
                });
            }
            if stale_claims.contains(id) {
                items.push(AttentionItem {
                    kind: AttentionKind::StaleClaim,
                });
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
        .map(|state| state.view.task)
        .collect()
}
