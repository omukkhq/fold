use fold_core::claims::is_stale;
use fold_core::events::types;
use fold_core::reviews::{MAX_REASON_CHARS, ScopeChoice};
use fold_core::{
    Actor, EventActor, FoldError, NewEvent, ProposalId, ReviewBatchId, Timestamp, WorkerName,
    codes, policy,
};
use fold_store::WriteTx;
use fold_store::agents::{self, WorkerRow};
use fold_store::reviews::{self, ProposalRow, ReviewClaimRow};
use serde_json::json;

use super::batches::{batch_in_tx, check_members};
use super::proposals::{proposal_not_found, require_open};
use super::scopes::{check_choices, choose_scope_in_tx};
use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;
use crate::services::tasks::{renew_leases, task_in_main_area, worker_row};

/// The `release_reason` of a claim that a stale take-over ended.
pub(super) const TAKEN_OVER: &str = "taken_over";

/// The `release_reason` of a release that names no reason.
const RELEASED: &str = "released";

/// The review claim on a proposal as callers see it. `stale` is computed at read time from the project's lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewClaimView {
    pub proposal: ProposalId,
    pub holder: WorkerName,
    pub acquired_at: Timestamp,
    pub renewed_at: Timestamp,
    pub stale: bool,
}

/// Review claims. A review claim lives in its own row beside the task's implementation claim and never changes it, so neither command writes a status. Leases are renewed by the same session touch as implementation claims.
impl ReviewService {
    /// Takes the review claim on an open proposal for the actor's worker; `policy::can_claim_review` refuses the author's agent and every agent that does not decide reviews. The insert runs before any read of the holder, so the partial unique index on unreleased review claims decides a race: the loser gets `already_claimed` naming the holder. A repeat call by the holder renews the claim without a second `claim.acquired`. A holder whose lease has run out is taken over in the same transaction: its claim is released as `taken_over` with `claim.taken_over`, and the new claim follows with `claim.acquired`. A proposal that waits in a review plan is refused with `already_in_plan`.
    pub async fn claim_review(
        &self,
        actor: &Actor,
        proposal: ProposalId,
    ) -> Result<ReviewClaimView, ReviewError> {
        self.claim_review_with_scope(actor, proposal, None).await
    }

    /// [`ReviewService::claim_review`] with the scope the claimant chooses for its own review of the current version, stored in the same transaction with `review.scope_chosen`; without one the review follows fold's suggestion.
    pub async fn claim_review_with_scope(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        choice: Option<ScopeChoice>,
    ) -> Result<ReviewClaimView, ReviewError> {
        if let Some(choice) = &choice {
            check_choices(&[proposal], std::slice::from_ref(choice))?;
        }
        let lease_minutes = self.lease_minutes().await?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let view = self
            .store
            .write(async |tx| {
                let row =
                    require_open(reviews::get_proposal(tx.conn(), proposal).await?, proposal)?;
                super::refuse_planned(tx, proposal).await?;
                let standing = agents::standing_of(tx.conn(), actor).await?;
                let main_task = task_in_main_area(tx.conn(), row.task).await?;
                let reviewer = policy::can_claim_review(actor, standing, &row.author, main_task)
                    .map_err(|denied| {
                        FoldError::from(denied).with_details([
                            ("proposal", json!(proposal.to_string())),
                            ("author", json!(row.author.as_str())),
                        ])
                    })?;
                let worker = worker_row(tx, reviewer).await?;
                let view =
                    take_review_claim(tx, &mut buffer, &row, &worker, lease_minutes, now).await?;
                if let Some(choice) = &choice {
                    let by = EventActor::worker(&worker.name);
                    choose_scope_in_tx(tx, &mut buffer, &by, &row, choice, now).await?;
                }
                Ok::<_, ReviewError>(view)
            })
            .await?;
        self.bus.publish(buffer);
        Ok(view)
    }

    /// Releases the review claim on `proposal`, by any worker of the holder's agent. `reason` is stored as the claim's `release_reason`; without one it is `released`. The proposal stays as it is, so another reviewer can claim it.
    pub async fn release_review(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        reason: Option<&str>,
    ) -> Result<(), ReviewError> {
        let reason = check_reason(reason)?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let row = reviews::get_proposal(tx.conn(), proposal)
                    .await?
                    .ok_or_else(|| proposal_not_found(proposal))?;
                let Some(holder) = reviews::unreleased_review_claim(tx.conn(), proposal).await?
                else {
                    return Err(FoldError::new(
                        codes::CLAIM_NOT_FOUND,
                        format!("{proposal} has no review claim to release"),
                    )
                    .with_details([("proposal", json!(proposal.to_string()))])
                    .with_hint("call claim_review to take the review")
                    .into());
                };
                policy::can_release_review(actor, &holder.worker).map_err(|denied| {
                    FoldError::from(denied).with_details([
                        ("proposal", json!(proposal.to_string())),
                        ("holder", json!(holder.worker.as_str())),
                    ])
                })?;
                reviews::release_review_claim(tx, proposal, reason, now).await?;
                let event = NewEvent::new(types::CLAIM_RELEASED, actor.event_actor())?
                    .task(row.task)
                    .proposal(proposal)
                    .data(json!({
                        "kind": "review",
                        "proposal": proposal.to_string(),
                        "worker": holder.worker.as_str(),
                        "reason": reason,
                    }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, ReviewError>(())
            })
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }
}

/// The review that the review lead gives a new reviewer at its start: the proposals, the scope it chose for some of them, and who it is.
pub(crate) struct Assignment<'a> {
    pub proposals: &'a [ProposalId],
    pub scopes: &'a [ScopeChoice],
    pub by: &'a EventActor,
}

/// Gives the new reviewer `worker` its review of the assigned proposals inside the caller's write, for the review lead that starts it with `start_worker`: one proposal takes its review claim as by `claim_review`, and two or more become a batch of the reviewer's, as by `create_review_batch`. Each chosen scope is stored with `review.scope_chosen` by the review lead. A proposal that waits in a review plan is refused with `already_in_plan`: the plan's start gives it with the others. The new reviewer is an agent of its own, so it is never the author's agent. Returns the batch.
pub(crate) async fn assign_reviews_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    worker: &WorkerRow,
    assignment: &Assignment<'_>,
    lease_minutes: u64,
    now: Timestamp,
) -> Result<Option<ReviewBatchId>, ReviewError> {
    let proposals = assignment.proposals;
    check_choices(proposals, assignment.scopes)?;
    let mut rows = Vec::with_capacity(proposals.len());
    for &proposal in proposals {
        let row = require_open(reviews::get_proposal(tx.conn(), proposal).await?, proposal)?;
        super::refuse_planned(tx, proposal).await?;
        let main_task = task_in_main_area(tx.conn(), row.task).await?;
        policy::can_assign_review(main_task).map_err(|denied| {
            FoldError::from(denied).with_details([("proposal", json!(proposal.to_string()))])
        })?;
        rows.push(row);
    }
    let batch = if let [row] = rows.as_slice() {
        take_review_claim(tx, buffer, row, worker, lease_minutes, now).await?;
        None
    } else {
        check_members(proposals)?;
        Some(batch_in_tx(tx, buffer, worker, rows.clone(), lease_minutes, now).await?)
    };
    for choice in assignment.scopes {
        if let Some(row) = rows.iter().find(|row| row.id == choice.proposal) {
            choose_scope_in_tx(tx, buffer, assignment.by, row, choice, now).await?;
        }
    }
    Ok(batch)
}

/// Takes the review claim on the open proposal `row` for `worker` inside the caller's write, by the rules of `claim_review`: the insert decides a race, a repeat by the holder renews the claim without an event, and a stale holder is taken over with `claim.taken_over` before `claim.acquired`.
pub(super) async fn take_review_claim(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    row: &ProposalRow,
    worker: &WorkerRow,
    lease_minutes: u64,
    now: Timestamp,
) -> Result<ReviewClaimView, ReviewError> {
    let proposal = row.id;
    match reviews::insert_review_claim(tx, proposal, worker.id, now).await {
        Ok(()) => {}
        Err(error) if error.unique_violation() => {
            let holder = reviews::unreleased_review_claim(tx.conn(), proposal)
                .await?
                .ok_or(error)?;
            if holder.worker_id == worker.id {
                renew_leases(tx, &[worker.id], now).await?;
                return Ok(ReviewClaimView {
                    proposal,
                    holder: holder.worker,
                    acquired_at: holder.acquired_at,
                    renewed_at: now,
                    stale: false,
                });
            }
            if !is_stale(holder.renewed_at, now, lease_minutes) {
                return Err(already_claimed(&holder));
            }
            reviews::release_review_claim(tx, proposal, TAKEN_OVER, now).await?;
            reviews::insert_review_claim(tx, proposal, worker.id, now).await?;
            let event = NewEvent::new(types::CLAIM_TAKEN_OVER, EventActor::worker(&worker.name))?
                .task(row.task)
                .proposal(proposal)
                .data(json!({
                    "kind": "review",
                    "proposal": proposal.to_string(),
                    "from": holder.worker.as_str(),
                    "to": worker.name.as_str(),
                }))?;
            buffer.append(tx, &event, now).await?;
        }
        Err(error) => return Err(error.into()),
    }
    let event = NewEvent::new(types::CLAIM_ACQUIRED, EventActor::worker(&worker.name))?
        .task(row.task)
        .proposal(proposal)
        .data(json!({
            "kind": "review",
            "proposal": proposal.to_string(),
            "worker": worker.name.as_str(),
        }))?;
    buffer.append(tx, &event, now).await?;
    Ok(ReviewClaimView {
        proposal,
        holder: worker.name.clone(),
        acquired_at: now,
        renewed_at: now,
        stale: false,
    })
}

/// A reason with no visible text is no reason.
fn check_reason(reason: Option<&str>) -> Result<&str, ReviewError> {
    let reason = reason.map(str::trim).filter(|text| !text.is_empty());
    match reason {
        None => Ok(RELEASED),
        Some(text) if text.chars().count() <= MAX_REASON_CHARS => Ok(text),
        Some(_) => Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("the reason must be at most {MAX_REASON_CHARS} characters"),
        )
        .with_details([("field", json!("reason"))])
        .with_hint("call release_review again with a shorter reason")
        .into()),
    }
}

/// `already_claimed` naming the other worker that holds the review. Its lease has not run out, or the claim would have been taken over.
pub(super) fn already_claimed(holder: &ReviewClaimRow) -> ReviewError {
    FoldError::new(
        codes::ALREADY_CLAIMED,
        format!("{} is under review by {}", holder.proposal, holder.worker),
    )
    .with_details([
        ("proposal", json!(holder.proposal.to_string())),
        ("holder", json!(holder.worker.as_str())),
    ])
    .with_hint("call list_proposals with unclaimed for other reviews, or claim_review again once the holder's lease runs out")
    .into()
}
