use std::cmp::Reverse;
use std::collections::BTreeMap;

use crate::agents::AreaName;
use crate::ids::TaskId;
use crate::tasks::dag::Dag;
use crate::time::Timestamp;

/// One worker start that waits in the queue for a place under the worker limits. `key` is the caller's handle of the start, such as its row id. `tasks` are the tasks the start works on; for a reviewer, the tasks of its proposals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedStart<K> {
    pub key: K,
    /// `None` for a start outside an area, such as a reviewer's; those starts share one turn of the rotation.
    pub area: Option<AreaName>,
    pub tasks: Vec<TaskId>,
    pub queued_at: Timestamp,
}

/// A start's place inside its area: more dependents first, then older, then the lower key.
type Rank<K> = (Reverse<usize>, Timestamp, K);

/// The order in which fold runs the queued starts, first to run first.
///
/// The places rotate across areas: each round gives every area with a waiting start one place. The area that waited longest since a queued start of it last ran goes first in a round, an area that never ran one before all others, so one busy area cannot take every free place. `last_started` holds, for each area, when a queued start of it last ran.
///
/// Inside an area, the start whose tasks unblock the most work runs first: the most transitive dependents of any of its tasks in `dag`. Then the oldest start, then the lower key, so the order is total and the same for the same inputs.
pub fn queue_order<K: Copy + Ord>(
    starts: &[QueuedStart<K>],
    dag: &Dag,
    last_started: &BTreeMap<Option<AreaName>, Timestamp>,
) -> Vec<K> {
    let mut areas: BTreeMap<Option<&AreaName>, Vec<Rank<K>>> = BTreeMap::new();
    for start in starts {
        let rank = (
            Reverse(dependents_of(dag, &start.tasks)),
            start.queued_at,
            start.key,
        );
        areas.entry(start.area.as_ref()).or_default().push(rank);
    }
    let mut groups: Vec<(Option<Timestamp>, Vec<Rank<K>>)> = areas
        .into_iter()
        .map(|(area, mut ranks)| {
            ranks.sort_unstable();
            let served = last_started.get(&area.cloned()).copied();
            (served, ranks)
        })
        .collect();
    // `None` sorts before every time, so an area that never ran a queued start comes first; the head start breaks a tie.
    groups
        .sort_unstable_by(|left, right| (left.0, left.1.first()).cmp(&(right.0, right.1.first())));
    let rounds = groups
        .iter()
        .map(|(_, ranks)| ranks.len())
        .max()
        .unwrap_or(0);
    let mut order = Vec::with_capacity(starts.len());
    for round in 0..rounds {
        for (_, ranks) in &groups {
            if let Some(&(_, _, key)) = ranks.get(round) {
                order.push(key);
            }
        }
    }
    order
}

/// The most tasks that need one of `tasks`, directly or not, counting each task's own dependents apart.
fn dependents_of(dag: &Dag, tasks: &[TaskId]) -> usize {
    tasks
        .iter()
        .map(|&task| dag.dependent_closure([task]).len().saturating_sub(1))
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashSet};

    use proptest::prelude::*;

    use super::*;

    fn task(number: u32) -> TaskId {
        TaskId::from_number(number).unwrap()
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis).unwrap()
    }

    fn area(name: &str) -> Option<AreaName> {
        Some(AreaName::parse(name).unwrap())
    }

    fn start(key: u32, area_name: Option<&str>, tasks: &[u32], queued_at: i64) -> QueuedStart<u32> {
        QueuedStart {
            key,
            area: area_name.and_then(area),
            tasks: tasks.iter().map(|&n| task(n)).collect(),
            queued_at: at(queued_at),
        }
    }

    #[test]
    fn an_empty_queue_has_no_order() {
        assert!(queue_order::<u32>(&[], &Dag::default(), &BTreeMap::new()).is_empty());
    }

    #[test]
    fn one_area_runs_the_oldest_first_when_no_task_unblocks_more() {
        let starts = [
            start(1, Some("web"), &[1], 30),
            start(2, Some("web"), &[2], 10),
            start(3, Some("web"), &[3], 20),
        ];
        assert_eq!(
            queue_order(&starts, &Dag::default(), &BTreeMap::new()),
            [2, 3, 1]
        );
    }

    #[test]
    fn the_start_with_the_most_transitive_dependents_runs_first() {
        // T-4 needs T-1, T-5 needs T-4: T-1 has two dependents, T-2 one, T-3 none.
        let dag = Dag::from_edges([(task(4), task(1)), (task(5), task(4)), (task(6), task(2))]);
        let starts = [
            start(1, Some("web"), &[3], 10),
            start(2, Some("web"), &[2], 20),
            start(3, Some("web"), &[1], 30),
        ];
        assert_eq!(queue_order(&starts, &dag, &BTreeMap::new()), [3, 2, 1]);
    }

    #[test]
    fn a_start_counts_the_best_of_its_tasks() {
        let dag = Dag::from_edges([(task(4), task(1)), (task(5), task(1)), (task(6), task(2))]);
        let starts = [
            start(1, Some("web"), &[2, 3], 10),
            start(2, Some("web"), &[3, 1], 20),
        ];
        assert_eq!(queue_order(&starts, &dag, &BTreeMap::new()), [2, 1]);
    }

    #[test]
    fn the_lower_key_breaks_a_full_tie() {
        let starts = [
            start(7, Some("web"), &[1], 10),
            start(4, Some("web"), &[2], 10),
        ];
        assert_eq!(
            queue_order(&starts, &Dag::default(), &BTreeMap::new()),
            [4, 7]
        );
    }

    #[test]
    fn places_alternate_between_areas() {
        let starts = [
            start(1, Some("web"), &[1], 10),
            start(2, Some("web"), &[2], 11),
            start(3, Some("web"), &[3], 12),
            start(4, Some("core"), &[4], 20),
            start(5, Some("core"), &[5], 21),
        ];
        // web holds the oldest head start, so it goes first in each round.
        assert_eq!(
            queue_order(&starts, &Dag::default(), &BTreeMap::new()),
            [1, 4, 2, 5, 3]
        );
    }

    #[test]
    fn the_area_that_waited_longest_since_its_last_start_goes_first() {
        let starts = [
            start(1, Some("web"), &[1], 10),
            start(2, Some("core"), &[2], 20),
            start(3, Some("docs"), &[3], 30),
        ];
        let mut last = BTreeMap::new();
        last.insert(area("web"), at(500));
        last.insert(area("core"), at(400));
        // docs never ran a queued start, then core ran one before web.
        assert_eq!(queue_order(&starts, &Dag::default(), &last), [3, 2, 1]);
    }

    #[test]
    fn starts_outside_an_area_share_one_turn() {
        let starts = [
            start(1, None, &[1], 10),
            start(2, None, &[2], 11),
            start(3, Some("web"), &[3], 12),
        ];
        assert_eq!(
            queue_order(&starts, &Dag::default(), &BTreeMap::new()),
            [1, 3, 2]
        );
    }

    #[test]
    fn a_start_without_tasks_has_no_dependents() {
        let dag = Dag::from_edges([(task(2), task(1))]);
        let starts = [
            start(1, Some("web"), &[], 10),
            start(2, Some("web"), &[1], 20),
        ];
        assert_eq!(queue_order(&starts, &dag, &BTreeMap::new()), [2, 1]);
    }

    fn arb_starts() -> impl Strategy<Value = Vec<QueuedStart<u32>>> {
        let names = prop::sample::select(vec![None, Some("web"), Some("core"), Some("docs")]);
        prop::collection::vec(
            (names, prop::collection::vec(1_u32..12, 0..3), 0_i64..50),
            0..16,
        )
        .prop_map(|rows| {
            rows.into_iter()
                .enumerate()
                .map(|(key, (name, tasks, queued_at))| {
                    start(u32::try_from(key).unwrap(), name, &tasks, queued_at)
                })
                .collect()
        })
    }

    fn arb_dag() -> impl Strategy<Value = Dag> {
        // An edge from a higher to a lower number keeps the graph acyclic.
        prop::collection::vec((2_u32..12, 1_u32..11), 0..20).prop_map(|edges| {
            Dag::from_edges(
                edges
                    .into_iter()
                    .filter(|(a, b)| a > b)
                    .map(|(a, b)| (task(a), task(b))),
            )
        })
    }

    proptest! {
        #[test]
        fn the_order_holds_every_start_once(starts in arb_starts(), dag in arb_dag()) {
            let order = queue_order(&starts, &dag, &BTreeMap::new());
            let keys: BTreeSet<u32> = starts.iter().map(|s| s.key).collect();
            prop_assert_eq!(order.len(), starts.len());
            prop_assert_eq!(order.iter().copied().collect::<BTreeSet<u32>>(), keys);
        }

        #[test]
        fn the_order_does_not_depend_on_the_input_order(
            starts in arb_starts(),
            dag in arb_dag(),
            seed in any::<u64>(),
        ) {
            let mut shuffled = starts.clone();
            let len = shuffled.len();
            if len > 1 {
                for i in 0..len {
                    let j = usize::try_from(seed.rotate_left(u32::try_from(i).unwrap()) % u64::try_from(len).unwrap()).unwrap();
                    shuffled.swap(i, j);
                }
            }
            prop_assert_eq!(
                queue_order(&starts, &dag, &BTreeMap::new()),
                queue_order(&shuffled, &dag, &BTreeMap::new())
            );
        }

        #[test]
        fn each_round_gives_an_area_at_most_one_place(starts in arb_starts(), dag in arb_dag()) {
            let order = queue_order(&starts, &dag, &BTreeMap::new());
            let area_of: BTreeMap<u32, Option<AreaName>> =
                starts.iter().map(|s| (s.key, s.area.clone())).collect();
            let areas: HashSet<Option<AreaName>> = starts.iter().map(|s| s.area.clone()).collect();
            // While every area still waits, the first places go to distinct areas.
            let first: Vec<&Option<AreaName>> =
                order.iter().take(areas.len()).map(|key| &area_of[key]).collect();
            let distinct: HashSet<&Option<AreaName>> = first.iter().copied().collect();
            prop_assert_eq!(distinct.len(), first.len());
        }

        #[test]
        fn inside_an_area_more_dependents_then_older_run_first(starts in arb_starts(), dag in arb_dag()) {
            let order = queue_order(&starts, &dag, &BTreeMap::new());
            let by_key: BTreeMap<u32, &QueuedStart<u32>> = starts.iter().map(|s| (s.key, s)).collect();
            for (i, earlier) in order.iter().enumerate() {
                for later in &order[i + 1..] {
                    let (a, b) = (by_key[earlier], by_key[later]);
                    if a.area != b.area {
                        continue;
                    }
                    let rank = |s: &QueuedStart<u32>| (Reverse(dependents_of(&dag, &s.tasks)), s.queued_at, s.key);
                    prop_assert!(rank(a) < rank(b));
                }
            }
        }
    }
}
