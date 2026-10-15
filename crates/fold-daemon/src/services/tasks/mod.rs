use std::collections::BTreeSet;
use std::sync::Arc;

use fold_core::events::types;
use fold_core::{
    Actor, ChecklistName, Clock, DependencyOrigin, EventError, FoldError, IdKind, LabelColor,
    LabelName, NewEvent, ProjectPaths, TaskDescription, TaskId, TaskStatus, TaskTitle, Timestamp,
    codes,
};
use fold_store::tasks::{self, NewTask};
use fold_store::{Store, StoreError, WriteTx, counters};
use serde_json::json;
use thiserror::Error;

use super::events::{EventBuffer, EventBus};
use super::projects::{Lease, ProjectError};
use crate::live::LiveState;
use crate::services::workspaces::WorkspaceLocks;

mod attention;
mod checklists;
mod claims;
mod dependencies;
mod leases;
mod ready;
mod recompute;
mod removal;
mod views;

pub use attention::{AttentionItem, AttentionKind, attention, stalled_tasks};
pub use checklists::{
    ChecklistItemPatch, ChecklistItemView, ChecklistProgress, ChecklistView, ItemContent,
    NewChecklistItem,
};
pub use claims::{ClaimView, Claimed, claim_view, renew_leases};
pub(crate) use claims::{already_claimed, claim_in_tx, closed_refusal, worker_row};
pub use dependencies::{Edge, add_edge_in_tx};
pub use fold_core::reviews::HumanReviewMark;
pub use leases::sweep_leases;
pub use ready::ReadyTask;
pub use recompute::{Recomputed, recompute};
pub use views::{
    BOARD_DONE_LIMIT, Board, BoardCard, BoardColumn, ChecklistCounts, DetailChecklist, DetailEdge,
    DetailItem, GraphTask, TaskDetail, TaskRef,
};

/// Refusals carry a stable code; anything else is a database failure.
#[derive(Debug, Error)]
pub enum TaskError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A command built an event the declared types refuse: a defect in the command, not in the input.
    #[error(transparent)]
    Event(#[from] EventError),
}

impl From<ProjectError> for TaskError {
    fn from(error: ProjectError) -> Self {
        match error {
            ProjectError::Fold(error) => Self::Fold(error),
            ProjectError::Store(error) => Self::Store(error),
        }
    }
}

impl TaskError {
    /// The stable code of a refusal, `None` for an internal failure.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            TaskError::Fold(error) => Some(error.code()),
            TaskError::Store(error) => error.code(),
            TaskError::Event(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreateTask {
    pub title: TaskTitle,
    pub description: TaskDescription,
    pub labels: Vec<LabelName>,
}

/// Fields left `None` stay as they are. `labels` is the whole new set.
#[derive(Debug, Clone, Default)]
pub struct TaskPatch {
    pub title: Option<TaskTitle>,
    pub description: Option<TaskDescription>,
    pub labels: Option<Vec<LabelName>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelView {
    pub name: LabelName,
    pub color: LabelColor,
    pub created_at: Timestamp,
}

/// A task with public ids only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskView {
    pub id: TaskId,
    pub title: TaskTitle,
    pub description: TaskDescription,
    pub created_by: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub archived_at: Option<Timestamp>,
    pub status: TaskStatus,
    pub blocked: bool,
    /// `None` while the mark is off.
    pub human_review: Option<HumanReviewMark>,
    pub labels: Vec<LabelView>,
}

fn label_view(row: tasks::LabelRow) -> LabelView {
    LabelView {
        name: row.name,
        color: row.color,
        created_at: row.created_at,
    }
}

/// The task commands of one project. Each command is one write transaction; its events reach the bus only after the commit.
#[derive(Clone)]
pub struct TaskService {
    store: Store,
    bus: EventBus,
    clock: Arc<dyn Clock>,
    lease: Lease,
    live: Option<LiveState>,
    /// The project's workspace locks (`ProjectHandle::workspace_locks`): a claim that adopts a workspace, and an archive that closes one, wait for a sync, restore or record there to finish.
    ops: Arc<WorkspaceLocks>,
}

impl TaskService {
    /// The claim lease is `lease_minutes` until [`TaskService::with_project_config`] sets the project's own.
    pub fn new(store: Store, bus: EventBus, clock: Arc<dyn Clock>, lease_minutes: u64) -> Self {
        Self {
            store,
            bus,
            clock,
            lease: Lease::Minutes(lease_minutes),
            live: None,
            ops: Arc::new(WorkspaceLocks::default()),
        }
    }

    /// Reads the lease from the project's `.fold/config.toml` on every claim command and every read of a claim; an invalid file fails those calls with its configuration error and leaves every other task call working.
    #[must_use]
    pub fn with_project_config(mut self, paths: ProjectPaths) -> Self {
        self.lease = Lease::Config(paths);
        self
    }

    async fn lease_minutes(&self) -> Result<u64, TaskError> {
        Ok(self.lease.minutes().await?)
    }

    /// Shares the workspace locks of the project's `WorkspaceService`. A service without them adopts or archives a workspace while a workspace command may still be writing its files.
    #[must_use]
    pub fn with_workspace_locks(mut self, ops: Arc<WorkspaceLocks>) -> Self {
        self.ops = ops;
        self
    }

    /// Lets archiving a task stop the live state of the workspace it archives, and a claim that adopts a workspace name its new owner there.
    #[must_use]
    pub fn with_live(mut self, live: LiveState) -> Self {
        self.live = Some(live);
        self
    }

    pub async fn create_task(
        &self,
        actor: &Actor,
        input: CreateTask,
    ) -> Result<TaskView, TaskError> {
        self.create_linked_task(actor, input, &[], None).await
    }

    /// Creates a task with a direct edge to each task in `needs` and, with a `parent`, a linked item on the parent's `Subtasks` checklist, so the parent needs the new task. Everything is one transaction: a refusal of any edge, such as a cycle, leaves nothing behind. A task named twice in `needs` gets one edge.
    pub async fn create_linked_task(
        &self,
        actor: &Actor,
        input: CreateTask,
        needs: &[TaskId],
        parent: Option<TaskId>,
    ) -> Result<TaskView, TaskError> {
        let now = self.clock.now();
        let mut needs = needs.to_vec();
        let mut seen = BTreeSet::new();
        needs.retain(|need| seen.insert(*need));
        let mut buffer = EventBuffer::new();
        let id = self
            .store
            .write(async |tx| {
                let id = create_in_tx(tx, &mut buffer, actor, &input, now).await?;
                for need in &needs {
                    add_edge_in_tx(
                        tx,
                        &mut buffer,
                        id,
                        *need,
                        DependencyOrigin::Direct,
                        actor,
                        now,
                    )
                    .await?;
                }
                let mut seeds = vec![id];
                if let Some(parent) = parent {
                    let item = NewChecklistItem {
                        content: ItemContent::Linked(id),
                        checklist: Some(ChecklistName::subtasks()),
                    };
                    checklists::add_item_in_tx(tx, &mut buffer, actor, parent, &item, now).await?;
                    seeds.push(parent);
                }
                buffer.extend(recompute(tx, &seeds, now).await?.events);
                Ok::<_, TaskError>(id)
            })
            .await?;
        self.bus.publish(buffer);
        self.get(id).await
    }

    pub async fn update_task(
        &self,
        actor: &Actor,
        id: TaskId,
        patch: TaskPatch,
    ) -> Result<TaskView, TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| update_in_tx(tx, &mut buffer, actor, id, &patch, now).await)
            .await?;
        self.bus.publish(buffer);
        self.get(id).await
    }

    pub async fn create_label(
        &self,
        actor: &Actor,
        name: LabelName,
        color: LabelColor,
    ) -> Result<LabelView, TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                match tasks::insert_label(tx, &name, color, now).await {
                    Ok(()) => {}
                    Err(error) if error.unique_violation() => {
                        return Err(TaskError::Fold(
                            FoldError::new(codes::LABEL_EXISTS, "a label with that name exists")
                                .with_hint("attach the existing label by name, or call create_label with another name"),
                        ));
                    }
                    Err(error) => return Err(error.into()),
                }
                let event = NewEvent::new(types::LABEL_CREATED, actor.event_actor())?
                    .data(json!({"name": name.as_str(), "color": color.as_str()}))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, TaskError>(())
            })
            .await?;
        self.bus.publish(buffer);
        Ok(LabelView {
            name,
            color,
            created_at: now,
        })
    }

    pub async fn get(&self, id: TaskId) -> Result<TaskView, TaskError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let row = tasks::fetch(&mut conn, id)
            .await?
            .ok_or_else(|| task_not_found(id))?;
        let labels = tasks::labels_for_conn(&mut conn, id).await?;
        Ok(view(row, labels))
    }

    /// Tasks in number order; archived tasks only when `include_archived`.
    pub async fn list(&self, include_archived: bool) -> Result<Vec<TaskView>, TaskError> {
        let rows = tasks::list(self.store.read(), include_archived).await?;
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let mut views = Vec::with_capacity(rows.len());
        for row in rows {
            let labels = tasks::labels_for_conn(&mut conn, row.id).await?;
            views.push(view(row, labels));
        }
        Ok(views)
    }

    pub async fn labels(&self) -> Result<Vec<LabelView>, TaskError> {
        Ok(tasks::list_labels(self.store.read())
            .await?
            .into_iter()
            .map(label_view)
            .collect())
    }
}

fn view(row: tasks::TaskRow, labels: Vec<tasks::LabelRow>) -> TaskView {
    TaskView {
        id: row.id,
        title: row.title,
        description: row.description,
        created_by: row.created_by,
        created_at: row.created_at,
        updated_at: row.updated_at,
        archived_at: row.archived_at,
        status: row.status,
        blocked: row.blocked,
        human_review: row.human_review,
        labels: labels.into_iter().map(label_view).collect(),
    }
}

pub(crate) fn task_not_found(id: TaskId) -> TaskError {
    TaskError::Fold(
        FoldError::new(codes::TASK_NOT_FOUND, format!("{id} does not exist"))
            .with_hint("call get_task_graph to see the tasks"),
    )
}

async fn require_labels(tx: &mut WriteTx, names: &[LabelName]) -> Result<(), TaskError> {
    let missing = tasks::missing_labels(tx.conn(), names).await?;
    if missing.is_empty() {
        return Ok(());
    }
    let names: Vec<&str> = missing.iter().map(LabelName::as_str).collect();
    Err(TaskError::Fold(
        FoldError::new(codes::LABEL_NOT_FOUND, "a label does not exist")
            .with_details([("labels", json!(names))])
            .with_hint("call create_label for each missing label, then retry"),
    ))
}

/// Creates a task and its label links inside the caller's transaction and appends `task.created`. The tool card calls this to create a task, its edges and its parent link in one transaction. The caller runs `recompute` on the new task before it commits, once every edge is in place.
pub(crate) async fn create_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    input: &CreateTask,
    now: Timestamp,
) -> Result<TaskId, TaskError> {
    require_labels(tx, &input.labels).await?;
    let number = counters::allocate(tx, IdKind::Task).await?;
    let id = TaskId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
    tasks::insert(
        tx,
        &NewTask {
            number: id,
            title: &input.title,
            description: &input.description,
            created_by: actor.event_actor().as_str(),
            now,
        },
    )
    .await?;
    tasks::replace_labels(tx, id, &input.labels).await?;
    let labels: BTreeSet<&str> = input.labels.iter().map(LabelName::as_str).collect();
    let event = NewEvent::new(types::TASK_CREATED, actor.event_actor())?
        .task(id)
        .data(json!({"labels": labels}))?;
    buffer.append(tx, &event, now).await?;
    Ok(id)
}

async fn update_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    id: TaskId,
    patch: &TaskPatch,
    now: Timestamp,
) -> Result<(), TaskError> {
    let row = tasks::fetch(tx.conn(), id)
        .await?
        .ok_or_else(|| task_not_found(id))?;
    if row.archived_at.is_some() {
        return Err(TaskError::Fold(
            FoldError::new(codes::TASK_CLOSED, format!("{id} is archived"))
                .with_hint("an archived task is read-only; call create_task for new work"),
        ));
    }
    if let Some(names) = &patch.labels {
        require_labels(tx, names).await?;
    }
    let title = patch.title.as_ref().filter(|title| **title != row.title);
    let description = patch
        .description
        .as_ref()
        .filter(|description| **description != row.description);
    let new_labels = match &patch.labels {
        Some(names) => {
            let current: BTreeSet<LabelName> = tasks::labels_for_conn(tx.conn(), id)
                .await?
                .into_iter()
                .map(|label| label.name)
                .collect();
            let wanted: BTreeSet<LabelName> = names.iter().cloned().collect();
            (wanted != current).then_some(names)
        }
        None => None,
    };
    let mut fields = Vec::new();
    if title.is_some() {
        fields.push("title");
    }
    if description.is_some() {
        fields.push("description");
    }
    if new_labels.is_some() {
        fields.push("labels");
    }
    if fields.is_empty() {
        return Ok(());
    }
    if title.is_some() || description.is_some() {
        tasks::update_text(
            tx,
            id,
            title.unwrap_or(&row.title),
            description.unwrap_or(&row.description),
            now,
        )
        .await?;
    } else {
        tasks::touch(tx, id, now).await?;
    }
    if let Some(names) = new_labels {
        tasks::replace_labels(tx, id, names).await?;
    }
    let event = NewEvent::new(types::TASK_UPDATED, actor.event_actor())?
        .task(id)
        .data(json!({"fields": fields}))?;
    buffer.append(tx, &event, now).await?;
    Ok(())
}
