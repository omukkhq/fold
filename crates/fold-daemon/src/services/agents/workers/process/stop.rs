use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::Duration;

use fold_core::events::types;
use fold_core::{AgentName, EventActor, FoldError, NewEvent, WorkerState, codes};
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

/// How long a stop waits for the exit after SIGKILL.
const KILL_WAIT: Duration = Duration::from_secs(10);

/// How often a stop looks at the clock while it waits for an exit.
const STOP_POLL: Duration = Duration::from_millis(50);

impl Supervisor {
    /// Marks the agent's running worker `stopping`, then ends its process: the interrupt of a working turn and a closed stdin, then SIGINT, SIGTERM and SIGKILL, each after its timeout. It returns once the process has exited and its exit is recorded. A worker that is not running is left as it is; one whose row runs without a process here ends `stopped` at once.
    pub(in crate::services::agents::workers) async fn stop(
        &self,
        agent: &AgentName,
    ) -> Result<(), AgentError> {
        self.stop_when(agent, |_| true).await.map(|_| ())
    }

    /// [`Supervisor::stop`] of a worker that `still` holds for, read in the transaction that marks it `stopping`. Returns whether it stopped the worker.
    async fn stop_when(
        &self,
        agent: &AgentName,
        still: impl Fn(&WorkerProcessRow) -> bool,
    ) -> Result<bool, AgentError> {
        let now = self.inner.clock.now();
        let started = self
            .inner
            .store
            .write(async |tx| {
                let Some(row) = agents::find_worker_process(tx.conn(), agent).await? else {
                    return Ok::<_, StoreError>(None);
                };
                if !row.state.is_running() || !still(&row) {
                    return Ok(None);
                }
                let working = row.state == WorkerState::Working;
                agents::set_worker_state(tx, row.id, WorkerState::Stopping, now).await?;
                Ok(Some((row, working)))
            })
            .await?;
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

    /// Stops `agent` in a task of its own, as a worker whose work is done.
    pub(super) fn spawn_stop(&self, agent: AgentName) {
        let supervisor = self.clone();
        tokio::spawn(async move {
            if let Err(error) = supervisor.stop(&agent).await {
                tracing::warn!(%agent, code = code_of(&error), "could not stop a finished worker");
            }
        });
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

    /// Waits until a process of `agent` that fold let go of has exited, such as one whose fold MCP server did not connect: its row has ended already, but a new start of the agent must not run beside it in the same row and session. It waits as long as a stop's steps take, then refuses with `worker_exists`.
    pub(super) async fn until_let_go_exited(&self, agent: &AgentName) -> Result<(), AgentError> {
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
        .with_hint("call list_workers, then call resume_worker again")
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
            if let Some(pid) = run
                .pid
                .and_then(|pid| i32::try_from(pid).ok())
                .and_then(rustix::process::Pid::from_raw)
            {
                let _ = rustix::process::kill_process_group(pid, signal);
            }
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

    /// Ends every worker process at the daemon's stop; see [`Supervisor::detach_all`].
    pub async fn shutdown(&self) {
        self.detach_all(false).await;
    }

    /// Ends every worker process the way a stop does, all at once, but keeps each row's state, so the next daemon start or the resume of a pause runs the workers that were working or idle again in their own sessions. A pause leaves a worker that is `stopping` to finish its stop.
    pub(in crate::services::agents::workers) async fn detach_all(&self, leave_stopping: bool) {
        let rows = match self.inner.store.read().acquire().await {
            Ok(mut conn) => agents::list_worker_processes(&mut conn)
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

    /// The short-lived workers whose work `event` finished: a task worker once no bound task awaits its work (see [`agents::tasks_awaiting_proposal`]), checked when it opens a proposal, sends a new version or reports a task without a proposal and when a bound task is archived; a task worker whose bound task is Done, by an integration or a completion, once it holds no other claim; a reviewer once every bound proposal has a verdict or is closed or integrated.
    pub(super) async fn done_workers(
        &self,
        event: &fold_core::Event,
    ) -> Result<Vec<AgentName>, StoreError> {
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
        } else if kind == types::REVIEW_BATCH_SUBMITTED {
            // A batch review names its batch, not each member, so every running reviewer is checked.
            let mut conn = read.acquire().await.map_err(StoreError::from)?;
            agents::list_worker_processes(&mut conn)
                .await?
                .into_iter()
                .filter(|row| {
                    row.role == fold_core::Role::Reviewer
                        && row.state.is_running()
                        && row.state != WorkerState::Stopping
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
        for reviewer in reviewers {
            if missing.get(&reviewer.id) == Some(&0) && !done.contains(&reviewer.agent) {
                done.push(reviewer.agent);
            }
        }
        Ok(done)
    }

    /// Stops every short-lived worker that has been idle for its role's `idle_minutes` at `now`, except while the project's agents are paused: a resume gives each worker a new idle time. A worker is stopped only while it is still idle since the same moment, so one that took a turn after the rows were read keeps running. A worker that cannot be stopped is logged and the sweep goes on. Returns the agents it stopped.
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
        let rows = agents::list_worker_processes(&mut conn).await?;
        drop(conn);
        let mut stopped = Vec::new();
        for row in rows {
            let Some(minutes) = config.role(row.role).idle_minutes else {
                continue;
            };
            let Some(since) = row.idle_since.filter(|_| row.state == WorkerState::Idle) else {
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
            match self.stop_when(&row.agent, still_idle).await {
                Ok(true) => stopped.push(row.agent),
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(agent = %row.agent, code = code_of(&error), "could not stop an idle worker");
                }
            }
        }
        Ok(stopped)
    }

    pub(super) async fn stop_done_workers(&self) {
        let rows = async {
            let read = self.inner.store.read();
            let mut conn = read.acquire().await.map_err(StoreError::from)?;
            let rows: Vec<WorkerProcessRow> = agents::list_worker_processes(&mut conn)
                .await?
                .into_iter()
                .filter(|row| matches!(row.state, WorkerState::Working | WorkerState::Idle))
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
                agents::proposals_without_verdict(read, &of_role(fold_core::Role::Reviewer))
                    .await?;
            Ok::<_, StoreError>(
                rows.into_iter()
                    .filter(|row| {
                        let open = match row.role {
                            // As the events decide it: a task worker whose task is Done stops once it holds no claim, and one without a Done task once no bound task awaits its work.
                            fold_core::Role::TaskWorker if with_done_task.contains(&row.id) => {
                                held.get(&row.id)
                            }
                            fold_core::Role::TaskWorker => waiting.get(&row.id),
                            fold_core::Role::Reviewer => missing.get(&row.id),
                            fold_core::Role::AreaLead | fold_core::Role::ReviewLead => None,
                        };
                        open == Some(&0)
                    })
                    .map(|row| row.agent)
                    .collect::<Vec<_>>(),
            )
        }
        .await;
        match rows {
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
}

pub(super) fn stopped_event(
    row: &WorkerProcessRow,
    code: Option<i32>,
) -> Result<NewEvent, fold_core::EventError> {
    NewEvent::new(types::WORKER_STOPPED, EventActor::fold())?.data(json!({
        "agent": row.agent.as_str(),
        "exit_code": code,
        "turns": row.turns,
        "cost_micro_usd": row.cost_micro_usd,
        "cache_read_tokens": row.cache_read_tokens,
        "cache_write_tokens": row.cache_write_tokens,
    }))
}
