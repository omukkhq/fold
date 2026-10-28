use std::collections::HashMap;

use fold_core::reviews::checks::Stage;
use fold_core::stall::{
    ProposalTurn, WorkspaceChecks, WorkspaceFact, is_stalled, present_workspace, stall_start,
};
use fold_core::{ProjectPaths, ProposalId, TaskId, Timestamp, WorkerName, WorkspaceId};
use fold_store::agents::{self, ClaimKind};
use fold_store::reviews::checks;
use fold_store::{Store, StoreError, reviews};

use super::WorkspaceError;
use crate::live::{LiveCheckRun, LiveState};
use crate::projects::ProjectHandle;
use crate::services::projects::load_config;

/// Decides the stalled flag of every live workspace of `project` at `now`, with the project's `stall_minutes`, and sets it in the live state, together with the check run that goes in each workspace. A workspace whose files are its open proposal's newest version never stalls while the proposal waits on others; once it comes back to its author, the stall time counts from that moment at the earliest. A workspace whose checks run never stalls, and the stall time counts from the end of its newest run at the earliest, so the worker gets the whole window to act on the result. A flag or run that changes publishes `workspace.changed`; nothing is stored and nothing else follows. Returns how many flags changed. While the project's agents are paused no flag changes, and after a resume the stall time counts from the resume at the earliest, so the paused time never stalls a worker.
pub async fn check_stalls(
    project: &ProjectHandle,
    now: Timestamp,
) -> Result<usize, WorkspaceError> {
    decide_stalls(&project.store, &project.paths, &project.live, now).await
}

/// [`check_stalls`] on the parts of a project. Decisions of one project run one at a time, so the facts of an older read never replace those of a newer one.
pub(super) async fn decide_stalls(
    store: &Store,
    paths: &ProjectPaths,
    live: &LiveState,
    now: Timestamp,
) -> Result<usize, WorkspaceError> {
    let stall_minutes = load_config(paths).await?.stall_minutes;
    let _turn = live.stall_turn().await;
    let states = live.all();
    if states.is_empty() {
        return Ok(0);
    }
    let inputs = stall_inputs(store).await?;
    let mut changed = 0;
    for state in states {
        let view = &state.view;
        let run = inputs.running.get(&view.workspace).copied();
        live.set_check_run(view.workspace, run);
        if inputs.paused {
            continue;
        }
        let has_impl_claim = view
            .task
            .and_then(|task| inputs.holders.get(&task))
            .is_some_and(|holder| holder.as_str() == view.worker);
        let turn = view
            .task
            .and_then(|task| inputs.turns.get(&task))
            .filter(|(proposal, _)| view.proposal == Some(*proposal))
            .map(|(_, turn)| *turn);
        let workspace = if state.missing {
            WorkspaceFact::Missing
        } else {
            present_workspace(view.edits_after_version, turn)
        };
        let checks = match (run, inputs.ended.get(&view.workspace)) {
            (Some(_), _) => WorkspaceChecks::Running,
            (None, Some(at)) => WorkspaceChecks::Ended { at: *at },
            (None, None) => WorkspaceChecks::None,
        };
        let start = stall_start(view.last_activity_at, turn, checks);
        let stalled = is_stalled(
            now,
            has_impl_claim,
            workspace,
            checks,
            start.max(inputs.resumed_at.unwrap_or(start)),
            stall_minutes,
        );
        if stalled == view.stalled
            || !live.set_stalled(view.workspace, view.last_activity_at, stalled)
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

/// What the stall rule reads from the store for one decision.
struct StallInputs {
    paused: bool,
    resumed_at: Option<Timestamp>,
    holders: HashMap<TaskId, WorkerName>,
    turns: HashMap<TaskId, (ProposalId, ProposalTurn)>,
    /// The proposal-stage run that goes in each workspace.
    running: HashMap<WorkspaceId, LiveCheckRun>,
    /// The end of the newest ended proposal-stage run of each workspace.
    ended: HashMap<WorkspaceId, Timestamp>,
}

/// The pause, the holder of each unreleased implementation claim by task, the turn of each open proposal by task, and the proposal-stage check runs by workspace, read in one read transaction.
async fn stall_inputs(store: &Store) -> Result<StallInputs, StoreError> {
    let mut tx = store.read().begin().await?;
    let pause = agents::pause_state(&mut *tx).await?;
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
    let running = checks::running_runs(&mut *tx)
        .await?
        .into_iter()
        .filter(|run| run.stage == Stage::Proposal)
        .filter_map(|run| {
            let view = LiveCheckRun {
                run: run.id,
                purpose: run.purpose.into(),
                proposal: run.proposal,
                started_at: run.created_at,
            };
            Some((run.workspace?, view))
        })
        .collect();
    let ended = checks::last_proposal_run_ends(&mut *tx)
        .await?
        .into_iter()
        .collect();
    tx.commit().await?;
    Ok(StallInputs {
        paused: pause.is_paused(),
        resumed_at: pause.resumed_at,
        holders,
        turns,
        running,
        ended,
    })
}
