use fold_core::events::types;
use fold_core::{AgentActor, EventActor, NewEvent, TaskId, Timestamp, WorkerName, WorkspaceId};
use fold_store::agents::WorkerRow;
use fold_store::reviews;
use fold_store::tasks::claims;
use fold_store::workspaces::{self, WorkspaceRow};
use fold_store::{StoreError, WriteTx};
use serde_json::json;

use super::lifecycle::workspace_exists;
use super::{WorkspaceError, WorkspaceService, WorkspaceView};
use crate::services::events::EventBuffer;
use crate::services::tasks::{TaskError, claim_in_tx, recompute, worker_row};

/// Why a workspace changed owner, as `workspace.adopted` records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptReason {
    /// The previous owner released its claim, or never had one, and the next claimant took the workspace.
    Released,
    /// `takeover_claim` moved the stale claim, and the workspace went with it.
    TakenOver,
    /// The main tag moved to another session, and the new main agent took the main workspace.
    MainAgent,
}

impl AdoptReason {
    pub fn as_str(self) -> &'static str {
        match self {
            AdoptReason::Released => "released",
            AdoptReason::TakenOver => "taken_over",
            AdoptReason::MainAgent => "main_agent",
        }
    }
}

/// Gives the live task workspace of `task` to `worker` inside the caller's transaction, when it is `workspace` and orphaned: its `worker_id` is null, or its worker does not hold the task's unreleased implementation claim. The caller has given that claim to `worker` earlier in the same transaction; a worker that does not hold it adopts nothing. The caller holds the operation lock of `workspace` from before the transaction, so no command of the old owner is writing its files. Only the owner changes: the directory, the base, the changes and the conflicted paths stay, and no file is touched. The task's open proposal goes to `worker` with the workspace. Appends `workspace.adopted` and returns the adopted workspace; `None`, with nothing changed, when there is nothing to adopt. The caller runs `recompute` for the task.
pub(crate) async fn adopt_if_orphaned(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    task: TaskId,
    workspace: WorkspaceId,
    worker: &WorkerRow,
    reason: AdoptReason,
    now: Timestamp,
) -> Result<Option<WorkspaceRow>, TaskError> {
    let Some(live) = workspaces::live_task_workspace(tx.conn(), task).await? else {
        return Ok(None);
    };
    if live.id != workspace {
        return Ok(None);
    }
    let holder = claims::unreleased_for_task(tx.conn(), task)
        .await?
        .map(|claim| claim.worker_id);
    if holder != Some(worker.id) || live.worker_id == holder {
        return Ok(None);
    }
    if !workspaces::set_worker(tx, live.id, worker.id).await? {
        return Ok(None);
    }
    let event = NewEvent::new(types::WORKSPACE_ADOPTED, EventActor::worker(&worker.name))?
        .task(task)
        .workspace(live.id)
        .data(json!({
            "reason": reason.as_str(),
            "from": live.worker.as_ref().map(WorkerName::as_str),
            "to": worker.name.as_str(),
        }))?;
    buffer.append(tx, &event, now).await?;
    hand_over_proposal(tx, buffer, task, worker, reason, now).await?;
    Ok(Some(WorkspaceRow {
        worker_id: Some(worker.id),
        worker: Some(worker.name.clone()),
        ..live
    }))
}

/// Makes `worker` the author of the open proposal of `task` inside the caller's transaction, so the worker that now holds the task's claim and workspace can add the next version: only the author's worker may, and a second proposal for the task is refused while this one is open. Appends `proposal.author_changed`; nothing changes when the task has no open proposal or `worker` already writes it. Versions, reviews and approvals stay with the proposal.
pub(crate) async fn hand_over_proposal(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    task: TaskId,
    worker: &WorkerRow,
    reason: AdoptReason,
    now: Timestamp,
) -> Result<(), TaskError> {
    let Some(proposal) = reviews::open_for_task(tx.conn(), task).await? else {
        return Ok(());
    };
    let row = reviews::get_proposal(tx.conn(), proposal)
        .await?
        .ok_or(StoreError::Corrupt("proposals.number"))?;
    if row.author_worker_id == worker.id
        || !reviews::set_author(tx, proposal, worker.id, now).await?
    {
        return Ok(());
    }
    let event = NewEvent::new(
        types::PROPOSAL_AUTHOR_CHANGED,
        EventActor::worker(&worker.name),
    )?
    .task(task)
    .proposal(proposal)
    .data(json!({
        "reason": reason.as_str(),
        "from": row.author.as_str(),
        "to": worker.name.as_str(),
    }))?;
    buffer.append(tx, &event, now).await?;
    Ok(())
}

impl WorkspaceService {
    /// The create path for a task whose live workspace is orphaned: the claim is taken, or renewed for its holder, and the workspace adopted in one transaction, so nothing is materialised. `None`, with nothing written, when the task has no live workspace any more by the time the transaction runs; the caller then creates one.
    pub(super) async fn adopt_on_create(
        &self,
        claimant: &AgentActor,
        task: TaskId,
        orphan: WorkspaceId,
        now: Timestamp,
        lease_minutes: u64,
    ) -> Result<Option<WorkspaceView>, WorkspaceError> {
        let _op = self.ops.lock(orphan).await;
        let mut buffer = EventBuffer::new();
        let adopted = self
            .store
            .write(async |tx| {
                if workspaces::live_task_workspace(tx.conn(), task)
                    .await?
                    .is_none()
                {
                    return Ok(None);
                }
                let worker = worker_row(tx, claimant).await?;
                claim_in_tx(tx, &mut buffer, &worker, task, now, lease_minutes).await?;
                let adopted = adopt_if_orphaned(
                    tx,
                    &mut buffer,
                    task,
                    orphan,
                    &worker,
                    AdoptReason::Released,
                    now,
                )
                .await?;
                let Some(adopted) = adopted else {
                    let live = workspaces::live_task_workspace(tx.conn(), task)
                        .await?
                        .ok_or(StoreError::Corrupt("workspaces.state"))?;
                    return Err(WorkspaceError::from(workspace_exists(&live)));
                };
                buffer.extend(recompute(tx, &[task], now).await?.events);
                Ok::<_, WorkspaceError>(Some(adopted))
            })
            .await?;
        self.bus.publish(buffer);
        if let (Some(live), Some(adopted)) = (&self.live, &adopted)
            && let Some(worker) = &adopted.worker
        {
            live.set_worker(adopted.id, worker);
        }
        Ok(adopted.map(WorkspaceView::from))
    }
}
