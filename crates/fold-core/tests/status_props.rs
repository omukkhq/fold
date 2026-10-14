use std::cmp::Reverse;
use std::collections::BTreeSet;

use fold_core::readiness::{Candidate, ready_order};
use fold_core::status::{ProposalFact, TaskFacts, TaskStatus, calculate};
use fold_core::{TaskId, Timestamp};
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

fn facts() -> impl Strategy<Value = TaskFacts> {
    (any::<bool>(), proposal(), any::<bool>(), prerequisites()).prop_map(
        |(completed, proposal, has_active_workspace, prerequisites)| TaskFacts {
            completed,
            proposal,
            has_active_workspace,
            prerequisites,
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
        let f = TaskFacts {
            completed: false,
            proposal: ProposalFact::None,
            has_active_workspace: false,
            prerequisites: prereqs.into_iter().map(|(n, s)| (t(n), s)).collect(),
        };
        prop_assert_eq!(calculate(&f).status, TaskStatus::Todo);
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
