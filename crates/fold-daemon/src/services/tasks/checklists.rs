use fold_core::events::types;
use fold_core::{
    Actor, ChecklistItemId, ChecklistName, DependencyOrigin, IdKind, NewEvent, StepText, TaskId,
    TaskStatus, Timestamp, codes,
};
use fold_store::tasks::checklists::{self, ItemRow, NewItem};
use fold_store::tasks::{self, TaskRow};
use fold_store::{StoreError, WriteTx, counters};
use serde_json::json;

pub use fold_store::tasks::checklists::ItemContent;

use super::dependencies::refusal;
use super::{TaskError, TaskService, add_edge_in_tx, recompute, task_not_found};
use crate::services::events::EventBuffer;

/// A new checklist item. Without a `checklist` it goes to the task's first checklist, and a task without one gets `Subtasks`.
#[derive(Debug, Clone)]
pub struct NewChecklistItem {
    pub content: ItemContent,
    pub checklist: Option<ChecklistName>,
}

/// Fields left `None` stay as they are. Only a plain step takes either.
#[derive(Debug, Clone, Default)]
pub struct ChecklistItemPatch {
    pub text: Option<StepText>,
    pub done: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecklistItemView {
    pub id: ChecklistItemId,
    pub task: TaskId,
    pub checklist: ChecklistName,
    pub content: ItemContent,
    pub done_at: Option<Timestamp>,
    pub done_by: Option<String>,
    pub created_at: Timestamp,
}

impl From<ItemRow> for ChecklistItemView {
    fn from(row: ItemRow) -> Self {
        ChecklistItemView {
            id: row.id,
            task: row.task,
            checklist: row.checklist,
            content: row.content,
            done_at: row.done_at,
            done_by: row.done_by,
            created_at: row.created_at,
        }
    }
}

/// One named checklist with its items in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecklistView {
    pub name: ChecklistName,
    pub items: Vec<ChecklistItemView>,
}

/// Ticked items and all items over every checklist of a task, as the board card shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChecklistProgress {
    pub done: u32,
    pub total: u32,
}

impl TaskService {
    /// Adds a plain step or a linked item and recomputes the task in the same transaction. A linked item adds the edge "task needs child" with the checklist origin, so it is refused as a direct edge would be; a child that is already Done ticks the item in the same transaction.
    pub async fn add_checklist_item(
        &self,
        actor: &Actor,
        task: TaskId,
        item: NewChecklistItem,
    ) -> Result<ChecklistItemView, TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let id = self
            .store
            .write(async |tx| {
                let id = add_item_in_tx(tx, &mut buffer, actor, task, &item, now).await?;
                buffer.extend(recompute(tx, &[task], now).await?.events);
                Ok::<_, TaskError>(id)
            })
            .await?;
        self.bus.publish(buffer);
        self.checklist_item(id).await
    }

    /// Edits or ticks a plain step and recomputes its task in the same transaction: ticking the last open step can complete the task by its children. A linked item ticks itself and is refused with `linked_item`.
    pub async fn update_checklist_item(
        &self,
        actor: &Actor,
        item: ChecklistItemId,
        patch: ChecklistItemPatch,
    ) -> Result<ChecklistItemView, TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| update_item_in_tx(tx, &mut buffer, actor, item, &patch, now).await)
            .await?;
        self.bus.publish(buffer);
        self.checklist_item(item).await
    }

    /// Removes an item, with its edge when it is linked, and recomputes its task in the same transaction.
    pub async fn remove_checklist_item(
        &self,
        actor: &Actor,
        item: ChecklistItemId,
    ) -> Result<(), TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| remove_item_in_tx(tx, &mut buffer, actor, item, now).await)
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }

    /// Moves an item to `position`, counted from 0, in `checklist` (its own when `None`), which must belong to the same task; a checklist of that name that does not exist yet is created. A position past the end puts the item last. Both checklists end with dense positions. A move that changes nothing appends no event; any other appends one `checklist.item_updated` naming the item. Order is not a status input, so nothing is recomputed.
    pub async fn move_checklist_item(
        &self,
        actor: &Actor,
        item: ChecklistItemId,
        position: u32,
        checklist: Option<ChecklistName>,
    ) -> Result<ChecklistItemView, TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                move_item_in_tx(
                    tx,
                    &mut buffer,
                    actor,
                    item,
                    position,
                    checklist.as_ref(),
                    now,
                )
                .await
            })
            .await?;
        self.bus.publish(buffer);
        self.checklist_item(item).await
    }

    pub async fn checklist_item(
        &self,
        item: ChecklistItemId,
    ) -> Result<ChecklistItemView, TaskError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        checklists::fetch_item(&mut conn, item)
            .await?
            .map(ChecklistItemView::from)
            .ok_or_else(|| item_not_found(item))
    }

    /// The checklists of `task` in order, each with its items in order. A checklist whose items were all removed stays, empty.
    pub async fn checklists(&self, task: TaskId) -> Result<Vec<ChecklistView>, TaskError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        tasks::fetch(&mut conn, task)
            .await?
            .ok_or_else(|| task_not_found(task))?;
        let mut views: Vec<ChecklistView> = checklists::checklists(&mut conn, task)
            .await?
            .into_iter()
            .map(|name| ChecklistView {
                name,
                items: Vec::new(),
            })
            .collect();
        for row in checklists::items(&mut conn, task).await? {
            if let Some(view) = views.iter_mut().find(|view| view.name == row.checklist) {
                view.items.push(row.into());
            }
        }
        Ok(views)
    }

    pub async fn checklist_progress(&self, task: TaskId) -> Result<ChecklistProgress, TaskError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        tasks::fetch(&mut conn, task)
            .await?
            .ok_or_else(|| task_not_found(task))?;
        let (done, total) = checklists::progress(&mut conn, task).await?;
        Ok(ChecklistProgress { done, total })
    }
}

fn item_not_found(item: ChecklistItemId) -> TaskError {
    refusal(
        codes::CHECKLIST_ITEM_NOT_FOUND,
        format!("{item} does not exist"),
        [("item", json!(item.to_string()))],
        "call get_task to see its checklists",
    )
}

fn archived(task: TaskId) -> TaskError {
    refusal(
        codes::TASK_CLOSED,
        format!("{task} is archived"),
        [
            ("reason", json!("archived")),
            ("task", json!(task.to_string())),
        ],
        "an archived task is read-only; call get_task_graph to pick an open task",
    )
}

/// The open task that owns or takes an item: archived tasks are read-only.
async fn open_task(tx: &mut WriteTx, task: TaskId) -> Result<TaskRow, TaskError> {
    let row = tasks::fetch(tx.conn(), task)
        .await?
        .ok_or_else(|| task_not_found(task))?;
    if row.archived_at.is_some() {
        return Err(archived(task));
    }
    Ok(row)
}

pub(super) async fn add_item_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    task: TaskId,
    item: &NewChecklistItem,
    now: Timestamp,
) -> Result<ChecklistItemId, TaskError> {
    let row = open_task(tx, task).await?;
    if row.status == TaskStatus::Done {
        return Err(refusal(
            codes::TASK_CLOSED,
            format!("{task} is done"),
            [("reason", json!("done")), ("task", json!(task.to_string()))],
            "a Done task takes no new checklist items; call create_task for the follow-up work",
        ));
    }
    let checklist = match &item.checklist {
        Some(name) => name.clone(),
        None => checklists::checklists(tx.conn(), task)
            .await?
            .into_iter()
            .next()
            .unwrap_or_else(ChecklistName::subtasks),
    };
    if let ItemContent::Linked(child) = item.content {
        add_edge_in_tx(
            tx,
            buffer,
            task,
            child,
            DependencyOrigin::Checklist,
            actor,
            now,
        )
        .await?;
    }
    if checklists::ensure_checklist(tx, task, &checklist, now).await? {
        let event = NewEvent::new(types::CHECKLIST_CREATED, actor.event_actor())?
            .task(task)
            .data(json!({"checklist": checklist.as_str()}))?;
        buffer.append(tx, &event, now).await?;
    }
    let number = counters::allocate(tx, IdKind::ChecklistItem).await?;
    let id = ChecklistItemId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
    checklists::insert_item(
        tx,
        &NewItem {
            id,
            task,
            checklist: &checklist,
            content: &item.content,
            now,
        },
    )
    .await?;
    let mut data = json!({"item": id.to_string(), "checklist": checklist.as_str()});
    if let ItemContent::Linked(child) = item.content {
        data["linked_task"] = json!(child.to_string());
    }
    let event = NewEvent::new(types::CHECKLIST_ITEM_ADDED, actor.event_actor())?
        .task(task)
        .data(data)?;
    buffer.append(tx, &event, now).await?;
    Ok(id)
}

async fn update_item_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    item: ChecklistItemId,
    patch: &ChecklistItemPatch,
    now: Timestamp,
) -> Result<(), TaskError> {
    let row = checklists::fetch_item(tx.conn(), item)
        .await?
        .ok_or_else(|| item_not_found(item))?;
    open_task(tx, row.task).await?;
    let current = match &row.content {
        ItemContent::Step(text) => text,
        ItemContent::Linked(child) => {
            return Err(refusal(
                codes::INVALID_REQUEST,
                format!("{item} links {child} and ticks itself when {child} is done"),
                [
                    ("reason", json!("linked_item")),
                    ("item", json!(item.to_string())),
                    ("linked_task", json!(child.to_string())),
                ],
                "a linked item cannot be edited or ticked; remove it with remove_checklist_item",
            ));
        }
    };
    let mut fields = Vec::new();
    if let Some(text) = patch.text.as_ref().filter(|text| *text != current) {
        checklists::set_text(tx, item, text).await?;
        fields.push("text");
    }
    let ticked = row.done_at.is_some();
    let tick = patch.done.filter(|done| *done != ticked);
    match tick {
        Some(true) => {
            checklists::set_done(tx, item, Some((now, actor.event_actor().as_str()))).await?
        }
        Some(false) => {
            checklists::set_done(tx, item, None).await?;
            fields.push("done");
        }
        None => {}
    }
    if !fields.is_empty() {
        let event = NewEvent::new(types::CHECKLIST_ITEM_UPDATED, actor.event_actor())?
            .task(row.task)
            .data(json!({"item": item.to_string(), "fields": fields}))?;
        buffer.append(tx, &event, now).await?;
    }
    if tick == Some(true) {
        let event = NewEvent::new(types::CHECKLIST_ITEM_DONE, actor.event_actor())?
            .task(row.task)
            .data(json!({"item": item.to_string()}))?;
        buffer.append(tx, &event, now).await?;
    }
    if tick.is_some() {
        buffer.extend(recompute(tx, &[row.task], now).await?.events);
    }
    Ok(())
}

async fn remove_item_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    item: ChecklistItemId,
    now: Timestamp,
) -> Result<(), TaskError> {
    let row = checklists::fetch_item(tx.conn(), item)
        .await?
        .ok_or_else(|| item_not_found(item))?;
    open_task(tx, row.task).await?;
    checklists::delete_item(tx, item).await?;
    let mut data = json!({"item": item.to_string()});
    if let ItemContent::Linked(child) = row.content {
        data["linked_task"] = json!(child.to_string());
    }
    let event = NewEvent::new(types::CHECKLIST_ITEM_REMOVED, actor.event_actor())?
        .task(row.task)
        .data(data)?;
    buffer.append(tx, &event, now).await?;
    let edge = match row.content {
        ItemContent::Linked(child) => tasks::fetch_edge(tx.conn(), row.task, child).await?,
        ItemContent::Step(_) => None,
    };
    if let Some(edge) = edge.filter(|edge| edge.origin == DependencyOrigin::Checklist) {
        let child = edge.needs;
        tasks::delete_edge(tx, row.task, child).await?;
        let event = NewEvent::new(types::DEPENDENCY_REMOVED, actor.event_actor())?
            .task(row.task)
            .data(
                json!({"needs": child.to_string(), "origin": DependencyOrigin::Checklist.as_str()}),
            )?;
        buffer.append(tx, &event, now).await?;
    }
    buffer.extend(recompute(tx, &[row.task], now).await?.events);
    Ok(())
}

async fn move_item_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    item: ChecklistItemId,
    position: u32,
    checklist: Option<&ChecklistName>,
    now: Timestamp,
) -> Result<(), TaskError> {
    let row = checklists::fetch_item(tx.conn(), item)
        .await?
        .ok_or_else(|| item_not_found(item))?;
    let task = row.task;
    open_task(tx, task).await?;
    let source = row.checklist;
    let target = checklist.cloned().unwrap_or_else(|| source.clone());
    if target != source && checklists::ensure_checklist(tx, task, &target, now).await? {
        let event = NewEvent::new(types::CHECKLIST_CREATED, actor.event_actor())?
            .task(task)
            .data(json!({"checklist": target.as_str()}))?;
        buffer.append(tx, &event, now).await?;
    }
    let items = checklists::items(tx.conn(), task).await?;
    let order = |name: &ChecklistName| -> Vec<ChecklistItemId> {
        items
            .iter()
            .filter(|other| other.checklist == *name && other.id != item)
            .map(|other| other.id)
            .collect()
    };
    let before = items
        .iter()
        .filter(|other| other.checklist == source)
        .position(|other| other.id == item);
    let mut placed = order(&target);
    let index = usize::try_from(position)
        .unwrap_or(usize::MAX)
        .min(placed.len());
    if target == source && before == Some(index) {
        return Ok(());
    }
    placed.insert(index, item);
    let left = order(&source);
    let lists: Vec<(&ChecklistName, &[ChecklistItemId])> = if target == source {
        vec![(&target, &placed)]
    } else {
        vec![(&source, &left), (&target, &placed)]
    };
    checklists::arrange(tx, task, &lists).await?;
    let mut fields = vec!["position"];
    if target != source {
        fields.push("checklist");
    }
    let event = NewEvent::new(types::CHECKLIST_ITEM_UPDATED, actor.event_actor())?
        .task(task)
        .data(json!({
            "item": item.to_string(),
            "fields": fields,
            "checklist": target.as_str(),
            "position": index,
        }))?;
    buffer.append(tx, &event, now).await?;
    Ok(())
}
