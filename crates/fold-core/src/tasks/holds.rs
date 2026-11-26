use std::collections::{BTreeMap, BTreeSet};

use crate::ids::TaskId;
use crate::tasks::dag::Dag;

/// For each task of `tasks` that is not Done, the held tasks upstream of it: every held task it needs directly or through other prerequisites, sorted by id. `tasks` gives each task with its Done state; a task that only the edges name counts as not Done and gets no entry. A held task does not list itself, but a held task still lists the holds upstream of it. A Done task holds nothing back and ends the walk on its path, so a hold beyond it is not listed. A held task that is Done holds nothing back.
pub fn held_back_by(
    dag: &Dag,
    tasks: impl IntoIterator<Item = (TaskId, bool)>,
    held: &BTreeSet<TaskId>,
) -> BTreeMap<TaskId, Vec<TaskId>> {
    let done_of: BTreeMap<TaskId, bool> = tasks.into_iter().collect();
    let is_done = |task: &TaskId| done_of.get(task).copied().unwrap_or(false);
    let mut result: BTreeMap<TaskId, Vec<TaskId>> = done_of
        .iter()
        .filter(|(_, done)| !**done)
        .map(|(&task, _)| (task, Vec::new()))
        .collect();
    // Holds are walked in id order, so each list is built sorted and without duplicates.
    for &hold in held.iter().filter(|hold| !is_done(hold)) {
        let mut seen = BTreeSet::from([hold]);
        let mut stack = vec![hold];
        while let Some(node) = stack.pop() {
            for next in dag.dependents(node) {
                if is_done(&next) || !seen.insert(next) {
                    continue;
                }
                if let Some(list) = result.get_mut(&next) {
                    list.push(hold);
                }
                stack.push(next);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn t(n: u32) -> TaskId {
        TaskId::from_number(n).unwrap()
    }

    fn dag(edges: &[(u32, u32)]) -> Dag {
        Dag::from_edges(edges.iter().map(|&(a, b)| (t(a), t(b))))
    }

    fn ids(numbers: &[u32]) -> Vec<TaskId> {
        numbers.iter().copied().map(t).collect()
    }

    fn open(numbers: &[u32]) -> Vec<(TaskId, bool)> {
        numbers.iter().map(|&n| (t(n), false)).collect()
    }

    #[test]
    fn a_hold_reaches_every_task_downstream_of_a_chain() {
        // 2 needs 1 (held), 3 needs 2.
        let result = held_back_by(
            &dag(&[(2, 1), (3, 2)]),
            open(&[1, 2, 3]),
            &BTreeSet::from([t(1)]),
        );
        assert_eq!(result[&t(1)], ids(&[]));
        assert_eq!(result[&t(2)], ids(&[1]));
        assert_eq!(result[&t(3)], ids(&[1]));
    }

    #[test]
    fn a_task_that_needs_two_held_tasks_lists_both_sorted() {
        let result = held_back_by(
            &dag(&[(3, 5), (3, 1)]),
            open(&[5, 1, 3]),
            &BTreeSet::from([t(5), t(1)]),
        );
        assert_eq!(result[&t(3)], ids(&[1, 5]));
    }

    #[test]
    fn a_done_task_ends_the_walk_on_its_path() {
        // 3 needs 2 (Done), 2 needs 1 (held); 4 needs 1 directly.
        let tasks = vec![(t(1), false), (t(2), true), (t(3), false), (t(4), false)];
        let result = held_back_by(
            &dag(&[(2, 1), (3, 2), (4, 1)]),
            tasks,
            &BTreeSet::from([t(1)]),
        );
        assert_eq!(result[&t(3)], ids(&[]));
        assert_eq!(result[&t(4)], ids(&[1]));
        assert!(!result.contains_key(&t(2)));
    }

    #[test]
    fn a_held_task_lists_the_holds_upstream_of_it_but_not_itself() {
        let result = held_back_by(
            &dag(&[(2, 1), (3, 2)]),
            open(&[1, 2, 3]),
            &BTreeSet::from([t(1), t(2)]),
        );
        assert_eq!(result[&t(1)], ids(&[]));
        assert_eq!(result[&t(2)], ids(&[1]));
        assert_eq!(result[&t(3)], ids(&[1, 2]));
    }

    #[test]
    fn a_done_hold_holds_nothing_back_and_unknown_tasks_get_no_entry() {
        // 2 needs 1, which is held and Done; 3 needs 9, which only the edges name, and 9 needs 4 (held).
        let tasks = vec![(t(1), true), (t(2), false), (t(3), false), (t(4), false)];
        let result = held_back_by(
            &dag(&[(2, 1), (3, 9), (9, 4)]),
            tasks,
            &BTreeSet::from([t(1), t(4)]),
        );
        assert_eq!(result[&t(2)], ids(&[]));
        assert_eq!(result[&t(3)], ids(&[4]));
        assert!(!result.contains_key(&t(9)));
        assert!(!result.contains_key(&t(1)));
    }

    #[test]
    fn a_diamond_lists_a_hold_once() {
        let result = held_back_by(
            &dag(&[(2, 1), (3, 1), (4, 2), (4, 3)]),
            open(&[1, 2, 3, 4]),
            &BTreeSet::from([t(1)]),
        );
        assert_eq!(result[&t(4)], ids(&[1]));
    }

    #[test]
    fn a_long_chain_is_walked_without_recursion() {
        let n = 10_000;
        let graph = Dag::from_edges((1..n).map(|i| (t(i + 1), t(i))));
        let result = held_back_by(
            &graph,
            (1..=n).map(|i| (t(i), false)),
            &BTreeSet::from([t(1)]),
        );
        assert_eq!(result.len(), n as usize);
        assert_eq!(result[&t(n)], ids(&[1]));
    }

    /// A random acyclic graph over tasks 1..=n: every edge points from a higher number to a lower one.
    fn graph_input() -> impl Strategy<Value = (Vec<(u32, u32)>, Vec<bool>, Vec<bool>)> {
        (2u32..12).prop_flat_map(|n| {
            (
                proptest::collection::vec((1..=n, 1..=n), 0..30).prop_map(|pairs| {
                    pairs
                        .into_iter()
                        .filter(|(a, b)| a != b)
                        .map(|(a, b)| (a.max(b), a.min(b)))
                        .collect()
                }),
                proptest::collection::vec(any::<bool>(), n as usize),
                proptest::collection::vec(proptest::bool::weighted(0.3), n as usize),
            )
        })
    }

    struct Input {
        dag: Dag,
        tasks: Vec<(TaskId, bool)>,
        done: BTreeSet<TaskId>,
        held: BTreeSet<TaskId>,
    }

    fn input(edges: &[(u32, u32)], done: &[bool], held: &[bool]) -> Input {
        let numbered = |flags: &[bool]| -> BTreeSet<TaskId> {
            (1u32..)
                .zip(flags)
                .filter(|(_, flag)| **flag)
                .map(|(n, _)| t(n))
                .collect()
        };
        Input {
            dag: dag(edges),
            tasks: (1u32..).zip(done).map(|(n, &d)| (t(n), d)).collect(),
            done: numbered(done),
            held: numbered(held),
        }
    }

    proptest! {
        #[test]
        fn lists_are_sorted_and_hold_no_done_or_own_task((edges, done, held) in graph_input()) {
            let input = input(&edges, &done, &held);
            let result = held_back_by(&input.dag, input.tasks.clone(), &input.held);
            for (task, list) in &result {
                prop_assert!(!input.done.contains(task));
                prop_assert!(!list.contains(task));
                prop_assert!(list.windows(2).all(|pair| pair[0] < pair[1]));
                for hold in list {
                    prop_assert!(input.held.contains(hold));
                    prop_assert!(!input.done.contains(hold));
                }
            }
        }

        #[test]
        fn holds_pass_through_open_prerequisites((edges, done, held) in graph_input()) {
            let input = input(&edges, &done, &held);
            let result = held_back_by(&input.dag, input.tasks.clone(), &input.held);
            for (task, list) in &result {
                let mut expected = BTreeSet::new();
                for need in input.dag.prerequisites(*task) {
                    if input.done.contains(&need) {
                        continue;
                    }
                    if input.held.contains(&need) {
                        expected.insert(need);
                    }
                    expected.extend(result[&need].iter().copied());
                }
                prop_assert_eq!(list.iter().copied().collect::<BTreeSet<_>>(), expected);
            }
        }

        #[test]
        fn a_release_removes_exactly_that_hold((edges, done, held) in graph_input(), pick in any::<prop::sample::Index>()) {
            let input = input(&edges, &done, &held);
            prop_assume!(!input.held.is_empty());
            let released = *pick.get(&input.held.iter().copied().collect::<Vec<_>>());
            let before = held_back_by(&input.dag, input.tasks.clone(), &input.held);
            let mut rest = input.held.clone();
            rest.remove(&released);
            let after = held_back_by(&input.dag, input.tasks.clone(), &rest);
            prop_assert_eq!(before.keys().collect::<Vec<_>>(), after.keys().collect::<Vec<_>>());
            for (task, list) in &before {
                let expected: Vec<TaskId> = list.iter().copied().filter(|hold| *hold != released).collect();
                prop_assert_eq!(&after[task], &expected);
                if list.as_slice() == [released] {
                    prop_assert!(after[task].is_empty());
                }
            }
        }
    }
}
