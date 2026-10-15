use fold_core::events::types;
use fold_core::merge::{Conflict, ConflictReport, ReportTrees};
use fold_core::{
    Actor, IdKind, IntegrationId, MessageBody, NewEvent, ProposalId, RevisionId, TaskId,
};
use fold_store::reviews::{self, IntegrationSubject, NewIntegration};
use fold_store::{StoreError, counters, workspaces};
use serde_json::json;

use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;
use crate::services::messages::{MessageError, send_from_fold_in_tx};
use crate::services::tasks::recompute;

/// A conflicting merge of the current version of a proposal onto main, as the integration procedure found it.
pub(super) struct FoundConflict<'a> {
    pub proposal: ProposalId,
    pub task: TaskId,
    pub version: u32,
    /// The head the merge ran onto.
    pub main: RevisionId,
    pub trees: ReportTrees,
    pub conflicts: &'a [Conflict],
}

impl ReviewService {
    /// Step 4 of the integration procedure, under the integration mutex. One write transaction records the `conflicted` integration with its report and `integration.conflicted`, sets the proposal's conflict flag, sends the author's worker a message from `fold` that names the paths, and recalculates the task, which returns to In Progress while the proposal stays open. Nothing is published: no revision, no move of main, no completion. False, with nothing written, when main moved or the proposal is no longer open at that version, so the procedure starts again.
    pub(super) async fn record_conflict(
        &self,
        actor: &Actor,
        found: FoundConflict<'_>,
    ) -> Result<bool, ReviewError> {
        let FoundConflict {
            proposal,
            task,
            version,
            main,
            trees,
            conflicts,
        } = found;
        let label = actor.event_actor();
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let recorded = self
            .store
            .write(async |tx| {
                if workspaces::main_head(tx.conn()).await? != Some(main) {
                    return Ok(None);
                }
                let Some(row) = reviews::get_proposal(tx.conn(), proposal).await? else {
                    return Ok(None);
                };
                if row.current_version != version
                    || !reviews::mark_conflicted(tx, proposal, now).await?
                {
                    return Ok(None);
                }
                let base_revision = reviews::list_versions(tx.conn(), proposal)
                    .await?
                    .into_iter()
                    .find(|row| row.version == version)
                    .ok_or(StoreError::Corrupt("proposal_versions.version"))?
                    .base;
                let report =
                    ConflictReport::new(proposal, version, base_revision, main, trees, conflicts);
                let number = counters::allocate(tx, IdKind::Integration).await?;
                let integration = IntegrationId::from_number(number)
                    .ok_or(StoreError::Corrupt("counters.next"))?;
                let new = NewIntegration {
                    number: integration,
                    subject: IntegrationSubject::Proposal { proposal, version },
                    actor: label.as_str(),
                    created_at: now,
                };
                reviews::insert_conflicted(tx, &new, &report).await?;
                let event = NewEvent::new(types::INTEGRATION_CONFLICTED, label.clone())?
                    .task(task)
                    .proposal(proposal)
                    .revision(main)
                    .data(json!({
                        "integration": integration.to_string(),
                        "proposal": proposal.to_string(),
                        "version": version,
                        "main_revision": main.to_string(),
                        "files": report.files.len(),
                    }))?;
                buffer.append(tx, &event, now).await?;
                let body = MessageBody::parse(report.author_message())?;
                send_from_fold_in_tx(tx, &mut buffer, &row.author, (task, proposal), &body, now)
                    .await?;
                buffer.extend(recompute(tx, &[task], now).await?.events);
                Ok::<_, ReviewError>(Some((integration, report.files.len())))
            })
            .await?;
        let Some((integration, files)) = recorded else {
            return Ok(false);
        };
        self.bus.publish(buffer);
        tracing::info!(proposal = %proposal, version, revision = %main, integration = %integration, files, "proposal conflicts with main");
        Ok(true)
    }

    /// The report of the latest conflicted integration of `version` of `proposal`, read as stored.
    pub(super) async fn conflict_report(
        &self,
        proposal: ProposalId,
        version: u32,
    ) -> Result<Option<ConflictReport>, ReviewError> {
        Ok(reviews::conflict_report(self.store.read(), proposal, version).await?)
    }
}

impl From<MessageError> for ReviewError {
    fn from(error: MessageError) -> Self {
        match error {
            MessageError::Fold(error) => Self::Fold(error),
            MessageError::Store(error) => Self::Store(error),
            MessageError::Event(error) => Self::Event(error),
        }
    }
}
