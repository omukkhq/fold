use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::Duration;

use fold_core::events::types;
use fold_core::{AgentName, EventActor, FoldError, NewEvent, Role, WorkerState, codes};
use fold_store::StoreError;
use fold_store::agents::{self, ProcessEnd, WorkerProcessRow};
use serde_json::json;
use tokio::sync::watch;

use super::{Input, Running, Supervisor, lock};
use crate::services::agents::AgentError;
use crate::services::agents::claude::{ClaudeInput, OutgoingRequest};
use crate::services::agents::workers::code_of;
use crate::services::events::EventBuffer;
use crate::services::projects::load_config;
use crate::services::reviews::{HeldBatch, held_open_batches, tell_batch_holder};

/// How long a stop waits for the exit after SIGKILL.
const KILL_WAIT: Duration = Duration::from_secs(10);

/// How often a stop looks at the clock while it waits for an exit.
const STOP_POLL: Duration = Duration::from_millis(50);

/// How long the stop of a finished worker waits for its turn to end before it stops the worker between two tool calls.
const DONE_SETTLE: Duration = Duration::from_secs(30);

/// How long the stop of a finished worker waits at most, for a tool call that never returns.
const DONE_GRACE: Duration = Duration::from_secs(120);

/// How often the stop of a finished worker reads its row while it waits for the turn to end.
const DONE_POLL: Duration = Duration::from_millis(100);

impl Supervisor {
    /// Marks the agent's running worker `stopping`, then ends its process: the interrupt of a working turn and a closed stdin, then SIGINT, SIGTERM and SIGKILL, each after its timeout. It returns once the process has exited and its exit is recorded, with whether it stopped a running worker. A worker that is not running is left as it is; one whose row runs without a process here ends `stopped` at once.
    pub(in crate::services::agents::workers) async fn stop(
        &self,
        agent: &AgentName,
    ) -> Result<bool, AgentError> {
        self.stop_when(agent, |_| true, false, None).await
    }

    /// [`Supervisor::stop`] that parks the worker for `park`: in the transaction that marks it `stopping`, or at once when it has ended already, the row records the agent that parked it and what it waits for, with `worker.parked`. The worker keeps its session and claims, and only the parking agent's message or a person's resumes it; another agent's message waits for that go. Returns whether it stopped a running worker.
    pub(in crate::services::agents::workers) async fn stop_and_park(
        &self,
        agent: &AgentName,
        park: &Park,
    ) -> Result<bool, AgentError> {
        self.stop_when(agent, |_| true, false, Some(park)).await
    }

    /// [`Supervisor::stop_and_park`] of a worker that `still` holds for, read in the transaction that marks it `stopping`, so a row that changed since the caller read it is left running.
    pub(in crate::services::agents::workers) async fn stop_and_park_when(
        &self,
        agent: &AgentName,
        park: &Park,
        still: impl Fn(&WorkerProcessRow) -> bool,
    ) -> Result<bool, AgentError> {
        self.stop_when(agent, still, false, Some(park)).await
    }

    /// [`Supervisor::stop`] of the supervised main for a human, who wants it to stay stopped: the row is held in the transaction that marks it `stopping`, or at once when it has ended already, so no notice, message or catch-up starts it until a human starts it again. Returns whether it stopped a running worker.
    pub(in crate::services::agents::workers) async fn stop_and_hold(
        &self,
        agent: &AgentName,
    ) -> Result<bool, AgentError> {
        self.stop_when(agent, |_| true, true, None).await
    }

    /// [`Supervisor::stop`] of a worker that `still` holds for, read in the transaction that marks it `stopping`; with `hold`, a supervised main is also held (see [`Supervisor::stop_and_hold`]); with `park`, the worker is parked (see [`Supervisor::stop_and_park`]). Returns whether it stopped the worker.
    async fn stop_when(
        &self,
        agent: &AgentName,
        still: impl Fn(&WorkerProcessRow) -> bool,
        hold: bool,
        park: Option<&Park>,
    ) -> Result<bool, AgentError> {
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        let started = self
            .inner
            .store
            .write(async |tx| {
                let Some(row) = agents::find_worker_process(tx.conn(), agent).await? else {
                    return Ok::<_, AgentError>(None);
                };
                let hold = hold && row.role == Role::Main;
                if !row.state.is_running() {
                    if hold && row.state != WorkerState::Starting {
                        agents::hold_worker(tx, row.id, now).await?;
                    }
                    if let Some(park) = park {
                        park_row(tx, &mut buffer, &row, park, now).await?;
                    }
                    return Ok(None);
                }
                if !still(&row) {
                    return Ok(None);
                }
                let working = row.state == WorkerState::Working;
                agents::set_worker_state(tx, row.id, WorkerState::Stopping, now).await?;
                if hold {
                    agents::hold_worker(tx, row.id, now).await?;
                }
                if let Some(park) = park {
                    park_row(tx, &mut buffer, &row, park, now).await?;
                }
                Ok(Some((row, working)))
            })
            .await?;
        self.inner.bus.publish(buffer);
        let Some((row, working)) = started else {
            return Ok(false);
        };
        let running = lock(&self.inner.running).get(agent).cloned();
        match running {
            Some(run) => self.terminate(&run, working).await,
            None => self.stopped_without_process(&row).await?,
        }
        Ok(true)
    }

    /// Stops `agent` in a task of its own, as a worker whose work is done, once its turn has settled (see [`Supervisor::until_turn_settled`]).
    ///
    /// A worker that kept working meanwhile is stopped only while it is still done by the rule of [`Supervisor::stop_done_workers`] and no user turn waits in its stdin, since a requested change or a message can have given it work again. While the stop waits, the worker's `wait_for_events` returns at once.
    pub(super) fn spawn_stop(&self, agent: AgentName) {
        let supervisor = self.clone();
        tokio::spawn(async move {
            supervisor.inner.finishing.send_modify(|set| {
                set.insert(agent.clone());
            });
            let stop = match supervisor.until_turn_settled(&agent).await {
                Settled::AtOnce => true,
                Settled::TurnEnded => supervisor.still_done(&agent, false).await,
                Settled::InTurn => supervisor.still_done(&agent, true).await,
            };
            if stop && let Err(error) = supervisor.stop(&agent).await {
                tracing::warn!(%agent, code = code_of(&error), "could not stop a finished worker");
            }
            supervisor.inner.finishing.send_modify(|set| {
                set.remove(&agent);
            });
        });
    }

    /// Whether `agent` is still a finished worker after the wait for its turn: done by the rule of [`Supervisor::stop_done_workers`], with no user turn waiting in its stdin besides the turn it is `in_turn`.
    async fn still_done(&self, agent: &AgentName, in_turn: bool) -> bool {
        let queued = lock(&self.inner.running)
            .get(agent)
            .is_some_and(|run| run.open_turns.load(Ordering::SeqCst) > u64::from(in_turn));
        if queued {
            return false;
        }
        match self.done_among(Some(agent)).await {
            Ok(done) => done.contains(agent),
            Err(error) => {
                tracing::warn!(%agent, code = error.code().unwrap_or(codes::INTERNAL_ERROR), "could not check whether a worker is still done");
                false
            }
        }
    }

    /// Waits until a finished worker's turn ends, so the call that finished its work, such as `create_proposal` or `submit_review`, returns its result into the session: an interrupt during the call would end it as a rejected tool use, which the resumed session starts with. After [`DONE_SETTLE`] it also ends once no tool call of the worker is open, and after [`DONE_GRACE`] in any case, for a call that never returns. The limits are read from the project's clock; a row that cannot be read ends the wait. Returns how the wait ended.
    async fn until_turn_settled(&self, agent: &AgentName) -> Settled {
        let started = self.inner.clock.now();
        let settle = started.checked_add(DONE_SETTLE);
        let grace = started.checked_add(DONE_GRACE);
        let mut waited = false;
        loop {
            let working = async {
                let mut conn = self
                    .inner
                    .store
                    .read()
                    .acquire()
                    .await
                    .map_err(StoreError::from)?;
                Ok::<_, StoreError>(
                    agents::find_worker_process(&mut conn, agent)
                        .await?
                        .is_some_and(|row| row.state == WorkerState::Working),
                )
            }
            .await
            .unwrap_or(false);
            if !working {
                return if waited {
                    Settled::TurnEnded
                } else {
                    Settled::AtOnce
                };
            }
            let now = self.inner.clock.now();
            if grace.is_none_or(|grace| now >= grace) {
                return Settled::InTurn;
            }
            let between_calls = self
                .inner
                .streams
                .facts(agent)
                .is_none_or(|facts| facts.open_call.is_none());
            if between_calls && settle.is_none_or(|settle| now >= settle) {
                return Settled::InTurn;
            }
            waited = true;
            tokio::time::sleep(DONE_POLL).await;
        }
    }

    /// Ends a `stopping` row that has no process here. A process that a racing launch started and ended itself may have recorded the stop already, so a row that has left `stopping` is left as it is.
    pub(super) async fn stopped_without_process(
        &self,
        row: &WorkerProcessRow,
    ) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        self.inner
            .store
            .write(async |tx| {
                let stopping = agents::find_worker_process(tx.conn(), &row.agent)
                    .await?
                    .is_some_and(|current| {
                        current.id == row.id && current.state == WorkerState::Stopping
                    });
                if !stopping {
                    return Ok(());
                }
                agents::record_exit(tx, row.id, ProcessEnd::Stopped, None, now).await?;
                agents::close_open_questions(tx, row.id, now).await?;
                buffer.append(tx, &stopped_event(row, None)?, now).await?;
                Ok::<_, AgentError>(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }

    /// Waits until a process of `agent` that fold let go of has exited, such as one whose fold MCP server did not connect: its row has ended already, but a new start of the agent must not run beside it in the same row and session. It waits as long as a stop's steps take, then refuses with `worker_exists`, whose hint names `retry`, the tool of the start that waits.
    pub(super) async fn until_let_go_exited(
        &self,
        agent: &AgentName,
        retry: &str,
    ) -> Result<(), AgentError> {
        let exited = lock(&self.inner.running)
            .get(agent)
            .filter(|run| run.detached.load(Ordering::SeqCst))
            .map(|run| run.exited.clone());
        let Some(mut exited) = exited else {
            return Ok(());
        };
        let timeouts = *lock(&self.inner.timeouts);
        let wait = timeouts.interrupt + timeouts.sigint + timeouts.sigterm + KILL_WAIT;
        if matches!(
            tokio::time::timeout(wait, exited.wait_for(|done| *done)).await,
            Ok(Ok(_))
        ) {
            return Ok(());
        }
        Err(FoldError::new(
            codes::WORKER_EXISTS,
            format!("the last process of worker {agent} is still ending"),
        )
        .with_details([("agent", json!(agent.as_str()))])
        .with_hint(format!("call list_workers, then call {retry} again"))
        .into())
    }

    pub(super) async fn terminate(&self, run: &Running, working: bool) {
        let timeouts = *lock(&self.inner.timeouts);
        if working {
            let interrupt = ClaudeInput::ControlRequest {
                request_id: self.request_id(),
                request: OutgoingRequest::Interrupt,
            };
            if let Ok(line) = interrupt.to_line() {
                let _ = run.input.send(Input::Line(line));
            }
        }
        let _ = run.input.send(Input::Close);
        let mut exited = run.exited.clone();
        for (signal, wait) in [
            (rustix::process::Signal::INT, timeouts.interrupt),
            (rustix::process::Signal::TERM, timeouts.sigint),
            (rustix::process::Signal::KILL, timeouts.sigterm),
        ] {
            if self.wait_exit(&mut exited, wait).await {
                return;
            }
            if run.reaped.load(Ordering::SeqCst) {
                continue;
            }
            signal_group(run, signal);
        }
        // SIGKILL cannot be ignored, so only the reader's last lines and the recorded exit remain; the timeout guards against a pid that was never known.
        let killed = tokio::time::timeout(KILL_WAIT, exited.wait_for(|done| *done)).await;
        if !matches!(killed, Ok(Ok(_))) {
            tracing::warn!("a worker process did not exit after SIGKILL");
        }
    }

    /// Waits until the process has exited or `wait` has passed on the project's clock.
    async fn wait_exit(&self, exited: &mut watch::Receiver<bool>, wait: Duration) -> bool {
        let Some(deadline) = self.inner.clock.now().checked_add(wait) else {
            return *exited.borrow();
        };
        loop {
            if *exited.borrow() {
                return true;
            }
            if self.inner.clock.now() >= deadline {
                return false;
            }
            tokio::select! {
                changed = exited.changed() => {
                    if changed.is_err() {
                        return true;
                    }
                }
                () = tokio::time::sleep(STOP_POLL) => {}
            }
        }
    }

    /// Ends every worker process as the daemon's stop does, with no time limit; see [`Supervisor::detach_all`].
    pub async fn shutdown(&self) {
        self.detach_all(false).await;
    }

    /// [`Supervisor::shutdown`] for the daemon's stop: it lets the stop go after [`crate::shutdown::WORKER_STOPS_LIMIT`], then sends SIGKILL to the process group of every process it found at the start that is not reaped yet, so neither a process nor a command of its tools runs on beside the worker that the next start resumes. The SIGKILL comes from a guard, so it also runs when the stop itself is dropped.
    pub async fn shutdown_within(&self, project: &str) {
        let running: Vec<(AgentName, Running)> = lock(&self.inner.running)
            .iter()
            .map(|(agent, run)| (agent.clone(), run.clone()))
            .collect();
        let _kill = KillLeft { project, running };
        crate::shutdown::within(
            "worker_stops",
            Some(project),
            crate::shutdown::WORKER_STOPS_LIMIT,
            self.detach_all(false),
        )
        .await;
    }

    /// Ends every worker process the way a stop does, all at once, but keeps each row's state, so the next daemon start or the resume of a pause runs the workers that were working or idle again in their own sessions. A pause leaves a worker that is `stopping` to finish its stop.
    pub(in crate::services::agents::workers) async fn detach_all(&self, leave_stopping: bool) {
        self.forget_queued();
        let rows = match self.inner.store.read().acquire().await {
            Ok(mut conn) => agents::list_running_worker_processes(&mut conn)
                .await
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let state_of = |agent: &AgentName| {
            rows.iter()
                .find(|row| row.agent == *agent)
                .map(|row| row.state)
        };
        let running: Vec<(AgentName, Running)> = lock(&self.inner.running)
            .iter()
            .filter(|(agent, _)| {
                !(leave_stopping && state_of(agent) == Some(WorkerState::Stopping))
            })
            .map(|(agent, run)| (agent.clone(), run.clone()))
            .collect();
        let mut stops = tokio::task::JoinSet::new();
        for (agent, run) in running {
            let working = state_of(&agent) == Some(WorkerState::Working);
            run.detached.store(true, Ordering::SeqCst);
            let supervisor = self.clone();
            stops.spawn(async move { supervisor.terminate(&run, working).await });
        }
        while stops.join_next().await.is_some() {}
    }

    /// The short-lived workers whose work `event` finished: a task worker once no bound task awaits its work (see [`agents::tasks_awaiting_proposal`]), checked when it opens a proposal, sends a new version or reports a task without a proposal and when a bound task is archived; a task worker whose bound task is Done, by an integration or a completion, once it holds no other claim; a reviewer once every bound proposal has a verdict or is closed or integrated and it holds no open batch with a member left, checked on its proposals' verdicts and ends and on a submitted or closed batch. A reviewer that is done but for such a batch is in `held`, with the batch, so fold tells it to send the batch's review or take the members out.
    pub(super) async fn done_workers(
        &self,
        event: &fold_core::Event,
    ) -> Result<DoneWorkers, StoreError> {
        let kind = event.event_type;
        let read = self.inner.store.read();
        let mut done = Vec::new();
        if (kind == types::PROPOSAL_OPENED
            || kind == types::PROPOSAL_VERSION_ADDED
            || kind == types::TASK_ARCHIVED
            || kind == types::TASK_REPORTED)
            && let Some(task) = event.task
        {
            let task_workers = agents::task_workers_of_task(read, task).await?;
            let ids: Vec<i64> = task_workers
                .iter()
                .map(|task_worker| task_worker.id)
                .collect();
            let waiting = agents::tasks_awaiting_proposal(read, &ids).await?;
            done.extend(
                task_workers
                    .into_iter()
                    .filter(|task_worker| waiting.get(&task_worker.id) == Some(&0))
                    .map(|task_worker| task_worker.agent),
            );
        }
        if (kind == types::PROPOSAL_INTEGRATED || kind == types::TASK_COMPLETED)
            && let Some(task) = event.task
        {
            let task_workers = agents::task_workers_of_task(read, task).await?;
            let ids: Vec<i64> = task_workers
                .iter()
                .map(|task_worker| task_worker.id)
                .collect();
            let held = agents::claims_held(read, &ids).await?;
            for task_worker in task_workers {
                if held.get(&task_worker.id) == Some(&0) && !done.contains(&task_worker.agent) {
                    done.push(task_worker.agent);
                }
            }
        }
        let reviewers = if (kind == types::REVIEW_SUBMITTED
            || kind == types::PROPOSAL_CLOSED
            || kind == types::PROPOSAL_INTEGRATED)
            && let Some(proposal) = event.proposal
        {
            agents::reviewers_of_proposal(read, proposal).await?
        } else if kind == types::REVIEW_BATCH_SUBMITTED || kind == types::REVIEW_BATCH_CLOSED {
            // A batch event names its batch, not each member, so every running reviewer is checked.
            let mut conn = read.acquire().await.map_err(StoreError::from)?;
            agents::list_running_worker_processes(&mut conn)
                .await?
                .into_iter()
                .filter(|row| {
                    row.role == fold_core::Role::Reviewer && row.state != WorkerState::Stopping
                })
                .map(|row| agents::BoundWorker {
                    id: row.id,
                    agent: row.agent,
                })
                .collect()
        } else {
            Vec::new()
        };
        let ids: Vec<i64> = reviewers.iter().map(|reviewer| reviewer.id).collect();
        let missing = agents::proposals_without_verdict(read, &ids).await?;
        let batches = if reviewers.is_empty() {
            Vec::new()
        } else {
            let mut conn = read.acquire().await.map_err(StoreError::from)?;
            held_open_batches(&mut conn).await?
        };
        let mut held = Vec::new();
        for reviewer in reviewers {
            if missing.get(&reviewer.id) != Some(&0) || done.contains(&reviewer.agent) {
                continue;
            }
            let own: Vec<HeldBatch> = batches
                .iter()
                .filter(|batch| batch.holder.belongs_to(&reviewer.agent))
                .cloned()
                .collect();
            if own.is_empty() {
                done.push(reviewer.agent);
            } else {
                held.extend(own);
            }
        }
        Ok(DoneWorkers { stop: done, held })
    }

    /// Tells the holder of each batch in `held` why fold keeps it running; see [`Supervisor::done_workers`].
    pub(super) async fn tell_batch_holders(&self, held: &[HeldBatch]) {
        for batch in held {
            let now = self.inner.clock.now();
            if let Err(error) =
                tell_batch_holder(&self.inner.store, &self.inner.bus, batch, now).await
            {
                tracing::warn!(batch = %batch.batch, code = error.log_code(), "could not tell a reviewer about the open batch it holds");
            }
        }
    }

    /// Stops every short-lived worker that has been idle for its role's `idle_minutes` at `now`, except one whose turn waits for the network, a usage limit or a working login, and except while the project's agents are paused: a resume gives each worker a new idle time. A worker is stopped only while it is still idle since the same moment, so one that took a turn after the rows were read keeps running. A worker that cannot be stopped is logged and the sweep goes on. Returns the agents it stopped.
    pub async fn sweep_idle(
        &self,
        now: fold_core::Timestamp,
    ) -> Result<Vec<AgentName>, AgentError> {
        if self.is_paused().await? {
            return Ok(Vec::new());
        }
        let config = load_config(&self.inner.paths).await?;
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let rows = agents::list_running_worker_processes(&mut conn).await?;
        drop(conn);
        let mut stopped = Vec::new();
        for row in rows {
            let Some(minutes) = config.role(row.role).idle_minutes else {
                continue;
            };
            // A worker whose turn waits is idle only until fold runs that turn again.
            let Some(since) = row
                .idle_since
                .filter(|_| row.state == WorkerState::Idle && row.wait.is_none())
            else {
                continue;
            };
            let due = since
                .checked_add(Duration::from_secs(minutes.saturating_mul(60)))
                .is_some_and(|due| due <= now);
            if !due {
                continue;
            }
            #[cfg(feature = "failpoints")]
            fail::fail_point!("worker_sweep_before_stop");
            let id = row.id;
            let still_idle = |current: &WorkerProcessRow| {
                current.id == id
                    && current.state == WorkerState::Idle
                    && current.idle_since == Some(since)
            };
            match self.stop_when(&row.agent, still_idle, false, None).await {
                Ok(true) => stopped.push(row.agent),
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(agent = %row.agent, code = code_of(&error), "could not stop an idle worker");
                }
            }
        }
        Ok(stopped)
    }

    /// Stops each running short-lived worker whose work is done by the stored state; see [`Supervisor::done_among`].
    pub(super) async fn stop_done_workers(&self) {
        match self.done_among(None).await {
            Ok(done) => {
                for agent in done {
                    self.spawn_stop(agent);
                }
            }
            Err(error) => tracing::warn!(
                code = error.code().unwrap_or(codes::INTERNAL_ERROR),
                "could not check for finished workers"
            ),
        }
    }

    /// The working or idle short-lived workers, of every agent or of `only`, whose work is done by the stored state, as the events of [`Supervisor::done_workers`] decide it.
    async fn done_among(&self, only: Option<&AgentName>) -> Result<Vec<AgentName>, StoreError> {
        let read = self.inner.store.read();
        let mut conn = read.acquire().await.map_err(StoreError::from)?;
        let rows: Vec<WorkerProcessRow> = agents::list_running_worker_processes(&mut conn)
            .await?
            .into_iter()
            .filter(|row| matches!(row.state, WorkerState::Working | WorkerState::Idle))
            .filter(|row| only.is_none_or(|agent| row.agent == *agent))
            .collect();
        let mut with_done_task = HashSet::new();
        for row in rows
            .iter()
            .filter(|row| row.role == fold_core::Role::TaskWorker)
        {
            for task in &row.tasks {
                let done = fold_store::tasks::fetch(&mut conn, *task)
                    .await?
                    .is_some_and(|task| task.status == fold_core::TaskStatus::Done);
                if done {
                    with_done_task.insert(row.id);
                }
            }
        }
        drop(conn);
        let of_role = |role| -> Vec<i64> {
            rows.iter()
                .filter(|row| row.role == role)
                .map(|row| row.id)
                .collect()
        };
        let task_workers = of_role(fold_core::Role::TaskWorker);
        let waiting = agents::tasks_awaiting_proposal(read, &task_workers).await?;
        let held = agents::claims_held(read, &task_workers).await?;
        let missing =
            agents::proposals_without_verdict(read, &of_role(fold_core::Role::Reviewer)).await?;
        let mut conn = read.acquire().await.map_err(StoreError::from)?;
        let batches = held_open_batches(&mut conn).await?;
        drop(conn);
        let holds_batch =
            |agent: &AgentName| batches.iter().any(|batch| batch.holder.belongs_to(agent));
        Ok(rows
            .into_iter()
            .filter(|row| {
                let open = match row.role {
                    // As the events decide it: a task worker whose task is Done stops once it holds no claim, and one without a Done task once no bound task awaits its work.
                    fold_core::Role::TaskWorker if with_done_task.contains(&row.id) => {
                        held.get(&row.id)
                    }
                    fold_core::Role::TaskWorker => waiting.get(&row.id),
                    // A reviewer that holds an open batch with a member left is told so by the event that would stop it; here it only keeps running.
                    fold_core::Role::Reviewer if holds_batch(&row.agent) => None,
                    fold_core::Role::Reviewer => missing.get(&row.id),
                    fold_core::Role::AreaLead
                    | fold_core::Role::ReviewLead
                    | fold_core::Role::Main => None,
                };
                open == Some(&0)
            })
            .map(|row| row.agent)
            .collect::<Vec<_>>())
    }
}

/// How the wait of [`Supervisor::until_turn_settled`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settled {
    /// The worker was not working when the wait began.
    AtOnce,
    /// The worker's turn ended while fold waited.
    TurnEnded,
    /// A limit passed while the worker's turn still ran.
    InTurn,
}

/// What an event finished: the workers to stop, and the open batches whose holders are done but for them.
#[derive(Debug, Default)]
pub(super) struct DoneWorkers {
    pub stop: Vec<AgentName>,
    pub held: Vec<HeldBatch>,
}

/// Sends `signal` to the process group of `run`, when its pid is known.
fn signal_group(run: &Running, signal: rustix::process::Signal) {
    if let Some(pid) = run
        .pid
        .and_then(|pid| i32::try_from(pid).ok())
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(pid, signal);
    }
}

/// The worker processes of a daemon stop, whose groups get SIGKILL when the stop ends or is dropped. A reaped process is skipped, so an ended process's reused pid is never signalled; one that has exited but is not reaped yet still holds its pid.
struct KillLeft<'a> {
    project: &'a str,
    running: Vec<(AgentName, Running)>,
}

impl Drop for KillLeft<'_> {
    fn drop(&mut self) {
        for (agent, run) in &self.running {
            if run.reaped.load(Ordering::SeqCst) || *run.exited.borrow() {
                continue;
            }
            tracing::warn!(project = self.project, %agent, "a worker process did not end in time; fold sent SIGKILL to its process group");
            signal_group(run, rustix::process::Signal::KILL);
        }
    }
}

/// Who parks a worker and what the worker waits for: the parking agent's event actor and agent, and its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Park {
    pub actor: EventActor,
    pub by: AgentName,
    pub wait_for: String,
}

/// Parks the stopping or ended `row` for `park` and appends `worker.parked`.
async fn park_row(
    tx: &mut fold_store::WriteTx,
    buffer: &mut EventBuffer,
    row: &WorkerProcessRow,
    park: &Park,
    now: fold_core::Timestamp,
) -> Result<(), AgentError> {
    let by = agents::find_agent(tx.conn(), &park.by)
        .await?
        .ok_or(StoreError::Invalid("the parking agent is not registered"))?;
    agents::park_worker(tx, row.id, by.id, &park.wait_for, now).await?;
    // A full lease from the park, so no reader sees the claims lapse before the sweeper renews them while the parker runs.
    let renewal = fold_store::tasks::claims::renew_of_agent(tx, row.agent_id, now).await?;
    crate::services::tasks::append_renewed::<AgentError>(tx, buffer, &renewal, now).await?;
    let event = NewEvent::new(types::WORKER_PARKED, park.actor.clone())?.data(json!({
        "agent": row.agent.as_str(),
        "by": park.by.as_str(),
        "wait_for": park.wait_for,
    }))?;
    buffer.append(tx, &event, now).await?;
    Ok(())
}

pub(super) fn stopped_event(
    row: &WorkerProcessRow,
    code: Option<i32>,
) -> Result<NewEvent, fold_core::EventError> {
    NewEvent::new(types::WORKER_STOPPED, EventActor::fold())?.data(json!({
        "agent": row.agent.as_str(),
        "role": row.role.as_str(),
        "area": row.area.as_ref().map(fold_core::AreaName::as_str),
        "exit_code": code,
        "turns": row.turns,
        "cost_micro_usd": row.cost_micro_usd,
        "cache_read_tokens": row.cache_read_tokens,
        "cache_write_tokens": row.cache_write_tokens,
    }))
}
