use fold_core::claims::is_stale;
use fold_core::events::types;
use fold_core::{
    Actor, AgentActor, EventActor, FoldError, NewEvent, TaskId, TaskStatus, Timestamp, WorkerName,
    WorkspaceId, codes, policy,
};
use fold_store::agents::{self, WorkerRow};
use fold_store::tasks::TaskRow;
use fold_store::tasks::claims::{self, ClaimRow, ReleaseReason};
use fold_store::{Store, StoreError, WriteTx, tasks, workspaces};
use serde_json::json;
use tokio::sync::OwnedMutexGuard;

use super::{TaskError, TaskService, recompute, task_not_found};
use crate::services::events::EventBuffer;
use crate::services::workspaces::{AdoptReason, adopt_if_orphaned};

/// The implementation claim on a task as callers see it. `stale` is computed at read time from the project's lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimView {
    pub task: TaskId,
    pub holder: WorkerName,
    pub acquired_at: Timestamp,
    pub renewed_at: Timestamp,
    pub stale: bool,
}

/// What `claim_task` and `takeover_claim` return: the claim, and the task's live workspace when it now belongs to the claim's holder, adopted by the command or already its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    pub claim: ClaimView,
    pub workspace: Option<WorkspaceId>,
    /// The command adopted `workspace` from another worker or from no owner.
    pub adopted: bool,
}

fn claim_view_of(row: ClaimRow, now: Timestamp, lease_minutes: u64) -> ClaimView {
    ClaimView {
        task: row.task,
        holder: row.worker,
        acquired_at: row.acquired_at,
        stale: is_stale(row.renewed_at, now, lease_minutes),
        renewed_at: row.renewed_at,
    }
}

/// Implementation claims. `policy::can_claim` admits only an agent, and the claim belongs to the agent's acting worker; the lease is the project's `lease_minutes`, read before the command's transaction.
impl TaskService {
    /// Takes the implementation claim on `task` for the actor's worker. A repeat call by the holder renews the claim and returns it without a second `claim.acquired`. A live workspace of the task that no claim holder owns, left by a worker that released the claim, is adopted with `workspace.adopted`. A claim moves no card; an adoption runs `recompute`, which leaves the status as it is.
    pub async fn claim_task(&self, actor: &Actor, task: TaskId) -> Result<Claimed, TaskError> {
        let claimant = policy::can_claim(actor).map_err(FoldError::from)?;
        let now = self.clock.now();
        let lease_minutes = self.lease_minutes().await?;
        let candidate = self.adoption_lock(task, claimant).await?;
        let mut buffer = EventBuffer::new();
        let claimed = self
            .store
            .write(async |tx| {
                let worker = worker_row(tx, claimant).await?;
                let claim = claim_in_tx(tx, &mut buffer, &worker, task, now, lease_minutes).await?;
                let adopted = match &candidate {
                    Some((workspace, _)) => {
                        let reason = AdoptReason::Released;
                        adopt_if_orphaned(tx, &mut buffer, task, *workspace, &worker, reason, now)
                            .await?
                    }
                    None => None,
                };
                if adopted.is_some() {
                    buffer.extend(recompute(tx, &[task], now).await?.events);
                }
                let was_adopted = adopted.is_some();
                let workspace = holder_workspace(tx, task, &worker, adopted).await?;
                Ok::<_, TaskError>(Claimed {
                    claim,
                    workspace,
                    adopted: was_adopted,
                })
            })
            .await?;
        self.bus.publish(buffer);
        self.adopted_live(&claimed);
        Ok(claimed)
    }

    /// The live workspace of `task` that the claimant might adopt, because another worker or nobody owns it, with its operation lock held. The lock is taken before the command's transaction, so an adoption waits for a sync, restore or record of the old owner that is writing there, and no file changes once the new owner has the workspace.
    async fn adoption_lock(
        &self,
        task: TaskId,
        claimant: &AgentActor,
    ) -> Result<Option<(WorkspaceId, OwnedMutexGuard<()>)>, TaskError> {
        let Some(live) = workspaces::live_task_workspace(self.store.read(), task).await? else {
            return Ok(None);
        };
        if live.worker.as_ref() == Some(&claimant.worker) {
            return Ok(None);
        }
        Ok(Some((live.id, self.ops.lock(live.id).await)))
    }

    /// Names the new owner in the live state of the workspace a claim command adopted, after its commit.
    fn adopted_live(&self, claimed: &Claimed) {
        if let (true, Some(live), Some(workspace)) =
            (claimed.adopted, &self.live, claimed.workspace)
        {
            live.set_worker(workspace, &claimed.claim.holder);
        }
    }

    /// Releases the claim the actor's worker holds on `task`. The task's workspace stays where it is.
    pub async fn release_task(&self, actor: &Actor, task: TaskId) -> Result<(), TaskError> {
        let claimant = policy::can_claim(actor).map_err(FoldError::from)?;
        let now = self.clock.now();
        let lease_minutes = self.lease_minutes().await?;
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let worker = &worker_row(tx, claimant).await?;
                if tasks::fetch(tx.conn(), task).await?.is_none() {
                    return Err(task_not_found(task));
                }
                let Some(claim) = claims::unreleased_for_task(tx.conn(), task).await? else {
                    return Err(FoldError::new(
                        codes::CLAIM_NOT_FOUND,
                        format!("{task} has no claim to release"),
                    )
                    .with_hint("call claim_task to take the task")
                    .into());
                };
                if claim.worker_id != worker.id {
                    let stale = is_stale(claim.renewed_at, now, lease_minutes);
                    return Err(FoldError::new(
                        codes::NOT_CLAIM_HOLDER,
                        format!("{task} is claimed by {}", claim.worker),
                    )
                    .with_details([
                        ("holder", json!(claim.worker.as_str())),
                        ("stale", json!(stale)),
                    ])
                    .with_hint("only the holder releases a claim; call get_task to see the holder")
                    .into());
                }
                let reason = ReleaseReason::Released;
                claims::release(tx, task, worker.id, reason, now).await?;
                let event = NewEvent::new(types::CLAIM_RELEASED, EventActor::worker(&worker.name))?
                    .task(task)
                    .data(json!({
                        "kind": "implementation",
                        "worker": worker.name.as_str(),
                        "reason": reason.as_str(),
                    }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, TaskError>(())
            })
            .await?;
        self.bus.publish(buffer);
        Ok(())
    }

    /// Moves the stale implementation claim on `task` and the task's live workspace to the actor's worker in one transaction: the old claim is released with `taken_over`, the new one inserted, `claim.taken_over` names both workers, and the workspace is adopted with `workspace.adopted`. Only the workspace's owner changes; its directory and files stay as they are. A claim whose lease has not run out is refused with `claim_not_stale`.
    pub async fn takeover_claim(&self, actor: &Actor, task: TaskId) -> Result<Claimed, TaskError> {
        let claimant = policy::can_claim(actor).map_err(FoldError::from)?;
        let now = self.clock.now();
        let lease_minutes = self.lease_minutes().await?;
        let candidate = self.adoption_lock(task, claimant).await?;
        let workspace = candidate.as_ref().map(|(workspace, _)| *workspace);
        let mut buffer = EventBuffer::new();
        let claimed = self
            .store
            .write(async |tx| {
                let worker = worker_row(tx, claimant).await?;
                let step = Takeover {
                    task,
                    workspace,
                    now,
                    lease_minutes,
                };
                takeover_in_tx(tx, &mut buffer, &worker, step).await
            })
            .await?;
        self.bus.publish(buffer);
        self.adopted_live(&claimed);
        Ok(claimed)
    }

    /// The claim on `task` at the clock's current time, or `None` when nobody holds it.
    pub async fn claim_view(&self, task: TaskId) -> Result<Option<ClaimView>, TaskError> {
        let now = self.clock.now();
        if tasks::get_by_number(self.store.read(), task)
            .await?
            .is_none()
        {
            return Err(task_not_found(task));
        }
        match claims::unreleased_for_task(self.store.read(), task).await? {
            Some(row) => Ok(Some(claim_view_of(row, now, self.lease_minutes().await?))),
            None => Ok(None),
        }
    }
}

/// The unreleased implementation claim on `task` read from the read pool, with `stale` decided at `now`.
pub async fn claim_view(
    store: &Store,
    task: TaskId,
    now: Timestamp,
    lease_minutes: u64,
) -> Result<Option<ClaimView>, TaskError> {
    if tasks::get_by_number(store.read(), task).await?.is_none() {
        return Err(task_not_found(task));
    }
    Ok(claims::unreleased_for_task(store.read(), task)
        .await?
        .map(|row| claim_view_of(row, now, lease_minutes)))
}

/// Renews every unreleased claim held by `worker_ids` inside the caller's transaction. The session touch calls it with every worker of the calling agent, so a lease and the agent's last call never disagree.
pub async fn renew_leases(
    tx: &mut WriteTx,
    worker_ids: &[i64],
    now: Timestamp,
) -> Result<u64, StoreError> {
    claims::renew(tx, worker_ids, now).await
}

/// The insert runs before any read of the holder, so the unique index on unreleased implementation claims decides a race and its violation becomes `already_claimed` or, for the holder, a renewal.
pub(crate) async fn claim_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    worker: &WorkerRow,
    task: TaskId,
    now: Timestamp,
    lease_minutes: u64,
) -> Result<ClaimView, TaskError> {
    let row = tasks::fetch(tx.conn(), task)
        .await?
        .ok_or_else(|| task_not_found(task))?;
    if let Some(refusal) = closed_refusal(&row) {
        return Err(refusal);
    }
    match claims::insert_implementation(tx, task, worker.id, now).await {
        Ok(()) => {}
        Err(error) if error.unique_violation() => {
            let holder = claims::unreleased_for_task(tx.conn(), task)
                .await?
                .ok_or(error)?;
            if holder.worker_id == worker.id {
                renew_leases(tx, &[worker.id], now).await?;
                let renewed = ClaimRow {
                    renewed_at: now,
                    stale_at: None,
                    ..holder
                };
                return Ok(claim_view_of(renewed, now, lease_minutes));
            }
            return Err(already_claimed(&holder, now, lease_minutes));
        }
        Err(error) => return Err(error.into()),
    }
    let event = NewEvent::new(types::CLAIM_ACQUIRED, EventActor::worker(&worker.name))?
        .task(task)
        .data(json!({"kind": "implementation", "worker": worker.name.as_str()}))?;
    buffer.append(tx, &event, now).await?;
    Ok(ClaimView {
        task,
        holder: worker.name.clone(),
        acquired_at: now,
        renewed_at: now,
        stale: false,
    })
}

/// The worker row of the claimant, read in the command's transaction. A worker that does not exist is `worker_not_found`.
pub(crate) async fn worker_row(
    tx: &mut WriteTx,
    claimant: &AgentActor,
) -> Result<WorkerRow, TaskError> {
    let not_found = || {
        TaskError::Fold(
            FoldError::new(codes::WORKER_NOT_FOUND, "the worker does not exist")
                .with_hint("call register_worker"),
        )
    };
    let agent = agents::find_agent(tx.conn(), &claimant.agent)
        .await?
        .ok_or_else(not_found)?;
    agents::find_worker(tx.conn(), agent.id, &claimant.worker)
        .await?
        .ok_or_else(not_found)
}

/// `task_closed` for a Done or archived task, which takes no claim; `None` for an open task.
pub(crate) fn closed_refusal(row: &TaskRow) -> Option<TaskError> {
    let reason = if row.archived_at.is_some() {
        "archived"
    } else if row.status == TaskStatus::Done {
        "done"
    } else {
        return None;
    };
    Some(
        FoldError::new(codes::TASK_CLOSED, format!("{} is {reason}", row.id))
            .with_details([("reason", json!(reason))])
            .with_hint("a Done or archived task takes no claim; call get_ready_tasks for open work")
            .into(),
    )
}

/// `already_claimed` naming the other worker that holds `holder`, with the staleness of its lease at `now`.
pub(crate) fn already_claimed(holder: &ClaimRow, now: Timestamp, lease_minutes: u64) -> TaskError {
    let stale = is_stale(holder.renewed_at, now, lease_minutes);
    FoldError::new(
        codes::ALREADY_CLAIMED,
        format!("{} is claimed by {}", holder.task, holder.worker),
    )
    .with_details([
        ("holder", json!(holder.worker.as_str())),
        ("stale", json!(stale)),
    ])
    .with_hint(
        "call takeover_claim when the holder's lease is stale, or call get_ready_tasks for other work",
    )
    .into()
}

/// The inputs of a takeover's transaction. `workspace` is the live workspace whose operation lock the command holds.
struct Takeover {
    task: TaskId,
    workspace: Option<WorkspaceId>,
    now: Timestamp,
    lease_minutes: u64,
}

async fn takeover_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    worker: &WorkerRow,
    step: Takeover,
) -> Result<Claimed, TaskError> {
    let Takeover {
        task,
        workspace,
        now,
        lease_minutes,
    } = step;
    let row = tasks::fetch(tx.conn(), task)
        .await?
        .ok_or_else(|| task_not_found(task))?;
    if let Some(refusal) = closed_refusal(&row) {
        return Err(refusal);
    }
    let Some(holder) = claims::unreleased_for_task(tx.conn(), task).await? else {
        return Err(FoldError::new(
            codes::NOT_CLAIMED,
            format!("{task} has no claim to take over"),
        )
        .with_hint("call claim_task to take the task")
        .into());
    };
    if holder.worker_id == worker.id {
        return Err(FoldError::new(
            codes::ALREADY_HOLDER,
            format!("{task} is already claimed by {}", worker.name),
        )
        .with_details([("holder", json!(worker.name.as_str()))])
        .with_hint("the claim is yours; call get_task to see it")
        .into());
    }
    if !is_stale(holder.renewed_at, now, lease_minutes) {
        return Err(FoldError::new(
            codes::CLAIM_NOT_STALE,
            format!("the claim of {} on {task} is not stale", holder.worker),
        )
        .with_details([
            ("holder", json!(holder.worker.as_str())),
            ("renewed_at", json!(holder.renewed_at)),
            ("lease_minutes", json!(lease_minutes)),
        ])
        .with_hint("wait until the lease runs out, or call get_ready_tasks for other work")
        .into());
    }
    claims::release(tx, task, holder.worker_id, ReleaseReason::TakenOver, now).await?;
    claims::insert_implementation(tx, task, worker.id, now).await?;
    let event = NewEvent::new(types::CLAIM_TAKEN_OVER, EventActor::worker(&worker.name))?
        .task(task)
        .data(json!({
            "kind": "implementation",
            "from": holder.worker.as_str(),
            "to": worker.name.as_str(),
        }))?;
    buffer.append(tx, &event, now).await?;
    let adopted = match workspace {
        Some(workspace) => {
            let reason = AdoptReason::TakenOver;
            adopt_if_orphaned(tx, buffer, task, workspace, worker, reason, now).await?
        }
        None => None,
    };
    buffer.extend(recompute(tx, &[task], now).await?.events);
    let was_adopted = adopted.is_some();
    let workspace = holder_workspace(tx, task, worker, adopted).await?;
    Ok(Claimed {
        adopted: was_adopted,
        claim: ClaimView {
            task,
            holder: worker.name.clone(),
            acquired_at: now,
            renewed_at: now,
            stale: false,
        },
        workspace,
    })
}

/// The workspace `adopted` just now, else the task's live workspace when `worker` already owns it.
async fn holder_workspace(
    tx: &mut WriteTx,
    task: TaskId,
    worker: &WorkerRow,
    adopted: Option<workspaces::WorkspaceRow>,
) -> Result<Option<WorkspaceId>, TaskError> {
    if let Some(adopted) = adopted {
        return Ok(Some(adopted.id));
    }
    Ok(workspaces::live_task_workspace(tx.conn(), task)
        .await?
        .filter(|live| live.worker_id == Some(worker.id))
        .map(|live| live.id))
}
