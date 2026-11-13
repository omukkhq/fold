use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use fold_core::agent_changes::may_write_files;
use fold_core::events::types;
use fold_core::roles::MCP_UNAVAILABLE;
use fold_core::turn_errors::MODEL_UNREACHABLE_TURNS;
use fold_core::turn_waits::{LimitKind, WaitGroup, released_by};
use fold_core::{AgentName, EventActor, LiveEvent, NewEvent, WorkerState, codes};
use fold_store::StoreError;
use fold_store::agents::{self, ProcessEnd, TurnTotals};
use serde_json::json;
use tokio::process::{Child, ChildStdout};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;

use super::super::output::{
    LineEffect, RowChange, TurnError, effect_of, fold_unreachable, micro_usd,
};
use super::lines::Lines;
use super::resume;
use super::stop::stopped_event;
use super::{Inner, Input, Process, Supervisor, lock};
use crate::services::agents::AgentError;
use crate::services::agents::claude::{ClaudeMessage, ContentBlock, UserContent};
use crate::services::agents::workers::code_of;
use crate::services::events::{BusMessage, EventBuffer};
use crate::services::projects::file_clock_now;

/// How long the reader waits for one more line after the process ended.
const LAST_LINES: Duration = Duration::from_millis(200);

impl Supervisor {
    /// Reads the process's stdout line by line until it closes, then records the exit. A line that is not UTF-8 or is longer than [`super::lines::MAX_LINE_BYTES`] is skipped, so the reader keeps draining the pipe whatever the process writes.
    pub(super) async fn drive(
        self,
        mut child: Child,
        stdout: ChildStdout,
        process: Process,
        exit: watch::Sender<bool>,
        detached: Arc<AtomicBool>,
        reaped: Arc<AtomicBool>,
    ) {
        let mut lines = Lines::new(stdout);
        let status = loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        self.on_line(&process, &line, detached.load(Ordering::SeqCst)).await;
                    }
                    Ok(None) | Err(_) => {
                        let status = child.wait().await;
                        reaped.store(true, Ordering::SeqCst);
                        break status;
                    }
                },
                status = child.wait() => {
                    reaped.store(true, Ordering::SeqCst);
                    // A child of the process can keep the pipe open after the process ended, so the lines it wrote last are read only while they keep coming.
                    while let Ok(Ok(Some(line))) =
                        tokio::time::timeout(LAST_LINES, lines.next_line()).await
                    {
                        self.on_line(&process, &line, detached.load(Ordering::SeqCst)).await;
                    }
                    break status;
                }
            }
        };
        self.inner
            .tool_windows
            .close_all(&process.agent, file_clock_now());
        let code = status.ok().and_then(exit_code);
        if let Err(error) = self
            .on_exit(&process, code, detached.load(Ordering::SeqCst))
            .await
        {
            tracing::warn!(agent = %process.agent, code = code_of(&error), "could not record a worker's exit");
        }
        // Only once the row says the process ended, so no stall decision reads a running row without its stream.
        self.inner.streams.ended(&process.agent, process.generation);
        // A newer process of the agent owns the replays from here on.
        let newer = lock(&self.inner.running)
            .get(&process.agent)
            .is_some_and(|run| run.generation != process.generation);
        if !newer {
            lock(&self.inner.replays).remove(&process.agent);
            self.forget_answering(&process.agent);
        }
        self.forget(&process);
        self.forget_waiters(process.generation);
        let _ = exit.send(true);
    }

    /// A process the daemon lets go of at its stop changes no row: the interrupted turn's result would make a working worker look idle, and the next start resumes the worker by the state it had.
    async fn on_line(&self, process: &Process, line: &str, detached: bool) {
        let Ok(message) = ClaudeMessage::parse(line) else {
            tracing::debug!(agent = %process.agent, "a worker wrote a line that is not JSON");
            return;
        };
        if process.confined {
            self.track_tool_calls(&process.agent, &message);
        }
        if !detached {
            self.inner.streams.line(
                &process.agent,
                process.generation,
                self.inner.clock.now(),
                &message,
            );
        }
        let LineEffect {
            step,
            mut change,
            reply,
            permission,
        } = effect_of(&message);
        if let Some(went) = resume::session_signal(&message, process.resumed) {
            process.session.send_if_modified(|seen| {
                let first = seen.is_none();
                if first {
                    *seen = Some(went);
                }
                first
            });
            // A lost session ran no turn, so its error result counts none.
            if !went {
                change = RowChange::None;
            }
        }
        if !detached && fold_unreachable(&message) {
            self.fail_unreachable(process).await;
            return;
        }
        if let Some(reply) = reply
            && let Ok(line) = reply.to_line()
        {
            let input = lock(&self.inner.running)
                .get(&process.agent)
                .map(|run| run.input.clone());
            if let Some(input) = input {
                let _ = input.send(Input::Line(line));
            }
        }
        if let Some(request) = permission
            && !detached
        {
            self.on_tool_request(process, request).await;
        }
        let id = process.id;
        let mut ended = None;
        let mut waits = false;
        let written: Result<(), AgentError> = match change {
            _ if detached => Ok(()),
            RowChange::None => Ok(()),
            RowChange::TurnStarted { model } => self
                .inner
                .store
                .write(async |tx| {
                    let model = (!model.is_empty()).then_some(model.as_str());
                    agents::set_worker_context(tx, id, model, None).await?;
                    agents::mark_worker_working(tx, id).await?;
                    Ok::<_, StoreError>(())
                })
                .await
                .map_err(AgentError::from),
            RowChange::Context { tokens, model } => {
                self.record_context(process, tokens, model.as_deref()).await
            }
            RowChange::TurnEnded {
                total_cost_usd,
                usage,
                windows,
                error,
            } => {
                ended = Some(windows);
                let totals = TurnTotals {
                    reported_micro_usd: micro_usd(total_cost_usd),
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_read_tokens: usage.cache_read_input_tokens,
                    cache_write_tokens: usage.cache_creation_input_tokens,
                    failed: error.is_some(),
                };
                let _ =
                    process
                        .open_turns
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |open| {
                            open.checked_sub(1)
                        });
                match self.end_turn(process, totals, error.as_ref()).await {
                    Ok(true) => {
                        waits = true;
                        Ok(())
                    }
                    Ok(false) => Ok(()),
                    Err(error) => Err(error),
                }
            }
        };
        if let Err(error) = &written {
            tracing::warn!(agent = %process.agent, code = code_of(error), "could not record a worker's output");
        }
        if let Ok(event) = LiveEvent::new(
            types::WORKER_OUTPUT,
            None,
            step.data(process.agent.as_str()),
        ) {
            self.inner.bus.publish_live(event);
        }
        // A turn that waits asks for no hand-off and is no answer: fold owes the worker the turn.
        if let Some(windows) = ended
            && written.is_ok()
            && !waits
        {
            self.after_turn(&process.agent, &windows).await;
        }
    }

    /// Notes each tool call of a confined worker's line that can write files where no guard looks ([`may_write_files`]) as it starts, from its `tool_use` block, and ends, from the `tool_result` block that names it, so a change of the project folder made meanwhile counts as the worker's.
    fn track_tool_calls(&self, agent: &AgentName, message: &ClaudeMessage) {
        let windows = &self.inner.tool_windows;
        match message {
            ClaudeMessage::Assistant(assistant) => {
                for block in &assistant.message.content {
                    if let ContentBlock::ToolUse { id, name, .. } = block
                        && may_write_files(name)
                    {
                        windows.open(agent, id, file_clock_now());
                    }
                }
            }
            ClaudeMessage::User(user) => {
                if let UserContent::Blocks(blocks) = &user.message.content {
                    for block in blocks {
                        if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                            windows.close(agent, tool_use_id, file_clock_now());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Records the context size of a request the model answered. A model that answers ends the worker's wait for the network or a usage limit at once, with `worker.wait_ended`, and releases the turns that wait for the same cause, so the turn that runs again shows as work while it works.
    async fn record_context(
        &self,
        process: &Process,
        tokens: u64,
        model: Option<&str>,
    ) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let id = process.id;
        let agent = &process.agent;
        let mut buffer = EventBuffer::new();
        let cleared = self
            .inner
            .store
            .write(async |tx| {
                agents::set_worker_context(tx, id, model, Some(tokens)).await?;
                if tokens == 0 {
                    return Ok::<_, AgentError>(None);
                }
                let cleared = agents::clear_turn_wait(tx, id).await?;
                if let Some(group) = cleared {
                    buffer.append(tx, &wait_ended(agent, group)?, now).await?;
                }
                Ok(cleared)
            })
            .await?;
        self.inner.bus.publish(buffer);
        if cleared.is_some() {
            self.owed_turns_changed(released_by(true, cleared));
        }
        Ok(())
    }

    /// Counts a finished turn. A turn that ended in an error leaves Claude Code's text on the row; the turn that makes [`MODEL_UNREACHABLE_TURNS`] in a row that reached no model appends `worker.model_unreachable`, once per run of such turns, so the main agent and the worker's starter hear of it. A turn that ended for a cause outside the worker, the network or a usage limit, is no failed turn: the row holds the wait, the start of a wait appends `worker.waiting`, and fold runs the turn again once the cause has passed (see [`Supervisor::owed_turns_changed`]). Any other end of a turn ends the row's wait with `worker.wait_ended`, and a turn that reached the model or ended well releases the turns that wait for it. A turn that ended well clears the error. Returns whether the turn waits.
    async fn end_turn(
        &self,
        process: &Process,
        totals: TurnTotals,
        error: Option<&TurnError>,
    ) -> Result<bool, AgentError> {
        let now = self.inner.clock.now();
        let id = process.id;
        let agent = &process.agent;
        let mut buffer = EventBuffer::new();
        let wait = error.and_then(|error| error.wait.as_ref());
        let cleared = self
            .inner
            .store
            .write(async |tx| {
                let mut cleared = None;
                match (error, wait) {
                    (Some(error), Some(wait)) => {
                        let new = agents::record_turn_wait(tx, id, &error.text, wait, now).await?;
                        if new == Some(true) {
                            let group = wait.group();
                            let event = NewEvent::new(types::WORKER_WAITING, EventActor::fold())?
                                .data(json!({
                                "agent": agent.as_str(),
                                "reason": group.reason.as_str(),
                                "limit": group.limit.map(LimitKind::as_str),
                            }))?;
                            buffer.append(tx, &event, now).await?;
                        }
                    }
                    (Some(error), None) => {
                        let failed = agents::record_turn_error(
                            tx,
                            id,
                            &error.text,
                            error.reached_model,
                            now,
                        )
                        .await?;
                        if failed == Some(MODEL_UNREACHABLE_TURNS) {
                            let event =
                                NewEvent::new(types::WORKER_MODEL_UNREACHABLE, EventActor::fold())?
                                    .data(json!({
                                        "agent": agent.as_str(),
                                        "turns": MODEL_UNREACHABLE_TURNS,
                                    }))?;
                            buffer.append(tx, &event, now).await?;
                        }
                    }
                    (None, _) => agents::clear_turn_error(tx, id).await?,
                }
                if wait.is_none() {
                    cleared = agents::clear_turn_wait(tx, id).await?;
                    if let Some(group) = cleared {
                        buffer.append(tx, &wait_ended(agent, group)?, now).await?;
                    }
                }
                agents::record_turn(tx, id, totals, now).await?;
                Ok::<_, AgentError>(cleared)
            })
            .await?;
        self.inner.bus.publish(buffer);
        if let Some(wait) = wait {
            tracing::info!(%agent, reason = wait.group().reason.as_str(), "a worker's turn waits for a cause outside it");
            self.owed_turns_changed(Vec::new());
        } else {
            let answered = error.is_none_or(|error| error.reached_model);
            let released = released_by(answered, cleared);
            if cleared.is_some() || self.has_owed_turns() {
                self.owed_turns_changed(released);
            }
        }
        Ok(wait.is_some())
    }

    /// Ends a worker whose fold MCP server did not connect, which without the fold tools can only end its turns: the row ends `failed` with [`MCP_UNAVAILABLE`] at once, and the process is let go of and stopped, so the rest of its turn changes no row. A worker that a stop is ending is left to the stop. A failure that cannot be recorded still stops the process, whose exit then records the failure. The stop runs in a task of its own, because it waits for the exit that this process's reader records.
    async fn fail_unreachable(&self, process: &Process) {
        let run = lock(&self.inner.running)
            .get(&process.agent)
            .filter(|run| run.generation == process.generation)
            .cloned();
        let Some(run) = run else {
            return;
        };
        tracing::warn!(agent = %process.agent, "the fold MCP server of a worker did not connect");
        // Let go of before the row ends, so a resume that sees the failed row also waits for this process. A process that a pause or the daemon's stop let go of already is theirs.
        if run.detached.swap(true, Ordering::SeqCst) {
            return;
        }
        let failed = self
            .fail_when(process.id, &process.agent, MCP_UNAVAILABLE, |row| {
                row.state != WorkerState::Stopping
            })
            .await;
        match failed {
            Ok(true) => {}
            Ok(false) => {
                run.detached.store(false, Ordering::SeqCst);
                return;
            }
            Err(error) => {
                run.detached.store(false, Ordering::SeqCst);
                tracing::warn!(agent = %process.agent, code = code_of(&error), "could not record a worker that cannot reach fold");
            }
        }
        let supervisor = self.clone();
        tokio::spawn(async move { supervisor.terminate(&run, true).await });
    }

    /// A stop that fold asked for ends `stopped` with `worker.stopped`; any other exit ends `failed` with `worker.failed`. A process the daemon let go of at its own stop leaves the row as it was. A process that exited with an error and wrote to stderr leaves the last lines it wrote as the row's last error, unless fold stopped it: the signals of a stop end it with an error too.
    async fn on_exit(
        &self,
        process: &Process,
        code: Option<i32>,
        detached: bool,
    ) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let id = process.id;
        if detached {
            if let Some(pid) = process.pid {
                self.inner
                    .store
                    .write(async |tx| agents::clear_worker_pid(tx, id, pid).await)
                    .await?;
            }
            return Ok(());
        }
        let stderr = match code {
            Some(0) => None,
            _ => process.stderr.text(LAST_LINES).await,
        };
        // A `--resume` that found no session exits too; fold starts the worker in a new session next, so the event says so.
        let session_lost = *process.session.borrow() == Some(false);
        let mut buffer = EventBuffer::new();
        let agent = &process.agent;
        self.inner
            .store
            .write(async |tx| {
                let Some(row) = agents::find_worker_process(tx.conn(), agent).await? else {
                    return Ok::<_, AgentError>(());
                };
                if row.id != id || !row.state.is_running() {
                    return Ok(());
                }
                agents::close_open_questions(tx, id, now).await?;
                let event = if row.state == WorkerState::Stopping {
                    agents::record_exit(tx, id, ProcessEnd::Stopped, code, now).await?;
                    stopped_event(&row, code)?
                } else {
                    if let Some(stderr) = &stderr {
                        agents::set_exit_error(tx, id, stderr, now).await?;
                    }
                    agents::record_exit(tx, id, ProcessEnd::Failed("exited"), code, now).await?;
                    let mut data = json!({
                        "agent": agent.as_str(),
                        "reason": "exited",
                        "exit_code": code,
                    });
                    if session_lost {
                        data["session_lost"] = json!(true);
                    }
                    NewEvent::new(types::WORKER_FAILED, EventActor::fold())?.data(data)?
                };
                buffer.append(tx, &event, now).await?;
                Ok(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }

    /// Does from the stored state what the events the watcher missed would have done: answers each `ask` question whose messages all have their replies, stops each short-lived worker whose work is done, tells each running task worker about changes requested of it, and serves the resumes, send-backs, failed proposal checks and reviews of new versions the limits held back.
    async fn recover(self) {
        self.answer_open_asks().await;
        self.stop_done_workers().await;
        self.notify_running_authors().await;
        self.clone().resume_owed_send_backs().await;
        self.clone().resume_owed_gate_runs().await;
        self.clone().resume_owed_reviewers(None).await;
        self.clone().resume_owed_messages().await;
        self.resume_pending().await;
    }
}

/// The `worker.wait_ended` of `agent`'s wait in `group`.
fn wait_ended(agent: &AgentName, group: WaitGroup) -> Result<NewEvent, AgentError> {
    Ok(
        NewEvent::new(types::WORKER_WAIT_ENDED, EventActor::fold())?.data(json!({
            "agent": agent.as_str(),
            "reason": group.reason.as_str(),
            "limit": group.limit.map(LimitKind::as_str),
        }))?,
    )
}

/// A signal's exit is reported as 128 plus its number, as a shell does.
fn exit_code(status: std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
}

/// Stops each short-lived worker whose work an event finished. A watcher that falls behind the bus checks the stored state instead of the events it missed. It holds the supervisor weakly, so the project's handle can drop.
pub(super) async fn watch_done(
    inner: Weak<Inner>,
    mut receiver: tokio::sync::broadcast::Receiver<BusMessage>,
) {
    loop {
        let event = match receiver.recv().await {
            Ok(BusMessage::Durable(event)) => event,
            Ok(BusMessage::Live(_)) => continue,
            Err(RecvError::Lagged(missed)) => {
                let Some(inner) = inner.upgrade() else {
                    return;
                };
                tracing::warn!(
                    missed,
                    "the worker supervisor missed events; it checks the stored state"
                );
                tokio::spawn(Supervisor { inner }.recover());
                continue;
            }
            Err(RecvError::Closed) => return,
        };
        let Some(inner) = inner.upgrade() else {
            return;
        };
        let supervisor = Supervisor { inner };
        supervisor.on_event(&event).await;
        tokio::spawn(supervisor.clone().wake_main(event.clone()));
        if event.event_type == types::WORKER_STOPPED {
            tokio::spawn(supervisor.clone().catch_up_after_stop(event.clone()));
        }
        let done = match supervisor.done_workers(&event).await {
            Ok(done) => done,
            Err(error) => {
                tracing::warn!(
                    seq = event.seq,
                    code = error.code().unwrap_or(codes::INTERNAL_ERROR),
                    "could not check for finished workers"
                );
                continue;
            }
        };
        for agent in done {
            supervisor.spawn_stop(agent);
        }
        let items = resume::resume_items(&event);
        if !items.is_empty() {
            tokio::spawn(supervisor.clone().resume_authors(items));
        }
        if let Some(run) = super::gate::gate_run(&event) {
            tokio::spawn(supervisor.clone().resume_gate_author(run));
        }
        if event.event_type == types::TASK_SENT_BACK
            && let Some(task) = event.task
        {
            tokio::spawn(supervisor.clone().resume_sent_back(task));
        }
        // A version that waits for a reviewer that has ended resumes that reviewer, which still holds the review claim.
        if (event.event_type == types::PROPOSAL_VERSION_ADDED
            || event.event_type == types::PROPOSAL_REVIEW_AGAIN_REQUESTED)
            && let Some(proposal) = event.proposal
        {
            tokio::spawn(supervisor.clone().resume_owed_reviewers(Some(proposal)));
        }
        if let Some(agent) = super::message::answering_agent(&event) {
            supervisor.stop_if_answered(&agent).await;
        }
        // A message to a stopped worker starts it; one sent while it was stopping waits for its stop.
        if event.event_type == types::MESSAGE_SENT
            || event.event_type == types::WORKER_STOPPED
            || event.event_type == types::WORKER_FAILED
        {
            tokio::spawn(supervisor.clone().resume_owed_messages());
        }
        // A freed worker slot or a raised limit can serve a resume that the limits refused earlier.
        if event.event_type == types::WORKER_STOPPED
            || event.event_type == types::WORKER_FAILED
            || event.event_type == types::WORKERS_LIMITS_CHANGED
        {
            tokio::spawn(supervisor.clone().resume_owed_send_backs());
            tokio::spawn(supervisor.clone().resume_owed_gate_runs());
            tokio::spawn(supervisor.clone().resume_owed_reviewers(None));
            tokio::spawn(supervisor.resume_pending());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signal_exit_counts_as_128_plus_the_signal() {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(exit_code(std::process::ExitStatus::from_raw(0)), Some(0));
        assert_eq!(exit_code(std::process::ExitStatus::from_raw(2)), Some(130));
        assert_eq!(
            exit_code(std::process::ExitStatus::from_raw(1 << 8)),
            Some(1)
        );
    }
}
