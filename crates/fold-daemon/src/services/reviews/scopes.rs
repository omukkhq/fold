use fold_core::events::types;
use fold_core::reviews::{
    ReviewScopeRecord, ScopeChoice, ScopeFacts, ScopeSuggestion, last_verdict_version,
    suggest_scope,
};
use fold_core::{Actor, EventActor, FoldError, NewEvent, ProposalId, Timestamp, codes, policy};
use fold_store::reviews::{self, ProposalRow};
use fold_store::{WriteTx, agents};
use serde_json::json;

use super::changes::SinceChange;
use super::proposals::{require_open, version_not_found};
use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;

/// fold's suggestion for a review of a revised version, with the earlier version it reads the change since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestedScope {
    pub since: u32,
    pub suggestion: ScopeSuggestion,
}

/// fold's suggested scope for a review of `version` from `change`, its change since an earlier version.
pub fn scope_suggestion(version: u32, change: &SinceChange) -> ScopeSuggestion {
    let lines = |totals: &fold_vcs::compare::Totals| {
        u64::from(totals.added_lines) + u64::from(totals.removed_lines)
    };
    suggest_scope(&ScopeFacts {
        version,
        new_files: change.new_files.len(),
        delta_lines: lines(&change.delta.totals),
        earlier_lines: lines(&change.earlier),
        base_conflicts: change.base_conflicts.len(),
    })
}

impl ReviewService {
    /// Chooses the scope of the review of `version` of an open proposal, for the review lead or the main agent: a version that came after the review was assigned, such as a reviewer's own fix version, or a choice made again. The choice needs a reason and is stored with `review.scope_chosen`; the newest choice for a version applies. A version that does not exist is `proposal_not_found` with the current version in its details.
    pub async fn set_review_scope(
        &self,
        actor: &Actor,
        version: u32,
        choice: ScopeChoice,
    ) -> Result<(), ReviewError> {
        let proposal = choice.proposal;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let standing = agents::standing_of(tx.conn(), actor).await?;
                policy::can_choose_review_scope(actor, standing).map_err(|denied| {
                    FoldError::from(denied)
                        .with_details([("proposal", json!(proposal.to_string()))])
                })?;
                let row =
                    require_open(reviews::get_proposal(tx.conn(), proposal).await?, proposal)?;
                if version == 0 || version > row.current_version {
                    return Err(version_not_found(proposal, version, row.current_version));
                }
                let by = actor.event_actor();
                choose_version_scope_in_tx(tx, &mut buffer, &by, &row, version, &choice, now).await
            })
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }

    /// fold's suggested scope for a review of `version`, from its change since the newest earlier version with a verdict; `None` when no earlier version has one, so the review reads the whole version. A read: it writes nothing.
    pub async fn suggested_scope(
        &self,
        proposal: ProposalId,
        version: u32,
    ) -> Result<Option<SuggestedScope>, ReviewError> {
        let Some(since) = self.last_reviewed(proposal, version).await? else {
            return Ok(None);
        };
        let change = self
            .proposal_since_change(proposal, Some(version), since)
            .await?;
        Ok(Some(SuggestedScope {
            since,
            suggestion: scope_suggestion(change.version, &change.change),
        }))
    }
}

/// Stores the review lead's `choice` of scope for the current version of the open proposal `row` inside the caller's write; see [`choose_version_scope_in_tx`].
pub(crate) async fn choose_scope_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    by: &EventActor,
    row: &ProposalRow,
    choice: &ScopeChoice,
    now: Timestamp,
) -> Result<(), ReviewError> {
    let version = row.current_version;
    choose_version_scope_in_tx(tx, buffer, by, row, version, choice, now).await
}

/// Stores the review lead's `choice` of scope for `version` of the open proposal `row` inside the caller's write, with `review.scope_chosen`; a newer choice for the same version replaces it. A version that no earlier version with a verdict comes before is reviewed whole, so a choice for it is `invalid_request`.
async fn choose_version_scope_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    by: &EventActor,
    row: &ProposalRow,
    version: u32,
    choice: &ScopeChoice,
    now: Timestamp,
) -> Result<(), ReviewError> {
    let rows = reviews::list_reviews(tx.conn(), row.id).await?;
    if last_verdict_version(
        rows.iter().map(|review| (review.version, review.verdict)),
        version,
    )
    .is_none()
    {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!(
                "{} v{version} has no earlier version with a verdict, so it is reviewed whole",
                row.id
            ),
        )
        .with_details([
            ("field", json!("scope")),
            ("proposal", json!(row.id.to_string())),
        ])
        .with_hint("leave the scope out for a proposal's first review")
        .into());
    }
    reviews::insert_scope_choice(
        tx,
        row.id,
        version,
        choice.scope,
        &choice.reason,
        by.as_str(),
        now,
    )
    .await?;
    let event = NewEvent::new(types::REVIEW_SCOPE_CHOSEN, by.clone())?
        .task(row.task)
        .proposal(row.id)
        .data(json!({
            "proposal": row.id.to_string(),
            "version": version,
            "scope": choice.scope.as_str(),
            "reason": choice.reason,
        }))?;
    buffer.append(tx, &event, now).await?;
    Ok(())
}

/// The scope a review of `version` follows, decided inside the review's write: the newest choice of the review lead for that version when it made one, else fold's `suggested` scope. `None` for a version reviewed whole.
pub(super) async fn scope_record_in_tx(
    tx: &mut WriteTx,
    proposal: ProposalId,
    version: u32,
    suggested: Option<&SuggestedScope>,
) -> Result<Option<ReviewScopeRecord>, ReviewError> {
    let Some(suggested) = suggested else {
        return Ok(None);
    };
    let choice = reviews::latest_scope_choice(tx.conn(), proposal, version)
        .await?
        .map(|row| (row.scope, row.reason));
    Ok(Some(ReviewScopeRecord::decide(
        suggested.since,
        suggested.suggestion.clone(),
        choice,
    )))
}

/// Refuses a scope choice that names a proposal outside `proposals`, or one proposal twice.
pub(crate) fn check_choices(
    proposals: &[ProposalId],
    choices: &[ScopeChoice],
) -> Result<(), ReviewError> {
    for (index, choice) in choices.iter().enumerate() {
        let repeated = choices[..index]
            .iter()
            .any(|earlier| earlier.proposal == choice.proposal);
        if repeated || !proposals.contains(&choice.proposal) {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!(
                    "the scope of {} must name one of the proposals, once",
                    choice.proposal
                ),
            )
            .with_details([
                ("field", json!("scopes")),
                ("proposal", json!(choice.proposal.to_string())),
            ])
            .with_hint("give at most one scope for each proposal you pass in proposals")
            .into());
        }
    }
    Ok(())
}
