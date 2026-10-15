use std::collections::BTreeSet;

use fold_core::events::types;
use fold_core::status::ProposalFact;
use fold_core::{Actor, EventActor, NewEvent, TaskId, Timestamp, codes};
use fold_store::tasks::claims::{self, ReleaseReason};
use fold_store::tasks::{self, EdgeRow};
use fold_store::{WriteTx, workspaces};
use serde_json::json;

use super::dependencies::refusal;
use super::{TaskError, TaskService, TaskView, recompute, task_not_found};
use crate::services::events::EventBuffer;
use crate::services::workspaces::{self as workspace_service, ArchiveReason};

impl TaskService {
    /// Removes a task that never had a workspace, a proposal, a completion or a message naming it, with its edges, the linked checklist items pointing at it, its labels, its own checklists and its claim rows. Its earlier events stay, and its number is never reused. Dependents recompute in the same transaction.
    pub async fn delete_task(&self, actor: &Actor, id: TaskId) -> Result<(), TaskError> {
        let actor = &actor.event_actor();
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| delete_in_tx(tx, &mut buffer, actor, id, now).await)
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }

    /// Hides a task that has started: removes its edges and the linked checklist items pointing at it, archives its live task workspace and releases its implementation claim. Its rows and events stay, and its status cache stays as it was. Dependents recompute in the same transaction. An open proposal must be closed first; archiving never closes it.
    pub async fn archive_task(&self, actor: &Actor, id: TaskId) -> Result<TaskView, TaskError> {
        let actor = &actor.event_actor();
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| archive_in_tx(tx, &mut buffer, actor, id, now).await)
            .await?;
        self.bus.publish(buffer);
        self.get(id).await
    }
}

async fn delete_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &EventActor,
    id: TaskId,
    now: Timestamp,
) -> Result<(), TaskError> {
    let row = tasks::fetch(tx.conn(), id)
        .await?
        .ok_or_else(|| task_not_found(id))?;
    let history = tasks::history(tx.conn(), id).await?;
    if history.any() {
        let kept: Vec<&str> = [
            (history.workspace, "workspace"),
            (history.proposal, "proposal"),
            (history.completion, "completion"),
            (history.message, "message"),
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
    let seeds = detach(tx, buffer, actor, id, now).await?;
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
    actor: &EventActor,
    id: TaskId,
    now: Timestamp,
) -> Result<(), TaskError> {
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
    let open_proposal = tasks::load_facts(tx, &[id])
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
    if let Some(live) = workspaces::live_task_workspace(tx.conn(), id).await? {
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
    Ok(())
}

/// Removes the linked checklist items pointing at `id` and every edge into or out of it, appends their events, and returns the tasks that needed `id`: the seeds of the recompute, collected before the edges go.
async fn detach(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &EventActor,
    id: TaskId,
    now: Timestamp,
) -> Result<Vec<TaskId>, TaskError> {
    let edges: Vec<EdgeRow> = tasks::edges_conn(tx.conn())
        .await?
        .into_iter()
        .filter(|edge| edge.task == id || edge.needs == id)
        .collect();
    let owners = tasks::linked_item_owners(tx.conn(), id).await?;
    tasks::delete_linked_items(tx, id).await?;
    tasks::delete_edges_touching(tx, id).await?;
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
