use std::collections::HashSet;

use fold_core::merge::ReportTrees;
use fold_core::{Actor, ProposalId, RevisionId};
use fold_store::reviews;
use fold_store::{StoreError, workspaces};

use super::conflicts::FoundConflict;
use super::{ReviewError, ReviewService};

/// How the test-merge of one queued version ended.
enum Merged {
    Clean,
    /// Recorded, or already recorded on the same main.
    Conflicted,
    /// The record found main moved, a new version or a gate that refuses fold: nothing was written, and the version's own turn merges again.
    Skipped,
    /// main is no longer at the head the merge was asked for.
    MainMoved,
}

impl ReviewService {
    /// Starts [`ReviewService::find_queue_conflicts`] onto `main` in the background, so the integration that published `main` and the next one wait for none of its merges.
    pub(super) fn find_queue_conflicts_later(&self, main: RevisionId) {
        let service = self.clone();
        self.checks.spawn(async move {
            if let Err(error) = service.find_queue_conflicts(main).await {
                tracing::warn!(revision = %main, code = error.log_code(), "the test-merge of the integration queue failed; the next integration tries again");
            }
        });
    }

    /// Test-merges the current version of each proposal that takes its own turn in the automatic integration queue onto `main`, the head an integration just published, without the integration mutex and without a write to main, a workspace or the object store. A version that conflicts is recorded as a conflict at integration is, by fold, with `before_turn` true: it leaves the queue and goes to the reviewer that approved it, or the review lead gets the notice. A version that merges cleanly keeps its place. A merge that a later integration makes stale does no harm: the record checks that main is still at `main`, and the version's own turn merges again. Returns the proposals it found in conflict, in queue order; nothing once main moved past `main`.
    pub async fn find_queue_conflicts(
        &self,
        main: RevisionId,
    ) -> Result<Vec<ProposalId>, ReviewError> {
        let mut found = Vec::new();
        for (proposal, version) in self.queued_versions().await? {
            match self.test_merge_one(proposal, version, main).await {
                Ok(Merged::Conflicted) => found.push(proposal),
                Ok(Merged::Clean | Merged::Skipped) => {}
                Ok(Merged::MainMoved) => break,
                Err(error) => {
                    tracing::warn!(proposal = %proposal, revision = %main, code = error.log_code(), "the test-merge of a queued proposal failed; its own turn merges again");
                }
            }
        }
        Ok(found)
    }

    /// The test-merge of `version`, the current version of `proposal`, onto `main`, and the record of its conflict.
    async fn test_merge_one(
        &self,
        proposal: ProposalId,
        version: u32,
        main: RevisionId,
    ) -> Result<Merged, ReviewError> {
        let head = workspaces::head(self.store.read())
            .await?
            .ok_or(StoreError::Corrupt("refs.main"))?;
        if head.number != main {
            return Ok(Merged::MainMoved);
        }
        let mut read = self.store.read().begin().await.map_err(StoreError::from)?;
        let facts = reviews::integration_facts(&mut read, proposal).await?;
        read.commit().await.map_err(StoreError::from)?;
        let Some(facts) = facts.filter(|facts| facts.target.version == version) else {
            return Ok(Merged::Skipped);
        };
        let target = facts.target;
        let conflicts = self
            .workspaces
            .test_merge(target.base_tree, head.tree_hash, target.tree)
            .await?;
        if conflicts.is_empty() {
            return Ok(Merged::Clean);
        }
        let conflict = FoundConflict {
            proposal,
            task: facts.proposal.task,
            version: target.version,
            main,
            trees: ReportTrees {
                base: target.base_tree,
                main: head.tree_hash,
                version: target.tree,
            },
            conflicts: &conflicts,
            expected_version: Some(target.version),
            automatic: true,
            before_turn: true,
        };
        Ok(if self.record_conflict(&Actor::System, conflict).await? {
            Merged::Conflicted
        } else {
            Merged::Skipped
        })
    }

    /// The proposals that take a turn of their own in the automatic integration now, each with the version it would integrate, in the job's order from [`ReviewService::automatic_standings`]. The members a review batch or a lane holds back are not among them.
    async fn queued_versions(&self) -> Result<Vec<(ProposalId, u32)>, ReviewError> {
        let holds = self.batch_holds().await?;
        let standings = self
            .automatic_standings(&HashSet::new(), &holds.held, false)
            .await?;
        Ok(standings
            .into_iter()
            .filter_map(|(proposal, standing)| standing.ok().map(|version| (proposal, version)))
            .collect())
    }
}
