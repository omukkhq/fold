use fold_core::events::types;
use fold_core::policy::{self, MarkChange, PolicyDenied};
use fold_core::reviews::{HumanReviewMark, changes_requested, human_approval_standing};
use fold_core::status::ProposalFact;
use fold_core::{
    Actor, FoldError, HumanName, NewEvent, ProposalId, TaskId, TaskStatus, Timestamp, codes,
};
use fold_store::tasks::TaskRow;
use fold_store::{Store, StoreError, projects, reviews, tasks};
use serde_json::json;

use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;
use crate::services::tasks::task_not_found;

/// The longest reason, in characters.
const MAX_REASON_CHARS: usize = 500;

/// An open proposal that is with the reviewer on a marked task whose current version has no standing human approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwaitingHumanReview {
    pub task: TaskId,
    pub proposal: ProposalId,
    /// The human the mark names; `None` when any human may approve.
    pub reviewer: Option<HumanName>,
    /// When the proposal started waiting: the later of the current version's creation and the last change of the mark.
    pub since: Timestamp,
}

impl ReviewService {
    /// Turns on the human review mark of an open task, or changes it when a human asks. A reviewer must be a human of the host registry. An agent repeating the mark changes nothing and appends no event. The mark never changes the task's status.
    pub async fn require_human_review(
        &self,
        actor: &Actor,
        task: TaskId,
        reviewer: Option<HumanName>,
        reason: &str,
    ) -> Result<HumanReviewMark, ReviewError> {
        let reason = check_reason(reason)?;
        if let Some(name) = &reviewer {
            self.require_known(name).await?;
        }
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let mark = self
            .store
            .write(async |tx| {
                let row = open_task(tasks::fetch(tx.conn(), task).await?, task)?;
                let current = row.human_review;
                let change = policy::can_set_human_review(
                    actor,
                    current.as_ref(),
                    reviewer.as_ref(),
                    &reason,
                )
                .map_err(|denied| refusal(denied, task))?;
                if let (MarkChange::Unchanged, Some(current)) = (change, current) {
                    return Ok(current);
                }
                let mark = HumanReviewMark {
                    reviewer: reviewer.clone(),
                    reason: reason.clone(),
                    set_by: actor.event_actor(),
                };
                tasks::set_human_review(tx, task, &mark, now).await?;
                let event = NewEvent::new(types::TASK_HUMAN_REVIEW_SET, actor.event_actor())?
                    .task(task)
                    .data(json!({
                        "reviewer": mark.reviewer.as_ref().map(HumanName::as_str),
                        "reason": mark.reason,
                        "set_by": mark.set_by.as_str(),
                    }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, ReviewError>(mark)
            })
            .await?;
        self.bus.publish(buffer);
        Ok(mark)
    }

    /// Turns the mark off. Only a human clears it; the event keeps the reviewer and reason it had. Clearing a task without a mark changes nothing and returns `None`.
    pub async fn clear_human_review(
        &self,
        actor: &Actor,
        task: TaskId,
    ) -> Result<Option<HumanReviewMark>, ReviewError> {
        policy::can_clear_human_review(actor).map_err(|denied| refusal(denied, task))?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let previous = self
            .store
            .write(async |tx| {
                let row = open_task(tasks::fetch(tx.conn(), task).await?, task)?;
                let Some(previous) = row.human_review else {
                    return Ok(None);
                };
                tasks::clear_human_review(tx, task, now).await?;
                let event = NewEvent::new(types::TASK_HUMAN_REVIEW_CLEARED, actor.event_actor())?
                    .task(task)
                    .data(json!({
                        "reviewer": previous.reviewer.as_ref().map(HumanName::as_str),
                        "reason": previous.reason,
                    }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, ReviewError>(Some(previous))
            })
            .await?;
        self.bus.publish(buffer);
        Ok(previous)
    }

    /// The proposals that wait for a human, in proposal number order.
    pub async fn awaiting_human_review(&self) -> Result<Vec<AwaitingHumanReview>, ReviewError> {
        Ok(awaiting_human_review(&self.store).await?)
    }

    async fn require_known(&self, name: &HumanName) -> Result<(), ReviewError> {
        let known = match &self.registry {
            Some(registry) => projects::find_human(registry.read(), name).await?.is_some(),
            None => false,
        };
        if known {
            return Ok(());
        }
        Err(
            FoldError::new(codes::UNKNOWN_HUMAN, format!("{name} is not a known human"))
                .with_details([("reviewer", json!(name.as_str()))])
                .with_hint(
                    "a human is known once they have used the CLI or the web interface; leave the reviewer out to let any human approve",
                )
                .into(),
        )
    }
}

/// The open proposals on marked tasks that are with the reviewer and whose current version has no standing human approval, read in one transaction of `read`'s read pool. It decides the `human_review` attention kind, so clients derive nothing.
pub async fn awaiting_human_review(read: &Store) -> Result<Vec<AwaitingHumanReview>, StoreError> {
    let mut tx = read.read().begin().await?;
    let marked = tasks::marked_open_proposals(&mut *tx).await?;
    let mut waiting = Vec::new();
    for row in marked {
        let facts = reviews::review_facts(&mut *tx, row.proposal, row.version).await?;
        let with_reviewer = ProposalFact::open(changes_requested(&facts), row.conflicted)
            == ProposalFact::WithReviewer;
        if !with_reviewer || human_approval_standing(Some(&row.mark), &facts, None) {
            continue;
        }
        waiting.push(AwaitingHumanReview {
            task: row.task,
            proposal: row.proposal,
            reviewer: row.mark.reviewer,
            since: row
                .marked_at
                .map_or(row.version_at, |marked| marked.max(row.version_at)),
        });
    }
    tx.commit().await?;
    Ok(waiting)
}

/// The trimmed reason, which says why a human must look.
fn check_reason(reason: &str) -> Result<String, ReviewError> {
    let reason = reason.trim();
    let problem = if reason.is_empty() {
        "the reason must not be empty"
    } else if reason.chars().count() > MAX_REASON_CHARS {
        "the reason must be at most 500 characters"
    } else {
        return Ok(reason.to_owned());
    };
    Err(FoldError::new(codes::INVALID_REQUEST, problem)
        .with_details([("field", json!("reason"))])
        .with_hint("say in one line why a human must look at the work")
        .into())
}

/// The task row of an open task: `task_not_found` when it does not exist and `task_closed` when it is Done or archived.
fn open_task(row: Option<TaskRow>, task: TaskId) -> Result<TaskRow, ReviewError> {
    let row = row.ok_or_else(|| task_not_found(task))?;
    let reason = if row.archived_at.is_some() {
        "archived"
    } else if row.status == TaskStatus::Done {
        "done"
    } else {
        return Ok(row);
    };
    Err(
        FoldError::new(codes::TASK_CLOSED, format!("{task} is {reason}"))
            .with_details([("reason", json!(reason))])
            .with_hint("a Done or archived task takes no human review mark")
            .into(),
    )
}

fn refusal(denied: PolicyDenied, task: TaskId) -> ReviewError {
    FoldError::from(denied)
        .with_details([("task", json!(task.to_string()))])
        .into()
}
