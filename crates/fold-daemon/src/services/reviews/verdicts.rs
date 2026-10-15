use fold_core::events::types;
use fold_core::reviews::{Reviewer, Verdict};
use fold_core::{Actor, CommentId, FoldError, IdKind, NewEvent, ProposalId, TaskId, codes, policy};
use fold_store::reviews::{self, NewComment, ProposalRow, ReviewerRef};
use fold_store::{StoreError, WriteTx, counters, workspaces};
use serde_json::json;

use super::comments::{CommentInput, MAX_BODY_CHARS, check_ranges, check_shape};
use super::proposals::{ProposalSummary, Retirement, require_open, retire_proposal};
use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;
use crate::services::tasks::{recompute, worker_row};

/// What a review command wrote: the version it reviewed, the new comments in order, and the proposal as it stands after the review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedReview {
    pub proposal: ProposalId,
    pub task: TaskId,
    pub version: u32,
    pub verdict: Verdict,
    pub comments: Vec<CommentId>,
    pub summary: ProposalSummary,
}

impl ReviewService {
    /// Reviews the current version of an open proposal with a verdict, a body and line comments, in one write transaction. The body may be empty only when the review carries a comment, and a reject needs a body, which becomes the close reason. Each comment's file and lines are checked against the version's base tree (side `old`) or its own tree (side `new`) before the transaction. `request_changes` puts the proposal with its author, so the task moves to In Progress; a later approval of the same version gives it back to the reviewer; `reject` closes the proposal and the task returns to In Progress.
    pub async fn submit_review(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        verdict: Verdict,
        body: &str,
        comments: Vec<CommentInput>,
    ) -> Result<SubmittedReview, ReviewError> {
        let body = check_body(verdict, body, comments.len())?;
        for (index, comment) in comments.iter().enumerate() {
            check_shape(index, comment)?;
        }
        let read = self.store.read();
        let row = require_open(reviews::get_proposal(read, proposal).await?, proposal)?;
        verdict_gate(actor, verdict, &row)?;
        let target = reviews::review_target(read, proposal)
            .await?
            .ok_or(StoreError::Corrupt("proposal_versions.version"))?;
        check_ranges(&self.workspaces, &target, &comments).await?;
        #[cfg(feature = "failpoints")]
        fail::fail_point!("review_after_range_check");
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let (task, ids) = self
            .store
            .write(async |tx| {
                let row =
                    require_open(reviews::get_proposal(tx.conn(), proposal).await?, proposal)?;
                let current = reviews::review_target(tx.conn(), proposal)
                    .await?
                    .ok_or(StoreError::Corrupt("proposal_versions.version"))?;
                if current.version != target.version {
                    return Err(version_changed(proposal, target.version, current.version));
                }
                let reviewer = actor.event_actor();
                let by = match verdict_gate(actor, verdict, &row)? {
                    Reviewer::Agent(agent) => ReviewerRef::Worker(worker_row(tx, agent).await?.id),
                    Reviewer::Human(name) => ReviewerRef::Human(name),
                };
                let review_id =
                    reviews::insert_review(tx, current.version_id, by, verdict, &body, now).await?;
                let ids = insert_comments(tx, proposal, review_id, by, &comments, now).await?;
                let submitted = NewEvent::new(types::REVIEW_SUBMITTED, reviewer.clone())?
                    .task(row.task)
                    .proposal(proposal)
                    .data(json!({
                        "verdict": verdict.as_str(),
                        "version": current.version,
                        "comments": ids.len(),
                    }))?;
                buffer.append(tx, &submitted, now).await?;
                for id in &ids {
                    let added = NewEvent::new(types::REVIEW_COMMENT_ADDED, reviewer.clone())?
                        .task(row.task)
                        .proposal(proposal)
                        .data(json!({"comment": id.to_string(), "version": current.version}))?;
                    buffer.append(tx, &added, now).await?;
                }
                if verdict == Verdict::Reject {
                    let reason = body.as_str();
                    let outcome = Retirement::Rejected { reason };
                    retire_proposal(tx, &mut buffer, &reviewer, proposal, outcome, now).await?;
                    let closed = NewEvent::new(types::PROPOSAL_CLOSED, reviewer)?
                        .task(row.task)
                        .proposal(proposal)
                        .data(json!({"reason": reason, "verdict": verdict.as_str()}))?;
                    buffer.append(tx, &closed, now).await?;
                }
                buffer.extend(recompute(tx, &[row.task], now).await?.events);
                Ok::<_, ReviewError>((row.task, ids))
            })
            .await?;
        self.bus.publish(buffer);
        if verdict == Verdict::Reject
            && let Some(live) = workspaces::live_task_workspace(self.store.read(), task).await?
        {
            self.workspaces.proposal_changed(live.id, None).await;
        }
        let summary = self.get_proposal(proposal, None).await?.summary;
        Ok(SubmittedReview {
            proposal,
            task,
            version: target.version,
            verdict,
            comments: ids,
            summary,
        })
    }
}

/// Inserts the comments of the review `review_id` in order, each with the next comment number. A reply must name a comment of the same proposal.
async fn insert_comments(
    tx: &mut WriteTx,
    proposal: ProposalId,
    review_id: i64,
    author: ReviewerRef<'_>,
    comments: &[CommentInput],
    now: fold_core::Timestamp,
) -> Result<Vec<CommentId>, ReviewError> {
    let mut ids = Vec::with_capacity(comments.len());
    for comment in comments {
        if let Some(reply) = comment.reply_to
            && reviews::comment_proposal(tx.conn(), reply).await? != Some(proposal)
        {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("{reply} is not a comment on {proposal}"),
            )
            .with_details([
                ("field", json!("reply_to")),
                ("comment", json!(reply.to_string())),
                ("proposal", json!(proposal.to_string())),
            ])
            .with_hint("call list_comments to see the comments of the proposal")
            .into());
        }
        let number = counters::allocate(tx, IdKind::Comment).await?;
        let id = CommentId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
        let new = NewComment {
            id,
            review_id,
            author,
            location: comment
                .location
                .as_ref()
                .map(|location| location.as_range()),
            reply_to: comment.reply_to,
            body: comment.body.trim(),
        };
        reviews::insert_comment(tx, &new, now).await?;
        ids.push(id);
    }
    Ok(ids)
}

/// The trimmed body. A review says something: a body, a comment, or both; a reject's body is the reason the proposal closes.
fn check_body(verdict: Verdict, body: &str, comments: usize) -> Result<String, ReviewError> {
    let body = body.trim();
    if body.chars().count() > MAX_BODY_CHARS {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("the body must be at most {MAX_BODY_CHARS} characters"),
        )
        .with_details([("field", json!("body"))])
        .with_hint("shorten the body, or move detail into line comments")
        .into());
    }
    if verdict == Verdict::Reject && body.is_empty() {
        return Err(FoldError::new(
            codes::EMPTY_REVIEW,
            "a reject needs a body: the reason the proposal closes",
        )
        .with_details([("verdict", json!(verdict.as_str()))])
        .with_hint("call submit_review again with the reason as the body")
        .into());
    }
    if body.is_empty() && comments == 0 {
        return Err(FoldError::new(
            codes::EMPTY_REVIEW,
            "a review needs a body or at least one comment",
        )
        .with_details([("verdict", json!(verdict.as_str()))])
        .with_hint("call submit_review again with a body or a comment")
        .into());
    }
    Ok(body.to_owned())
}

fn verdict_gate<'a>(
    actor: &'a Actor,
    verdict: Verdict,
    row: &ProposalRow,
) -> Result<Reviewer<'a>, ReviewError> {
    policy::can_submit_verdict(actor, verdict, &row.author).map_err(|denied| {
        FoldError::from(denied)
            .with_details([
                ("proposal", json!(row.id.to_string())),
                ("author", json!(row.author.as_str())),
                ("verdict", json!(verdict.as_str())),
            ])
            .into()
    })
}

/// The comments were checked against `checked`; a version added meanwhile has other trees, so the review is refused rather than filed against a version it did not read.
fn version_changed(proposal: ProposalId, checked: u32, current: u32) -> ReviewError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!("{proposal} gained v{current} while the review of v{checked} was checked"),
    )
    .with_details([
        ("reason", json!("version_changed")),
        ("proposal", json!(proposal.to_string())),
        ("version", json!(checked)),
        ("current_version", json!(current)),
    ])
    .with_hint("call get_proposal and review the current version")
    .into()
}
