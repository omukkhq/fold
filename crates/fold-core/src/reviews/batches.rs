use std::collections::BTreeSet;
use std::time::Duration;

use crate::ids::{ProposalId, TaskId};
use crate::policy::GateRefusal;
use crate::reviews::Verdict;
use crate::tasks::define_names;
use crate::time::Timestamp;

/// How long a submitted batch waits for members without a final state, counted from the submission of its review. After it, those members leave the batch's turn, so the approved members integrate together and the others later on their own rules.
pub const BATCH_WAIT: Duration = Duration::from_secs(60 * 60);

/// The latest submission time whose batch's wait has ended at `now`; `None` only at the start of time.
pub fn batch_wait_cutoff(now: Timestamp) -> Option<Timestamp> {
    now.checked_sub(BATCH_WAIT)
}

define_names!(
    /// What fold recorded about a submitted batch, once each. `wait_ended` is the end of the batch's wait after [`BATCH_WAIT`], which turns its members without a final state out of its turn. `ready` means the review lead and the holder know the batch is ready: it was ready when the holder submitted its review, or fold told them when it became ready later.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum BatchMark {
        WaitEnded = "wait_ended",
        Ready = "ready",
    }
);

/// Where one member of a submitted review batch stands for the batch's integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberStanding {
    /// The member left the batch's integration: it integrates later on its own rules.
    Out,
    /// The member's final state for the batch is not known yet, so the batch waits for it.
    Pending,
    /// Every gate but the prerequisites is met for the automatic caller. `waits_for` are the prerequisites that are not Done; the member integrates with the batch only when each of them is the task of another member that integrates with it.
    Approved { waits_for: Vec<TaskId> },
}

/// What decides a member's standing, as one read of the store sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingFacts {
    /// The holder took the member out of the batch before its review.
    pub removed: bool,
    /// The proposal is integrated, closed or rejected.
    pub ended: bool,
    /// An integration of the current version conflicted or failed its integration checks, so the proposal is with its author.
    pub flagged: bool,
    /// A version from the one the member joined at got changes requested, so the member went back to its author. A later version of it is reviewed on its own rules.
    pub sent_back: bool,
    /// The integration gates for the automatic caller on the current version.
    pub gates: Result<(), GateRefusal>,
}

/// The standing of one member: out once it left, ended or went back to its author, approved while the gates for the automatic caller pass but for prerequisites, out when it waits for a human, and pending while it waits for a verdict, such as on a reviewer's fix.
pub fn member_standing(facts: &StandingFacts) -> MemberStanding {
    if facts.removed || facts.ended || facts.flagged || facts.sent_back {
        return MemberStanding::Out;
    }
    match &facts.gates {
        Ok(()) => MemberStanding::Approved {
            waits_for: Vec::new(),
        },
        Err(GateRefusal::PrerequisitesNotDone { unfinished }) => MemberStanding::Approved {
            waits_for: unfinished.iter().map(|(task, _)| *task).collect(),
        },
        Err(GateRefusal::NotApproved) => MemberStanding::Pending,
        Err(
            GateRefusal::ProposalNotOpen
            | GateRefusal::ChangesRequested
            | GateRefusal::HumanApprovalMissing { .. }
            | GateRefusal::AgentApprovalRequired,
        ) => MemberStanding::Out,
        // The automatic caller names no version and is no agent, so these never refuse it.
        Err(
            GateRefusal::StaleVersion { .. }
            | GateRefusal::IntegratorRequired
            | GateRefusal::AuthorCannotIntegrate,
        ) => MemberStanding::Pending,
    }
}

/// Whether a pending member waits only because its current version got comments and no verdict that decides: its reviewer must approve or request changes before the batch can integrate, so fold tells the review lead. `verdicts` are those on the member's current version.
pub fn waits_on_comment(standing: &MemberStanding, verdicts: &[Verdict]) -> bool {
    *standing == MemberStanding::Pending
        && !verdicts.is_empty()
        && verdicts.iter().all(|verdict| *verdict == Verdict::Comment)
}

/// One member as the batch decision reads it, in the batch's merge order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchMemberFacts {
    pub proposal: ProposalId,
    pub task: TaskId,
    pub standing: MemberStanding,
}

/// What a submitted batch's integration can do now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchReadiness {
    /// Some members have no final state yet; the approved members wait for them.
    Waiting { pending: Vec<ProposalId> },
    /// Every member has its final state. `members` integrate together in this order, each after the members it needs; it is empty when no member can.
    Final { members: Vec<ProposalId> },
}

impl BatchReadiness {
    /// Whether the batch holds `proposal` back from integrating on its own: a member that waits for the others, or one that integrates with them.
    pub fn holds(&self, members: &[BatchMemberFacts], proposal: ProposalId) -> bool {
        match self {
            BatchReadiness::Waiting { .. } => members.iter().any(|member| {
                member.proposal == proposal && member.standing != MemberStanding::Out
            }),
            BatchReadiness::Final { members } => members.contains(&proposal),
        }
    }
}

/// The batch decides once no member is pending, or once its wait ended (`wait_ended`), which leaves a pending member out of its turn. An approved member integrates with it when each prerequisite that is not Done is the task of another member that integrates with it, so a member that needs one that went back waits and is left out. The members come in dependency order, ties in the batch's own order.
pub fn batch_readiness(members: &[BatchMemberFacts], wait_ended: bool) -> BatchReadiness {
    let pending: Vec<ProposalId> = members
        .iter()
        .filter(|member| member.standing == MemberStanding::Pending)
        .map(|member| member.proposal)
        .collect();
    if !pending.is_empty() && !wait_ended {
        return BatchReadiness::Waiting { pending };
    }
    let mut ready: Vec<(&BatchMemberFacts, &[TaskId])> = members
        .iter()
        .filter_map(|member| match &member.standing {
            MemberStanding::Approved { waits_for } => Some((member, waits_for.as_slice())),
            MemberStanding::Out | MemberStanding::Pending => None,
        })
        .collect();
    loop {
        let tasks: BTreeSet<TaskId> = ready.iter().map(|(member, _)| member.task).collect();
        let before = ready.len();
        ready.retain(|(_, waits_for)| waits_for.iter().all(|task| tasks.contains(task)));
        if ready.len() == before {
            break;
        }
    }
    let mut placed: BTreeSet<TaskId> = BTreeSet::new();
    let mut order = Vec::with_capacity(ready.len());
    while !ready.is_empty() {
        let Some(next) = ready
            .iter()
            .position(|(_, waits_for)| waits_for.iter().all(|task| placed.contains(task)))
        else {
            // A cycle among members cannot pass the task graph's rules; none of them integrates together.
            break;
        };
        let (member, _) = ready.remove(next);
        placed.insert(member.task);
        order.push(member.proposal);
    }
    BatchReadiness::Final { members: order }
}

define_names!(
    /// How the integration turn of a submitted batch ended. `together` published its members as one integration after one check run; `one_at_a_time` followed a failed run, each member with its own; `none` found no member to integrate.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum BatchOutcome {
        Together = "together",
        OneAtATime = "one_at_a_time",
        Nothing = "none",
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskStatus;

    fn p(number: u32) -> ProposalId {
        ProposalId::from_number(number).unwrap()
    }

    fn t(number: u32) -> TaskId {
        TaskId::from_number(number).unwrap()
    }

    fn member(number: u32, standing: MemberStanding) -> BatchMemberFacts {
        BatchMemberFacts {
            proposal: p(number),
            task: t(number),
            standing,
        }
    }

    fn approved(needs: &[u32]) -> MemberStanding {
        MemberStanding::Approved {
            waits_for: needs.iter().copied().map(t).collect(),
        }
    }

    fn facts(gates: Result<(), GateRefusal>) -> StandingFacts {
        StandingFacts {
            removed: false,
            ended: false,
            flagged: false,
            sent_back: false,
            gates,
        }
    }

    #[test]
    fn a_member_that_left_ended_or_went_back_is_out() {
        for facts in [
            StandingFacts {
                removed: true,
                ..facts(Ok(()))
            },
            StandingFacts {
                ended: true,
                ..facts(Ok(()))
            },
            StandingFacts {
                flagged: true,
                ..facts(Ok(()))
            },
            StandingFacts {
                sent_back: true,
                ..facts(Err(GateRefusal::NotApproved))
            },
        ] {
            assert_eq!(member_standing(&facts), MemberStanding::Out, "{facts:?}");
        }
    }

    #[test]
    fn the_gates_decide_the_standing_of_a_member_still_in() {
        assert_eq!(member_standing(&facts(Ok(()))), approved(&[]));
        let unfinished = vec![(t(4), TaskStatus::Review)];
        assert_eq!(
            member_standing(&facts(Err(GateRefusal::PrerequisitesNotDone {
                unfinished
            }))),
            approved(&[4])
        );
        assert_eq!(
            member_standing(&facts(Err(GateRefusal::NotApproved))),
            MemberStanding::Pending
        );
        assert_eq!(
            member_standing(&facts(Err(GateRefusal::IntegratorRequired))),
            MemberStanding::Pending
        );
        for out in [
            GateRefusal::ChangesRequested,
            GateRefusal::ProposalNotOpen,
            GateRefusal::HumanApprovalMissing { reviewer: None },
            GateRefusal::AgentApprovalRequired,
        ] {
            assert_eq!(member_standing(&facts(Err(out))), MemberStanding::Out);
        }
    }

    #[test]
    fn the_batch_waits_while_a_member_is_pending() {
        let members = [
            member(1, approved(&[])),
            member(2, MemberStanding::Pending),
            member(3, MemberStanding::Out),
        ];
        let readiness = batch_readiness(&members, false);
        assert_eq!(
            readiness,
            BatchReadiness::Waiting {
                pending: vec![p(2)]
            }
        );
        assert!(readiness.holds(&members, p(1)));
        assert!(readiness.holds(&members, p(2)));
        assert!(!readiness.holds(&members, p(3)), "an out member is free");
    }

    #[test]
    fn once_the_wait_ended_a_pending_member_leaves_and_the_approved_ones_integrate() {
        let members = [
            member(1, approved(&[])),
            member(2, MemberStanding::Pending),
            member(3, approved(&[2])),
        ];
        let readiness = batch_readiness(&members, true);
        assert_eq!(
            readiness,
            BatchReadiness::Final {
                members: vec![p(1)]
            }
        );
        assert!(
            !readiness.holds(&members, p(2)),
            "the pending member is free"
        );
        assert!(
            !readiness.holds(&members, p(3)),
            "it needs the one that left"
        );
    }

    #[test]
    fn the_wait_ends_an_hour_after_the_submission() {
        let submitted = Timestamp::from_millis(10_000_000).unwrap();
        let hour_later = submitted.checked_add(BATCH_WAIT).unwrap();
        assert_eq!(batch_wait_cutoff(hour_later), Some(submitted));
        assert_eq!(BatchMark::WaitEnded.as_str(), "wait_ended");
        assert_eq!(BatchMark::parse("wait_ended"), Some(BatchMark::WaitEnded));
        assert_eq!(BatchMark::Ready.as_str(), "ready");
    }

    #[test]
    fn approved_members_integrate_together_and_one_that_needs_a_sent_back_member_waits() {
        // T-3 needs T-1, which went back; T-4 needs T-2, which integrates with it.
        let members = [
            member(1, MemberStanding::Out),
            member(2, approved(&[])),
            member(3, approved(&[1])),
            member(4, approved(&[2])),
            member(5, approved(&[])),
        ];
        let readiness = batch_readiness(&members, false);
        assert_eq!(
            readiness,
            BatchReadiness::Final {
                members: vec![p(2), p(4), p(5)]
            }
        );
        assert!(!readiness.holds(&members, p(3)));
        assert!(readiness.holds(&members, p(4)));
    }

    #[test]
    fn a_member_comes_after_the_members_it_needs_whatever_its_place() {
        let members = [
            member(1, approved(&[3])),
            member(2, approved(&[])),
            member(3, approved(&[2])),
        ];
        assert_eq!(
            batch_readiness(&members, false),
            BatchReadiness::Final {
                members: vec![p(2), p(3), p(1)]
            }
        );
    }

    #[test]
    fn a_wait_outside_the_batch_or_a_cycle_leaves_the_members_out() {
        let outside = [member(1, approved(&[9])), member(2, approved(&[1]))];
        assert_eq!(
            batch_readiness(&outside, false),
            BatchReadiness::Final { members: vec![] }
        );
        let cycle = [member(1, approved(&[2])), member(2, approved(&[1]))];
        assert_eq!(
            batch_readiness(&cycle, false),
            BatchReadiness::Final { members: vec![] }
        );
    }

    #[test]
    fn only_a_pending_member_with_nothing_but_comments_waits_on_a_comment() {
        let pending = MemberStanding::Pending;
        assert!(waits_on_comment(&pending, &[Verdict::Comment]));
        assert!(waits_on_comment(
            &pending,
            &[Verdict::Comment, Verdict::Comment]
        ));
        assert!(!waits_on_comment(&pending, &[]), "no review yet");
        assert!(!waits_on_comment(
            &pending,
            &[Verdict::Comment, Verdict::RequestChanges]
        ));
        assert!(!waits_on_comment(&approved(&[]), &[Verdict::Comment]));
        assert!(!waits_on_comment(&MemberStanding::Out, &[Verdict::Comment]));
    }

    #[test]
    fn outcomes_have_stable_names() {
        assert_eq!(BatchOutcome::Together.as_str(), "together");
        assert_eq!(BatchOutcome::OneAtATime.as_str(), "one_at_a_time");
        assert_eq!(BatchOutcome::Nothing.as_str(), "none");
        assert_eq!(BatchOutcome::parse("none"), Some(BatchOutcome::Nothing));
    }
}
