use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use fold_core::reviews::checks::Stage;
use fold_core::stall::{
    ProposalTurn, WorkerTurn, WorkspaceChecks, WorkspaceFact, is_stalled, present_workspace,
    stall_start,
};
use fold_core::{AgentName, ProjectPaths, ProposalId, TaskId, Timestamp, WorkerName, WorkspaceId};
use fold_store::agents::{self, ClaimKind};
use fold_store::reviews::checks;
use fold_store::tasks::reports;
use fold_store::{Store, StoreError, messages, reviews};

use fold_core::events::types;
use fold_core::{Event, EventActor};
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};

use super::{WorkspaceError, WorkspaceService};
use crate::live::{LiveCheckRun, LiveState};
use crate::projects::ProjectHandle;
use crate::services::events::{BusMessage, EventBus};
use crate::services::projects::load_config;

/// Decides the stalled flag of every live workspace of `project` at `now`, with the project's `stall_minutes`, and sets it in the live state, together with the check run that goes in each workspace. A workspace whose files are its open proposal's newest version never stalls while the proposal waits on others; once it comes back to its author, the stall time counts from that moment at the earliest. A workspace whose checks run never stalls, and the stall time counts from the end of its newest run at the earliest, so the worker gets the whole window to act on the result. A worker never stalls while an ask about its task or one of its permission questions waits for a person, or while its report waits for its lead check, and the stall time counts from a person's newest answer, the newest send-back and the start of its current process at the earliest, so an answer clears the flag and only a new full window of silence sets it again. A flag or run that changes publishes `workspace.changed`; nothing is stored and nothing else follows. Returns how many flags changed, the ones a starting run cleared included. While the project's agents are paused no flag changes except the clear at a run's start, and after a resume the stall time counts from the resume at the earliest, so the paused time never stalls a worker.
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
        if live.set_check_run(view.workspace, run) {
            changed += 1;
        }
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
        let worker = view
            .task
            .zip(WorkerName::parse(&view.worker).ok())
            .map(|(task, worker)| inputs.worker_turn(task, &worker))
            .unwrap_or_default();
        let start = stall_start(view.last_activity_at, turn, checks, worker);
        let stalled = is_stalled(
            now,
            has_impl_claim,
            workspace,
            checks,
            worker,
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

/// Decides the stalled flags of the project's live workspaces again after each commit that can make a worker wait for someone or give it the turn, so a flag goes at once instead of on the stall job's next tick. It ends when the project closes.
pub async fn follow_stall_turns(
    service: WorkspaceService,
    bus: EventBus,
    mut events: broadcast::Receiver<BusMessage>,
) {
    let Some(live) = service.live.clone() else {
        return;
    };
    let mut closing = bus.closing();
    loop {
        let due = tokio::select! {
            biased;
            _ = closing.wait_for(|closed| *closed) => return,
            received = events.recv() => match received {
                Ok(BusMessage::Durable(event)) => gives_a_turn(&event),
                Ok(BusMessage::Live(_)) => false,
                Err(RecvError::Lagged(_)) => true,
                Err(RecvError::Closed) => return,
            },
        };
        if !due {
            continue;
        }
        // Everything queued so far was committed before it was sent, so the decision below covers it.
        while !matches!(
            events.try_recv(),
            Err(TryRecvError::Empty | TryRecvError::Closed)
        ) {}
        if let Err(error) =
            decide_stalls(&service.store, &service.paths, &live, service.clock.now()).await
        {
            tracing::warn!(
                code = error.log_code(),
                "the stalled flags were not decided; the stall job's next tick decides them"
            );
        }
    }
}

/// A message of a person or an agent, which can be an answer or an ask, the closure of an ask, a permission question or its answer, a verdict, a report or its send-back, and a start of a worker process. fold's own messages, such as check results, give no turn.
fn gives_a_turn(event: &Event) -> bool {
    if event.event_type == types::MESSAGE_SENT {
        return event.actor != EventActor::fold();
    }
    [
        types::MESSAGE_ASK_CLOSED,
        types::WORKER_PERMISSION_ASKED,
        types::WORKER_PERMISSION_ANSWERED,
        types::REVIEW_SUBMITTED,
        types::REVIEW_BATCH_SUBMITTED,
        types::TASK_REPORTED,
        types::TASK_SENT_BACK,
        types::WORKER_STARTED,
        types::WORKER_RESUMED,
        types::WORKER_REFRESHED,
    ]
    .contains(&event.event_type)
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
    /// The tasks with an ask that waits for a person.
    asked: BTreeSet<TaskId>,
    /// The agents with a permission question that waits for a person.
    questioned: HashSet<AgentName>,
    /// A person's newest answer about each task.
    task_answers: BTreeMap<TaskId, Timestamp>,
    /// A human's newest message to each worker.
    worker_messages: HashMap<WorkerName, Timestamp>,
    /// The newest permission answer to each agent.
    permission_answers: HashMap<AgentName, Timestamp>,
    /// The last start of each supervised agent's process.
    starts: HashMap<AgentName, Timestamp>,
    /// The reports of each task that has one.
    reports: HashMap<TaskId, reports::ReportTurn>,
}

impl StallInputs {
    /// What `worker`, which works on `task`, waits for and when it was last given the turn.
    fn worker_turn(&self, task: TaskId, worker: &WorkerName) -> WorkerTurn {
        let agent = worker.agent();
        let report = self.reports.get(&task);
        let answered_at = [
            self.task_answers.get(&task),
            self.worker_messages.get(worker),
            self.permission_answers.get(&agent),
        ]
        .into_iter()
        .flatten()
        .max()
        .copied();
        WorkerTurn {
            waits_for_person: self.asked.contains(&task) || self.questioned.contains(&agent),
            waits_for_lead_check: report.is_some_and(|report| report.open),
            answered_at,
            sent_back_at: report.and_then(|report| report.sent_back_at),
            started_at: self.starts.get(&agent).copied(),
        }
    }
}

/// The pause, the holder of each unreleased implementation claim by task, the turn of each open proposal by task, the proposal-stage check runs by workspace, and what each worker waits for and when it was last given the turn, read in one read transaction.
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
    let asked = messages::tasks_awaiting_human(&mut *tx)
        .await?
        .into_keys()
        .collect();
    let questioned = agents::open_questions(&mut *tx)
        .await?
        .into_iter()
        .map(|question| question.agent)
        .collect();
    let task_answers = messages::last_answers_by_task(&mut *tx).await?;
    let worker_messages = messages::last_human_messages_by_worker(&mut *tx).await?;
    let permission_answers = agents::last_answers_by_agent(&mut *tx)
        .await?
        .into_iter()
        .collect();
    let starts = agents::process_starts(&mut *tx)
        .await?
        .into_iter()
        .collect();
    let reports = reports::report_turns(&mut *tx)
        .await?
        .into_iter()
        .map(|report| (report.task, report))
        .collect();
    tx.commit().await?;
    Ok(StallInputs {
        paused: pause.is_paused(),
        resumed_at: pause.resumed_at,
        holders,
        turns,
        running,
        ended,
        asked,
        questioned,
        task_answers,
        worker_messages,
        permission_answers,
        starts,
        reports,
    })
}
