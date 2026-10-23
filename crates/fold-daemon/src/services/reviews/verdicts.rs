use fold_core::events::types;
use fold_core::policy::{AgentStanding, ReviewTask};
use fold_core::reviews::{Reviewer, Verdict};
use fold_core::{
    Actor, CheckRunId, CommentId, FoldError, IdKind, NewEvent, ProposalId, ReviewBatchId, TaskId,
    codes, policy,
};
use fold_store::agents;
use fold_store::reviews::{self, NewComment, ProposalRow, ReviewerRef, batches};
use fold_store::{SqliteConnection, StoreError, WriteTx, counters, workspaces};
use serde_json::json;

use super::checks::running_update;
use super::comments::{CommentInput, MAX_BODY_CHARS, check_ranges, check_shape};
use super::integrate::checks_failed_refusal;
use super::proposals::{ProposalSummary, Retirement, require_open, retire_proposal};
use super::scopes::{SuggestedScope, scope_record_in_tx};
use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;
use crate::services::tasks::{recompute, task_in_main_area, worker_row};

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
    /// Reviews `version` of an open proposal with a verdict, a body and line comments, in one write transaction. `version` is the version the reviewer read; when another version is current, before the comments are checked or inside the transaction, the review is `stale_version` and writes nothing, so a verdict never lands on a version its reviewer did not read. The body may be empty only when the review carries a comment, and a reject needs a body, which becomes the close reason. Each comment's file and lines are checked against the version's base tree (side `old`) or its own tree (side `new`) before the transaction. `request_changes` puts the proposal with its author, so the task moves to In Progress; a later approval of the same version gives it back to the reviewer; `reject` closes the proposal and the task returns to In Progress.
    pub async fn submit_review(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        version: u32,
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
        let standing = agents::standing_of(read, actor).await?;
        let mut conn = read.acquire().await.map_err(StoreError::from)?;
        let task = review_task(&mut conn, &row).await?;
        let update = running_update(&mut conn, row.task, proposal).await?;
        drop(conn);
        verdict_gate(actor, standing, verdict, &row, task, update)?;
        let target = reviews::review_target(read, proposal)
            .await?
            .ok_or(StoreError::Corrupt("proposal_versions.version"))?;
        require_version(proposal, version, target.version)?;
        check_ranges(&self.workspaces, &target, &comments).await?;
        let scope = match actor {
            Actor::Agent(_) => self.suggested_scope(proposal, target.version).await?,
            Actor::Human { .. } | Actor::System => None,
        };
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
                require_version(proposal, version, current.version)?;
                let record = ReviewRecord {
                    row: &row,
                    version_id: current.version_id,
                    version: current.version,
                    verdict,
                    body: &body,
                    comments: &comments,
                    batch: None,
                    batch_comments: &[],
                    scope: scope.as_ref(),
                };
                let ids = record_review(tx, &mut buffer, actor, &record, now).await?;
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

/// One review of one proposal version to write inside a command's transaction. `batch` names the review batch it comes with, and `batch_comments` the comments of that batch to put on this member.
pub(super) struct ReviewRecord<'a> {
    pub row: &'a ProposalRow,
    pub version_id: i64,
    pub version: u32,
    pub verdict: Verdict,
    pub body: &'a str,
    pub comments: &'a [CommentInput],
    pub batch: Option<ReviewBatchId>,
    pub batch_comments: &'a [BatchCommentLink],
    /// fold's suggestion for an agent's review of a revised version, read before the command's transaction; `None` for a version reviewed whole. A human's review ignores it.
    pub scope: Option<&'a SuggestedScope>,
}

/// A comment of a review batch about members together: its row id and its number in the batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BatchCommentLink {
    pub row_id: i64,
    pub number: u32,
}

/// Writes the review, its scope, its comments and their events inside the caller's write, and closes the proposal on a reject. The scope of an agent's review of a revised version follows the review lead's choice for that version when it made one, else fold's suggestion; a human's review has no scope, since the review lead assigns only agents' reviews. The caller checks the body and the comments first, and runs `recompute` for the task before it commits.
pub(super) async fn record_review(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    record: &ReviewRecord<'_>,
    now: fold_core::Timestamp,
) -> Result<Vec<CommentId>, ReviewError> {
    let (row, verdict, version) = (record.row, record.verdict, record.version);
    let proposal = row.id;
    let reviewer = actor.event_actor();
    let standing = agents::standing_of(tx.conn(), actor).await?;
    let task = review_task(tx.conn(), row).await?;
    let update = running_update(tx.conn(), row.task, proposal).await?;
    let (by, suggested) = match verdict_gate(actor, standing, verdict, row, task, update)? {
        Reviewer::Agent(agent) => (
            ReviewerRef::Worker(worker_row(tx, agent).await?.id),
            record.scope,
        ),
        Reviewer::Human(name) => {
            if verdict == Verdict::Approve {
                human_approval_gate(tx, proposal, version, task.main).await?;
            }
            (ReviewerRef::Human(name), None)
        }
    };
    let review_id = match record.batch {
        None => {
            reviews::insert_review(tx, record.version_id, by, verdict, record.body, now).await?
        }
        Some(batch) => {
            batches::insert_batch_review(
                tx,
                batch,
                record.version_id,
                by,
                verdict,
                record.body,
                now,
            )
            .await?
        }
    };
    let scope = scope_record_in_tx(tx, proposal, version, suggested).await?;
    if let Some(scope) = &scope {
        reviews::insert_review_scope(tx, review_id, scope).await?;
    }
    let mut ids = insert_comments(tx, proposal, review_id, by, record.comments, now).await?;
    let mut linked = Vec::with_capacity(record.batch_comments.len());
    for link in record.batch_comments {
        linked.push((
            insert_batch_comment_row(tx, review_id, by, *link, now).await?,
            link.number,
        ));
    }
    let mut data = json!({
        "verdict": verdict.as_str(),
        "version": version,
        "comments": ids.len() + linked.len(),
    });
    if let Some(batch) = record.batch {
        data["batch"] = json!(batch.to_string());
    }
    if let Some(scope) = &scope {
        data["scope"] = json!(scope.scope.as_str());
    }
    let submitted = NewEvent::new(types::REVIEW_SUBMITTED, reviewer.clone())?
        .task(row.task)
        .proposal(proposal)
        .data(data)?;
    buffer.append(tx, &submitted, now).await?;
    for id in &ids {
        let added = NewEvent::new(types::REVIEW_COMMENT_ADDED, reviewer.clone())?
            .task(row.task)
            .proposal(proposal)
            .data(json!({"comment": id.to_string(), "version": version}))?;
        buffer.append(tx, &added, now).await?;
    }
    for (id, number) in &linked {
        let added = NewEvent::new(types::REVIEW_COMMENT_ADDED, reviewer.clone())?
            .task(row.task)
            .proposal(proposal)
            .data(json!({
                "comment": id.to_string(),
                "version": version,
                "batch": record.batch.map(|batch| batch.to_string()),
                "batch_comment": number,
            }))?;
        buffer.append(tx, &added, now).await?;
    }
    ids.extend(linked.into_iter().map(|(id, _)| id));
    if verdict == Verdict::Reject {
        let reason = record.body;
        let outcome = Retirement::Rejected { reason };
        retire_proposal(tx, buffer, &reviewer, proposal, outcome, now).await?;
        let closed = NewEvent::new(types::PROPOSAL_CLOSED, reviewer)?
            .task(row.task)
            .proposal(proposal)
            .data(json!({"reason": reason, "verdict": verdict.as_str()}))?;
        buffer.append(tx, &closed, now).await?;
    }
    Ok(ids)
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
            .with_hint("call get_review_comments to see the comments of the proposal")
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
            batch_comment: None,
        };
        reviews::insert_comment(tx, &new, now).await?;
        ids.push(id);
    }
    Ok(ids)
}

/// Puts the batch comment `link` on the version of the review `review_id` as a whole-version comment whose body stays with the batch comment.
async fn insert_batch_comment_row(
    tx: &mut WriteTx,
    review_id: i64,
    author: ReviewerRef<'_>,
    link: BatchCommentLink,
    now: fold_core::Timestamp,
) -> Result<CommentId, ReviewError> {
    let number = counters::allocate(tx, IdKind::Comment).await?;
    let id = CommentId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
    let new = NewComment {
        id,
        review_id,
        author,
        location: None,
        reply_to: None,
        body: "",
        batch_comment: Some(link.row_id),
    };
    reviews::insert_comment(tx, &new, now).await?;
    Ok(id)
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

/// A human approves a marked task only after a reviewer agent approved `version`, except on a task of Main (`main_task`), which no review agent reviews (`policy::can_human_approve`). The mark and the reviews are read in the review's transaction, so an approval and a mark that land meanwhile count.
async fn human_approval_gate(
    tx: &mut WriteTx,
    proposal: ProposalId,
    version: u32,
    main_task: bool,
) -> Result<(), ReviewError> {
    let row = reviews::get_proposal(tx.conn(), proposal)
        .await?
        .ok_or(StoreError::Corrupt("proposals.number"))?;
    let facts = reviews::review_facts(tx.conn(), proposal, version).await?;
    policy::can_human_approve(
        row.human_review.as_ref(),
        &facts,
        &row.author.agent(),
        main_task,
    )
    .map_err(|denied| {
        FoldError::from(denied)
            .with_details([
                ("proposal", json!(proposal.to_string())),
                ("version", json!(version)),
            ])
            .into()
    })
}

/// Who decides the proposal of `row`: whether its task is in Main, read on `conn`, and whether the task has the human review mark.
pub(super) async fn review_task(
    conn: &mut SqliteConnection,
    row: &ProposalRow,
) -> Result<ReviewTask, StoreError> {
    Ok(ReviewTask {
        main: task_in_main_area(conn, row.task).await?,
        marked: row.human_review.is_some(),
    })
}

/// `standing` is the acting agent's, `task` the facts of [`review_task`] and `update` the run of [`running_update`], read by the caller. An approval of a version whose integration checks failed is refused, as its integration is, until the author sends a new version; an approval while an update of the proposal runs its checks waits for that run to end.
pub(super) fn verdict_gate<'a>(
    actor: &'a Actor,
    standing: AgentStanding,
    verdict: Verdict,
    row: &ProposalRow,
    task: ReviewTask,
    update: Option<CheckRunId>,
) -> Result<Reviewer<'a>, ReviewError> {
    if verdict == Verdict::Approve && row.checks_failed {
        return Err(checks_failed_refusal(row.id, row.current_version).into());
    }
    let reviewer = policy::can_submit_verdict(actor, standing, verdict, &row.author, task)
        .map_err(|denied| {
            FoldError::from(denied).with_details([
                ("proposal", json!(row.id.to_string())),
                ("author", json!(row.author.as_str())),
                ("verdict", json!(verdict.as_str())),
            ])
        })?;
    policy::can_review_while_updating(verdict, update.is_some())
        .map_err(|denied| update_refusal(denied, row.id, row.current_version, update))?;
    Ok(reviewer)
}

/// The refusal of an approval or an integration of `version`, the current version of `proposal`, while the update run `update` goes, with the run to wait for.
pub(super) fn update_refusal(
    denied: policy::PolicyDenied,
    proposal: ProposalId,
    version: u32,
    update: Option<CheckRunId>,
) -> FoldError {
    FoldError::from(denied).with_details([
        ("proposal", json!(proposal.to_string())),
        ("version", json!(version)),
        ("run", json!(update.map(|run| run.to_string()))),
    ])
}

/// `stale_version` unless `expected` is the current version. A version added after the reviewer read `expected` has other trees, so neither its verdict nor its line comments can be filed on it.
pub(super) fn require_version(
    proposal: ProposalId,
    expected: u32,
    current: u32,
) -> Result<(), ReviewError> {
    if expected == current {
        return Ok(());
    }
    Err(FoldError::new(
        codes::STALE_VERSION,
        format!("{proposal} is at v{current}, not v{expected}"),
    )
    .with_details([
        ("proposal", json!(proposal.to_string())),
        ("expected_version", json!(expected)),
        ("current_version", json!(current)),
    ])
    .with_hint("call get_proposal, then create_review_checkout and review the current version")
    .into())
}
