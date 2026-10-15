use fold_core::claims::is_stale;
use fold_core::events::types;
use fold_core::{
    Actor, EventActor, FoldError, NewEvent, ProposalId, Timestamp, WorkerName, codes, policy,
};
use fold_store::reviews::{self, ReviewClaimRow};
use serde_json::json;

use super::proposals::{proposal_not_found, require_open};
use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;
use crate::services::tasks::{renew_leases, worker_row};

/// The `release_reason` of a claim that a stale take-over ended.
const TAKEN_OVER: &str = "taken_over";

/// The `release_reason` of a release that names no reason.
const RELEASED: &str = "released";

/// The longest release reason, in characters.
const MAX_REASON_CHARS: usize = 500;

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
    /// Takes the review claim on an open proposal for the actor's worker; `policy::can_claim_review` refuses the author's agent. The insert runs before any read of the holder, so the partial unique index on unreleased review claims decides a race: the loser gets `already_claimed` naming the holder. A repeat call by the holder renews the claim without a second `claim.acquired`. A holder whose lease has run out is taken over in the same transaction: its claim is released as `taken_over` with `claim.taken_over`, and the new claim follows with `claim.acquired`.
    pub async fn claim_review(
        &self,
        actor: &Actor,
        proposal: ProposalId,
    ) -> Result<ReviewClaimView, ReviewError> {
        let lease_minutes = self.lease_minutes().await?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let view = self
            .store
            .write(async |tx| {
                let row =
                    require_open(reviews::get_proposal(tx.conn(), proposal).await?, proposal)?;
                let reviewer = policy::can_claim_review(actor, &row.author).map_err(|denied| {
                    FoldError::from(denied).with_details([
                        ("proposal", json!(proposal.to_string())),
                        ("author", json!(row.author.as_str())),
                    ])
                })?;
                let worker = worker_row(tx, reviewer).await?;
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
                        let event = NewEvent::new(
                            types::CLAIM_TAKEN_OVER,
                            EventActor::worker(&worker.name),
                        )?
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
                Ok::<_, ReviewError>(ReviewClaimView {
                    proposal,
                    holder: worker.name.clone(),
                    acquired_at: now,
                    renewed_at: now,
                    stale: false,
                })
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
fn already_claimed(holder: &ReviewClaimRow) -> ReviewError {
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
