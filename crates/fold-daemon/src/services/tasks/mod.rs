use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use fold_core::events::types;
use fold_core::tasks::areas::{areas_conflict, check_one_area, in_main_area};
use fold_core::{
    Actor, AreaName, ChecklistName, Clock, DependencyOrigin, EventError, FoldError, HoldMark,
    IdKind, LabelColor, LabelName, NewEvent, ProjectPaths, TaskDescription, TaskId, TaskKind,
    TaskStatus, TaskTitle, Timestamp, codes, policy,
};
use fold_store::tasks::{self, NewTask};
use fold_store::{SqliteConnection, Store, StoreError, WriteTx, agents, counters};
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
mod hold;
mod lead_checks;
mod leases;
mod plans;
mod ready;
mod recompute;
mod removal;
mod requests;
mod views;

pub use attention::{AttentionItem, AttentionKind, attention, stalled_tasks};
pub use checklists::{
    ChecklistItemPatch, ChecklistItemView, ChecklistProgress, ChecklistView, ItemContent,
    NewChecklistItem, item_content,
};
pub(crate) use claims::{
    Assign, already_claimed, assign_in_tx, check_claim_right, claim_in_tx, closed_refusal,
    holder_has_ended, worker_row,
};
pub use claims::{ClaimView, Claimed, renew_leases};
pub use dependencies::{Edge, add_edge_in_tx};
pub use fold_core::reviews::HumanReviewMark;
pub(crate) use hold::agent_refusal;
pub use hold::{HumanTaskRights, human_task_rights};
pub use lead_checks::{MAX_NOTE_CHARS, REPORT_LIMIT, ReportView};
pub(crate) use lead_checks::{areas_of, proposal_kind_refusal};
pub use leases::sweep_leases;
pub use ready::ReadyTask;
pub use recompute::{Recomputed, recompute};
pub(crate) use requests::request_missing;
pub use requests::{
    ApprovedRequest, DEFAULT_REQUEST_LIST, MAX_REQUEST_LIST, NewRequestAttachment,
    RequestAttachment, RequestComment, RequestDetail, RequestDraft, RequestFilter, RequestThread,
    RequestView, SubmitDraft,
};
pub(crate) use views::readers_in;
/// Whether `actor` may clear the human review mark, read for a task view. Any agent or human may turn the mark on, but only an actor that may clear it may also change what it says, so this one rule decides whether the view offers the human-review controls.
pub fn can_change_human_review(actor: &Actor) -> bool {
    fold_core::policy::can_clear_human_review(actor).is_ok()
}

pub use views::{
    Activity, BOARD_DONE_LIMIT, Board, BoardCard, BoardColumn, CardProposal, ChangesetSummary,
    ChecklistCounts, DetailChecklist, DetailEdge, DetailItem, GraphTask, HISTORY_LIMIT, TaskDetail,
    TaskGraph, TaskIntegration, TaskProposal, TaskRef, WorkspaceInfo,
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

    /// The code a log line names: the stable code, or `internal_error` for an internal failure.
    pub fn log_code(&self) -> &'static str {
        self.code().unwrap_or(codes::INTERNAL_ERROR)
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
    pub kind: TaskKind,
    /// `None` while the mark is off.
    pub human_review: Option<HumanReviewMark>,
    /// `None` while the task is not on hold.
    pub hold: Option<HoldMark>,
    pub labels: Vec<LabelView>,
    /// The labels hold two or more areas' labels. Every write refuses such a label set, but the database can hold one; such a task's title and description change freely, and each change of its labels must leave at most one area label.
    pub areas_conflict: bool,
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
    /// Where request attachments are stored, with the project's object lock (`ProjectHandle::objects`). Without it a request with attachments is refused.
    objects: Option<ObjectAccess>,
}

#[derive(Clone)]
struct ObjectAccess {
    fold_dir: PathBuf,
    lock: Arc<tokio::sync::RwLock<()>>,
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
            objects: None,
        }
    }

    /// Stores request attachments in the project's object store under the project's object lock, which garbage collection takes for its sweep.
    #[must_use]
    pub fn with_objects(
        mut self,
        paths: &ProjectPaths,
        lock: Arc<tokio::sync::RwLock<()>>,
    ) -> Self {
        self.objects = Some(ObjectAccess {
            fold_dir: paths.fold_dir(),
            lock,
        });
        self
    }

    /// Reads the lease from the project's `.fold/config.toml` on every claim command and every read of a claim; an invalid file fails those calls with its configuration error and leaves every other task call working. The near-stale count of the tool-call footer reads it again only when the file changed (see [`Lease::recent_minutes`]).
    #[must_use]
    pub fn with_project_config(mut self, paths: ProjectPaths) -> Self {
        self.lease = Lease::config(paths);
        self
    }

    async fn lease_minutes(&self) -> Result<u64, TaskError> {
        Ok(self.lease.staleness_minutes(&self.store).await?)
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

    /// [`TaskService::create_task_of_kind`] for a task with a proposal.
    pub async fn create_linked_task(
        &self,
        actor: &Actor,
        input: CreateTask,
        needs: &[TaskId],
        parent: Option<TaskId>,
    ) -> Result<TaskView, TaskError> {
        self.create_task_of_kind(actor, input, TaskKind::Proposal, needs, parent)
            .await
    }

    /// Creates a task of `kind` with a direct edge to each task in `needs` and, with a `parent`, a linked item on the parent's `Subtasks` checklist, so the parent needs the new task. Everything is one transaction: a refusal of any edge, such as a cycle, leaves nothing behind. A task named twice in `needs` gets one edge. A `no_proposal` or `human` task is created only by whoever plans the work (`policy::can_choose_no_proposal`), judged by the areas of its labels.
    pub async fn create_task_of_kind(
        &self,
        actor: &Actor,
        input: CreateTask,
        kind: TaskKind,
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
                if kind != TaskKind::Proposal {
                    let starter = crate::services::agents::starter_of(tx, actor).await?;
                    let areas = areas_of(tx, &input.labels).await?;
                    policy::can_choose_no_proposal(&starter, &areas).map_err(FoldError::from)?;
                }
                let id = create_in_tx(tx, &mut buffer, actor, &input, kind, now).await?;
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
                    checklists::add_item_in_tx(tx, &mut buffer, actor, parent, &item, now)
                        .await
                        .map_err(|error| parent_refusal(error, parent))?;
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

    /// Puts one label on the task, or takes it off, keeping every other label the task has when the write runs. The set is read inside the write, so two label changes never undo each other.
    pub async fn change_label(
        &self,
        actor: &Actor,
        id: TaskId,
        name: LabelName,
        attach: bool,
    ) -> Result<TaskView, TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let mut names: Vec<LabelName> = tasks::labels_for_conn(tx.conn(), id)
                    .await?
                    .into_iter()
                    .map(|label| label.name)
                    .collect();
                if attach {
                    if !names.contains(&name) {
                        names.push(name.clone());
                    }
                } else {
                    names.retain(|existing| *existing != name);
                }
                let patch = TaskPatch {
                    labels: Some(names),
                    ..TaskPatch::default()
                };
                update_in_tx(tx, &mut buffer, actor, id, &patch, now).await
            })
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
        let areas = area_labels(&mut conn).await?;
        Ok(view(row, labels, &areas))
    }

    /// Tasks in number order with their labels, read in one transaction; archived tasks only when `include_archived`.
    pub async fn list(&self, include_archived: bool) -> Result<Vec<TaskView>, TaskError> {
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        let rows = tasks::list_conn(&mut tx, include_archived).await?;
        let mut labels = tasks::labels_by_task(&mut tx, include_archived).await?;
        let areas = area_labels(&mut tx).await?;
        tx.commit().await.map_err(StoreError::from)?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let own = labels.remove(&row.id).unwrap_or_default();
                view(row, own, &areas)
            })
            .collect())
    }

    pub async fn labels(&self) -> Result<Vec<LabelView>, TaskError> {
        Ok(tasks::list_labels(self.store.read())
            .await?
            .into_iter()
            .map(label_view)
            .collect())
    }
}

fn view(
    row: tasks::TaskRow,
    labels: Vec<tasks::LabelRow>,
    areas: &[(AreaName, LabelName)],
) -> TaskView {
    let names: Vec<LabelName> = labels.iter().map(|label| label.name.clone()).collect();
    TaskView {
        areas_conflict: areas_conflict(&names, areas),
        id: row.id,
        title: row.title,
        description: row.description,
        created_by: row.created_by,
        created_at: row.created_at,
        updated_at: row.updated_at,
        archived_at: row.archived_at,
        status: row.status,
        blocked: row.blocked,
        kind: row.kind,
        human_review: row.human_review,
        hold: row.hold,
        labels: labels.into_iter().map(label_view).collect(),
    }
}

/// Every area with the label that puts a task in it, as the one-area rule reads them.
pub(crate) async fn area_labels(
    conn: &mut SqliteConnection,
) -> Result<Vec<(AreaName, LabelName)>, StoreError> {
    Ok(agents::list_areas(conn)
        .await?
        .into_iter()
        .map(|area| (area.name, area.label))
        .collect())
}

/// Refuses a label set that would put a task in two areas, read inside the write that stores the set. A set of fewer than two labels holds at most one area, so it needs no read.
async fn require_one_area(tx: &mut WriteTx, names: &[LabelName]) -> Result<(), TaskError> {
    if names.len() < 2 {
        return Ok(());
    }
    let areas = area_labels(tx.conn()).await?;
    Ok(check_one_area(names, &areas)?)
}

pub(crate) fn task_not_found(id: TaskId) -> TaskError {
    TaskError::Fold(task_missing(id))
}

pub(crate) fn task_missing(id: TaskId) -> FoldError {
    FoldError::new(codes::TASK_NOT_FOUND, format!("{id} does not exist"))
        .with_hint("call get_task_graph to see the tasks")
}

/// A `parent` adds a linked item to the parent's checklist, so a caller without the checklist rights is refused on a create call; the refusal names the parent, because the task it would have created does not exist.
fn parent_refusal(error: TaskError, parent: TaskId) -> TaskError {
    match error {
        TaskError::Fold(error) if error.code() == codes::CHECKLIST_DENIED => {
            FoldError::new(codes::CHECKLIST_DENIED, error.message().to_owned())
                .with_details([("parent", json!(parent.to_string()))])
                .with_hint("leave out parent, or call get_task on the parent and ask its holder, its lead or the main agent to link the new task with add_checklist_item")
                .into()
        }
        other => other,
    }
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

/// Creates a task of `kind` and its label links inside the caller's transaction and appends `task.created`, whose data names the kind of a task without a proposal or a human task. A task created in Main gets fold's human review mark ([`mark_main_task`]). Task creation, request approval and the sync task of a conflicted pull call it, so each adds its edges and links in the same transaction. The caller runs `recompute` on the new task before it commits, once every edge is in place.
pub(crate) async fn create_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    input: &CreateTask,
    kind: TaskKind,
    now: Timestamp,
) -> Result<TaskId, TaskError> {
    let id = insert_in_tx(tx, buffer, actor, input, kind, now).await?;
    mark_main_task(tx, buffer, id, in_main_area(&input.labels), now).await?;
    Ok(id)
}

/// [`create_in_tx`] without the mark of Main, for a plan, which sets its own marks first.
pub(crate) async fn insert_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    input: &CreateTask,
    kind: TaskKind,
    now: Timestamp,
) -> Result<TaskId, TaskError> {
    require_labels(tx, &input.labels).await?;
    require_one_area(tx, &input.labels).await?;
    let number = counters::allocate(tx, IdKind::Task).await?;
    let id = TaskId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
    tasks::insert(
        tx,
        &NewTask {
            number: id,
            title: &input.title,
            description: &input.description,
            created_by: actor.event_actor().as_str(),
            kind,
            now,
        },
    )
    .await?;
    tasks::replace_labels(tx, id, &input.labels).await?;
    let labels: BTreeSet<&str> = input.labels.iter().map(LabelName::as_str).collect();
    let mut data = json!({"labels": labels});
    if kind != TaskKind::Proposal {
        data["kind"] = json!(kind.as_str());
    }
    let event = NewEvent::new(types::TASK_CREATED, actor.event_actor())?
        .task(id)
        .data(data)?;
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
                .with_details([("reason", json!("archived"))])
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
    let mut labels_added: Vec<&str> = Vec::new();
    let mut leaving_main = false;
    let new_labels = match &patch.labels {
        Some(names) => {
            let current: BTreeSet<LabelName> = tasks::labels_for_conn(tx.conn(), id)
                .await?
                .into_iter()
                .map(|label| label.name)
                .collect();
            let wanted: BTreeSet<LabelName> = names.iter().cloned().collect();
            leaving_main =
                current.iter().any(LabelName::is_main) && !wanted.iter().any(LabelName::is_main);
            labels_added = names
                .iter()
                .filter(|name| !current.contains(*name))
                .map(LabelName::as_str)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
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
    if let Some(names) = &patch.labels {
        require_one_area(tx, names).await?;
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
    let entering_main = labels_added.contains(&fold_core::MAIN_AREA);
    let event = NewEvent::new(types::TASK_UPDATED, actor.event_actor())?
        .task(id)
        .data(json!({"fields": fields, "labels_added": labels_added}))?;
    buffer.append(tx, &event, now).await?;
    mark_main_task(tx, buffer, id, entering_main, now).await?;
    unmark_main_task(tx, buffer, id, leaving_main, now).await?;
    crate::services::reviews::settle_plans(tx, buffer, now).await?;
    Ok(())
}

/// Takes fold's own mark of Main off a task that leaves Main (`leaving`), with `task.human_review_cleared` by fold, so the review agents review it again; a mark a person set stays (`policy::main_area_unmark`).
async fn unmark_main_task(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    task: TaskId,
    leaving: bool,
    now: Timestamp,
) -> Result<(), TaskError> {
    if !leaving {
        return Ok(());
    }
    let current = tasks::fetch(tx.conn(), task)
        .await?
        .ok_or_else(|| task_not_found(task))?
        .human_review;
    if !policy::main_area_unmark(current.as_ref(), leaving) {
        return Ok(());
    }
    tasks::clear_human_review(tx, task, now).await?;
    let event = NewEvent::new(
        types::TASK_HUMAN_REVIEW_CLEARED,
        fold_core::EventActor::fold(),
    )?
    .task(task)
    .data(json!({"reviewer": null, "reason": policy::MAIN_AREA_REASON}))?;
    buffer.append(tx, &event, now).await?;
    Ok(())
}

/// Gives a task that comes into Main (`entering`) fold's human review mark, with `task.human_review_set`, unless it has a mark: no review agent reviews a task of Main, so a human approves it until a human clears the mark (`policy::main_area_mark`).
pub(crate) async fn mark_main_task(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    task: TaskId,
    entering: bool,
    now: Timestamp,
) -> Result<(), TaskError> {
    if !entering {
        return Ok(());
    }
    let current = tasks::fetch(tx.conn(), task)
        .await?
        .ok_or_else(|| task_not_found(task))?
        .human_review;
    let Some(mark) = policy::main_area_mark(current.as_ref(), entering) else {
        return Ok(());
    };
    tasks::set_human_review(tx, task, &mark, now).await?;
    let event = NewEvent::new(types::TASK_HUMAN_REVIEW_SET, mark.set_by.clone())?
        .task(task)
        .data(json!({
            "reviewer": null,
            "reason": mark.reason,
            "set_by": mark.set_by.as_str(),
        }))?;
    buffer.append(tx, &event, now).await?;
    Ok(())
}

/// Whether `task` carries the label of Main, read inside the caller's snapshot.
pub(crate) async fn task_in_main_area(
    conn: &mut SqliteConnection,
    task: TaskId,
) -> Result<bool, StoreError> {
    let labels: Vec<LabelName> = tasks::labels_for_conn(conn, task)
        .await?
        .into_iter()
        .map(|label| label.name)
        .collect();
    Ok(in_main_area(&labels))
}
