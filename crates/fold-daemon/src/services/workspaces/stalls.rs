use std::collections::HashMap;

use fold_core::stall::{WorkspaceFact, is_stalled};
use fold_core::{TaskId, Timestamp, WorkerName};
use fold_store::agents::{self, ClaimKind};
use fold_store::{Store, StoreError};

use super::WorkspaceError;
use crate::projects::ProjectHandle;
use crate::services::projects::load_config;

/// Decides the stalled flag of every live workspace of `project` at `now`, with the project's `stall_minutes`, and sets it in the live state. A flag that changes publishes `workspace.changed`; nothing is stored and nothing else follows. Returns how many flags changed. While the project's agents are paused no flag changes, and after a resume the stall time counts from the resume at the earliest, so the paused time never stalls a worker.
pub async fn check_stalls(
    project: &ProjectHandle,
    now: Timestamp,
) -> Result<usize, WorkspaceError> {
    let stall_minutes = load_config(&project.paths).await?.stall_minutes;
    let live = project.live.all();
    if live.is_empty() {
        return Ok(0);
    }
    let pause = agents::pause_state(project.store.read()).await?;
    if pause.is_paused() {
        return Ok(0);
    }
    let holders = implementation_holders(&project.store).await?;
    let mut changed = 0;
    for state in live {
        let view = &state.view;
        let has_impl_claim = view
            .task
            .and_then(|task| holders.get(&task))
            .is_some_and(|holder| holder.as_str() == view.worker);
        let workspace = if state.missing {
            WorkspaceFact::Missing
        } else {
            WorkspaceFact::Active
        };
        let stalled = is_stalled(
            now,
            has_impl_claim,
            workspace,
            view.last_activity_at
                .max(pause.resumed_at.unwrap_or(view.last_activity_at)),
            stall_minutes,
        );
        if stalled == view.stalled
            || !project
                .live
                .set_stalled(view.workspace, view.last_activity_at, stalled)
        {
            continue;
        }
        changed += 1;
        if stalled {
            tracing::info!(workspace = %view.workspace, worker = %view.worker, "the worker of a workspace is stalled");
        }
    }
    Ok(changed)
}

/// The holder of each unreleased implementation claim by task, read in one read transaction.
async fn implementation_holders(store: &Store) -> Result<HashMap<TaskId, WorkerName>, StoreError> {
    let mut tx = store.read().begin().await?;
    let names: HashMap<i64, WorkerName> = agents::all_workers(&mut *tx)
        .await?
        .into_iter()
        .map(|worker| (worker.id, worker.name))
        .collect();
    let holders = agents::held_claims(&mut *tx)
        .await?
        .into_iter()
        .filter(|claim| claim.kind == ClaimKind::Implementation)
        .filter_map(|claim| Some((claim.task, names.get(&claim.worker_id)?.clone())))
        .collect();
    tx.commit().await?;
    Ok(holders)
}
