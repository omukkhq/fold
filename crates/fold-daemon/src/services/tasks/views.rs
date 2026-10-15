use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use fold_core::claims::is_stale;
use fold_core::{
    ChecklistName, DependencyOrigin, TaskId, TaskStatus, TaskTitle, Timestamp, WorkerName,
    WorkspaceId,
};
use fold_store::StoreError;
use fold_store::tasks::checklists::{self, ItemContent};
use fold_store::tasks::claims;
use fold_store::tasks::{self, BoardSnapshot};

pub use fold_store::tasks::{ChecklistCounts, TaskRef};

use super::attention::{AttentionItem, attention, stalled_tasks};
use super::{
    ChecklistItemView, ClaimView, HumanReviewMark, LabelView, TaskError, TaskService, TaskView,
    label_view, task_not_found, view,
};

/// How many Done cards the board returns: the most recently completed ones. The Done column's `total` counts them all.
pub const BOARD_DONE_LIMIT: usize = 20;

/// A checklist item with the title and status of the task it links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailItem {
    pub item: ChecklistItemView,
    pub linked: Option<TaskRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailChecklist {
    pub name: ChecklistName,
    pub items: Vec<DetailItem>,
}

/// One end of an edge as the task at the other end sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailEdge {
    pub task: TaskRef,
    pub origin: DependencyOrigin,
}

/// Everything one task shows, with public ids only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskDetail {
    pub task: TaskView,
    pub checklists: Vec<DetailChecklist>,
    pub progress: ChecklistCounts,
    /// The tasks this task needs.
    pub needs: Vec<DetailEdge>,
    /// The tasks that need this task.
    pub needed_by: Vec<DetailEdge>,
    pub claim: Option<ClaimView>,
    pub workspace: Option<WorkspaceId>,
    pub attention: Vec<AttentionItem>,
}

/// One node of the task graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphTask {
    pub id: TaskId,
    pub title: TaskTitle,
    pub status: TaskStatus,
    pub blocked: bool,
    pub labels: Vec<LabelView>,
    /// Every direct prerequisite, Done or not.
    pub needs: Vec<TaskId>,
    pub holder: Option<WorkerName>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardCard {
    pub id: TaskId,
    pub title: TaskTitle,
    pub status: TaskStatus,
    pub blocked: bool,
    pub labels: Vec<LabelView>,
    pub checklist: ChecklistCounts,
    /// Every direct prerequisite.
    pub needs: Vec<TaskId>,
    /// The direct prerequisites that are not Done.
    pub blocked_by: Vec<TaskId>,
    /// How many direct dependents that are not Done wait for this task; 0 once it is Done.
    pub blocks: u32,
    pub holder: Option<WorkerName>,
    pub workspace: Option<WorkspaceId>,
    pub human_review: Option<HumanReviewMark>,
    /// How many messages name the task.
    pub messages: u32,
    pub completed_at: Option<Timestamp>,
    pub attention: Vec<AttentionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardColumn {
    pub status: TaskStatus,
    pub cards: Vec<BoardCard>,
    /// Every task with this status; more than `cards.len()` only in a Done column cut at [`BOARD_DONE_LIMIT`].
    pub total: u32,
}

/// The five columns in board order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    pub columns: Vec<BoardColumn>,
}

fn counts_of(items: &[checklists::ItemRow]) -> ChecklistCounts {
    let mut counts = ChecklistCounts::default();
    for item in items {
        let done = u32::from(item.done_at.is_some());
        match item.content {
            ItemContent::Step(_) => {
                counts.steps_total += 1;
                counts.steps_done += done;
            }
            ItemContent::Linked(_) => {
                counts.linked_total += 1;
                counts.linked_done += done;
            }
        }
    }
    counts
}

fn total(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// Read models shared by the MCP tools and the HTTP routes.
impl TaskService {
    /// One task with its checklists, both directions of its edges, its claim and its live workspace, read in one transaction. An archived task is found too. The lease is read only when the task has a claim, so a task without one stays readable under an invalid configuration.
    pub async fn get_view(&self, id: TaskId) -> Result<TaskDetail, TaskError> {
        let now = self.clock.now();
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        let row = tasks::fetch(&mut tx, id)
            .await?
            .ok_or_else(|| task_not_found(id))?;
        let labels = tasks::labels_for_conn(&mut tx, id).await?;
        let names = checklists::checklists(&mut tx, id).await?;
        let items = checklists::items(&mut tx, id).await?;
        let edges = tasks::edges_conn(&mut tx).await?;
        let claim = claims::unreleased_for_task(&mut *tx, id).await?;
        let workspace = tasks::live_workspace(&mut tx, id).await?;
        let needs: Vec<(TaskId, DependencyOrigin)> = edges
            .iter()
            .filter(|edge| edge.task == id)
            .map(|edge| (edge.needs, edge.origin))
            .collect();
        let needed_by: Vec<(TaskId, DependencyOrigin)> = edges
            .iter()
            .filter(|edge| edge.needs == id)
            .map(|edge| (edge.task, edge.origin))
            .collect();
        let mut related: BTreeSet<TaskId> = needs.iter().chain(&needed_by).map(|e| e.0).collect();
        related.extend(items.iter().filter_map(|item| match item.content {
            ItemContent::Linked(child) => Some(child),
            ItemContent::Step(_) => None,
        }));
        let related: Vec<TaskId> = related.into_iter().collect();
        let refs: BTreeMap<TaskId, TaskRef> = tasks::task_refs(&mut tx, &related)
            .await?
            .into_iter()
            .map(|task| (task.id, task))
            .collect();
        tx.commit().await.map_err(StoreError::from)?;

        let edge = |(task, origin): &(TaskId, DependencyOrigin)| {
            refs.get(task).map(|task| DetailEdge {
                task: task.clone(),
                origin: *origin,
            })
        };
        let progress = counts_of(&items);
        let mut checklists: Vec<DetailChecklist> = names
            .into_iter()
            .map(|name| DetailChecklist {
                name,
                items: Vec::new(),
            })
            .collect();
        for item in items {
            let linked = match item.content {
                ItemContent::Linked(child) => refs.get(&child).cloned(),
                ItemContent::Step(_) => None,
            };
            if let Some(list) = checklists
                .iter_mut()
                .find(|list| list.name == item.checklist)
            {
                list.items.push(DetailItem {
                    item: item.into(),
                    linked,
                });
            }
        }
        let stalled = stalled_tasks(self.live.as_ref());
        let attention = attention(&self.store, &[id], &stalled, now)
            .await?
            .remove(&id)
            .unwrap_or_default();
        let claim = match claim {
            Some(claim) => Some(ClaimView {
                task: id,
                stale: is_stale(claim.renewed_at, now, self.lease_minutes().await?),
                holder: claim.worker,
                acquired_at: claim.acquired_at,
                renewed_at: claim.renewed_at,
            }),
            None => None,
        };
        Ok(TaskDetail {
            task: view(row, labels),
            checklists,
            progress,
            needs: needs.iter().filter_map(edge).collect(),
            needed_by: needed_by.iter().filter_map(edge).collect(),
            claim,
            workspace,
            attention,
        })
    }

    /// The tasks that are not archived, in number order, with their direct prerequisites and claim holders. Done tasks only when `include_done`.
    pub async fn graph(&self, include_done: bool) -> Result<Vec<GraphTask>, TaskError> {
        let mut snapshot = tasks::board_snapshot(self.store.read()).await?;
        let needs = needs_by_task(&snapshot);
        Ok(std::mem::take(&mut snapshot.tasks)
            .into_iter()
            .filter(|row| include_done || row.status != TaskStatus::Done)
            .map(|row| GraphTask {
                needs: needs.get(&row.id).cloned().unwrap_or_default(),
                labels: labels_of(&mut snapshot, row.id),
                holder: snapshot.holders.remove(&row.id),
                id: row.id,
                title: row.title,
                status: row.status,
                blocked: row.blocked,
            })
            .collect())
    }

    /// The five columns in board order. Backlog to Review hold every task in number order; Done holds the [`BOARD_DONE_LIMIT`] most recently completed tasks, newest first, and counts the rest in `total`.
    pub async fn board(&self) -> Result<Board, TaskError> {
        let now = self.clock.now();
        let mut snapshot = tasks::board_snapshot(self.store.read()).await?;
        let ids: Vec<TaskId> = snapshot.tasks.iter().map(|row| row.id).collect();
        let stalled = stalled_tasks(self.live.as_ref());
        let mut attention = attention(&self.store, &ids, &stalled, now).await?;
        let statuses: BTreeMap<TaskId, TaskStatus> = snapshot
            .tasks
            .iter()
            .map(|row| (row.id, row.status))
            .collect();
        let is_done = |task: &TaskId| statuses.get(task) == Some(&TaskStatus::Done);
        let needs = needs_by_task(&snapshot);
        let mut blocks: BTreeMap<TaskId, u32> = BTreeMap::new();
        for edge in &snapshot.edges {
            if !is_done(&edge.task) && !is_done(&edge.needs) {
                *blocks.entry(edge.needs).or_default() += 1;
            }
        }
        let mut columns: Vec<BoardColumn> = TaskStatus::ALL
            .iter()
            .map(|status| BoardColumn {
                status: *status,
                cards: Vec::new(),
                total: 0,
            })
            .collect();
        for row in std::mem::take(&mut snapshot.tasks) {
            let needs = needs.get(&row.id).cloned().unwrap_or_default();
            let card = BoardCard {
                blocked_by: needs
                    .iter()
                    .filter(|need| !is_done(need))
                    .copied()
                    .collect(),
                needs,
                blocks: blocks.get(&row.id).copied().unwrap_or(0),
                labels: labels_of(&mut snapshot, row.id),
                checklist: snapshot
                    .checklists
                    .get(&row.id)
                    .copied()
                    .unwrap_or_default(),
                holder: snapshot.holders.remove(&row.id),
                workspace: snapshot.workspaces.get(&row.id).copied(),
                human_review: row.human_review,
                messages: snapshot.messages.get(&row.id).copied().unwrap_or(0),
                completed_at: snapshot.completions.get(&row.id).copied(),
                attention: attention.remove(&row.id).unwrap_or_default(),
                id: row.id,
                title: row.title,
                status: row.status,
                blocked: row.blocked,
            };
            if let Some(column) = columns.iter_mut().find(|c| c.status == card.status) {
                column.cards.push(card);
            }
        }
        for column in &mut columns {
            column.total = total(column.cards.len());
            if column.status == TaskStatus::Done {
                column
                    .cards
                    .sort_by_key(|card| Reverse((card.completed_at, card.id)));
                column.cards.truncate(BOARD_DONE_LIMIT);
            }
        }
        Ok(Board { columns })
    }
}

fn needs_by_task(snapshot: &BoardSnapshot) -> BTreeMap<TaskId, Vec<TaskId>> {
    let mut needs: BTreeMap<TaskId, Vec<TaskId>> = BTreeMap::new();
    for edge in &snapshot.edges {
        needs.entry(edge.task).or_default().push(edge.needs);
    }
    for list in needs.values_mut() {
        list.sort();
    }
    needs
}

fn labels_of(snapshot: &mut BoardSnapshot, task: TaskId) -> Vec<LabelView> {
    snapshot
        .labels
        .remove(&task)
        .unwrap_or_default()
        .into_iter()
        .map(label_view)
        .collect()
}
