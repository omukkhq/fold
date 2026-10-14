use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};

use crate::ids::TaskId;

/// Why an edge cannot be added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeRefusal {
    SelfDependency,
    /// The path `[task, needs, ..., task]` that the new edge would close.
    Cycle(Vec<TaskId>),
}

/// An immutable dependency graph. The edge `(A, B)` means A needs B. The graph does not know which tasks exist: a task absent from the edges is a node with no neighbours.
#[derive(Debug, Clone, Default)]
pub struct Dag {
    needs: BTreeMap<TaskId, BTreeSet<TaskId>>,
    needed_by: BTreeMap<TaskId, BTreeSet<TaskId>>,
}

impl Dag {
    /// Duplicate edges collapse. The input is not checked for cycles; callers add edges only after `check_edge`.
    pub fn from_edges(edges: impl IntoIterator<Item = (TaskId, TaskId)>) -> Self {
        let mut dag = Self::default();
        for (task, needs) in edges {
            dag.needs.entry(task).or_default().insert(needs);
            dag.needed_by.entry(needs).or_default().insert(task);
        }
        dag
    }

    /// Refuses an edge that would make the graph cyclic. The cycle reported is the shortest one, and among equal lengths the one that visits lower task numbers first.
    pub fn check_edge(&self, task: TaskId, needs: TaskId) -> Result<(), EdgeRefusal> {
        if task == needs {
            return Err(EdgeRefusal::SelfDependency);
        }
        // Breadth-first from `needs` along needs edges, neighbours in id order: the first parent recorded for a node is the one on the lexicographically smallest shortest path.
        let mut parent: BTreeMap<TaskId, TaskId> = BTreeMap::new();
        let mut queue = VecDeque::from([needs]);
        let mut seen = BTreeSet::from([needs]);
        while let Some(node) = queue.pop_front() {
            for &next in self.needs.get(&node).into_iter().flatten() {
                if !seen.insert(next) {
                    continue;
                }
                parent.insert(next, node);
                if next == task {
                    let mut path = vec![task];
                    let mut at = task;
                    while let Some(&before) = parent.get(&at) {
                        path.push(before);
                        at = before;
                    }
                    path.push(task);
                    path.reverse();
                    return Err(EdgeRefusal::Cycle(path));
                }
                queue.push_back(next);
            }
        }
        Ok(())
    }

    /// The tasks `task` needs directly, sorted by id.
    pub fn prerequisites(&self, task: TaskId) -> Vec<TaskId> {
        self.needs
            .get(&task)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default()
    }

    /// The tasks that need `task` directly, sorted by id.
    pub fn dependents(&self, task: TaskId) -> Vec<TaskId> {
        self.needed_by
            .get(&task)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Every task `task` needs, directly or not. `task` itself is included only when a cycle leads back to it.
    pub fn prerequisite_closure(&self, task: TaskId) -> BTreeSet<TaskId> {
        reach(&self.needs, [task], false)
    }

    /// The seeds and every task that needs one of them, directly or not.
    pub fn dependent_closure(&self, seeds: impl IntoIterator<Item = TaskId>) -> BTreeSet<TaskId> {
        reach(&self.needed_by, seeds, true)
    }

    /// Orders `subset` so that a task comes after every subset member it needs, directly or through tasks outside the subset. Ties go to the lower task number. Tasks on or behind a cycle, which a checked graph never has, come last in id order.
    pub fn topological_order(&self, subset: impl IntoIterator<Item = TaskId>) -> Vec<TaskId> {
        let subset: BTreeSet<TaskId> = subset.into_iter().collect();
        // Every prerequisite of a task in this set is in the set too, so a task waits on all of its needs edges.
        let mut waiting: BTreeMap<TaskId, usize> = reach(&self.needs, subset.iter().copied(), true)
            .into_iter()
            .map(|task| (task, self.needs.get(&task).map_or(0, BTreeSet::len)))
            .collect();
        let mut ready_members: BinaryHeap<Reverse<TaskId>> = BinaryHeap::new();
        let mut ready_outside: Vec<TaskId> = Vec::new();
        for (&task, &count) in &waiting {
            if count == 0 {
                if subset.contains(&task) {
                    ready_members.push(Reverse(task));
                } else {
                    ready_outside.push(task);
                }
            }
        }
        // Tasks outside the subset are released before any member is chosen, so a member is ready exactly when every subset member it reaches is placed.
        let mut order = Vec::with_capacity(subset.len());
        loop {
            let task = if let Some(task) = ready_outside.pop() {
                task
            } else if let Some(Reverse(task)) = ready_members.pop() {
                order.push(task);
                task
            } else {
                break;
            };
            for &next in self.needed_by.get(&task).into_iter().flatten() {
                if let Some(count) = waiting.get_mut(&next) {
                    *count -= 1;
                    if *count == 0 {
                        if subset.contains(&next) {
                            ready_members.push(Reverse(next));
                        } else {
                            ready_outside.push(next);
                        }
                    }
                }
            }
        }
        if order.len() < subset.len() {
            let placed: BTreeSet<TaskId> = order.iter().copied().collect();
            order.extend(subset.into_iter().filter(|task| !placed.contains(task)));
        }
        order
    }

    /// True when neither task reaches the other. A task is never independent of itself.
    pub fn independent(&self, a: TaskId, b: TaskId) -> bool {
        a != b
            && !self.prerequisite_closure(a).contains(&b)
            && !self.prerequisite_closure(b).contains(&a)
    }
}

fn reach(
    edges: &BTreeMap<TaskId, BTreeSet<TaskId>>,
    seeds: impl IntoIterator<Item = TaskId>,
    include_seeds: bool,
) -> BTreeSet<TaskId> {
    let mut found = BTreeSet::new();
    let mut stack: Vec<TaskId> = Vec::new();
    for seed in seeds {
        if include_seeds {
            found.insert(seed);
        }
        stack.push(seed);
    }
    while let Some(node) = stack.pop() {
        for &next in edges.get(&node).into_iter().flatten() {
            if found.insert(next) {
                stack.push(next);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(n: u32) -> TaskId {
        TaskId::from_number(n).unwrap()
    }

    fn dag(edges: &[(u32, u32)]) -> Dag {
        Dag::from_edges(edges.iter().map(|&(a, b)| (t(a), t(b))))
    }

    #[test]
    fn self_edge_is_refused() {
        assert_eq!(
            Dag::default().check_edge(t(1), t(1)),
            Err(EdgeRefusal::SelfDependency)
        );
    }

    #[test]
    fn two_node_cycle_is_refused() {
        let g = dag(&[(2, 1)]);
        assert_eq!(
            g.check_edge(t(1), t(2)),
            Err(EdgeRefusal::Cycle(vec![t(1), t(2), t(1)]))
        );
    }

    #[test]
    fn long_cycle_path_runs_from_task_through_needs_back_to_task() {
        let g = dag(&[(2, 3), (3, 4), (4, 1)]);
        assert_eq!(
            g.check_edge(t(1), t(2)),
            Err(EdgeRefusal::Cycle(vec![t(1), t(2), t(3), t(4), t(1)]))
        );
    }

    #[test]
    fn diamond_accepts_and_a_back_edge_is_refused() {
        let g = dag(&[(1, 2), (1, 3), (2, 4), (3, 4)]);
        assert_eq!(g.check_edge(t(5), t(1)), Ok(()));
        assert_eq!(g.check_edge(t(1), t(4)), Ok(()));
        assert!(g.check_edge(t(4), t(1)).is_err());
    }

    #[test]
    fn shortest_path_wins_then_lower_numbers() {
        // 9 needs 5 needs 1 (length 2) and 9 needs 2 needs 3 needs 1 (length 3).
        let g = dag(&[(9, 5), (5, 1), (9, 2), (2, 3), (3, 1)]);
        assert_eq!(
            g.check_edge(t(1), t(9)),
            Err(EdgeRefusal::Cycle(vec![t(1), t(9), t(5), t(1)]))
        );
        // Two equal routes: through 2 and through 3. The lower one is chosen.
        let g = dag(&[(9, 3), (3, 1), (9, 2), (2, 1)]);
        assert_eq!(
            g.check_edge(t(1), t(9)),
            Err(EdgeRefusal::Cycle(vec![t(1), t(9), t(2), t(1)]))
        );
    }

    #[test]
    fn empty_graph_has_no_neighbours() {
        let g = Dag::default();
        assert_eq!(g.check_edge(t(1), t(2)), Ok(()));
        assert!(g.prerequisites(t(1)).is_empty());
        assert!(g.dependents(t(1)).is_empty());
        assert!(g.prerequisite_closure(t(1)).is_empty());
        assert_eq!(g.dependent_closure([t(1)]), BTreeSet::from([t(1)]));
        assert!(g.topological_order([]).is_empty());
        assert_eq!(g.topological_order([t(2), t(1)]), vec![t(1), t(2)]);
        assert!(!g.independent(t(1), t(1)));
        assert!(g.independent(t(1), t(2)));
    }

    #[test]
    fn duplicate_edges_collapse_and_neighbours_are_sorted() {
        let g = dag(&[(1, 3), (1, 2), (1, 3), (4, 2)]);
        assert_eq!(g.prerequisites(t(1)), vec![t(2), t(3)]);
        assert_eq!(g.dependents(t(2)), vec![t(1), t(4)]);
    }

    #[test]
    fn closures_follow_edges_transitively() {
        let g = dag(&[(1, 2), (2, 3), (4, 3)]);
        assert_eq!(g.prerequisite_closure(t(1)), BTreeSet::from([t(2), t(3)]));
        assert_eq!(
            g.dependent_closure([t(3)]),
            BTreeSet::from([t(1), t(2), t(3), t(4)])
        );
        assert!(g.independent(t(1), t(4)));
        assert!(!g.independent(t(1), t(3)));
    }

    #[test]
    fn topological_order_uses_paths_through_tasks_outside_the_subset() {
        let g = dag(&[(1, 5), (5, 3), (2, 4)]);
        assert_eq!(g.topological_order([t(1), t(3)]), vec![t(3), t(1)]);
        assert_eq!(
            g.topological_order([t(1), t(2), t(3), t(4)]),
            vec![t(3), t(1), t(4), t(2)]
        );
    }

    #[test]
    fn topological_order_places_cyclic_tasks_last() {
        let g = dag(&[(1, 2), (2, 1), (3, 1)]);
        assert_eq!(
            g.topological_order([t(1), t(2), t(3)]),
            vec![t(1), t(2), t(3)]
        );
        let g = dag(&[(1, 2), (2, 1)]);
        assert_eq!(
            g.topological_order([t(2), t(1), t(7)]),
            vec![t(7), t(1), t(2)]
        );
    }

    #[test]
    fn a_long_chain_builds_and_answers_without_recursion() {
        let n = 10_000;
        let g = Dag::from_edges((1..n).map(|i| (t(i + 1), t(i))));
        assert_eq!(g.check_edge(t(n + 1), t(1)), Ok(()));
        let mut path = vec![t(1)];
        path.extend((1..=n).rev().map(t));
        assert_eq!(g.check_edge(t(1), t(n)), Err(EdgeRefusal::Cycle(path)));
        assert_eq!(g.prerequisite_closure(t(n)).len(), n as usize - 1);
        assert_eq!(g.dependent_closure([t(1)]).len(), n as usize);
        assert!(!g.independent(t(1), t(n)));
        assert_eq!(g.topological_order([t(n), t(1)]), vec![t(1), t(n)]);
    }
}
