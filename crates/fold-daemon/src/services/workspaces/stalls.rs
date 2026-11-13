use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use fold_core::reviews::checks::Stage;
use fold_core::roles::{OwedResume, Role, WorkerLimits, waits_for_place};
use fold_core::stall::{
    Activity, OwedTurn, ProposalTurn, Slot, StallFacts, WorkerStream, WorkerTurn, WorkspaceChecks,
    WorkspaceFact, activity, needs_person, present_workspace, stall_receivers, stall_start,
    turn_given_at,
};
use fold_core::{
    AgentName, ProjectPaths, ProposalId, TaskId, Timestamp, WorkerName, WorkerState, WorkspaceId,
};
use fold_store::agents::{self, ClaimKind, OwedStart};
use fold_store::reviews::checks;
use fold_store::tasks::reports;
use fold_store::{Store, StoreError, events, messages, reviews};

use fold_core::events::types;
use fold_core::roles::needs_restart;
use fold_core::turn_errors::model_unreachable;
use fold_core::turn_waits::{WaitReason, shows_wait};
use fold_core::{Event, EventActor, NewEvent};
use serde_json::json;
use tokio::sync::broadcast::{self, error::RecvError, error::TryRecvError};

use super::{WorkspaceError, WorkspaceService};
use crate::live::{LiveActivity, LiveCheckRun, LiveState, Told};
use crate::projects::ProjectHandle;
use crate::push::starter_agent;
use crate::services::events::{BusMessage, EventBuffer, EventBus};
use crate::services::projects::load_config;

/// Decides what fold saw of the worker of every live workspace of `project` at `now`, with the project's `stall_minutes`, sets it in the live state with the check run that goes in each workspace, and tells the worker's lead of each new stall. A workspace whose files are its open proposal's newest version never stalls while the proposal waits on others; once it comes back to its author, the stall time counts from that moment at the earliest. A workspace whose checks run never stalls, and the stall time counts from the end of its newest run at the earliest, so the worker gets the whole window to act on the result. A worker never stalls while an ask about its task or one of its permission questions waits for a person, and the stall time counts from a person's newest answer and the start of its current process at the earliest. A worker whose process fold supervises counts from its newest output too: an open call is work, a call that runs long, a silence of `stall_minutes` and a process that ended without anyone asking and without a report are stalls, and a stop that a person, an agent or fold asked for is not. A worker whose resume fold owes while the worker limits hold it waits for a slot, which is never a stall; once a place is free for it, the stall time counts from the newest stop of a worker of its role or change of the limits at the earliest. Each new stall appends `worker.stalled`, which tells the worker's lead, or else the main agents; the stall needs a person once one more `stall_minutes` passed after that, or at once when no agent can act. A stall that a `worker.stalled` from before the daemon started already told is not told again, and its person's window counts from that event. A change publishes `workspace.changed`. Returns how many workspaces changed, the ones a starting run cleared included. While the project's agents are paused no worker stalls: the pause clears every activity and stalled flag and appends no `worker.stalled`, and after a resume the stall time counts from the resume at the earliest, so the paused time never stalls a worker.
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
    let config = load_config(paths).await?;
    let stall_minutes = config.stall_minutes;
    let limits = config.workers.limits();
    let _turn = live.stall_turn().await;
    let states = live.all();
    live.keep_told(&states.iter().map(|state| state.view.workspace).collect());
    if states.is_empty() {
        live.mark_stalls_decided();
        return Ok(0);
    }
    let inputs = stall_inputs(store, live.opened_at()).await?;
    let mut changed = 0;
    let mut new_stalls = Vec::new();
    for state in states {
        let view = &state.view;
        let run = inputs.running.get(&view.workspace).copied();
        if live.set_check_run(view.workspace, run) {
            changed += 1;
        }
        let worker = WorkerName::parse(&view.worker).ok();
        let agent = worker.as_ref().map(WorkerName::agent);
        let process = agent.as_ref().and_then(|agent| inputs.processes.get(agent));
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
        let worker_turn = view
            .task
            .zip(worker.as_ref())
            .map(|(task, worker)| inputs.worker_turn(task, worker))
            .unwrap_or_default();
        let start = stall_start(view.last_activity_at, turn, checks, worker_turn);
        let start = start.max(inputs.resumed_at.unwrap_or(start));
        let in_session = agent
            .as_ref()
            .is_some_and(|agent| inputs.sessions.contains(agent));
        let stream = match (process, &agent) {
            // A session fold does not supervise can act as the worker while its process is ended.
            (Some(process), _) if !process.running && in_session => WorkerStream::Unread,
            (Some(process), _) if !process.running => WorkerStream::Ended {
                at: process.ended_at,
                failed: process.failed,
            },
            (Some(process), Some(agent)) => {
                match live.streams().and_then(|streams| streams.facts(agent)) {
                    Some(facts) => WorkerStream::Running {
                        last_output_at: facts.last_output_at,
                        open_call: facts.open_call,
                    },
                    None => WorkerStream::Running {
                        last_output_at: process.started_at.max(live.opened_at()),
                        open_call: None,
                    },
                }
            }
            _ => WorkerStream::Unread,
        };
        let facts = StallFacts {
            has_impl_claim,
            workspace,
            checks,
            worker: worker_turn,
            reported: view
                .task
                .is_some_and(|task| inputs.reported.contains(&task)),
            explained: process.is_some_and(|process| process.explained),
            paused: inputs.paused,
            owed: agent
                .as_ref()
                .and_then(|agent| inputs.owed_turn(agent, limits)),
            given_at: turn_given_at(turn, checks, worker_turn),
            waits: process.and_then(|process| process.waits),
        };
        let verdict = activity(now, &facts, stream, start, stall_minutes);
        let (told, stalled) = match (verdict.reason(), verdict_since(verdict), &agent) {
            (Some(reason), Some(since), Some(agent)) => {
                let told = live
                    .told(view.workspace)
                    .filter(|told| told.reason == reason && told.since == since);
                let told = match told {
                    Some(told) => told,
                    None => {
                        let starter = process
                            .and_then(|process| process.starter.as_ref())
                            .filter(|starter| inputs.known.contains(*starter));
                        let agents = stall_receivers(agent, starter, &inputs.main_agents);
                        match inputs.told_before.get(&view.workspace) {
                            // The newest event came after this stall started, so it told this stall before the daemon started.
                            Some(at) if *at >= since => {
                                let told = Told {
                                    reason,
                                    since,
                                    at: *at,
                                    agents,
                                };
                                live.set_told(view.workspace, Some(told.clone()));
                                told
                            }
                            _ => {
                                let told = Told {
                                    reason,
                                    since,
                                    at: now,
                                    agents,
                                };
                                new_stalls.push((
                                    view.workspace,
                                    view.task,
                                    agent.clone(),
                                    verdict,
                                    told.clone(),
                                ));
                                told
                            }
                        }
                    }
                };
                let to_agent = (!told.agents.is_empty()).then_some(told.at);
                let stalled = needs_person(now, to_agent, stall_minutes);
                (told.agents, stalled)
            }
            _ => {
                live.set_told(view.workspace, None);
                (Vec::new(), false)
            }
        };
        let supervised = !matches!(stream, WorkerStream::Unread);
        let shown = LiveActivity::of(verdict, supervised, &told);
        if live.set_activity(view.workspace, view.last_activity_at, shown, stalled) {
            changed += 1;
            if stalled && !view.stalled {
                tracing::info!(workspace = %view.workspace, worker = %view.worker, "the stall of a worker needs a person");
            }
        }
    }
    tell_stalls(store, live, now, new_stalls).await;
    live.mark_stalls_decided();
    Ok(changed)
}

/// The start of a stall.
fn verdict_since(verdict: Activity) -> Option<Timestamp> {
    match verdict {
        Activity::Silent { since }
        | Activity::LongCall { since, .. }
        | Activity::Failed { since } => Some(since),
        Activity::Quiet
        | Activity::Working { .. }
        | Activity::Stopped { .. }
        | Activity::WaitsForSlot { .. }
        | Activity::WaitsForNetwork { .. }
        | Activity::WaitsForUsageLimit { .. } => None,
    }
}

/// Appends one `worker.stalled` for each new stall in one write, which tells the agents each names, then remembers who was told. A failed write remembers nothing, so the next decision tells them again.
async fn tell_stalls(
    store: &Store,
    live: &LiveState,
    now: Timestamp,
    stalls: Vec<(WorkspaceId, Option<TaskId>, AgentName, Activity, Told)>,
) {
    if stalls.is_empty() {
        return;
    }
    let mut events = Vec::new();
    for (workspace, task, agent, verdict, told) in &stalls {
        let minutes = now
            .duration_since(told.since)
            .map_or(0, |elapsed| elapsed.as_secs() / 60);
        let call = match verdict {
            Activity::LongCall { call, .. } => Some(call.as_str()),
            _ => None,
        };
        let event = NewEvent::new(types::WORKER_STALLED, EventActor::fold()).and_then(|event| {
            event.task(*task).workspace(*workspace).data(json!({
                "agent": agent.as_str(),
                "reason": told.reason,
                "call": call,
                "minutes": minutes,
                "told": told.agents.iter().map(AgentName::as_str).collect::<Vec<_>>(),
            }))
        });
        match event {
            Ok(event) => events.push(event),
            Err(_) => tracing::warn!(%workspace, "a stall event could not be made"),
        }
    }
    let written = store
        .write(async |tx| {
            let mut buffer = EventBuffer::new();
            for event in &events {
                buffer.append(tx, event, now).await?;
            }
            Ok::<_, StoreError>(buffer)
        })
        .await;
    match written {
        Ok(buffer) => {
            for (workspace, _, agent, _, told) in stalls {
                tracing::info!(%workspace, %agent, reason = told.reason, told = told.agents.len(), "a worker stalled");
                live.set_told(workspace, Some(told));
            }
            live.bus().publish(buffer);
        }
        Err(error) => tracing::warn!(
            code = error.log_code(),
            "the stalls were not told; the next decision tells them"
        ),
    }
}

/// Decides the stalled flags of the project's live workspaces again after each commit that can make a worker wait for someone, give it the turn or end its process, so a flag goes or a stop shows at once instead of on the stall job's next tick. It ends when the project closes.
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

/// A message of a person or an agent, which can be an answer or an ask, the closure of an ask, a permission question or its answer, a verdict, a start of a worker process, the end of a process or a session, which shows the worker stopped at once or frees a place for an owed resume, a send-back, which owes its task worker a resume, a change of the worker limits, a pause or its resume, which clears or restarts every stall time, and the start or end of a worker's wait for the network or a usage limit. fold's own messages, such as check results, give no turn.
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
        types::WORKER_STARTED,
        types::WORKER_RESUMED,
        types::WORKER_REFRESHED,
        types::WORKER_STOPPED,
        types::WORKER_FAILED,
        types::SESSION_ENDED,
        types::PROJECT_PAUSED,
        types::PROJECT_RESUMED,
        types::TASK_SENT_BACK,
        types::WORKERS_LIMITS_CHANGED,
        types::WORKER_WAITING,
        types::WORKER_WAIT_ENDED,
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
    /// The process of each supervised agent.
    processes: HashMap<AgentName, ProcessFacts>,
    /// The tasks with an open report.
    reported: BTreeSet<TaskId>,
    /// The agents tagged main.
    main_agents: BTreeSet<AgentName>,
    /// Every agent of the project.
    known: HashSet<AgentName>,
    /// The agents with an open session.
    sessions: HashSet<AgentName>,
    /// The resumes fold owes stopped or failed workers, oldest first.
    owed: Vec<OwedStart>,
    /// The newest change of the worker limits.
    limits_changed_at: Option<Timestamp>,
    /// The newest `worker.stalled` of each workspace from before the daemon started.
    told_before: HashMap<WorkspaceId, Timestamp>,
}

/// A supervised agent's process as the stall rule reads it.
struct ProcessFacts {
    role: Role,
    running: bool,
    started_at: Timestamp,
    /// When the process ended; its start for a row that has no end.
    ended_at: Timestamp,
    /// It ended without anyone asking, by a crash or a failure; a stop that someone asked for leaves the row `stopped`.
    failed: bool,
    /// The agent that started it, its lead; `None` for a person or fold.
    starter: Option<AgentName>,
    /// Needs you already names why it cannot work.
    explained: bool,
    /// The cause outside the worker its idle process waits for, since the end of the first turn of the wait.
    waits: Option<(WaitReason, Timestamp)>,
}

impl StallInputs {
    /// The resume fold owes `agent` as the stall rule sees it: it waits for a place while the `limits` refuse its start or an older owed resume of its role takes the free place, and else counts from the later of the time its work came and the newest stop of a worker of its role or change of the limits.
    fn owed_turn(&self, agent: &AgentName, limits: WorkerLimits) -> Option<OwedTurn> {
        let start = self.owed.iter().find(|owed| owed.resume.agent == *agent)?;
        let role = start.role;
        let slot = Slot::of(role)?;
        let same: Vec<OwedResume> = self
            .owed
            .iter()
            .filter(|owed| owed.role == role)
            .map(|owed| owed.resume.clone())
            .collect();
        let of_role = || {
            self.processes
                .values()
                .filter(move |process| process.role == role)
        };
        let running = of_role().filter(|process| process.running).count() as u64;
        let in_area = start
            .resume
            .area
            .as_ref()
            .map(|(name, count)| (name, *count));
        let since = start.resume.since;
        if waits_for_place(limits, role, running, in_area, &same, agent) {
            return Some(OwedTurn::WaitsForPlace { slot, since });
        }
        let freed = of_role()
            .filter(|process| !process.running)
            .map(|process| process.ended_at)
            .chain(self.limits_changed_at)
            .max();
        Some(OwedTurn::PlaceFree {
            since: freed.map_or(since, |freed| since.max(freed)),
        })
    }

    /// What `worker`, which works on `task`, waits for and when it was last given the turn.
    fn worker_turn(&self, task: TaskId, worker: &WorkerName) -> WorkerTurn {
        let agent = worker.agent();
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
            answered_at,
            started_at: self.starts.get(&agent).copied(),
        }
    }
}

/// The pause, the holder of each unreleased implementation claim by task, the turn of each open proposal by task, the proposal-stage check runs by workspace, and what each worker waits for and when it was last given the turn, read in one read transaction.
async fn stall_inputs(store: &Store, opened_at: Timestamp) -> Result<StallInputs, StoreError> {
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
    let processes = agents::list_worker_processes(&mut tx)
        .await?
        .into_iter()
        .map(|row| {
            let explained = row.failed_reason.as_deref().is_some_and(needs_restart)
                || model_unreachable(row.failed_turns, row.state);
            let facts = ProcessFacts {
                role: row.role,
                running: row.state.is_running(),
                started_at: row.started_at,
                ended_at: row.stopped_at.unwrap_or(row.started_at),
                failed: row.state == WorkerState::Failed,
                starter: starter_agent(&row.started_by),
                explained,
                waits: row
                    .wait
                    .as_ref()
                    .filter(|_| shows_wait(row.state))
                    .map(|wait| (wait.reason, wait.since)),
            };
            (row.agent, facts)
        })
        .collect();
    let reported = reports::tasks_with_open_reports(&mut *tx)
        .await?
        .into_iter()
        .collect();
    let tagged: HashSet<i64> = agents::all_tags(&mut *tx)
        .await?
        .into_iter()
        .filter(|(_, tag)| tag.is_main())
        .map(|(agent, _)| agent)
        .collect();
    let every = agents::list_agents(&mut *tx).await?;
    let main_agents = every
        .iter()
        .filter(|agent| tagged.contains(&agent.id))
        .map(|agent| agent.name.clone())
        .collect();
    let known = every.into_iter().map(|agent| agent.name).collect();
    let sessions = agents::open_bound_sessions(&mut *tx)
        .await?
        .into_iter()
        .map(|session| session.agent)
        .collect();
    let owed = agents::owed_starts(&mut *tx).await?;
    let limits_changed_at =
        events::newest_at_of_type(&mut *tx, types::WORKERS_LIMITS_CHANGED).await?;
    let told_before = events::newest_at_by_workspace(&mut *tx, types::WORKER_STALLED, opened_at)
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
        asked,
        questioned,
        task_answers,
        worker_messages,
        permission_answers,
        starts,
        processes,
        reported,
        main_agents,
        known,
        sessions,
        owed,
        limits_changed_at,
        told_before,
    })
}
