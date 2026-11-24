use std::collections::HashSet;

use crate::ProposalId;
use crate::time::Timestamp;

/// One open proposal whose current version changes a path of a lane, with the time of its first approval on any of its versions; `None` while no version has an approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneMember {
    pub proposal: ProposalId,
    pub lane: String,
    pub first_approval: Option<Timestamp>,
}

/// Where a member stands in its lane. `position` counts from 1 at the front among the members that have an approval; a member without one has no position yet, holds no other member back and waits behind every member that has one, so a human who approves by integrating cannot pass the lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanePlace {
    pub proposal: ProposalId,
    pub lane: String,
    pub position: Option<u32>,
    /// How many members with a position are ahead of this one.
    pub ahead: u32,
}

impl LanePlace {
    /// Whether the member waits for another one: a member with a position is ahead of it.
    pub fn held(&self) -> bool {
        self.ahead > 0
    }

    /// The hold reason that integrate refusals, views and logs show, such as `lane migrations, position 2`; the position of a member without one is the next free place.
    pub fn reason(&self) -> Option<String> {
        self.held().then(|| {
            format!(
                "lane {}, position {}",
                self.lane,
                self.ahead.saturating_add(1)
            )
        })
    }
}

/// The places of `members`, one entry per proposal. Within a lane the members with an approval are ordered by their first approval and then by proposal number, so two approvals at the same millisecond still give one order; a first approval keeps its place across later versions and verdicts. The result is sorted by lane, then position with the members without one last, then proposal, so it never depends on the order of `members`. When a proposal is listed twice, its earliest entry by that order counts.
pub fn lane_places(members: &[LaneMember]) -> Vec<LanePlace> {
    let mut sorted: Vec<&LaneMember> = members.iter().collect();
    sorted.sort_by(|a, b| {
        a.lane
            .cmp(&b.lane)
            .then_with(|| a.first_approval.is_none().cmp(&b.first_approval.is_none()))
            .then_with(|| a.first_approval.cmp(&b.first_approval))
            .then_with(|| a.proposal.cmp(&b.proposal))
    });
    let mut seen = HashSet::new();
    let mut places = Vec::with_capacity(sorted.len());
    let mut lane: Option<&str> = None;
    let mut next = 0u32;
    for member in sorted {
        if !seen.insert(member.proposal) {
            continue;
        }
        if lane != Some(member.lane.as_str()) {
            lane = Some(member.lane.as_str());
            next = 0;
        }
        let ahead = next;
        let position = member.first_approval.map(|_| {
            next = next.saturating_add(1);
            next
        });
        places.push(LanePlace {
            proposal: member.proposal,
            lane: member.lane.clone(),
            position,
            ahead,
        });
    }
    places
}

/// The members with a position ahead of `proposal` in its lane, front first: every member with a position for a member without one. Empty for the front and for a proposal outside every lane.
pub fn ahead_of(places: &[LanePlace], proposal: ProposalId) -> Vec<ProposalId> {
    let Some(place) = places.iter().find(|place| place.proposal == proposal) else {
        return Vec::new();
    };
    places
        .iter()
        .filter(|other| {
            other.lane == place.lane
                && other
                    .position
                    .is_some_and(|ahead| place.position.is_none_or(|position| ahead < position))
        })
        .map(|other| other.proposal)
        .collect()
}

/// Whether a group that integrates together, such as the ready members of a review batch, must wait for its lanes: some member has a member ahead of it that is not in the group.
pub fn group_held(places: &[LanePlace], group: &[ProposalId]) -> bool {
    group.iter().any(|member| {
        ahead_of(places, *member)
            .iter()
            .any(|ahead| !group.contains(ahead))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn p(number: u32) -> ProposalId {
        ProposalId::from_number(number).unwrap()
    }

    fn at(millis: i64) -> Option<Timestamp> {
        Some(Timestamp::from_millis(millis).unwrap())
    }

    fn member(number: u32, lane: &str, first_approval: Option<Timestamp>) -> LaneMember {
        LaneMember {
            proposal: p(number),
            lane: lane.to_owned(),
            first_approval,
        }
    }

    fn positions(places: &[LanePlace]) -> Vec<(u32, &str, Option<u32>)> {
        places
            .iter()
            .map(|place| (place.proposal.number(), place.lane.as_str(), place.position))
            .collect()
    }

    #[test]
    fn members_line_up_by_their_first_approval() {
        let places = lane_places(&[
            member(3, "migrations", at(10)),
            member(1, "migrations", at(30)),
            member(2, "migrations", at(20)),
            member(4, "migrations", None),
            member(5, "lockfiles", at(40)),
        ]);
        assert_eq!(
            positions(&places),
            [
                (5, "lockfiles", Some(1)),
                (3, "migrations", Some(1)),
                (2, "migrations", Some(2)),
                (1, "migrations", Some(3)),
                (4, "migrations", None),
            ]
        );
        assert!(!places[0].held());
        assert!(!places[1].held());
        assert!(places[2].held());
        assert!(
            places[4].held(),
            "a member without an approval waits behind the others"
        );
        assert_eq!(places[1].reason(), None);
        assert_eq!(
            places[3].reason().as_deref(),
            Some("lane migrations, position 3")
        );
        assert_eq!(
            places[4].reason().as_deref(),
            Some("lane migrations, position 4")
        );
        let unapproved = lane_places(&[member(1, "m", None), member(2, "m", None)]);
        assert!(
            unapproved.iter().all(|place| !place.held()),
            "nobody is ahead"
        );
    }

    #[test]
    fn the_same_approval_time_is_ordered_by_proposal_number() {
        let places = lane_places(&[member(9, "m", at(5)), member(2, "m", at(5))]);
        assert_eq!(positions(&places), [(2, "m", Some(1)), (9, "m", Some(2))]);
    }

    #[test]
    fn a_proposal_listed_twice_keeps_its_earliest_place() {
        let places = lane_places(&[
            member(1, "m", at(50)),
            member(2, "m", at(20)),
            member(1, "m", at(10)),
        ]);
        assert_eq!(positions(&places), [(1, "m", Some(1)), (2, "m", Some(2))]);
    }

    #[test]
    fn only_members_ahead_in_the_same_lane_hold_a_member() {
        let places = lane_places(&[
            member(1, "m", at(10)),
            member(2, "m", at(20)),
            member(3, "m", at(30)),
            member(4, "l", at(5)),
            member(5, "m", None),
        ]);
        assert_eq!(ahead_of(&places, p(3)), [p(1), p(2)]);
        assert!(ahead_of(&places, p(1)).is_empty());
        assert!(ahead_of(&places, p(4)).is_empty());
        assert_eq!(ahead_of(&places, p(5)), [p(1), p(2), p(3)]);
        assert!(ahead_of(&places, p(77)).is_empty(), "outside every lane");
        assert!(
            !group_held(&places, &[p(1), p(2)]),
            "the group holds the front"
        );
        assert!(
            group_held(&places, &[p(2), p(4)]),
            "p(1) is ahead and outside"
        );
        assert!(!group_held(&places, &[p(4), p(77)]));
        assert!(!group_held(&places, &[]));
    }

    fn arbitrary_members() -> impl Strategy<Value = Vec<LaneMember>> {
        proptest::collection::btree_map(1u32..40, (0usize..3, proptest::option::of(0i64..6)), 0..20)
            .prop_map(|chosen| {
                chosen
                    .into_iter()
                    .map(|(number, (lane, time))| {
                        member(number, ["a", "b", "c"][lane], time.and_then(at))
                    })
                    .collect()
            })
    }

    proptest! {
        #[test]
        fn the_order_does_not_depend_on_the_input_order(
            members in arbitrary_members(),
            seed in any::<u64>(),
        ) {
            let mut shuffled = members.clone();
                        let len = shuffled.len().max(1);
            shuffled.rotate_left(usize::try_from(seed).unwrap_or(0) % len);
            if seed % 2 == 0 {
                shuffled.reverse();
            }
            prop_assert_eq!(lane_places(&members), lane_places(&shuffled));
        }

        #[test]
        fn each_lane_has_one_total_order_from_one(members in arbitrary_members()) {
            let places = lane_places(&members);
            prop_assert_eq!(places.len(), members.len());
            for lane in ["a", "b", "c"] {
                let of_lane: Vec<&LanePlace> = places.iter().filter(|place| place.lane == lane).collect();
                let numbered: Vec<u32> = of_lane.iter().filter_map(|place| place.position).collect();
                let expected: Vec<u32> = (1..=u32::try_from(numbered.len()).unwrap()).collect();
                prop_assert_eq!(&numbered, &expected);
                let approved = u32::try_from(numbered.len()).unwrap();
                for place in &of_lane {
                    prop_assert_eq!(place.ahead, place.position.map_or(approved, |position| position - 1));
                }
                let fronts = of_lane.iter().filter(|place| place.position == Some(1)).count();
                prop_assert!(fronts <= 1);
                for place in &of_lane {
                    let source = members.iter().find(|m| m.proposal == place.proposal).unwrap();
                    prop_assert_eq!(place.position.is_some(), source.first_approval.is_some());
                    for other in &of_lane {
                        let other_source = members.iter().find(|m| m.proposal == other.proposal).unwrap();
                        if let (Some(mine), Some(theirs)) = (place.position, other.position)
                            && mine < theirs
                        {
                            prop_assert!(
                                (source.first_approval, source.proposal)
                                    < (other_source.first_approval, other_source.proposal)
                            );
                        }
                    }
                }
            }
        }
    }
}
