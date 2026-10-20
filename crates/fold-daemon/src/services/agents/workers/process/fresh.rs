use std::fmt::Write as _;
use std::sync::atomic::Ordering;

use fold_core::events::types;
use fold_core::roles::{Role, StartFacts, first_turn};
use fold_core::{AgentName, AreaName, EventActor, FoldError, NewEvent, WorkerState, codes};
use fold_store::StoreError;
use fold_store::agents::{self, ProcessEnd, SessionEnd, WorkerProcessRow};
use serde_json::json;

use super::stop::stopped_event;
use super::{Supervisor, lock, resume, reviewer_batch};
use crate::services::agents::AgentError;
use crate::services::agents::workers::limits::check_running;
use crate::services::agents::workers::{new_session, not_supervised, refuse_while_paused};
use crate::services::events::EventBuffer;
use crate::services::projects::load_config;

/// What every worker that a human or the main agent started again in a new session is told first.
const FRESH_START: &str = "A human or the main agent asked fold to start you again in a new session, so you do not remember your earlier session. You keep your name, your claims and your workers.";

/// What a task worker is told after [`FRESH_START`].
const FRESH_TASK_WORKER: &str = "Check your claims, your task workspaces and the proposals you already sent before you go on. Send a new version of an open proposal with update_proposal; do not open a second one.";

/// What a reviewer is told after [`FRESH_START`].
const FRESH_REVIEWER: &str = "Read each of your proposals with get_proposal and give a verdict only to those that have none from you.";

/// What a lead is told after [`FRESH_START`], before its latest hand-off note.
const FRESH_LEAD: &str =
    "Read your area with get_task_graph and your workers with list_workers before you go on.";

/// The tool a caller retries a restart with.
const RESTART_WORKER: &str = "restart_worker";

/// The row changed while fold ended its process, such as by another start.
fn changed(agent: &AgentName) -> AgentError {
    FoldError::new(
        codes::WORKER_EXISTS,
        format!("worker {agent} started again while fold ended its process"),
    )
    .with_details([("agent", json!(agent.as_str()))])
    .with_hint("call list_workers, then call restart_worker again")
    .into()
}

impl Supervisor {
    /// Starts the worker again in a new Claude Code session, for a worker of any role whose session a human or the main agent wants to leave. The steps run on a task of their own, so a caller that goes away never leaves a process ended under a row that still runs, and one new session of an agent is under way at a time, a hand-off refresh included. A paused project refuses with `project_paused`, and a stopped or failed short-lived worker over the worker limits with `worker_limit_reached`.
    pub(in crate::services::agents::workers) async fn restart_fresh(
        &self,
        agent: &AgentName,
        by: &EventActor,
        instructions: Option<&str>,
    ) -> Result<WorkerProcessRow, AgentError> {
        let supervisor = self.clone();
        let agent = agent.clone();
        let by = by.clone();
        let instructions = instructions.map(str::to_owned);
        tokio::spawn(async move {
            if !lock(&supervisor.inner.refreshing).insert(agent.clone()) {
                return Err(FoldError::new(
                    codes::WORKER_EXISTS,
                    format!("worker {agent} is starting in a new session already"),
                )
                .with_details([("agent", json!(agent.as_str()))])
                .with_hint("call list_workers")
                .into());
            }
            let restarted = supervisor
                .restart_held(&agent, &by, instructions.as_deref())
                .await;
            lock(&supervisor.inner.refreshing).remove(&agent);
            restarted
        })
        .await
        .unwrap_or_else(|error| {
            Err(FoldError::new(
                codes::INTERNAL_ERROR,
                format!("the restart of a worker stopped: {error}"),
            )
            .into())
        })
    }

    /// The steps of [`Supervisor::restart_fresh`]. A worker with a process here ends it the way a hand-off refresh ends a lead's: its row keeps its state and its place under the limits, so no `worker.stopped` resumes it in its old session and no other start takes its place. A worker without one is stopped first if it runs. Then one transaction with `worker.refreshed` keeps the old session id as a previous session, closes the questions of the old session and gives the row a new session, and Claude Code runs the role's first turn in it, with the work a task worker still owes on its proposals and reports. The worker keeps its name, its claims, its bound tasks and proposals, and the workers it started. A stop that comes while the process ends wins: the worker ends `stopped` and the restart is refused with `worker_busy`.
    async fn restart_held(
        &self,
        agent: &AgentName,
        by: &EventActor,
        instructions: Option<&str>,
    ) -> Result<WorkerProcessRow, AgentError> {
        self.inner
            .store
            .write(async |tx| refuse_while_paused(tx).await)
            .await?;
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let row = agents::find_worker_process(&mut conn, agent)
            .await?
            .ok_or_else(|| not_supervised(agent))?;
        drop(conn);
        let run = lock(&self.inner.running).get(agent).cloned();
        let kept = match run {
            Some(run) if row.state.is_running() && row.state != WorkerState::Stopping => {
                run.detached.store(true, Ordering::SeqCst);
                self.terminate(&run, row.state == WorkerState::Working)
                    .await;
                true
            }
            _ => {
                if row.state.is_running() {
                    self.stop(agent).await?;
                }
                false
            }
        };
        self.until_let_go_exited(agent, RESTART_WORKER).await?;
        let owed = self.owed_to_unstarted(&row).await?;
        let sent_back = self.send_backs_of_unstarted(&row).await?;
        let config = load_config(&self.inner.paths).await?;
        let limits = config.workers.limits();
        let session = new_session()?;
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        let fresh = self
            .inner
            .store
            .write(async |tx| {
                let current = agents::find_worker_process(tx.conn(), agent)
                    .await?
                    .ok_or_else(|| not_supervised(agent))?;
                if kept {
                    if current.state == WorkerState::Stopping && current.id == row.id {
                        agents::record_exit(tx, current.id, ProcessEnd::Stopped, None, now).await?;
                        agents::close_open_questions(tx, current.id, now).await?;
                        buffer
                            .append(tx, &stopped_event(&current, None)?, now)
                            .await?;
                        return Ok(None);
                    }
                    if current.id != row.id
                        || current.session_id != row.session_id
                        || !current.state.is_running()
                    {
                        return Err(changed(agent));
                    }
                    refuse_while_paused(tx).await?;
                } else {
                    if current.state.is_running() {
                        return Err(changed(agent));
                    }
                    refuse_while_paused(tx).await?;
                    check_running(
                        tx,
                        limits,
                        current.role,
                        current.area_id.zip(current.area.as_ref()),
                    )
                    .await?;
                }
                agents::close_open_questions(tx, current.id, now).await?;
                agents::record_previous_session(tx, current.id, SessionEnd::Requested, now).await?;
                agents::refresh_session(tx, current.id, &session, now).await?;
                let event = NewEvent::new(types::WORKER_REFRESHED, by.clone())?.data(json!({
                    "agent": agent.as_str(),
                    "role": current.role.as_str(),
                    "area": current.area.as_ref().map(AreaName::as_str),
                    "old_session": current.session_id,
                    "new_session": session,
                    "context_tokens": current.context_tokens,
                    "handoff": false,
                    "requested": true,
                }))?;
                buffer.append(tx, &event, now).await?;
                agents::find_worker_process(tx.conn(), agent)
                    .await?
                    .ok_or(StoreError::Corrupt("worker_processes"))
                    .map(Some)
                    .map_err(AgentError::from)
            })
            .await?;
        self.inner.bus.publish(buffer);
        let Some(fresh) = fresh else {
            return Err(FoldError::new(
                codes::WORKER_BUSY,
                format!("worker {agent} was stopped while fold started it in a new session"),
            )
            .with_details([("agent", json!(agent.as_str()))])
            .with_hint("call list_workers, then call restart_worker again")
            .into());
        };
        // The old session's sockets and answered questions belong to a process that has ended; the new session registers its own.
        self.inner.push.clear(agent);
        lock(&self.inner.replays).remove(agent);
        lock(&self.inner.held_lost).remove(agent);
        let mut text = self.fresh_instructions(&fresh, instructions).await?;
        if !owed.is_empty() {
            let _ = write!(text, "\n\n{}", resume::notice_turn(&owed));
        }
        if let Some(part) = sent_back {
            let _ = write!(text, "\n\n{part}");
        }
        let label = match &fresh.area {
            Some(area) => agents::find_area(self.inner.store.read(), area)
                .await?
                .map(|area| area.label),
            None => None,
        };
        let turn = first_turn(&StartFacts {
            role: fresh.role,
            name: agent,
            area: fresh.area.as_ref().zip(label.as_ref()),
            tasks: &fresh.tasks,
            proposals: &fresh.proposals,
            batch: reviewer_batch(&self.inner.store, &fresh).await?,
            instructions: Some(&text),
        });
        self.launch(&fresh, false, Some(turn)).await?;
        Ok(fresh)
    }

    /// What the new session is told after its role's start line: that it starts again, what to check for its role, a lead's latest hand-off note, then the instructions of whoever asked for the new session.
    async fn fresh_instructions(
        &self,
        row: &WorkerProcessRow,
        instructions: Option<&str>,
    ) -> Result<String, AgentError> {
        let mut text = FRESH_START.to_owned();
        match row.role {
            Role::TaskWorker => {
                let _ = write!(text, " {FRESH_TASK_WORKER}");
            }
            Role::Reviewer => {
                let _ = write!(text, " {FRESH_REVIEWER}");
            }
            Role::AreaLead | Role::ReviewLead => {
                let _ = write!(text, " {FRESH_LEAD}");
                if let Some(note) =
                    agents::latest_handoff(self.inner.store.read(), row.agent_id).await?
                {
                    let _ = write!(text, " Your latest hand-off note:\n\n{}", note.body.trim());
                }
            }
        }
        if let Some(extra) = instructions.map(str::trim).filter(|text| !text.is_empty()) {
            let _ = write!(text, "\n\n{extra}");
        }
        Ok(text)
    }
}
