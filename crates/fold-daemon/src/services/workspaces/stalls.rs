use std::collections::HashMap;

use fold_core::stall::{ProposalTurn, WorkspaceFact, is_stalled, present_workspace, stall_start};
use fold_core::{ProposalId, TaskId, Timestamp, WorkerName};
use fold_store::agents::{self, ClaimKind};
use fold_store::{Store, StoreError, reviews};

use super::WorkspaceError;
use crate::projects::ProjectHandle;
use crate::services::projects::load_config;

/// Decides the stalled flag of every live workspace of `project` at `now`, with the project's `stall_minutes`, and sets it in the live state. A workspace whose files are its open proposal's newest version never stalls while the proposal waits on others; once it comes back to its author, the stall time counts from that moment at the earliest. A flag that changes publishes `workspace.changed`; nothing is stored and nothing else follows. Returns how many flags changed. While the project's agents are paused no flag changes, and after a resume the stall time counts from the resume at the earliest, so the paused time never stalls a worker.
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
    let (holders, turns) = stall_inputs(&project.store).await?;
    let mut changed = 0;
    for state in live {
        let view = &state.view;
        let has_impl_claim = view
            .task
            .and_then(|task| holders.get(&task))
            .is_some_and(|holder| holder.as_str() == view.worker);
        let turn = view
            .task
            .and_then(|task| turns.get(&task))
            .filter(|(proposal, _)| view.proposal == Some(*proposal))
            .map(|(_, turn)| *turn);
        let workspace = if state.missing {
            WorkspaceFact::Missing
        } else {
            present_workspace(view.edits_after_version, turn)
        };
        let start = stall_start(view.last_activity_at, turn);
        let stalled = is_stalled(
            now,
            has_impl_claim,
            workspace,
            start.max(pause.resumed_at.unwrap_or(start)),
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

/// The holder of each unreleased implementation claim by task, and the turn of each open proposal by task, read in one read transaction.
async fn stall_inputs(store: &Store) -> Result<(Holders, Turns), StoreError> {
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
    let turns = reviews::open_turns(&mut tx)
        .await?
        .into_iter()
        .map(|open| {
            let turn = ProposalTurn::of(open.changes_requested_at, open.flagged_at);
            (open.task, (open.proposal, turn))
        })
        .collect();
    tx.commit().await?;
    Ok((holders, turns))
}

type Holders = HashMap<TaskId, WorkerName>;
type Turns = HashMap<TaskId, (ProposalId, ProposalTurn)>;
