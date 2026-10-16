use std::collections::BTreeSet;

use fold_core::TaskId;
use fold_core::dag::{Dag, EdgeRefusal};
use proptest::prelude::*;

const NODES: usize = 40;

#[expect(
    clippy::unwrap_used,
    reason = "a helper outside #[test] in a test crate; the number is never zero"
)]
fn t(n: usize) -> TaskId {
    TaskId::from_number(n as u32 + 1).unwrap()
}

/// Random acyclic edge lists over the first `nodes` tasks: an edge always points from a higher index to a lower one.
fn acyclic_edges_over(nodes: usize) -> impl Strategy<Value = Vec<(usize, usize)>> {
    prop::collection::vec((1..nodes, 0..nodes), 0..120).prop_map(|pairs| {
        pairs
            .into_iter()
            .filter_map(|(a, b)| (b < a).then_some((a, b)))
            .collect()
    })
}

fn acyclic_edges() -> impl Strategy<Value = Vec<(usize, usize)>> {
    acyclic_edges_over(NODES)
}

/// A graph and a candidate edge. Small node counts give dense graphs, where equal-length cycles are common.
fn graph_and_edge() -> impl Strategy<Value = (Vec<(usize, usize)>, usize, usize)> {
    (2..=NODES).prop_flat_map(|nodes| (acyclic_edges_over(nodes), 0..nodes, 0..nodes))
}

/// Floyd-Warshall shortest distances: `dist[a][b]` is the fewest needs edges from a to b, `None` when a does not reach b.
fn distances(edges: &[(usize, usize)]) -> Vec<Vec<Option<usize>>> {
    let mut dist = vec![vec![None; NODES]; NODES];
    for &(a, b) in edges {
        dist[a][b] = Some(1);
    }
    for k in 0..NODES {
        for i in 0..NODES {
            for j in 0..NODES {
                if let (Some(ik), Some(kj)) = (dist[i][k], dist[k][j])
                    && dist[i][j].is_none_or(|ij| ik + kj < ij)
                {
                    dist[i][j] = Some(ik + kj);
                }
            }
        }
    }
    dist
}

/// Transitive closure: `reach[a][b]` when a needs b through one or more edges.
fn closure(edges: &[(usize, usize)]) -> Vec<Vec<bool>> {
    distances(edges)
        .into_iter()
        .map(|row| row.into_iter().map(|d| d.is_some()).collect())
        .collect()
}

/// The shortest path from `from` to `to`, built greedily: each step goes to the lowest-numbered neighbour one edge closer to `to`.
fn shortest_path(edges: &[(usize, usize)], from: usize, to: usize) -> Option<Vec<usize>> {
    let dist = distances(edges);
    let have: BTreeSet<(usize, usize)> = edges.iter().copied().collect();
    let mut left = dist[from][to]?;
    let mut path = vec![from];
    let mut at = from;
    while left > 0 {
        at = (0..NODES).find(|&next| {
            have.contains(&(at, next))
                && (next == to && left == 1 || dist[next][to] == Some(left - 1))
        })?;
        path.push(at);
        left -= 1;
    }
    Some(path)
}

/// Repeatedly places the lowest-numbered member whose reachable subset members are all placed.
fn reference_order(reach: &[Vec<bool>], subset: &BTreeSet<usize>) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::new();
    while let Some(&next) = subset.iter().find(|&&a| {
        !order.contains(&a)
            && subset
                .iter()
                .all(|&b| a == b || !reach[a][b] || order.contains(&b))
    }) {
        order.push(next);
    }
    order
}

fn build(edges: &[(usize, usize)]) -> Dag {
    Dag::from_edges(edges.iter().map(|&(a, b)| (t(a), t(b))))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn check_edge_agrees_with_the_reference((edges, task, needs) in graph_and_edge()) {
        let dag = build(&edges);
        let mut with = edges.clone();
        with.push((task, needs));
        let reach = closure(&with);
        let cyclic = (0..NODES).any(|i| reach[i][i]);
        match dag.check_edge(t(task), t(needs)) {
            Ok(()) => prop_assert!(!cyclic),
            Err(EdgeRefusal::SelfDependency) => prop_assert_eq!(task, needs),
            Err(EdgeRefusal::Cycle(path)) => {
                prop_assert!(cyclic && task != needs);
                let mut expected = vec![t(task)];
                if let Some(route) = shortest_path(&edges, needs, task) {
                    expected.extend(route.into_iter().map(t));
                }
                prop_assert_eq!(path, expected);
            }
        }
    }

    #[test]
    fn topological_order_never_precedes_a_prerequisite(edges in acyclic_edges()) {
        let dag = build(&edges);
        let order = dag.topological_order((0..NODES).map(t));
        prop_assert_eq!(order.len(), NODES);
        let position = |task: TaskId| order.iter().position(|x| *x == task).unwrap();
        for &(a, b) in &edges {
            prop_assert!(position(t(b)) < position(t(a)));
        }
    }

    #[test]
    fn topological_order_of_a_subset_respects_transitive_needs(
        edges in acyclic_edges(),
        picks in prop::collection::vec(0..NODES, 0..NODES),
    ) {
        let dag = build(&edges);
        let reach = closure(&edges);
        let subset: BTreeSet<usize> = picks.into_iter().collect();
        let order = dag.topological_order(subset.iter().map(|&i| t(i)));
        prop_assert_eq!(order.len(), subset.len());
        let position = |task: TaskId| order.iter().position(|x| *x == task).unwrap();
        for &a in &subset {
            for &b in &subset {
                if reach[a][b] {
                    prop_assert!(position(t(b)) < position(t(a)));
                }
            }
        }
    }

    #[test]
    fn topological_order_matches_the_reference(
        edges in acyclic_edges(),
        picks in prop::collection::vec(0..NODES, 0..NODES),
    ) {
        let dag = build(&edges);
        let subset: BTreeSet<usize> = picks.into_iter().collect();
        let expected: Vec<TaskId> = reference_order(&closure(&edges), &subset)
            .into_iter()
            .map(t)
            .collect();
        prop_assert_eq!(dag.topological_order(subset.iter().map(|&i| t(i))), expected);
    }

    #[test]
    fn closures_match_the_reference(edges in acyclic_edges(), seed in 0..NODES, other in 0..NODES) {
        let dag = build(&edges);
        let reach = closure(&edges);
        let mut dependents: BTreeSet<TaskId> =
            (0..NODES).filter(|&i| reach[i][seed] || reach[i][other]).map(t).collect();
        dependents.insert(t(seed));
        dependents.insert(t(other));
        prop_assert_eq!(dag.dependent_closure([t(seed), t(other)]), dependents);
    }
}
