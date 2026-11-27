use fold_core::object::ObjectHash;
use fold_core::reviews::Verdict;
use fold_core::reviews::checks::{ChecksFile, Lane};
use fold_core::reviews::lanes::{LaneMember, LanePlace, lane_places};
use fold_core::{FoldError, HumanName, ProposalId, codes};
use fold_store::reviews::{self, ProposalState};
use fold_store::{StoreError, workspaces};
use serde_json::json;

use super::conflicts::{ConflictedReason, conflicted_reason};
use super::queue::IntegrationQueue;
use super::{ProposalFilter, ProposalListing, ProposalSummary, ReviewError, ReviewService};

impl ReviewService {
    /// The lanes of main's own `fold-checks.toml`; empty when main has no checks file, an invalid one or no `[[serial]]` table. Every member is placed by main's lanes, so one definition orders a lane whatever a proposal's own copy of the file says.
    async fn main_lanes(&self) -> Result<Option<ChecksFile>, ReviewError> {
        let head = workspaces::head(self.store.read())
            .await?
            .ok_or(StoreError::Corrupt("refs.main"))?;
        let input = self
            .workspaces
            .checks_input(head.tree_hash, head.tree_hash)
            .await?;
        let file = input
            .file
            .and_then(|text| ChecksFile::parse(&text).ok())
            .filter(|file| !file.lanes.is_empty());
        Ok(file)
    }

    /// The place of every open proposal whose current version changes a path of one of main's lanes, from the paths the version changes against its own base and the time of the proposal's first approval on any version.
    pub(crate) async fn lane_places(&self) -> Result<Vec<LanePlace>, ReviewError> {
        let Some(file) = self.main_lanes().await? else {
            return Ok(Vec::new());
        };
        let read = self.store.read();
        let open = reviews::list_proposals(read, Some(ProposalState::Open), None).await?;
        let mut members = Vec::new();
        for row in open {
            let Some(target) = reviews::review_target(read, row.id).await? else {
                continue;
            };
            let Some(lane) = self
                .lane_of_change(&file, target.base_tree, target.tree)
                .await?
            else {
                continue;
            };
            let first_approval = reviews::list_reviews(read, row.id)
                .await?
                .into_iter()
                .filter(|review| review.verdict == Verdict::Approve)
                .map(|review| review.created_at)
                .min();
            members.push(LaneMember {
                proposal: row.id,
                lane: lane.name.clone(),
                first_approval,
            });
        }
        Ok(lane_places(&members))
    }

    async fn lane_of_change<'a>(
        &self,
        file: &'a ChecksFile,
        base: ObjectHash,
        tree: ObjectHash,
    ) -> Result<Option<&'a Lane>, ReviewError> {
        let changed = self.workspaces.checks_input(base, tree).await?.changed;
        Ok(file.lane_of(&changed))
    }

    /// The place of `proposal` when it waits behind another member of its lane.
    pub(super) async fn lane_hold(
        &self,
        proposal: ProposalId,
    ) -> Result<Option<LanePlace>, ReviewError> {
        Ok(self
            .lane_places()
            .await?
            .into_iter()
            .find(|place| place.proposal == proposal && place.held()))
    }

    /// The lane of `proposal` when main moved in it since the version's base: the version changes a path of a lane of main, and main's head `head_tree` differs from the version's base in a path of that lane, such as a member that integrated first. `None` for a proposal outside every lane and for a lane main did not move in.
    pub(super) async fn moved_lane(
        &self,
        proposal: ProposalId,
        head_tree: ObjectHash,
    ) -> Result<Option<String>, ReviewError> {
        let Some(file) = self.main_lanes().await? else {
            return Ok(None);
        };
        let Some(target) = reviews::review_target(self.store.read(), proposal).await? else {
            return Ok(None);
        };
        let Some(lane) = self
            .lane_of_change(&file, target.base_tree, target.tree)
            .await?
        else {
            return Ok(None);
        };
        let moved = self
            .workspaces
            .checks_input(target.base_tree, head_tree)
            .await?
            .changed;
        Ok(moved
            .iter()
            .any(|path| lane.matches(path))
            .then(|| lane.name.clone()))
    }
}

impl ReviewService {
    /// The proposals of `filter` with the place of each lane member among all open proposals and the automatic integration queue.
    pub async fn list_proposals_in_lanes(
        &self,
        filter: ProposalFilter,
    ) -> Result<(Vec<ProposalSummary>, Vec<LanePlace>, IntegrationQueue), ReviewError> {
        let proposals = self.list_proposals(filter).await?;
        Ok((
            proposals,
            self.lane_places().await?,
            self.integration_queue().await?,
        ))
    }

    /// [`ReviewService::list_proposals_for`] with the place of each lane member among all open proposals and the reason of each listed open proposal flagged `conflicted`.
    pub async fn list_proposals_in_lanes_for(
        &self,
        filter: ProposalFilter,
        human: Option<&HumanName>,
    ) -> Result<LaneListing, ReviewError> {
        let listing = self.list_proposals_for(filter, human).await?;
        let lanes = self.lane_places().await?;
        let mut read = self.store.read().begin().await.map_err(StoreError::from)?;
        let mut conflicted = Vec::new();
        for summary in &listing.proposals {
            if summary.state != ProposalState::Open || !summary.conflicted {
                continue;
            }
            if let Some(reason) =
                conflicted_reason(&mut read, summary.id, summary.current_version).await?
            {
                conflicted.push((summary.id, reason));
            }
        }
        read.commit().await.map_err(StoreError::from)?;
        let queue = self.integration_queue().await?;
        Ok(LaneListing {
            listing,
            lanes,
            conflicted,
            queue,
        })
    }

    /// The places of the open lane members that wait for a reviewer's fix on main: flagged with a conflict, or with failed integration checks after their lane moved.
    pub async fn lane_fixes(&self) -> Result<Vec<LanePlace>, ReviewError> {
        let places = self.lane_places().await?;
        let mut fixes = Vec::new();
        for place in places {
            let row = reviews::get_proposal(self.store.read(), place.proposal).await?;
            if row.is_some_and(|row| row.state == ProposalState::Open && row.conflicted) {
                fixes.push(place);
            }
        }
        Ok(fixes)
    }
}

/// A proposal listing with what the lanes and the handoffs add to it.
#[derive(Debug, Clone)]
pub struct LaneListing {
    pub listing: ProposalListing,
    /// The place of every open lane member, listed or not.
    pub lanes: Vec<LanePlace>,
    /// The reason of each listed open proposal flagged `conflicted`.
    pub conflicted: Vec<(ProposalId, ConflictedReason)>,
    /// The automatic integration queue of the whole project, listed or not.
    pub queue: IntegrationQueue,
}

/// The refusal of an integration of a lane member behind the front: `invalid_request` with the reason `lane_held`, the lane and the position.
pub(super) fn lane_held_refusal(place: &LanePlace) -> FoldError {
    let reason = place.reason().unwrap_or_default();
    FoldError::new(
        codes::INVALID_REQUEST,
        format!(
            "{} waits for its turn in {reason}, so fold did not integrate it",
            place.proposal
        ),
    )
    .with_details([
        ("reason", json!("lane_held")),
        ("proposal", json!(place.proposal.to_string())),
        ("lane", json!(place.lane)),
        ("position", json!(place.position)),
    ])
    .with_hint("the members of a lane integrate one at a time in the order of their first approval; fold integrates this one when the members ahead of it are integrated or closed")
}

/// The note on a conflict or a failed integration check of a lane member, for the reviewer that fixes it and for the review lead.
pub(crate) fn lane_moved_note(lane: &str) -> String {
    format!(
        "Lane {lane} moved: another member of the lane integrated after this version was made, so fix it on the new main."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refusal_names_the_lane_and_the_position() {
        let place = LanePlace {
            proposal: ProposalId::from_number(7).unwrap(),
            lane: "migrations".to_owned(),
            position: Some(2),
            ahead: 1,
        };
        let error = lane_held_refusal(&place);
        assert_eq!(error.code(), codes::INVALID_REQUEST);
        assert!(
            error
                .message()
                .contains("P-7 waits for its turn in lane migrations, position 2")
        );
        let details = serde_json::to_value(error.details()).unwrap();
        assert_eq!(details["reason"], "lane_held");
        assert_eq!(details["position"], 2);
        assert!(lane_moved_note("migrations").starts_with("Lane migrations moved"));
    }
}
