use std::collections::BTreeSet;

use fold_core::object::ObjectHash;
use fold_core::reviews::last_verdict_version;
use fold_core::{FoldError, ProposalId, TaskId, codes};
use fold_store::StoreError;
use fold_store::reviews::{self, VersionRow};
use fold_store::workspaces as revisions;
use fold_vcs::compare::{FileDiff, Totals};
use fold_vcs::paths::RepoPath;
use serde_json::json;

use super::proposals::{proposal_not_found, version_not_found};
use super::{ReviewError, ReviewService};
use crate::services::workspaces::{Carry, StateComparison, StateKind, StateRef, StateSummary};

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

/// The earlier version's own change and how the base moved under the later one: the facts from which fold suggests a review of the change since that version or a full review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinceChange {
    /// What the version changed after the earlier one, with the earlier version carried onto the version's base when the base moved, so main's own changes stay out of it.
    pub delta: StateComparison,
    /// The earlier version's change against its own base.
    pub earlier: Totals,
    /// The paths of `delta` that the earlier version did not change against its base, in path order.
    pub new_files: Vec<String>,
    /// The version has another base revision than the earlier one, as after a sync with a newer main.
    pub base_moved: bool,
    /// Where the earlier version's change conflicts with what main gained between the two bases, in path order: the files where the worker merged main by hand. Each shows its change from the earlier version's file.
    pub base_conflicts: Vec<String>,
}

/// The two sides a change compares, and how each side is named to callers.
struct Sides {
    version: u32,
    since: Option<u32>,
    /// The version `since` names.
    earlier: Option<VersionRow>,
    from: FromSide,
    from_summary: StateSummary,
    to: ObjectHash,
    to_summary: StateSummary,
}

/// The earlier side of a change: a stored state, or an earlier version carried onto a later base.
enum FromSide {
    Stored(StateRef),
    Carried(Carry),
}

impl ReviewService {
    /// The files a proposal version changes against its base, or against the earlier version `since`, with line counts. The earlier side is the base revision's tree, the earlier version's tree, or, when the version's base is another revision than the earlier version's, the earlier version carried onto the version's base with the three-way merge, so the change holds only what the worker changed and not what main gained. A version side is named by its changeset. A read: it writes nothing.
    pub async fn proposal_changes(
        &self,
        proposal: ProposalId,
        range: ChangeRange,
    ) -> Result<ProposalChange<StateComparison>, ReviewError> {
        let sides = self.change_sides(proposal, range).await?;
        let (comparison, _) = self.compare_sides(&sides).await?;
        Ok(ProposalChange {
            version: sides.version,
            since: sides.since,
            change: comparison,
        })
    }

    /// [`ReviewService::proposal_changes`] since the earlier version `since`, with the facts that tell how far that change reaches. A read: it writes nothing.
    pub async fn proposal_since_change(
        &self,
        proposal: ProposalId,
        version: Option<u32>,
        since: u32,
    ) -> Result<ProposalChange<SinceChange>, ReviewError> {
        let range = ChangeRange {
            version,
            since: Some(since),
        };
        let sides = self.change_sides(proposal, range).await?;
        let (delta, conflicts) = self.compare_sides(&sides).await?;
        let earlier_row = sides.earlier.as_ref().ok_or(StoreError::Invalid(
            "a change since a version has that version",
        ))?;
        let earlier = self
            .workspaces
            .compare_states(
                StateRef::Revision(earlier_row.base),
                StateRef::Tree(earlier_row.tree),
            )
            .await?;
        let touched: BTreeSet<&RepoPath> = earlier
            .files
            .iter()
            .flat_map(|file| std::iter::once(&file.path).chain(file.old_path.as_ref()))
            .collect();
        let new_files = delta
            .files
            .iter()
            .filter(|file| {
                !touched.contains(&file.path)
                    && file
                        .old_path
                        .as_ref()
                        .is_none_or(|old| !touched.contains(old))
            })
            .map(|file| file.path.to_string())
            .collect();
        Ok(ProposalChange {
            version: sides.version,
            since: sides.since,
            change: SinceChange {
                delta,
                earlier: earlier.totals,
                new_files,
                base_moved: matches!(sides.from, FromSide::Carried(_)),
                base_conflicts: conflicts.iter().map(ToString::to_string).collect(),
            },
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
        let diff = match sides.from {
            FromSide::Stored(from) => {
                self.workspaces
                    .diff_file(from, StateRef::Tree(sides.to), path)
                    .await?
            }
            FromSide::Carried(carry) => {
                self.workspaces
                    .diff_file_carried(carry, sides.to, path)
                    .await?
            }
        };
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

    /// The comparison of the two sides and, for a carried earlier version, the paths where carrying it conflicted.
    async fn compare_sides(
        &self,
        sides: &Sides,
    ) -> Result<(StateComparison, Vec<RepoPath>), ReviewError> {
        let (from, to) = (sides.from_summary.clone(), sides.to_summary.clone());
        Ok(match sides.from {
            FromSide::Stored(state) => {
                let comparison = self
                    .workspaces
                    .compare_states(state, StateRef::Tree(sides.to))
                    .await?;
                (
                    StateComparison {
                        from,
                        to,
                        ..comparison
                    },
                    Vec::new(),
                )
            }
            FromSide::Carried(carry) => {
                let carried = self.workspaces.compare_carried(carry, sides.to).await?;
                let comparison = StateComparison {
                    from,
                    to,
                    files: carried.files,
                    totals: carried.totals,
                    warnings: Vec::new(),
                };
                (comparison, carried.conflicts)
            }
        })
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
        let mut earlier_row = None;
        let (from, from_summary) = match range.since {
            None => (
                FromSide::Stored(StateRef::Revision(shown.base)),
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
            Some(since) => {
                let earlier = find(since)?;
                let from = if earlier.base == shown.base {
                    FromSide::Stored(StateRef::Tree(earlier.tree))
                } else {
                    FromSide::Carried(Carry {
                        base: revisions::get_by_number(read, earlier.base)
                            .await?
                            .tree_hash,
                        tree: earlier.tree,
                        onto: revisions::get_by_number(read, shown.base).await?.tree_hash,
                    })
                };
                earlier_row = Some(earlier.clone());
                (from, version_side(earlier, row.task))
            }
        };
        Ok(Sides {
            version: shown.version,
            since: range.since,
            earlier: earlier_row,
            from,
            from_summary,
            to: shown.tree,
            to_summary: version_side(shown, row.task),
        })
    }
}

fn version_side(version: &VersionRow, task: TaskId) -> StateSummary {
    StateSummary {
        kind: StateKind::Changeset,
        label: version.head_changeset.to_string(),
        base: None,
        task: Some(task),
        worker: None,
    }
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
