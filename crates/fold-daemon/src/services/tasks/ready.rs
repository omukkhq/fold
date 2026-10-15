use std::collections::BTreeMap;

use fold_core::dag::Dag;
use fold_core::readiness::{Candidate, ready_order, unblock_count};
use fold_core::{TaskId, TaskTitle, Timestamp};
use fold_store::tasks;

use super::{LabelView, TaskError, TaskService, label_view};

/// One task an agent can claim now. `unblocks` is how many direct dependents only this task holds back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyTask {
    pub id: TaskId,
    pub title: TaskTitle,
    pub labels: Vec<LabelView>,
    pub unblocks: usize,
    pub created_at: Timestamp,
}

impl TaskService {
    /// The Todo tasks that are not archived and that no one holds an implementation claim on, the one that frees the most tasks first, then the oldest, then the lowest number.
    pub async fn ready(&self) -> Result<Vec<ReadyTask>, TaskError> {
        let snapshot = tasks::ready_snapshot(self.store.read()).await?;
        let dag = Dag::from_edges(snapshot.edges);
        let mut rows: Vec<(usize, tasks::ReadyRow)> = snapshot
            .candidates
            .into_iter()
            .map(|row| (unblock_count(row.id, &dag, &snapshot.statuses), row))
            .collect();
        let candidates: Vec<Candidate> = rows
            .iter()
            .map(|(unblocks, row)| Candidate {
                task: row.id,
                unblock_count: *unblocks,
                created_at: row.created_at,
            })
            .collect();
        let rank: BTreeMap<TaskId, usize> = ready_order(&candidates)
            .into_iter()
            .enumerate()
            .map(|(rank, task)| (task, rank))
            .collect();
        rows.sort_by_key(|(_, row)| rank.get(&row.id).copied());
        Ok(rows
            .into_iter()
            .map(|(unblocks, row)| ReadyTask {
                id: row.id,
                title: row.title,
                labels: row.labels.into_iter().map(label_view).collect(),
                unblocks,
                created_at: row.created_at,
            })
            .collect())
    }
}
