use fold_core::object::ObjectHash;
use fold_core::{FoldError, ProposalId, ReviewBatchId, RevisionId, codes};
use fold_store::StoreError;
use fold_store::reviews::batches::{self, BatchState, MemberRelease, MemberRow};
use fold_store::reviews::{self, ProposalState};
use fold_store::workspaces;
use fold_vcs::compare::{FileChange, FileDiff, Totals};

use super::batches::{batch_not_found, merge_inputs};
use super::{ReviewError, ReviewService};
use crate::services::workspaces::StateRef;

/// What the merged change of a batch compares, or why it compares nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergedBasis {
    /// main's head `base` with the members merged onto it in merge order, built in memory.
    Merged { base: RevisionId },
    /// The revisions the batch's integration published together: from `base`, the revision before the first of them, to `head`, the last member's.
    Integrated { base: RevisionId, head: RevisionId },
    /// The batch has no open member to merge.
    NoMembers,
    /// Every member left the batch without a batch review.
    Closed,
    /// The batch's integration ended without publishing members together.
    NotTogether,
}

/// A member version the compared tree holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergedMember {
    pub proposal: ProposalId,
    pub version: u32,
}

/// The first member that does not merge onto main with the members before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedConflict {
    pub proposal: ProposalId,
    pub version: u32,
    /// The earlier member it conflicts with; `None` when it conflicts with main itself.
    pub with: Option<ProposalId>,
    /// The conflicted paths, in path order.
    pub paths: Vec<String>,
}

/// The change a batch's members make on main together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchMergedChange {
    pub batch: ReviewBatchId,
    pub basis: MergedBasis,
    /// The members the compared tree holds, in merge order.
    pub members: Vec<MergedMember>,
    pub conflict: Option<MergedConflict>,
    /// In path order.
    pub files: Vec<FileChange>,
    pub totals: Totals,
    /// For `Merged`, the hash of the merged tree, calculated in memory: the tree a batch checkout of the same members stores. `None` for every other basis.
    pub merged_tree: Option<ObjectHash>,
}

/// The two sides of a batch's merged change.
enum Sides {
    Merge {
        base: RevisionId,
        tree: ObjectHash,
        members: Vec<MemberRow>,
    },
    Revisions {
        base: RevisionId,
        head: RevisionId,
        members: Vec<MergedMember>,
    },
    Nothing(MergedBasis),
}

impl ReviewService {
    /// The files a batch changes on main with every member merged in, with line counts. Before the batch's integration it is main's head against the members merged onto it in merge order, by the merge a batch checkout is made with, so the view and the checkout hold the same tree; the tree is built in memory and nothing is written. The members are those still in the batch, or, once it is submitted, those its review covered, each at the version it joined at, while the proposal is open. A member that conflicts stops the merge: the change holds the members before it and names it. After the batch's integration it is the change of main from the revision before the members it published together to the last of them. A read: it writes nothing.
    pub async fn batch_merged_changes(
        &self,
        batch: ReviewBatchId,
    ) -> Result<BatchMergedChange, ReviewError> {
        let empty = |basis| BatchMergedChange {
            batch,
            basis,
            members: Vec::new(),
            conflict: None,
            files: Vec::new(),
            totals: Totals::default(),
            merged_tree: None,
        };
        match self.merged_sides(batch).await? {
            Sides::Nothing(basis) => Ok(empty(basis)),
            Sides::Revisions {
                base,
                head,
                members,
            } => {
                let comparison = self
                    .workspaces
                    .compare_states(StateRef::Revision(base), StateRef::Revision(head))
                    .await?;
                Ok(BatchMergedChange {
                    members,
                    files: comparison.files,
                    totals: comparison.totals,
                    ..empty(MergedBasis::Integrated { base, head })
                })
            }
            Sides::Merge {
                base,
                tree,
                members,
            } => {
                let comparison = self
                    .workspaces
                    .compare_batch_merge(tree, merge_inputs(&members))
                    .await?;
                let conflict = comparison.conflict.and_then(|conflict| {
                    let member = members.get(conflict.member)?;
                    Some(MergedConflict {
                        proposal: member.proposal,
                        version: member.version,
                        with: conflict
                            .with
                            .and_then(|index| members.get(index))
                            .map(|with| with.proposal),
                        paths: conflict
                            .conflicts
                            .iter()
                            .map(|c| String::from_utf8_lossy(&c.path).into_owned())
                            .collect(),
                    })
                });
                Ok(BatchMergedChange {
                    members: members
                        .iter()
                        .take(comparison.merged)
                        .map(|member| MergedMember {
                            proposal: member.proposal,
                            version: member.version,
                        })
                        .collect(),
                    conflict,
                    files: comparison.files,
                    totals: comparison.totals,
                    merged_tree: Some(comparison.tree),
                    ..empty(MergedBasis::Merged { base })
                })
            }
        }
    }

    /// The hunks of one file of [`ReviewService::batch_merged_changes`]. A path the change does not list is `path_not_in_comparison`. A read: it writes nothing.
    pub async fn batch_merged_file_diff(
        &self,
        batch: ReviewBatchId,
        path: &str,
    ) -> Result<FileDiff, ReviewError> {
        match self.merged_sides(batch).await? {
            Sides::Nothing(_) => Err(FoldError::new(
                codes::PATH_NOT_IN_COMPARISON,
                "the path is not a changed file of this comparison",
            )
            .with_details([("path", serde_json::json!(path))])
            .into()),
            Sides::Revisions { base, head, .. } => Ok(self
                .workspaces
                .diff_file(StateRef::Revision(base), StateRef::Revision(head), path)
                .await?),
            Sides::Merge { tree, members, .. } => Ok(self
                .workspaces
                .diff_file_batch_merge(tree, merge_inputs(&members), path)
                .await?),
        }
    }

    async fn merged_sides(&self, batch: ReviewBatchId) -> Result<Sides, ReviewError> {
        let read = self.store.read();
        let row = batches::get_batch(read, batch)
            .await?
            .ok_or_else(|| batch_not_found(batch))?;
        if batches::batch_integration(read, batch).await?.is_some() {
            let revisions = batches::batch_revisions(read, batch).await?;
            let (Some(first), Some(last)) = (revisions.first(), revisions.last()) else {
                return Ok(Sides::Nothing(MergedBasis::NotTogether));
            };
            let base = workspaces::get_by_number(read, first.revision)
                .await?
                .parent
                .ok_or(StoreError::Corrupt("revisions.parent_id"))?;
            return Ok(Sides::Revisions {
                base,
                head: last.revision,
                members: revisions
                    .iter()
                    .map(|row| MergedMember {
                        proposal: row.proposal,
                        version: row.version,
                    })
                    .collect(),
            });
        }
        if row.state == BatchState::Closed {
            return Ok(Sides::Nothing(MergedBasis::Closed));
        }
        let mut members = Vec::new();
        for member in batches::members(read, batch).await? {
            if !matches!(member.release, None | Some(MemberRelease::Submitted)) {
                continue;
            }
            let open = reviews::get_proposal(read, member.proposal)
                .await?
                .is_some_and(|proposal| proposal.state == ProposalState::Open);
            if open {
                members.push(member);
            }
        }
        if members.is_empty() {
            return Ok(Sides::Nothing(MergedBasis::NoMembers));
        }
        let head = workspaces::head(read).await?.ok_or_else(|| {
            FoldError::new(codes::REVISION_NOT_FOUND, "main has no revision")
                .with_hint("run `fld init` to create the first revision")
        })?;
        Ok(Sides::Merge {
            base: head.number,
            tree: head.tree_hash,
            members,
        })
    }
}
