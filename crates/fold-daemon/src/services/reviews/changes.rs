use fold_core::reviews::last_verdict_version;
use fold_core::{FoldError, ProposalId, TaskId, codes};
use fold_store::reviews::{self, VersionRow};
use fold_vcs::compare::FileDiff;
use serde_json::json;

use super::proposals::{proposal_not_found, version_not_found};
use super::{ReviewError, ReviewService};
use crate::services::workspaces::{StateComparison, StateKind, StateRef, StateSummary};

/// Which change of a proposal to read: a version against its base revision, or against an earlier version of the same proposal, so a reviewer of a new version reads only what changed since the version it reviewed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChangeRange {
    /// The current version when `None`.
    pub version: Option<u32>,
    /// An earlier version; `None` compares the version with its base revision.
    pub since: Option<u32>,
}

/// A change of a proposal with the versions it was read between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalChange<T> {
    pub version: u32,
    pub since: Option<u32>,
    pub change: T,
}

/// The two stored trees a change compares, and how each side is named to callers.
struct Sides {
    version: u32,
    since: Option<u32>,
    from: (StateRef, StateSummary),
    to: (StateRef, StateSummary),
}

impl ReviewService {
    /// The files a proposal version changes against its base, or against the earlier version `since`, with line counts. Both sides are stored trees: the base revision's or a version's. A version side is named by its changeset, which holds the same tree. A read: it writes nothing.
    pub async fn proposal_changes(
        &self,
        proposal: ProposalId,
        range: ChangeRange,
    ) -> Result<ProposalChange<StateComparison>, ReviewError> {
        let sides = self.change_sides(proposal, range).await?;
        let mut comparison = self
            .workspaces
            .compare_states(sides.from.0, sides.to.0)
            .await?;
        comparison.from = sides.from.1;
        comparison.to = sides.to.1;
        Ok(ProposalChange {
            version: sides.version,
            since: sides.since,
            change: comparison,
        })
    }

    /// The hunks of one file of [`ReviewService::proposal_changes`]. On a change since an earlier version the new side's line numbers are the version's own, so a `new` line comment taken from them lands on the version.
    pub async fn proposal_file_diff(
        &self,
        proposal: ProposalId,
        range: ChangeRange,
        path: &str,
    ) -> Result<ProposalChange<FileDiff>, ReviewError> {
        let sides = self.change_sides(proposal, range).await?;
        let diff = self
            .workspaces
            .diff_file(sides.from.0, sides.to.0, path)
            .await?;
        Ok(ProposalChange {
            version: sides.version,
            since: sides.since,
            change: diff,
        })
    }

    /// The newest version of the proposal before `version` that has a verdict, by anyone: what changed since it is what is left to review on `version`.
    pub async fn last_reviewed(
        &self,
        proposal: ProposalId,
        version: u32,
    ) -> Result<Option<u32>, ReviewError> {
        let rows = reviews::list_reviews(self.store.read(), proposal).await?;
        Ok(last_verdict_version(
            rows.iter().map(|review| (review.version, review.verdict)),
            version,
        ))
    }

    async fn change_sides(
        &self,
        proposal: ProposalId,
        range: ChangeRange,
    ) -> Result<Sides, ReviewError> {
        let read = self.store.read();
        let row = reviews::get_proposal(read, proposal)
            .await?
            .ok_or_else(|| proposal_not_found(proposal))?;
        let versions = reviews::list_versions(read, proposal).await?;
        let current = row.current_version;
        let find = |wanted: u32| {
            versions
                .iter()
                .find(|version| version.version == wanted)
                .ok_or_else(|| version_not_found(proposal, wanted, current))
        };
        let shown = find(range.version.unwrap_or(current))?;
        let from = match range.since {
            None => (
                StateRef::Revision(shown.base),
                StateSummary {
                    kind: StateKind::Revision,
                    label: shown.base.to_string(),
                    base: None,
                    task: None,
                    worker: None,
                },
            ),
            Some(since) if since >= shown.version => {
                return Err(since_not_earlier(proposal, since, shown.version));
            }
            Some(since) => version_side(find(since)?, row.task),
        };
        Ok(Sides {
            version: shown.version,
            since: range.since,
            from,
            to: version_side(shown, row.task),
        })
    }
}

fn version_side(version: &VersionRow, task: TaskId) -> (StateRef, StateSummary) {
    (
        StateRef::Tree(version.tree),
        StateSummary {
            kind: StateKind::Changeset,
            label: version.head_changeset.to_string(),
            base: None,
            task: Some(task),
            worker: None,
        },
    )
}

fn since_not_earlier(proposal: ProposalId, since: u32, version: u32) -> ReviewError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!("since must be a version of {proposal} before v{version}"),
    )
    .with_details([
        ("field", json!("since")),
        ("since", json!(since)),
        ("version", json!(version)),
    ])
    .with_hint("pass an earlier version as since, such as the last_reviewed version of get_proposal, or leave it out for the full change")
    .into()
}
