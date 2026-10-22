use std::collections::BTreeSet;

use fold_core::events::types;
use fold_core::policy::{self, TaskHolder};
use fold_core::roles::Role;
use fold_core::status::ProposalFact;
use fold_core::{
    Actor, ChecklistItemId, DependencyOrigin, EventActor, FoldError, NewEvent, TaskId, TaskStatus,
    Timestamp, WorkspaceId, codes,
};
use fold_store::tasks::checklists;
use fold_store::tasks::claims::{self, ReleaseReason};
use fold_store::tasks::{self, EdgeRow};
use fold_store::{StoreError, WriteTx, agents, workspaces};
use serde_json::json;

use super::dependencies::refusal;
use super::{TaskError, TaskService, TaskView, recompute, task_not_found};
use crate::services::events::EventBuffer;
use crate::services::workspaces::{self as workspace_service, ArchiveReason};

impl TaskService {
    /// Removes a task that never had a workspace, a proposal, a completion, a report or a message naming it, with its edges, the linked checklist items pointing at it, its labels, its own checklists and its claim rows; a live implementation claim it removes appends `claim.released`. A held task is deleted only by the holder's agent, its lead or a human (`policy::can_delete_task`). A task that a Done task's checklist links is refused with `task_closed`, because a Done task's checklists are locked. Its earlier events stay, and its number is never reused. Dependents recompute in the same transaction.
    pub async fn delete_task(&self, actor: &Actor, id: TaskId) -> Result<(), TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| delete_in_tx(tx, &mut buffer, actor, id, now).await)
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }

    /// Hides a task that has started: removes its edges and the linked checklist items pointing at it, archives its live task workspace and releases its implementation claim. A held task is archived only by the holder's agent, its lead or a human (`policy::can_archive_task`). A Done task's checklists are locked, so a linked item of a Done task and its checklist edge stay. Its rows and events stay, and its status cache stays as it was. Dependents recompute in the same transaction. An open proposal must be closed first; archiving never closes it.
    ///
    /// The operation lock of the live workspace is taken before the transaction, so the archive waits for a sync, restore or record that is writing there, and no command writes in the workspace after it is archived.
    pub async fn archive_task(&self, actor: &Actor, id: TaskId) -> Result<TaskView, TaskError> {
        loop {
            let locked = workspaces::live_task_workspace(self.store.read(), id)
                .await?
                .map(|live| live.id);
            let _op = match locked {
                Some(workspace) => Some(self.ops.lock(workspace).await),
                None => None,
            };
            let now = self.clock.now();
            let mut buffer = EventBuffer::new();
            let step = self
                .store
                .write(async |tx| archive_in_tx(tx, &mut buffer, actor, id, locked, now).await)
                .await?;
            let Archive::Done(archived) = step else {
                continue;
            };
            self.bus.publish(buffer);
            if let (Some(live), Some(workspace)) = (&self.live, archived) {
                live.detach(workspace).await;
            }
            return self.get(id).await;
        }
    }
}

/// What the transaction of an archive did. `Moved` writes nothing: the task's live workspace is no longer the one whose lock the command holds, and the command reads it again.
enum Archive {
    Done(Option<WorkspaceId>),
    Moved,
}

async fn delete_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    acting: &Actor,
    id: TaskId,
    now: Timestamp,
) -> Result<(), TaskError> {
    let actor = &acting.event_actor();
    let row = tasks::fetch(tx.conn(), id)
        .await?
        .ok_or_else(|| task_not_found(id))?;
    let holder = holder_of(tx, id).await?;
    policy::can_delete_task(acting, holder.as_ref()).map_err(|denied| refused(denied, id))?;
    let history = tasks::history(tx.conn(), id).await?;
    if history.any() {
        let kept: Vec<&str> = [
            (history.workspace, "workspace"),
            (history.proposal, "proposal"),
            (history.completion, "completion"),
            (history.message, "message"),
            (history.turn, "turn"),
            (history.review_plan, "review_plan"),
        ]
        .into_iter()
        .filter_map(|(present, name)| present.then_some(name))
        .collect();
        return Err(refusal(
            codes::INVALID_REQUEST,
            format!("{id} has history and cannot be deleted"),
            [
                ("reason", json!("task_has_history")),
                ("task", json!(id.to_string())),
                ("history", json!(kept)),
            ],
            "call archive_task to hide it and keep its history",
        ));
    }
    let owners = tasks::linked_item_owners(tx.conn(), id).await?;
    if let Some(owner) = done_owners(tx, &owners).await?.first() {
        return Err(refusal(
            codes::TASK_CLOSED,
            format!("{owner} is done and its checklist links {id}"),
            [
                ("reason", json!("done_checklist")),
                ("task", json!(id.to_string())),
                ("owner", json!(owner.to_string())),
            ],
            "call archive_task to hide the task; the Done task keeps its checklist item",
        ));
    }
    let seeds = detach(tx, buffer, actor, id, now).await?;
    if let Some(holder) = &holder {
        let event = NewEvent::new(types::CLAIM_RELEASED, actor.clone())?
            .task(id)
            .data(json!({
                "kind": "implementation",
                "worker": holder.worker.as_str(),
                "reason": ReleaseReason::TaskDeleted.as_str(),
            }))?;
        buffer.append(tx, &event, now).await?;
    }
    tasks::delete(tx, id).await?;
    let event = NewEvent::new(types::TASK_DELETED, actor.clone())?
        .task(id)
        .data(json!({"number": id.number(), "title": row.title.as_str()}))?;
    buffer.append(tx, &event, now).await?;
    buffer.extend(recompute(tx, &seeds, now).await?.events);
    Ok(())
}

async fn archive_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    acting: &Actor,
    id: TaskId,
    locked: Option<WorkspaceId>,
    now: Timestamp,
) -> Result<Archive, TaskError> {
    let actor = &acting.event_actor();
    let row = tasks::fetch(tx.conn(), id)
        .await?
        .ok_or_else(|| task_not_found(id))?;
    if row.archived_at.is_some() {
        return Err(refusal(
            codes::TASK_CLOSED,
            format!("{id} is archived"),
            [
                ("reason", json!("archived")),
                ("task", json!(id.to_string())),
            ],
            "the task is already archived; call get_task_graph to pick an open task",
        ));
    }
    let holder = holder_of(tx, id).await?;
    policy::can_archive_task(acting, holder.as_ref()).map_err(|denied| refused(denied, id))?;
    let workspace = workspaces::live_task_workspace(tx.conn(), id).await?;
    if workspace.as_ref().map(|live| live.id) != locked {
        return Ok(Archive::Moved);
    }
    let open_proposal = tasks::load_facts(tx.conn(), &[id])
        .await?
        .pop()
        .is_some_and(|loaded| loaded.facts.proposal != ProposalFact::None);
    if open_proposal {
        return Err(refusal(
            codes::INVALID_REQUEST,
            format!("{id} has an open proposal"),
            [
                ("reason", json!("proposal_open")),
                ("task", json!(id.to_string())),
            ],
            "call close_proposal first, then archive_task",
        ));
    }
    let seeds = detach(tx, buffer, actor, id, now).await?;
    if let Some(live) = &workspace {
        let reason = ArchiveReason::TaskArchived;
        workspace_service::archive_in_tx::<TaskError>(tx, buffer, actor, live.id, reason, now)
            .await?;
    }
    if let Some(claim) = claims::unreleased_for_task(tx.conn(), id).await? {
        let reason = ReleaseReason::TaskArchived;
        claims::release(tx, id, claim.worker_id, reason, now).await?;
        let event = NewEvent::new(types::CLAIM_RELEASED, actor.clone())?
            .task(id)
            .data(json!({
                "kind": "implementation",
                "worker": claim.worker.as_str(),
                "reason": reason.as_str(),
            }))?;
        buffer.append(tx, &event, now).await?;
    }
    tasks::set_archived(tx, id, now).await?;
    let event = NewEvent::new(types::TASK_ARCHIVED, actor.clone())?.task(id);
    buffer.append(tx, &event, now).await?;
    buffer.extend(recompute(tx, &seeds, now).await?.events);
    Ok(Archive::Done(workspace.map(|live| live.id)))
}

/// The worker holding `id`'s unreleased implementation claim and the agents that lead it: the area lead of a task worker's area, or the review lead of a reviewer.
pub(super) async fn holder_of(
    tx: &mut WriteTx,
    id: TaskId,
) -> Result<Option<TaskHolder>, StoreError> {
    let Some(claim) = claims::unreleased_for_task(tx.conn(), id).await? else {
        return Ok(None);
    };
    let process = agents::find_worker_process(tx.conn(), &claim.worker.agent()).await?;
    let leads = match process.map(|process| (process.role, process.area)) {
        Some((Role::TaskWorker, Some(area))) => agents::find_area(tx.conn(), &area)
            .await?
            .and_then(|area| area.lead)
            .into_iter()
            .collect(),
        Some((Role::Reviewer, _)) => agents::list_worker_processes(tx.conn())
            .await?
            .into_iter()
            .filter(|process| process.role == Role::ReviewLead)
            .map(|process| process.agent)
            .collect(),
        Some(_) | None => Vec::new(),
    };
    Ok(Some(TaskHolder {
        worker: claim.worker,
        leads,
    }))
}

fn refused(denied: policy::PolicyDenied, id: TaskId) -> TaskError {
    FoldError::from(denied)
        .with_details([("task", json!(id.to_string()))])
        .into()
}

/// The owners in `owners` whose task is Done, in number order. A Done task's checklists are locked, so its linked item and the checklist edge behind it stay when the linked task goes.
async fn done_owners(
    tx: &mut WriteTx,
    owners: &[(TaskId, ChecklistItemId)],
) -> Result<Vec<TaskId>, TaskError> {
    let mut done = BTreeSet::new();
    for (owner, _) in owners {
        let row = tasks::fetch(tx.conn(), *owner)
            .await?
            .ok_or(StoreError::Corrupt("checklists.task_id"))?;
        if row.status == TaskStatus::Done {
            done.insert(*owner);
        }
    }
    Ok(done.into_iter().collect())
}

/// Removes the linked checklist items pointing at `id` and every edge into or out of it, appends their events, and returns the tasks that needed `id`: the seeds of the recompute, collected before the edges go. A Done task's checklist is locked, so its linked item for `id` and its checklist edge to `id` stay.
async fn detach(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &EventActor,
    id: TaskId,
    now: Timestamp,
) -> Result<Vec<TaskId>, TaskError> {
    let all_owners = tasks::linked_item_owners(tx.conn(), id).await?;
    let locked = done_owners(tx, &all_owners).await?;
    let owners: Vec<(TaskId, ChecklistItemId)> = all_owners
        .into_iter()
        .filter(|(owner, _)| !locked.contains(owner))
        .collect();
    let edges: Vec<EdgeRow> = tasks::edges_conn(tx.conn())
        .await?
        .into_iter()
        .filter(|edge| edge.task == id || edge.needs == id)
        .filter(|edge| {
            !(edge.needs == id
                && edge.origin == DependencyOrigin::Checklist
                && locked.contains(&edge.task))
        })
        .collect();
    if locked.is_empty() {
        tasks::delete_linked_items(tx, id).await?;
        tasks::delete_edges_touching(tx, id).await?;
    } else {
        for (_, item) in &owners {
            checklists::delete_item(tx, *item).await?;
        }
        for edge in &edges {
            tasks::delete_edge(tx, edge.task, edge.needs).await?;
        }
    }
    for (owner, item) in &owners {
        let event = NewEvent::new(types::CHECKLIST_ITEM_REMOVED, actor.clone())?
            .task(*owner)
            .data(json!({"item": item.to_string(), "linked_task": id.to_string()}))?;
        buffer.append(tx, &event, now).await?;
    }
    for edge in &edges {
        let event = NewEvent::new(types::DEPENDENCY_REMOVED, actor.clone())?
            .task(edge.task)
            .data(json!({"needs": edge.needs.to_string(), "origin": edge.origin.as_str()}))?;
        buffer.append(tx, &event, now).await?;
    }
    let dependents: BTreeSet<TaskId> = edges
        .iter()
        .filter(|edge| edge.needs == id)
        .map(|edge| edge.task)
        .chain(owners.into_iter().map(|(owner, _)| owner))
        .collect();
    Ok(dependents.into_iter().collect())
}
