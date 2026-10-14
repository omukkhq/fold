use std::collections::BTreeMap;

use crate::dag::Dag;
use crate::ids::TaskId;
use crate::status::TaskStatus;
use crate::time::Timestamp;

/// A task is ready when it is in Todo and no agent holds an unreleased implementation claim on it.
pub fn is_ready(status: TaskStatus, has_unreleased_implementation_claim: bool) -> bool {
    status == TaskStatus::Todo && !has_unreleased_implementation_claim
}

/// How many direct dependents only `task` holds back: those in Backlog, In Progress or Review whose every other prerequisite is Done. A started dependent counts because an agent already waits on it. A task missing from `statuses` counts as not Done and not started.
pub fn unblock_count(task: TaskId, dag: &Dag, statuses: &BTreeMap<TaskId, TaskStatus>) -> usize {
    dag.dependents(task)
        .into_iter()
        .filter(|dependent| {
            matches!(
                statuses.get(dependent),
                Some(TaskStatus::Backlog | TaskStatus::InProgress | TaskStatus::Review)
            )
        })
        .filter(|dependent| {
            dag.prerequisites(*dependent)
                .into_iter()
                .filter(|&other| other != task)
                .all(|other| statuses.get(&other) == Some(&TaskStatus::Done))
        })
        .count()
}

/// One ready task with the inputs of its ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub task: TaskId,
    pub unblock_count: usize,
    pub created_at: Timestamp,
}

/// Most unblocking first, then oldest, then lowest task number. The task number is unique, so the order is total and does not depend on the input order.
pub fn ready_order(candidates: &[Candidate]) -> Vec<TaskId> {
    let mut sorted = candidates.to_vec();
    sorted.sort_by_key(|c| (std::cmp::Reverse(c.unblock_count), c.created_at, c.task));
    sorted.into_iter().map(|c| c.task).collect()
}

/// Two tasks can be worked on at the same time when neither needs the other, directly or through others.
pub fn can_run_in_parallel(a: TaskId, b: TaskId, dag: &Dag) -> bool {
    dag.independent(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use TaskStatus::*;

    fn t(n: u32) -> TaskId {
        TaskId::from_number(n).unwrap()
    }

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms).unwrap()
    }

    fn statuses(list: &[(u32, TaskStatus)]) -> BTreeMap<TaskId, TaskStatus> {
        list.iter().map(|&(n, s)| (t(n), s)).collect()
    }

    #[test]
    fn ready_needs_todo_and_no_claim() {
        assert!(is_ready(Todo, false));
        assert!(!is_ready(Todo, true));
        for s in [Backlog, InProgress, Review, Done] {
            assert!(!is_ready(s, false));
        }
    }

    #[test]
    fn unblock_count_counts_dependents_whose_other_prerequisites_are_done() {
        // 2, 3, 4 need 1; 3 also needs 5 (Done); 4 also needs 6 (Todo); 7 needs 1 but is Todo.
        let dag = Dag::from_edges(
            [(2, 1), (3, 1), (3, 5), (4, 1), (4, 6), (7, 1)].map(|(a, b)| (t(a), t(b))),
        );
        let s = statuses(&[
            (1, Todo),
            (2, Backlog),
            (3, Backlog),
            (4, Backlog),
            (5, Done),
            (6, Todo),
            (7, Todo),
        ]);
        assert_eq!(unblock_count(t(1), &dag, &s), 2);
    }

    #[test]
    fn unblock_count_ignores_indirect_dependents_and_unknown_statuses() {
        let dag = Dag::from_edges([(2, 1), (3, 2)].map(|(a, b)| (t(a), t(b))));
        let s = statuses(&[(1, Todo), (2, Backlog), (3, Backlog)]);
        assert_eq!(unblock_count(t(1), &dag, &s), 1);
        assert_eq!(unblock_count(t(1), &dag, &statuses(&[])), 0);
        assert_eq!(unblock_count(t(9), &dag, &s), 0);
    }

    #[test]
    fn unblock_count_counts_started_dependents_held_back_only_by_this_task() {
        for status in [Backlog, InProgress, Review] {
            let dag = Dag::from_edges([(t(2), t(1))]);
            let s = statuses(&[(1, Todo), (2, status)]);
            assert_eq!(unblock_count(t(1), &dag, &s), 1, "{status:?}");
        }
        let dag = Dag::from_edges([(t(2), t(1))]);
        assert_eq!(
            unblock_count(t(1), &dag, &statuses(&[(1, Todo), (2, Done)])),
            0
        );
        let dag = Dag::from_edges(
            [(2, 1), (2, 3), (4, 1), (4, 3), (5, 1), (5, 3)].map(|(a, b)| (t(a), t(b))),
        );
        let s = statuses(&[
            (1, Todo),
            (3, Todo),
            (2, Backlog),
            (4, InProgress),
            (5, Review),
        ]);
        assert_eq!(unblock_count(t(1), &dag, &s), 0);
    }

    fn cand(task: u32, unblock: usize, created: i64) -> Candidate {
        Candidate {
            task: t(task),
            unblock_count: unblock,
            created_at: at(created),
        }
    }

    #[test]
    fn ready_order_prefers_more_unblocked_tasks() {
        let order = ready_order(&[cand(1, 0, 1), cand(2, 3, 9), cand(3, 1, 5)]);
        assert_eq!(order, vec![t(2), t(3), t(1)]);
    }

    #[test]
    fn ready_order_breaks_ties_by_age_then_number() {
        let order = ready_order(&[
            cand(4, 1, 20),
            cand(3, 1, 10),
            cand(2, 1, 10),
            cand(1, 1, 30),
        ]);
        assert_eq!(order, vec![t(2), t(3), t(4), t(1)]);
    }

    #[test]
    fn ready_order_puts_a_task_with_a_started_dependent_first() {
        // 3 is In Progress and waits only on 2; 1 holds nothing back and is older.
        let dag = Dag::from_edges([(t(3), t(2))]);
        let s = statuses(&[(1, Todo), (2, Todo), (3, InProgress)]);
        let candidates: Vec<Candidate> = [(1, 1), (2, 5)]
            .map(|(task, created)| cand(task, unblock_count(t(task), &dag, &s), created))
            .to_vec();
        assert_eq!(ready_order(&candidates), vec![t(2), t(1)]);
    }

    #[test]
    fn ready_order_of_nothing_is_empty() {
        assert!(ready_order(&[]).is_empty());
    }

    #[test]
    fn parallel_means_independent() {
        let dag = Dag::from_edges([(t(2), t(1))]);
        assert!(!can_run_in_parallel(t(1), t(2), &dag));
        assert!(can_run_in_parallel(t(1), t(3), &dag));
    }
}
