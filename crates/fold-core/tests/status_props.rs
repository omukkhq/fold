use std::cmp::Reverse;
use std::collections::BTreeSet;

use fold_core::readiness::{Candidate, ready_order};
use fold_core::status::{ProposalFact, TaskFacts, TaskStatus, calculate};
use fold_core::{TaskId, TaskKind, Timestamp};
use proptest::prelude::*;

#[expect(
    clippy::unwrap_used,
    reason = "a helper outside #[test] in a test crate; the number is never zero"
)]
fn t(n: u32) -> TaskId {
    TaskId::from_number(n).unwrap()
}

#[expect(
    clippy::unwrap_used,
    reason = "a helper outside #[test] in a test crate; the millis are in range"
)]
fn ts(ms: i64) -> Timestamp {
    Timestamp::from_millis(ms).unwrap()
}

fn status() -> impl Strategy<Value = TaskStatus> {
    prop_oneof![
        Just(TaskStatus::Backlog),
        Just(TaskStatus::Todo),
        Just(TaskStatus::InProgress),
        Just(TaskStatus::Review),
        Just(TaskStatus::Done),
    ]
}

fn proposal() -> impl Strategy<Value = ProposalFact> {
    prop_oneof![
        Just(ProposalFact::None),
        Just(ProposalFact::WithReviewer),
        Just(ProposalFact::WithAuthor),
    ]
}

/// Distinct prerequisites in an arbitrary order, as a loader may return them.
fn prerequisites() -> impl Strategy<Value = Vec<(TaskId, TaskStatus)>> {
    prop::collection::btree_map(1u32..30, status(), 0..8)
        .prop_map(|m| m.into_iter().map(|(n, s)| (t(n), s)).collect::<Vec<_>>())
        .prop_shuffle()
}

fn kind() -> impl Strategy<Value = TaskKind> {
    prop_oneof![
        Just(TaskKind::Proposal),
        Just(TaskKind::NoProposal),
        Just(TaskKind::Human)
    ]
}

fn facts() -> impl Strategy<Value = TaskFacts> {
    (
        kind(),
        any::<bool>(),
        proposal(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        prerequisites(),
    )
        .prop_map(
            |(
                kind,
                completed,
                proposal,
                has_active_workspace,
                claimed,
                reported,
                prerequisites,
            )| {
                TaskFacts {
                    kind,
                    completed,
                    proposal,
                    has_active_workspace,
                    claimed,
                    reported,
                    prerequisites,
                }
            },
        )
}

fn candidates() -> impl Strategy<Value = Vec<Candidate>> {
    prop::collection::btree_map(1u32..60, (0usize..4, 0i64..5), 0..20).prop_map(|raw| {
        raw.into_iter()
            .map(|(n, (u, ms))| Candidate {
                task: t(n),
                unblock_count: u,
                created_at: ts(ms),
            })
            .collect()
    })
}

proptest! {
    #[test]
    fn done_iff_completed(f in facts()) {
        prop_assert_eq!(calculate(&f).status == TaskStatus::Done, f.completed);
    }

    #[test]
    fn untouched_task_without_unfinished_prerequisites_is_todo(
        prereqs in prop::collection::btree_map(1u32..30, Just(TaskStatus::Done), 0..8),
    ) {
        for kind in TaskKind::ALL {
            let f = TaskFacts {
                kind,
                completed: false,
                proposal: ProposalFact::None,
                has_active_workspace: false,
                claimed: false,
                reported: false,
                prerequisites: prereqs.iter().map(|(n, s)| (t(*n), *s)).collect(),
            };
            prop_assert_eq!(calculate(&f).status, TaskStatus::Todo);
        }
    }

    #[test]
    fn a_kind_never_reads_the_facts_of_the_other_kind(
        f in facts(),
        proposal in proposal(),
        workspace in any::<bool>(),
        claimed in any::<bool>(),
        reported in any::<bool>(),
    ) {
        let mut other = f.clone();
        match f.kind {
            TaskKind::Proposal => {
                other.claimed = claimed;
                other.reported = reported;
            }
            TaskKind::NoProposal => {
                other.proposal = proposal;
                other.has_active_workspace = workspace;
            }
            TaskKind::Human => {
                other.proposal = proposal;
                other.has_active_workspace = workspace;
                other.claimed = claimed;
                other.reported = reported;
            }
        }
        prop_assert_eq!(calculate(&other), calculate(&f));
    }

    #[test]
    fn a_human_task_moves_only_by_its_completion_and_prerequisites(mut f in facts()) {
        f.kind = TaskKind::Human;
        let calculated = calculate(&f);
        let expected = if f.completed {
            TaskStatus::Done
        } else if f.prerequisites.iter().all(|(_, s)| *s == TaskStatus::Done) {
            TaskStatus::Todo
        } else {
            TaskStatus::Backlog
        };
        prop_assert_eq!(calculated.status, expected);
        prop_assert!(!calculated.blocked());
    }

    #[test]
    fn an_open_report_puts_an_unfinished_task_without_a_proposal_in_review(mut f in facts()) {
        f.kind = TaskKind::NoProposal;
        prop_assume!(!f.completed);
        let status = calculate(&f).status;
        prop_assert_eq!(status == TaskStatus::Review, f.reported);
        prop_assert_eq!(status == TaskStatus::InProgress, f.claimed && !f.reported);
    }

    #[test]
    fn blocked_by_is_a_sorted_subset_of_unfinished_prerequisites(f in facts()) {
        let c = calculate(&f);
        let unfinished: BTreeSet<TaskId> = f
            .prerequisites
            .iter()
            .filter(|(_, s)| *s != TaskStatus::Done)
            .map(|(id, _)| *id)
            .collect();
        prop_assert!(c.blocked_by.iter().all(|id| unfinished.contains(id)));
        prop_assert!(c.blocked_by.windows(2).all(|w| w[0] < w[1]));
        if !matches!(c.status, TaskStatus::InProgress | TaskStatus::Review) {
            prop_assert!(c.blocked_by.is_empty());
        } else {
            prop_assert_eq!(c.blocked_by.len(), unfinished.len());
        }
    }

    #[test]
    fn prerequisite_order_does_not_change_the_result(f in facts()) {
        let mut sorted = f.clone();
        sorted.prerequisites.sort();
        prop_assert_eq!(calculate(&sorted), calculate(&f));
    }

    #[test]
    fn finishing_a_prerequisite_never_moves_a_task_back(
        f in facts(),
        pick in any::<prop::sample::Index>(),
    ) {
        let open: Vec<usize> = f
            .prerequisites
            .iter()
            .enumerate()
            .filter(|(_, (_, s))| *s != TaskStatus::Done)
            .map(|(i, _)| i)
            .collect();
        prop_assume!(!open.is_empty());
        let i = open[pick.index(open.len())];
        let finished_id = f.prerequisites[i].0;
        let mut after = f.clone();
        after.prerequisites[i].1 = TaskStatus::Done;

        let (b, a) = (calculate(&f), calculate(&after));
        prop_assert!(
            a.status == b.status || (b.status, a.status) == (TaskStatus::Backlog, TaskStatus::Todo)
        );
        let expected: Vec<TaskId> =
            b.blocked_by.into_iter().filter(|id| *id != finished_id).collect();
        prop_assert_eq!(a.blocked_by, expected);
    }

    #[test]
    fn ready_order_is_a_sorted_stable_permutation(
        (candidates, shuffled) in candidates()
            .prop_flat_map(|c| (Just(c.clone()), Just(c).prop_shuffle())),
    ) {
        let order = ready_order(&candidates);

        let mut sorted_ids: Vec<TaskId> = candidates.iter().map(|c| c.task).collect();
        let mut got_ids = order.clone();
        sorted_ids.sort();
        got_ids.sort();
        prop_assert_eq!(got_ids, sorted_ids);

        let key = |id: &TaskId| {
            candidates
                .iter()
                .find(|c| c.task == *id)
                .map(|c| (Reverse(c.unblock_count), c.created_at, c.task))
        };
        prop_assert!(order.windows(2).all(|w| key(&w[0]) < key(&w[1])));

        prop_assert_eq!(ready_order(&shuffled), order);
    }
}
