use fold_core::claims::is_stale;
use fold_core::merge::{Conflict, ReportFile};
use fold_core::policy::{GateState, IntegrateInput, ProposalFacts, gate_report};
use fold_core::reviews::{deciding_approval, last_verdict_version};
use fold_core::{
    Actor, ChangesetId, FoldError, ProposalId, TaskDescription, TaskId, TaskStatus, TaskTitle,
    codes,
};
use fold_store::reviews::{self, ProposalState};
use fold_store::tasks::{self, TaskRef};
use fold_store::{StoreError, agents};
use fold_vcs::compare::{FileChange, Totals};
use serde_json::json;

pub use fold_store::reviews::ReviewRow as ReviewView;

use super::checks::CheckRunView;
use super::claims::ReviewClaimView;
use super::integrate::caller_facts;
use super::proposals::{ProposalDetail, proposal_detail, proposal_not_found};
use super::{ReviewError, ReviewService};
use crate::services::workspaces::StateRef;

/// What the task of a proposal asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRequirements {
    pub id: TaskId,
    pub title: TaskTitle,
    pub description: TaskDescription,
    pub status: TaskStatus,
}

/// The comments of a proposal, counted from its reviews.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommentCounts {
    pub total: u32,
    /// On the selected version.
    pub selected: u32,
    /// On a version before the current one.
    pub outdated: u32,
}

/// Everything the proposal screen and `get_proposal` show, read for one caller: the gate report says what stands between that caller and integrating the selected version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalView {
    pub detail: ProposalDetail,
    pub task: TaskRequirements,
    /// The direct prerequisites of the task with their statuses.
    pub prerequisites: Vec<TaskRef>,
    pub claim: Option<ReviewClaimView>,
    /// The reviews of every version, oldest first.
    pub reviews: Vec<ReviewView>,
    pub comments: CommentCounts,
    /// The newest version before the selected one that has a verdict, by anyone: what changed since it is what is left to review.
    pub last_reviewed: Option<u32>,
    /// In the order the integration procedure checks the gates.
    pub gates: Vec<GateState>,
    /// An agent that decides reviews, other than the author's, approved the current version and no request for changes came after it. On a task with the human review mark a human approves only then.
    pub agent_approved: bool,
    /// The files the selected version changes against its base, in path order.
    pub files: Vec<FileChange>,
    pub totals: Totals,
    /// The check runs of the selected version, newest first, without logs: the proposal-stage runs of its exact files and its integration runs.
    pub checks: Vec<CheckRunView>,
}

impl ReviewService {
    /// A proposal as `caller` sees it: the detail of [`ReviewService::get_proposal`] with the task, its prerequisites, the review claim, the reviews, the comment counts, the newest earlier version with a verdict, the gate report for integrating the selected version, and the files that version changes against its base. A read: it writes nothing.
    pub async fn view_proposal(
        &self,
        caller: &Actor,
        proposal: ProposalId,
        version: Option<u32>,
    ) -> Result<ProposalView, ReviewError> {
        let now = self.clock.now();
        let mut read = self.store.read().begin().await.map_err(StoreError::from)?;
        let standing = agents::standing_of(&mut *read, caller).await?;
        let caller = caller_facts(caller, standing, false)?;
        let detail = proposal_detail(&mut read, proposal, version).await?;
        let facts = reviews::integration_facts(&mut read, proposal)
            .await?
            .ok_or_else(|| proposal_not_found(proposal))?;
        let rows = reviews::list_reviews(&mut *read, proposal).await?;
        let task = tasks::fetch(&mut read, facts.proposal.task)
            .await?
            .ok_or(StoreError::Corrupt("proposals.task_id"))?;
        let needs: Vec<TaskId> = facts.prerequisites.iter().map(|(id, _)| *id).collect();
        let prerequisites = tasks::task_refs(&mut read, &needs).await?;
        let holder = reviews::unreleased_review_claim(&mut *read, proposal).await?;
        let checks = ReviewService::version_checks(
            &mut read,
            facts.proposal.task,
            proposal,
            detail.selected.version,
        )
        .await?;
        read.commit().await.map_err(StoreError::from)?;
        let claim = match holder {
            Some(row) => Some(ReviewClaimView {
                proposal,
                stale: is_stale(row.renewed_at, now, self.lease_minutes().await?),
                holder: row.worker,
                acquired_at: row.acquired_at,
                renewed_at: row.renewed_at,
            }),
            None => None,
        };
        let last_reviewed = last_verdict_version(
            rows.iter().map(|review| (review.version, review.verdict)),
            detail.selected.version,
        );
        let selected = detail.selected.version;
        let current = detail.summary.current_version;
        let mut comments = CommentCounts::default();
        for review in &rows {
            comments.total = comments.total.saturating_add(review.comments);
            if review.version == selected {
                comments.selected = comments.selected.saturating_add(review.comments);
            }
            if review.version != current {
                comments.outdated = comments.outdated.saturating_add(review.comments);
            }
        }
        let agent_approved = deciding_approval(&facts.reviews, &facts.proposal.author.agent());
        let gates = gate_report(&IntegrateInput {
            caller: &caller,
            proposal: &ProposalFacts {
                open: facts.proposal.state == ProposalState::Open,
                author: facts.proposal.author.agent(),
                current_version: facts.target.version,
            },
            expected_version: Some(selected),
            reviews: &facts.reviews,
            human_review: facts.human_review.as_ref(),
            prerequisites: &facts.prerequisites,
        });
        let comparison = self
            .workspaces
            .compare_states(
                StateRef::Revision(detail.selected.base),
                StateRef::Tree(detail.selected.tree),
            )
            .await?;
        Ok(ProposalView {
            task: TaskRequirements {
                id: task.id,
                title: task.title,
                description: task.description,
                status: task.status,
            },
            detail,
            prerequisites,
            claim,
            reviews: rows,
            comments,
            last_reviewed,
            gates,
            agent_approved,
            files: comparison.files,
            totals: comparison.totals,
            checks,
        })
    }
}

/// A direct change that conflicts with main: nothing was written and the main workspace stays open, so the main agent brings it up to date and submits again.
pub fn direct_conflicted(changeset: ChangesetId, conflicts: &[Conflict]) -> FoldError {
    let files: Vec<_> = conflicts
        .iter()
        .map(ReportFile::from)
        .map(|file| json!({"path": file.path, "kind": file.kind}))
        .collect();
    FoldError::new(
        codes::INTEGRATION_CONFLICTED,
        format!(
            "{changeset} conflicts with main in {} files and was not integrated",
            files.len()
        ),
    )
    .with_details([
        ("changeset", json!(changeset.to_string())),
        ("files", json!(files)),
    ])
    .with_hint("call sync_workspace on the main workspace, resolve the files, call record_changes, show the user the new diff and call submit_main_change again")
}

/// The refusal a transport reports for an integration that conflicted. The details name each path and its kind only, never a hunk, because an error detail carries no file content; the full report is read with the proposal.
pub fn integration_conflicted(proposal: ProposalId, conflicts: &[Conflict]) -> FoldError {
    let files: Vec<_> = conflicts
        .iter()
        .map(ReportFile::from)
        .map(|file| json!({"path": file.path, "kind": file.kind}))
        .collect();
    FoldError::new(
        codes::INTEGRATION_CONFLICTED,
        format!(
            "{proposal} conflicts with main in {} files and was not integrated",
            files.len()
        ),
    )
    .with_details([
        ("proposal", json!(proposal.to_string())),
        ("files", json!(files)),
    ])
    .with_hint("call get_proposal to read the conflict report; the author syncs the workspace and calls update_proposal")
}
