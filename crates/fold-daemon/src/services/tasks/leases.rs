use fold_core::claims::{HolderRuns, is_lapsed, near_stale_cutoff};
use fold_core::events::types;
use fold_core::{EventActor, NewEvent, Timestamp};
use fold_store::reviews::checks;
use fold_store::tasks::claims::{self, ClaimTarget, StaleClaim};
use serde_json::json;

use super::{TaskError, TaskService};
use crate::projects::ProjectHandle;
use crate::services::events::EventBuffer;

impl TaskService {
    /// How many unreleased claims the workers of agent `agent_id` hold that are near stale now (see [`near_stale_cutoff`]), with the project's lease. Every tool call's footer counts them, so the lease comes from [`crate::services::projects::Lease::recent_minutes`].
    pub async fn claims_near_stale(&self, agent_id: i64) -> Result<u64, TaskError> {
        let lease_minutes = self.lease.recent_staleness_minutes(&self.store).await?;
        let Some(cutoff) = near_stale_cutoff(self.clock.now(), lease_minutes) else {
            return Ok(0);
        };
        let count = claims::count_renewed_by(self.store.read(), agent_id, cutoff).await?;
        Ok(count)
    }
}

/// Marks each unreleased claim of `project` whose lease ran out at `now` and that is not marked yet, with one `claim.stale` per claim, in one write transaction; the events reach the bus after the commit. The lease is the project's `lease_minutes`, counted by [`is_lapsed`]: in the same transaction the claims of each holder whose own check run goes are renewed at `now`, because the holder waits for the run and makes no calls, so no claim runs out during the run and every reader of `renewed_at` agrees. A marked claim keeps its holder, its workspace and its task's status: only `takeover_claim` moves it, and a renewal clears the mark. Returns the claims it marked.
pub async fn sweep_leases(
    project: &ProjectHandle,
    now: Timestamp,
) -> Result<Vec<StaleClaim>, TaskError> {
    let tasks = &project.tasks;
    let lease_minutes = tasks.lease_minutes().await?;
    let mut buffer = EventBuffer::new();
    let marked = tasks
        .store
        .write(async |tx| {
            let runs = checks::holder_runs(tx.conn()).await?;
            let running: Vec<i64> = runs
                .iter()
                .filter(|(_, runs)| **runs == HolderRuns::Running)
                .map(|(worker, _)| *worker)
                .collect();
            claims::renew(tx, &running, now).await?;
            let marked = claims::mark_stale(tx, now, |renewed_at, worker| {
                let runs = runs.get(&worker).copied().unwrap_or_default();
                is_lapsed(renewed_at, runs, now, lease_minutes)
            })
            .await?;
            for claim in &marked {
                buffer.append(tx, &stale_event(claim)?, now).await?;
            }
            Ok::<_, TaskError>(marked)
        })
        .await?;
    tasks.bus.publish(buffer);
    for claim in &marked {
        match claim.target {
            ClaimTarget::Task(task) => {
                tracing::info!(task = %task, worker = %claim.worker, "an implementation claim went stale");
            }
            ClaimTarget::Review { proposal, .. } => {
                tracing::info!(proposal = %proposal, worker = %claim.worker, "a review claim went stale");
            }
        }
    }
    Ok(marked)
}

fn stale_event(claim: &StaleClaim) -> Result<NewEvent, TaskError> {
    let event = NewEvent::new(types::CLAIM_STALE, EventActor::fold())?;
    let renewed_at = claim.renewed_at.as_millis();
    let worker = claim.worker.as_str();
    let event = match claim.target {
        ClaimTarget::Task(task) => event.task(task).data(json!({
            "kind": "implementation",
            "worker": worker,
            "renewed_at": renewed_at,
        }))?,
        ClaimTarget::Review { proposal, task } => {
            event.task(task).proposal(proposal).data(json!({
                "kind": "review",
                "proposal": proposal.to_string(),
                "worker": worker,
                "renewed_at": renewed_at,
            }))?
        }
    };
    Ok(event)
}
