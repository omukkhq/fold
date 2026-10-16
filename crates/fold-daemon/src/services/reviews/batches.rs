use std::collections::{BTreeMap, BTreeSet};

use fold_core::dag::Dag;
use fold_core::events::types;
use fold_core::reviews::{Side, Verdict, last_verdict_version};
use fold_core::{
    Actor, AgentActor, CommentId, EventActor, FoldError, IdKind, NewEvent, ProposalId,
    ReviewBatchId, TaskId, Timestamp, WorkerName, WorkspaceId, codes, policy,
};
use fold_store::reviews::batches::{
    self, BatchRow, BatchState, MemberRelease, MemberRow, NewBatchCheckout, NewLineOrigin,
};
use fold_store::reviews::{self, ProposalRow, ProposalState, ReviewTarget};
use fold_store::{Store, StoreError, WriteTx, agents, counters, tasks, workspaces};
use fold_vcs::batch::{BatchMember, BatchMerge, FileOrigins};
use fold_vcs::browse;
use fold_vcs::compare::Totals;
use serde_json::{Value, json};

use super::changes::ChangeRange;
use super::checkouts::remove_unnamed_dir;
use super::claims::take_review_claim;
use super::comments::{CommentInput, CommentLocation, MAX_BODY_CHARS, check_ranges, check_shape};
use super::proposals::require_open;
use super::verdicts::{BatchCommentLink, ReviewRecord, record_review, verdict_gate};
use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;
use crate::services::tasks::{recompute, worker_row};
use crate::services::workspaces::{
    ArchiveReason, WorkspaceError, WorkspaceRef, WorkspaceView, archive_in_tx,
};

/// The earlier version whose change a member shows: only an open proposal that has a version after the newest one with a verdict, or after the one it joined at, has one.
async fn since_version(
    store: &Store,
    member: &MemberRow,
    proposal: &ProposalRow,
) -> Result<Option<u32>, ReviewError> {
    let current = proposal.current_version;
    if proposal.state != ProposalState::Open {
        return Ok(None);
    }
    let rows = reviews::list_reviews(store.read(), member.proposal).await?;
    let reviewed = last_verdict_version(
        rows.iter().map(|review| (review.version, review.verdict)),
        current,
    );
    Ok(reviewed.or((member.version < current).then_some(member.version)))
}

/// The fewest proposals a batch is made of; one proposal is reviewed with `submit_review`.
const MIN_MEMBERS: usize = 2;

/// The most proposals one batch holds, so one checkout and one verdict call stay a reviewable size.
const MAX_MEMBERS: usize = 20;

/// The `release_reason` of a review claim that leaves with its member.
const REMOVED_FROM_BATCH: &str = "removed_from_batch";

/// One member of a batch as callers see it: the version it joined at and where its proposal stands now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchMemberView {
    pub proposal: ProposalId,
    pub task: TaskId,
    pub position: u32,
    pub version: u32,
    pub current_version: u32,
    pub proposal_state: ProposalState,
    /// `None` while the proposal is in the batch.
    pub released: Option<MemberRelease>,
    /// What an open proposal changed after an earlier version a reviewer read.
    pub since: Option<MemberSince>,
}

/// The change of a member's current version since an earlier version: the newest one with a verdict, by anyone, else the version the member joined at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberSince {
    pub version: u32,
    pub totals: Totals,
}

/// A verdict the batch gave, on the member version it reviewed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchVerdictView {
    pub proposal: ProposalId,
    pub version: u32,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewBatchView {
    pub id: ReviewBatchId,
    pub holder: WorkerName,
    pub state: BatchState,
    pub created_at: Timestamp,
    pub submitted_at: Option<Timestamp>,
    pub closed_at: Option<Timestamp>,
    /// Every member the batch has had, in merge order.
    pub members: Vec<BatchMemberView>,
    /// The batch's active checkout.
    pub checkout: Option<WorkspaceView>,
    pub verdicts: Vec<BatchVerdictView>,
    /// The comments about members together, each with the members it is on.
    pub comments: Vec<BatchCommentView>,
}

/// A comment of a batch review about members together. Each member in `members` has a whole-version comment that points to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchCommentView {
    pub number: u32,
    pub body: String,
    pub members: Vec<ProposalId>,
    pub created_at: Timestamp,
}

/// A batch checkout with the member versions merged into it, in merge order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchCheckout {
    pub workspace: WorkspaceView,
    pub batch: ReviewBatchId,
    pub members: Vec<(ProposalId, u32)>,
}

/// One member's verdict in a batch review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchVerdict {
    pub proposal: ProposalId,
    pub verdict: Verdict,
    pub body: String,
}

/// A run of lines of a file in the batch checkout, 1-based and inclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchLines {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// A comment of a batch review: on lines of the batch checkout, which fold stores on the member that brought them, or about the batch, stored on each member it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchComment {
    pub location: Option<BatchLines>,
    pub members: Vec<ProposalId>,
    pub body: String,
}

/// Why a member's verdict was left out of a batch review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftOutReason {
    /// The member got a new version after it joined; the new version needs its own review.
    NewVersion,
    /// The proposal was integrated or closed meanwhile.
    NotOpen,
    /// The member's review claim is held by someone other than the batch's holder, or by nobody.
    ClaimLost,
}

impl LeftOutReason {
    pub fn as_str(self) -> &'static str {
        match self {
            LeftOutReason::NewVersion => "new_version",
            LeftOutReason::NotOpen => "not_open",
            LeftOutReason::ClaimLost => "claim_lost",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchReviewed {
    pub proposal: ProposalId,
    pub version: u32,
    pub verdict: Verdict,
    pub comments: Vec<CommentId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftOut {
    pub proposal: ProposalId,
    pub version: u32,
    pub reason: LeftOutReason,
}

/// What a batch review wrote: the members it reviewed and those whose verdict it left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmittedBatch {
    pub batch: ReviewBatchId,
    pub reviewed: Vec<BatchReviewed>,
    pub left_out: Vec<LeftOut>,
}

/// What the transaction of a checkout create found: its own new row, or the checkout another create inserted first.
enum Inserted {
    Created(WorkspaceId),
    Existing(WorkspaceId),
}

/// Review batches: related open proposals one reviewer reviews together, with one checkout of all of them merged onto main and one verdict per member.
impl ReviewService {
    /// Makes a batch of two or more open proposals for the actor's worker and takes the review claim of each, all in one transaction, by the rules of `claim_review`: the author's agent is refused, a claim another worker holds refuses the whole batch, and a stale one is taken over. Members are placed in dependency order of their tasks, ties by task number. A proposal in another open batch is `already_in_batch`; the partial unique index on open members decides a race.
    pub async fn create_review_batch(
        &self,
        actor: &Actor,
        proposals: Vec<ProposalId>,
    ) -> Result<ReviewBatchView, ReviewError> {
        check_members(&proposals)?;
        let lease_minutes = self.lease_minutes().await?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let batch = self
            .store
            .write(async |tx| {
                let mut rows = Vec::with_capacity(proposals.len());
                let mut reviewer = None;
                for &proposal in &proposals {
                    let row =
                        require_open(reviews::get_proposal(tx.conn(), proposal).await?, proposal)?;
                    let agent = policy::can_claim_review(actor, &row.author).map_err(|denied| {
                        FoldError::from(denied).with_details([
                            ("proposal", json!(proposal.to_string())),
                            ("author", json!(row.author.as_str())),
                        ])
                    })?;
                    reviewer = Some(agent);
                    rows.push(row);
                }
                let reviewer = reviewer.ok_or(StoreError::Invalid("a batch has members"))?;
                let worker = worker_row(tx, reviewer).await?;
                let rows = dependency_order(tx, rows).await?;
                let number = counters::allocate(tx, IdKind::ReviewBatch).await?;
                let batch = ReviewBatchId::from_number(number)
                    .ok_or(StoreError::Corrupt("counters.next"))?;
                batches::insert_batch(tx, batch, worker.id, now).await?;
                let mut members = Vec::with_capacity(rows.len());
                for (position, row) in (1u32..).zip(&rows) {
                    take_review_claim(tx, &mut buffer, row, &worker, lease_minutes, now).await?;
                    match batches::insert_member(tx, batch, row.id, row.current_version, position)
                        .await
                    {
                        Ok(()) => {}
                        Err(error) if error.unique_violation() => {
                            let other = batches::open_batch_of(tx.conn(), row.id).await?;
                            return Err(already_in_batch(row.id, other));
                        }
                        Err(error) => return Err(error.into()),
                    }
                    members.push(json!({
                        "proposal": row.id.to_string(),
                        "version": row.current_version,
                        "position": position,
                    }));
                }
                let event = NewEvent::new(
                    types::REVIEW_BATCH_CREATED,
                    EventActor::worker(&worker.name),
                )?
                .data(json!({
                    "batch": batch.to_string(),
                    "holder": worker.name.as_str(),
                    "members": members,
                }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, ReviewError>(batch)
            })
            .await?;
        self.bus.publish(buffer);
        self.get_review_batch(batch).await
    }

    /// Takes a proposal out of an open batch, by any worker of the holder's agent, and releases the holder's review claim on it as `removed_from_batch`, so the proposal can be reviewed on its own or in another batch. The batch's checkout then holds a member the batch lacks, so it is archived; `create_batch_checkout` makes a new one. When no member with an open proposal is left, the batch closes.
    pub async fn remove_from_batch(
        &self,
        actor: &Actor,
        batch: ReviewBatchId,
        proposal: ProposalId,
    ) -> Result<ReviewBatchView, ReviewError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let row = open_batch(batches::get_batch(tx.conn(), batch).await?, batch)?;
                batch_gate(actor, &row)?;
                let members = batches::members(tx.conn(), batch).await?;
                let Some(member) = members
                    .iter()
                    .find(|member| member.proposal == proposal && member.is_open())
                else {
                    return Err(not_member(batch, proposal));
                };
                batches::remove_member(tx, batch, proposal, now).await?;
                let claim = reviews::unreleased_review_claim(tx.conn(), proposal).await?;
                if claim.is_some_and(|claim| claim.worker_id == row.holder_worker_id) {
                    reviews::release_review_claim(tx, proposal, REMOVED_FROM_BATCH, now).await?;
                    let event = NewEvent::new(types::CLAIM_RELEASED, actor.event_actor())?
                        .task(member.task)
                        .proposal(proposal)
                        .data(json!({
                            "kind": "review",
                            "proposal": proposal.to_string(),
                            "worker": row.holder.as_str(),
                            "reason": REMOVED_FROM_BATCH,
                            "batch": batch.to_string(),
                        }))?;
                    buffer.append(tx, &event, now).await?;
                }
                let by = actor.event_actor();
                if !close_when_done(tx, &mut buffer, &by, batch, now).await? {
                    archive_checkout(
                        tx,
                        &mut buffer,
                        &by,
                        batch,
                        ArchiveReason::BatchChanged,
                        now,
                    )
                    .await?;
                }
                Ok::<_, ReviewError>(())
            })
            .await?;
        self.bus.publish(buffer);
        self.get_review_batch(batch).await
    }

    /// A read-only checkout of the batch for the actor's worker under `.fold/ws/review-rb-<n>`: main's head with each member's batched version merged onto it in merge order, by the three-way tree merge an integration uses, so the checks run once for all members. Only the holder's agent creates it, and a repeat call returns the active checkout. fold records which member brought each changed run of lines, so a line comment on the checkout is stored on that member's version.
    ///
    /// A member that conflicts is `batch_conflict` naming it and the earlier member it conflicts with, or `main` when it conflicts with main itself, and the paths; no checkout is made. The objects are merged and the files written and renamed into place under the project's object lock before the transaction that inserts the row; a failure to write them is `checkout_failed`, and a directory no row names is removed again.
    pub async fn create_batch_checkout(
        &self,
        actor: &Actor,
        batch: ReviewBatchId,
    ) -> Result<BatchCheckout, ReviewError> {
        let read = self.store.read();
        let row = open_batch(batches::get_batch(read, batch).await?, batch)?;
        batch_gate(actor, &row)?;
        let members = open_members(batches::members(read, batch).await?);
        let merged_members = member_versions(&members);
        if let Some(existing) = batches::active_batch_checkout(read, batch).await? {
            return self.batch_checkout(existing, batch, merged_members).await;
        }
        if members.is_empty() {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("{batch} has no members left"),
            )
            .with_details([
                ("reason", json!("no_members")),
                ("batch", json!(batch.to_string())),
            ])
            .with_hint("call create_review_batch with the proposals to review")
            .into());
        }
        let head = workspaces::head(read).await?.ok_or_else(|| {
            FoldError::new(codes::REVISION_NOT_FOUND, "main has no revision")
                .with_hint("run fld init to create the first revision")
        })?;
        let inputs = members
            .iter()
            .map(|member| BatchMember {
                base: member.base_tree,
                tree: member.tree,
            })
            .collect();
        let locks = self.workspaces.lock_for_integration(None).await;
        let merged = self
            .workspaces
            .merge_review_batch(&locks, head.tree_hash, inputs)
            .await
            .map_err(|error| checkout_failed(batch, error))?;
        let (tree, origins) = match merged {
            BatchMerge::Merged { tree_hash, origins } => (tree_hash, origins),
            BatchMerge::Conflicted {
                member,
                with,
                conflicts,
            } => {
                let paths = conflicts
                    .iter()
                    .map(|conflict| String::from_utf8_lossy(&conflict.path).into_owned())
                    .collect();
                let proposal = members.get(member).map(|member| member.proposal);
                let with = with
                    .and_then(|index| members.get(index))
                    .map(|m| m.proposal);
                return Err(batch_conflict(batch, proposal, with, paths));
            }
        };
        let taken = batches::batch_checkout_paths(read, batch).await?;
        let (path, dir) = self
            .workspaces
            .place_review_batch_checkout(batch, taken, tree)
            .await
            .map_err(|error| checkout_failed(batch, error))?;
        drop(locks);
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let written = self
            .store
            .write(async |tx| {
                let row = open_batch(batches::get_batch(tx.conn(), batch).await?, batch)?;
                let agent = batch_gate(actor, &row)?;
                let current = open_members(batches::members(tx.conn(), batch).await?);
                if member_versions(&current) != merged_members {
                    return Err(batch_changed(batch));
                }
                if let Some(existing) = batches::active_batch_checkout(tx.conn(), batch).await? {
                    return Ok(Inserted::Existing(existing));
                }
                let worker = worker_row(tx, agent).await?;
                let first = current
                    .first()
                    .ok_or(StoreError::Invalid("a batch checkout has members"))?;
                let number = counters::allocate(tx, IdKind::Workspace).await?;
                let id =
                    WorkspaceId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
                let new = NewBatchCheckout {
                    id,
                    batch,
                    task: first.task,
                    worker_id: worker.id,
                    path: &path,
                    base: head.number,
                    now,
                };
                batches::insert_batch_checkout(tx, &new).await?;
                let lines = line_origins(&current, &origins);
                batches::insert_line_origins(tx, id, batch, &lines).await?;
                let event =
                    NewEvent::new(types::WORKSPACE_CREATED, EventActor::worker(&worker.name))?
                        .task(first.task)
                        .workspace(id)
                        .revision(head.number)
                        .data(json!({
                            "kind": "review",
                            "batch": batch.to_string(),
                            "path": path,
                            "worker": worker.name.as_str(),
                            "members": members_json(&current),
                        }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, ReviewError>(Inserted::Created(id))
            })
            .await;
        let id = match written {
            Ok(Inserted::Created(id)) => id,
            Ok(Inserted::Existing(existing)) => {
                remove_unnamed_dir(&path, dir).await;
                return self.batch_checkout(existing, batch, merged_members).await;
            }
            Err(error) => {
                remove_unnamed_dir(&path, dir).await;
                return Err(error);
            }
        };
        self.bus.publish(buffer);
        self.batch_checkout(id, batch, merged_members).await
    }

    /// Reviews every member of an open batch in one transaction: one verdict per member, each a review of the version the member joined at (Invariant 4), with line comments on the batch checkout stored on the member that brought the lines and batch comments stored on each member they name. A member that got a new version, was integrated or closed, or whose review claim the holder lost is left out: its verdict and comments are not written, and its new version needs its own review. Every other member needs a verdict. A comment about members together is stored once as a batch comment and put on each named member that is reviewed. The batch is then submitted, its members leave it and its checkout is archived; the review claims stay with the holder, as after `submit_review`.
    pub async fn submit_batch_review(
        &self,
        actor: &Actor,
        batch: ReviewBatchId,
        verdicts: Vec<BatchVerdict>,
        comments: Vec<BatchComment>,
    ) -> Result<SubmittedBatch, ReviewError> {
        check_verdict_shapes(&verdicts)?;
        let read = self.store.read();
        let row = open_batch(batches::get_batch(read, batch).await?, batch)?;
        batch_gate(actor, &row)?;
        let members = open_members(batches::members(read, batch).await?);
        for verdict in &verdicts {
            member_of(&members, batch, verdict.proposal)?;
        }
        let standing = agents::standing_of(read, actor).await?;
        for verdict in &verdicts {
            if let Some(proposal) = reviews::get_proposal(read, verdict.proposal).await? {
                verdict_gate(actor, standing, verdict.verdict, &proposal)?;
            }
        }
        let checkout = batches::active_batch_checkout(read, batch).await?;
        let mut by_member: BTreeMap<ProposalId, Vec<CommentInput>> = BTreeMap::new();
        let mut notes: Vec<(String, BTreeSet<ProposalId>)> = Vec::new();
        for (index, comment) in comments.into_iter().enumerate() {
            let Some(lines) = &comment.location else {
                if comment.members.is_empty() {
                    return Err(unnamed_batch_comment(index));
                }
                for &proposal in &comment.members {
                    member_of(&members, batch, proposal)?;
                }
                let input = CommentInput {
                    location: None,
                    reply_to: None,
                    body: comment.body.clone(),
                };
                check_shape(index, &input)?;
                let named = comment.members.iter().copied().collect();
                notes.push((comment.body.trim().to_owned(), named));
                continue;
            };
            let (proposal, input) = self
                .located(batch, checkout, index, lines, &comment)
                .await?;
            check_shape(index, &input)?;
            by_member.entry(proposal).or_default().push(input);
        }
        for member in &members {
            if let Some(inputs) = by_member.get(&member.proposal) {
                let target = ReviewTarget {
                    version_id: member.version_id,
                    version: member.version,
                    base_tree: member.base_tree,
                    tree: member.tree,
                };
                check_ranges(&self.workspaces, &target, inputs).await?;
            }
        }
        for verdict in &verdicts {
            let lines = by_member.get(&verdict.proposal).map_or(0, Vec::len);
            let named = notes
                .iter()
                .filter(|(_, named)| named.contains(&verdict.proposal))
                .count();
            check_verdict_body(verdict, lines + named)?;
        }
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let submitted = self
            .store
            .write(async |tx| {
                let row = open_batch(batches::get_batch(tx.conn(), batch).await?, batch)?;
                batch_gate(actor, &row)?;
                let members = open_members(batches::members(tx.conn(), batch).await?);
                for verdict in &verdicts {
                    member_of(&members, batch, verdict.proposal)?;
                }
                let mut reviewed = Vec::new();
                let mut left_out = Vec::new();
                let mut pending = Vec::new();
                for member in &members {
                    let proposal = reviews::get_proposal(tx.conn(), member.proposal)
                        .await?
                        .ok_or(StoreError::Corrupt("review_batch_members.proposal_id"))?;
                    match left_out_reason(tx, &row, member, &proposal).await? {
                        Some(reason) => left_out.push(LeftOut {
                            proposal: member.proposal,
                            version: member.version,
                            reason,
                        }),
                        None => pending.push((member, proposal)),
                    }
                }
                let missing: Vec<String> = pending
                    .iter()
                    .filter(|(member, _)| !verdicts.iter().any(|v| v.proposal == member.proposal))
                    .map(|(member, _)| member.proposal.to_string())
                    .collect();
                if !missing.is_empty() {
                    return Err(missing_verdicts(batch, missing));
                }
                // The batch ends before the verdicts are written, so a reject that closes the last member does not also close the batch.
                batches::submit_batch(tx, batch, now).await?;
                let mut links: BTreeMap<ProposalId, Vec<BatchCommentLink>> = BTreeMap::new();
                for (body, named) in &notes {
                    let on: Vec<ProposalId> = pending
                        .iter()
                        .map(|(member, _)| member.proposal)
                        .filter(|proposal| named.contains(proposal))
                        .collect();
                    if on.is_empty() {
                        continue;
                    }
                    let (row_id, number) =
                        batches::insert_batch_comment(tx, batch, body, now).await?;
                    for proposal in on {
                        links
                            .entry(proposal)
                            .or_default()
                            .push(BatchCommentLink { row_id, number });
                    }
                }
                let empty = Vec::new();
                let no_links = Vec::new();
                let mut seeds = Vec::new();
                for (member, proposal) in &pending {
                    let Some(verdict) = verdicts.iter().find(|v| v.proposal == member.proposal)
                    else {
                        continue;
                    };
                    let comments = by_member.get(&member.proposal).unwrap_or(&empty);
                    let record = ReviewRecord {
                        row: proposal,
                        version_id: member.version_id,
                        version: member.version,
                        verdict: verdict.verdict,
                        body: verdict.body.trim(),
                        comments,
                        batch: Some(batch),
                        batch_comments: links.get(&member.proposal).unwrap_or(&no_links),
                    };
                    let ids = record_review(tx, &mut buffer, actor, &record, now).await?;
                    seeds.push(member.task);
                    reviewed.push(BatchReviewed {
                        proposal: member.proposal,
                        version: member.version,
                        verdict: verdict.verdict,
                        comments: ids,
                    });
                }
                buffer.extend(recompute(tx, &seeds, now).await?.events);
                archive_checkout(
                    tx,
                    &mut buffer,
                    &actor.event_actor(),
                    batch,
                    ArchiveReason::BatchSubmitted,
                    now,
                )
                .await?;
                let event = NewEvent::new(types::REVIEW_BATCH_SUBMITTED, actor.event_actor())?
                    .data(json!({
                        "batch": batch.to_string(),
                        "reviewed": reviewed.iter().map(|review| json!({
                            "proposal": review.proposal.to_string(),
                            "version": review.version,
                            "verdict": review.verdict.as_str(),
                            "comments": review.comments.len(),
                        })).collect::<Vec<Value>>(),
                        "left_out": left_out.iter().map(|left| json!({
                            "proposal": left.proposal.to_string(),
                            "version": left.version,
                            "reason": left.reason.as_str(),
                        })).collect::<Vec<Value>>(),
                    }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, ReviewError>(SubmittedBatch {
                    batch,
                    reviewed,
                    left_out,
                })
            })
            .await?;
        self.bus.publish(buffer);
        for review in &submitted.reviewed {
            if review.verdict != Verdict::Reject {
                continue;
            }
            let task = members
                .iter()
                .find(|member| member.proposal == review.proposal)
                .map(|member| member.task);
            if let Some(task) = task
                && let Some(live) = workspaces::live_task_workspace(self.store.read(), task).await?
            {
                self.workspaces.proposal_changed(live.id, None).await;
            }
        }
        Ok(submitted)
    }

    /// One batch with its members, its active checkout and its verdicts. It reads only.
    pub async fn get_review_batch(
        &self,
        batch: ReviewBatchId,
    ) -> Result<ReviewBatchView, ReviewError> {
        let row = batches::get_batch(self.store.read(), batch)
            .await?
            .ok_or_else(|| batch_not_found(batch))?;
        self.batch_view(row).await
    }

    /// The batches in number order, only the open ones when `open_only`. It reads only.
    pub async fn list_review_batches(
        &self,
        open_only: bool,
    ) -> Result<Vec<ReviewBatchView>, ReviewError> {
        let rows = batches::list_batches(self.store.read(), open_only).await?;
        let mut views = Vec::with_capacity(rows.len());
        for row in rows {
            views.push(self.batch_view(row).await?);
        }
        Ok(views)
    }

    async fn batch_view(&self, row: BatchRow) -> Result<ReviewBatchView, ReviewError> {
        let read = self.store.read();
        let mut members = Vec::new();
        for member in batches::members(read, row.id).await? {
            let proposal = reviews::get_proposal(read, member.proposal)
                .await?
                .ok_or(StoreError::Corrupt("review_batch_members.proposal_id"))?;
            let since = match since_version(&self.store, &member, &proposal).await? {
                Some(version) => {
                    let range = ChangeRange {
                        version: Some(proposal.current_version),
                        since: Some(version),
                    };
                    let change = self.proposal_changes(member.proposal, range).await?;
                    Some(MemberSince {
                        version,
                        totals: change.change.totals,
                    })
                }
                None => None,
            };
            members.push(BatchMemberView {
                proposal: member.proposal,
                task: member.task,
                position: member.position,
                version: member.version,
                current_version: proposal.current_version,
                proposal_state: proposal.state,
                released: member.release,
                since,
            });
        }
        let checkout = match batches::active_batch_checkout(read, row.id).await? {
            Some(id) => Some(self.workspaces.get(WorkspaceRef::Workspace(id)).await?),
            None => None,
        };
        let verdicts = batches::batch_reviews(read, row.id)
            .await?
            .into_iter()
            .map(|review| BatchVerdictView {
                proposal: review.proposal,
                version: review.version,
                verdict: review.verdict,
            })
            .collect();
        let comments = batches::batch_comments(read, row.id)
            .await?
            .into_iter()
            .map(|comment| BatchCommentView {
                number: comment.number,
                body: comment.body,
                members: comment.members,
                created_at: comment.created_at,
            })
            .collect();
        Ok(ReviewBatchView {
            id: row.id,
            holder: row.holder,
            state: row.state,
            created_at: row.created_at,
            submitted_at: row.submitted_at,
            closed_at: row.closed_at,
            members,
            checkout,
            verdicts,
            comments,
        })
    }

    async fn batch_checkout(
        &self,
        workspace: WorkspaceId,
        batch: ReviewBatchId,
        members: Vec<(ProposalId, u32)>,
    ) -> Result<BatchCheckout, ReviewError> {
        let workspace = self
            .workspaces
            .get(WorkspaceRef::Workspace(workspace))
            .await?;
        Ok(BatchCheckout {
            workspace,
            batch,
            members,
        })
    }

    /// The member and the comment on its own lines for a comment on lines of the batch checkout. Every line of the range must come from one run of one member.
    async fn located(
        &self,
        batch: ReviewBatchId,
        checkout: Option<WorkspaceId>,
        index: usize,
        lines: &BatchLines,
        comment: &BatchComment,
    ) -> Result<(ProposalId, CommentInput), ReviewError> {
        let Some(checkout) = checkout else {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("{batch} has no checkout to comment on"),
            )
            .with_details([
                ("reason", json!("no_batch_checkout")),
                ("field", json!(format!("comments[{index}]"))),
            ])
            .with_hint("call create_batch_checkout, then comment on its lines")
            .into());
        };
        if lines.start_line == 0 || lines.start_line > lines.end_line {
            return Err(FoldError::new(
                codes::INVALID_COMMENT_RANGE,
                format!(
                    "lines {}-{} are not a forward run of lines from line 1",
                    lines.start_line, lines.end_line
                ),
            )
            .with_details([("field", json!(format!("comments[{index}]")))])
            .with_hint("pick lines that exist in the batch checkout")
            .into());
        }
        let path = browse::parse_path(&lines.path)?.ok_or_else(|| {
            FoldError::new(codes::INVALID_PATH, "a line comment needs a file path")
                .with_details([("field", json!(format!("comments[{index}].path")))])
                .with_hint("name the file the lines belong to, or drop the lines")
        })?;
        let path = String::from_utf8_lossy(path.as_bytes()).into_owned();
        let origin = batches::line_origin(self.store.read(), checkout, &path, lines.start_line)
            .await?
            .filter(|origin| origin.end_line >= lines.end_line);
        let Some(origin) = origin else {
            return Err(FoldError::new(
                codes::UNATTRIBUTED_LINES,
                format!(
                    "no single member of {batch} brought lines {}-{} of this file",
                    lines.start_line, lines.end_line
                ),
            )
            .with_details([
                ("field", json!(format!("comments[{index}]"))),
                ("path", json!(path)),
                ("start_line", json!(lines.start_line)),
                ("end_line", json!(lines.end_line)),
            ])
            .with_hint("comment on lines one member changed, or leave out the lines and name the members to comment on the batch")
            .into());
        };
        let offset = lines.start_line - origin.start_line;
        let start_line = origin.member_start_line + offset;
        let input = CommentInput {
            location: Some(CommentLocation {
                path,
                side: Side::New,
                start_line,
                end_line: start_line + (lines.end_line - lines.start_line),
            }),
            reply_to: None,
            body: comment.body.clone(),
        };
        Ok((origin.proposal, input))
    }
}

/// A batch names two or more distinct proposals, at most `MAX_MEMBERS`.
fn check_members(proposals: &[ProposalId]) -> Result<(), ReviewError> {
    let distinct: BTreeSet<ProposalId> = proposals.iter().copied().collect();
    if distinct.len() != proposals.len() {
        return Err(
            FoldError::new(codes::INVALID_REQUEST, "a batch names each proposal once")
                .with_details([("field", json!("proposals"))])
                .with_hint("call create_review_batch again without the repeated proposal")
                .into(),
        );
    }
    if !(MIN_MEMBERS..=MAX_MEMBERS).contains(&proposals.len()) {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("a batch has {MIN_MEMBERS} to {MAX_MEMBERS} proposals"),
        )
        .with_details([("field", json!("proposals"))])
        .with_hint("review one proposal with claim_review and submit_review, or split a large group into several batches")
        .into());
    }
    Ok(())
}

/// The proposals in dependency order of their tasks: a task comes after every member task it needs, directly or through other tasks.
async fn dependency_order(
    tx: &mut WriteTx,
    rows: Vec<ProposalRow>,
) -> Result<Vec<ProposalRow>, ReviewError> {
    let graph = Dag::from_edges(
        tasks::edges_conn(tx.conn())
            .await?
            .into_iter()
            .map(|edge| (edge.task, edge.needs)),
    );
    let mut by_task: BTreeMap<TaskId, ProposalRow> =
        rows.into_iter().map(|row| (row.task, row)).collect();
    let order = graph.topological_order(by_task.keys().copied().collect::<Vec<_>>());
    Ok(order
        .into_iter()
        .filter_map(|task| by_task.remove(&task))
        .collect())
}

fn open_members(members: Vec<MemberRow>) -> Vec<MemberRow> {
    members.into_iter().filter(MemberRow::is_open).collect()
}

fn member_versions(members: &[MemberRow]) -> Vec<(ProposalId, u32)> {
    members
        .iter()
        .map(|member| (member.proposal, member.version))
        .collect()
}

fn members_json(members: &[MemberRow]) -> Vec<Value> {
    members
        .iter()
        .map(|member| json!({"proposal": member.proposal.to_string(), "version": member.version}))
        .collect()
}

/// The line origin rows of a checkout, each run tied to the member that brought it.
fn line_origins<'a>(members: &[MemberRow], files: &'a [FileOrigins]) -> Vec<NewLineOrigin<'a>> {
    let mut rows = Vec::new();
    for file in files {
        for run in &file.runs {
            let Some(member) = members.get(run.member) else {
                continue;
            };
            rows.push(NewLineOrigin {
                proposal: member.proposal,
                path: &file.path,
                start_line: run.start_line,
                end_line: run.end_line,
                member_start_line: run.member_start_line,
            });
        }
    }
    rows
}

fn member_of(
    members: &[MemberRow],
    batch: ReviewBatchId,
    proposal: ProposalId,
) -> Result<(), ReviewError> {
    if members.iter().any(|member| member.proposal == proposal) {
        Ok(())
    } else {
        Err(not_member(batch, proposal))
    }
}

/// Why a member's verdict cannot be written now, or `None` when it can.
async fn left_out_reason(
    tx: &mut WriteTx,
    batch: &BatchRow,
    member: &MemberRow,
    proposal: &ProposalRow,
) -> Result<Option<LeftOutReason>, ReviewError> {
    if proposal.state != ProposalState::Open {
        return Ok(Some(LeftOutReason::NotOpen));
    }
    if proposal.current_version != member.version {
        return Ok(Some(LeftOutReason::NewVersion));
    }
    let claim = reviews::unreleased_review_claim(tx.conn(), member.proposal).await?;
    if claim.is_none_or(|claim| claim.worker_id != batch.holder_worker_id) {
        return Ok(Some(LeftOutReason::ClaimLost));
    }
    Ok(None)
}

/// Archives the batch's active checkout, when it has one, inside the caller's write.
async fn archive_checkout(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &EventActor,
    batch: ReviewBatchId,
    reason: ArchiveReason,
    now: Timestamp,
) -> Result<(), ReviewError> {
    if let Some(checkout) = batches::active_batch_checkout(tx.conn(), batch).await? {
        archive_in_tx::<ReviewError>(tx, buffer, actor, checkout, reason, now).await?;
    }
    Ok(())
}

/// Closes the open batch the proposal is in once the proposal has ended, when no other member is left to review, inside the caller's write.
pub(super) async fn close_batch_of(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &EventActor,
    proposal: ProposalId,
    now: Timestamp,
) -> Result<(), ReviewError> {
    if let Some(batch) = batches::open_batch_of(tx.conn(), proposal).await? {
        close_when_done(tx, buffer, actor, batch, now).await?;
    }
    Ok(())
}

/// Closes an open batch that has no member with an open proposal left, archives its checkout and appends `review.batch_closed`, inside the caller's write. False when the batch stays open.
async fn close_when_done(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &EventActor,
    batch: ReviewBatchId,
    now: Timestamp,
) -> Result<bool, ReviewError> {
    if batches::has_open_member_to_review(tx.conn(), batch).await?
        || !batches::close_batch(tx, batch, now).await?
    {
        return Ok(false);
    }
    archive_checkout(tx, buffer, actor, batch, ArchiveReason::BatchClosed, now).await?;
    let event = NewEvent::new(types::REVIEW_BATCH_CLOSED, actor.clone())?
        .data(json!({"batch": batch.to_string()}))?;
    buffer.append(tx, &event, now).await?;
    Ok(true)
}

fn check_verdict_shapes(verdicts: &[BatchVerdict]) -> Result<(), ReviewError> {
    let mut seen = BTreeSet::new();
    for (index, verdict) in verdicts.iter().enumerate() {
        if !seen.insert(verdict.proposal) {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("{} has more than one verdict", verdict.proposal),
            )
            .with_details([("field", json!(format!("verdicts[{index}]")))])
            .with_hint("give each member one verdict")
            .into());
        }
        if verdict.body.trim().chars().count() > MAX_BODY_CHARS {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("a verdict body must be at most {MAX_BODY_CHARS} characters"),
            )
            .with_details([("field", json!(format!("verdicts[{index}].body")))])
            .with_hint("shorten the body, or move detail into line comments")
            .into());
        }
    }
    Ok(())
}

/// A reject needs its reason, and a change request or a comment says something: a body or a comment on the member. An approval may be empty, because the batch review is its content.
fn check_verdict_body(verdict: &BatchVerdict, comments: usize) -> Result<(), ReviewError> {
    let empty = verdict.body.trim().is_empty();
    let refusal = match verdict.verdict {
        Verdict::Reject if empty => "a reject needs a body: the reason the proposal closes",
        Verdict::RequestChanges | Verdict::Comment if empty && comments == 0 => {
            "this verdict needs a body or a comment on the member"
        }
        _ => return Ok(()),
    };
    Err(FoldError::new(codes::EMPTY_REVIEW, refusal)
        .with_details([
            ("proposal", json!(verdict.proposal.to_string())),
            ("verdict", json!(verdict.verdict.as_str())),
        ])
        .with_hint("call submit_batch_review again with a body for that member")
        .into())
}

fn batch_gate<'a>(actor: &'a Actor, row: &BatchRow) -> Result<&'a AgentActor, ReviewError> {
    policy::can_review_batch(actor, &row.holder).map_err(|denied| {
        FoldError::from(denied)
            .with_details([
                ("batch", json!(row.id.to_string())),
                ("holder", json!(row.holder.as_str())),
            ])
            .into()
    })
}

fn open_batch(row: Option<BatchRow>, batch: ReviewBatchId) -> Result<BatchRow, ReviewError> {
    let row = row.ok_or_else(|| batch_not_found(batch))?;
    if row.state == BatchState::Open {
        return Ok(row);
    }
    Err(FoldError::new(
        codes::REVIEW_BATCH_NOT_OPEN,
        format!("{batch} is {}", row.state.as_str()),
    )
    .with_details([("batch", json!(batch.to_string()))])
    .with_hint(
        "call create_review_batch for a new batch, or review each proposal with submit_review",
    )
    .into())
}

fn batch_not_found(batch: ReviewBatchId) -> ReviewError {
    FoldError::new(
        codes::REVIEW_BATCH_NOT_FOUND,
        format!("{batch} does not exist"),
    )
    .with_hint("call list_review_batches to see the batches")
    .into()
}

fn not_member(batch: ReviewBatchId, proposal: ProposalId) -> ReviewError {
    FoldError::new(
        codes::NOT_BATCH_MEMBER,
        format!("{proposal} is not in {batch}"),
    )
    .with_details([
        ("batch", json!(batch.to_string())),
        ("proposal", json!(proposal.to_string())),
    ])
    .with_hint("call get_review_batch to see its members")
    .into()
}

fn already_in_batch(proposal: ProposalId, other: Option<ReviewBatchId>) -> ReviewError {
    FoldError::new(
        codes::ALREADY_IN_BATCH,
        format!("{proposal} is already in another open batch"),
    )
    .with_details([
        ("proposal", json!(proposal.to_string())),
        ("batch", json!(other.map(|batch| batch.to_string()))),
    ])
    .with_hint("call remove_from_batch on the other batch first, or leave the proposal out")
    .into()
}

fn batch_conflict(
    batch: ReviewBatchId,
    proposal: Option<ProposalId>,
    with: Option<ProposalId>,
    paths: Vec<String>,
) -> ReviewError {
    let name = |id: Option<ProposalId>| id.map_or_else(|| "main".to_owned(), |id| id.to_string());
    let (proposal, with) = (name(proposal), name(with));
    let hint = if with == "main" {
        "ask the author to call sync_workspace and update_proposal, or call remove_from_batch for that member"
    } else {
        "call remove_from_batch for one of the two, or ask the later author to sync after the earlier one integrates"
    };
    FoldError::new(
        codes::BATCH_CONFLICT,
        format!("{proposal} conflicts with {with} in {batch}"),
    )
    .with_details([
        ("batch", json!(batch.to_string())),
        ("proposal", json!(proposal)),
        ("with", json!(with)),
        ("paths", json!(paths)),
    ])
    .with_hint(hint)
    .into()
}

fn batch_changed(batch: ReviewBatchId) -> ReviewError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!("the members of {batch} changed while its checkout was written"),
    )
    .with_details([
        ("reason", json!("batch_changed")),
        ("batch", json!(batch.to_string())),
    ])
    .with_hint("call create_batch_checkout again")
    .into()
}

fn missing_verdicts(batch: ReviewBatchId, missing: Vec<String>) -> ReviewError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!("every member of {batch} needs a verdict"),
    )
    .with_details([
        ("field", json!("verdicts")),
        ("batch", json!(batch.to_string())),
        ("missing", json!(missing)),
    ])
    .with_hint(
        "give each member one verdict, or call remove_from_batch for a member you do not review",
    )
    .into()
}

fn unnamed_batch_comment(index: usize) -> ReviewError {
    FoldError::new(
        codes::INVALID_REQUEST,
        "a comment on the batch names the members it is about",
    )
    .with_details([("field", json!(format!("comments[{index}].members")))])
    .with_hint("name the members the problem concerns, or give the lines of the batch checkout")
    .into()
}

/// `checkout_failed` with the code of what stopped the merge or the write. A refusal that is not about the objects or files keeps its own code.
fn checkout_failed(batch: ReviewBatchId, error: WorkspaceError) -> ReviewError {
    match error {
        WorkspaceError::Fold(cause) => FoldError::new(
            codes::CHECKOUT_FAILED,
            format!("the checkout of {batch} could not be written"),
        )
        .with_details([
            ("batch", json!(batch.to_string())),
            ("cause", json!(cause.code())),
        ])
        .with_hint("check the disk space and the permissions of ws/, then call create_batch_checkout again")
        .into(),
        other => other.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_out_reasons_have_stable_names() {
        assert_eq!(LeftOutReason::NewVersion.as_str(), "new_version");
        assert_eq!(LeftOutReason::NotOpen.as_str(), "not_open");
        assert_eq!(LeftOutReason::ClaimLost.as_str(), "claim_lost");
    }

    #[test]
    fn a_batch_names_two_to_twenty_distinct_proposals() {
        let p = |n| ProposalId::from_number(n).unwrap();
        assert!(check_members(&[p(1), p(2)]).is_ok());
        assert!(check_members(&[p(1)]).is_err());
        assert!(check_members(&[p(1), p(1)]).is_err());
        let many: Vec<_> = (1..=21).map(p).collect();
        assert!(check_members(&many).is_err());
    }
}
