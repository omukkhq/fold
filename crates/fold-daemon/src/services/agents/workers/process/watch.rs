use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use fold_core::events::types;
use fold_core::{EventActor, LiveEvent, NewEvent, WorkerState, codes};
use fold_store::StoreError;
use fold_store::agents::{self, TurnTotals};
use serde_json::json;
use tokio::process::{Child, ChildStdout};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;

use super::super::output::{LineEffect, RowChange, effect_of, micro_usd};
use super::lines::Lines;
use super::resume;
use super::stop::stopped_event;
use super::{Inner, Input, Process, Supervisor, lock};
use crate::services::agents::AgentError;
use crate::services::agents::claude::ClaudeMessage;
use crate::services::agents::workers::code_of;
use crate::services::events::{BusMessage, EventBuffer};

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
        let code = status.ok().and_then(exit_code);
        if let Err(error) = self
            .on_exit(&process, code, detached.load(Ordering::SeqCst))
            .await
        {
            tracing::warn!(agent = %process.agent, code = code_of(&error), "could not record a worker's exit");
        }
        lock(&self.inner.replays).remove(&process.agent);
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
        let now = self.inner.clock.now();
        let id = process.id;
        let mut ended = None;
        let written = match change {
            _ if detached => Ok(()),
            RowChange::None => Ok(()),
            RowChange::TurnStarted { model } => {
                self.inner
                    .store
                    .write(async |tx| {
                        let model = (!model.is_empty()).then_some(model.as_str());
                        agents::set_worker_context(tx, id, model, None).await?;
                        agents::mark_worker_working(tx, id).await?;
                        Ok::<_, StoreError>(())
                    })
                    .await
            }
            RowChange::Context { tokens, model } => {
                self.inner
                    .store
                    .write(async |tx| {
                        agents::set_worker_context(tx, id, model.as_deref(), Some(tokens)).await
                    })
                    .await
            }
            RowChange::TurnEnded {
                total_cost_usd,
                usage,
                windows,
            } => {
                ended = Some(windows);
                let totals = TurnTotals {
                    cost_micro_usd: process.base_cost.saturating_add(micro_usd(total_cost_usd)),
                    cache_read_tokens: usage.cache_read_input_tokens,
                    cache_write_tokens: usage.cache_creation_input_tokens,
                };
                self.inner
                    .store
                    .write(async |tx| agents::record_turn(tx, id, totals, now).await)
                    .await
            }
        };
        if let Err(error) = &written {
            tracing::warn!(agent = %process.agent, code = error.log_code(), "could not record a worker's output");
        }
        if let Ok(event) = LiveEvent::new(
            types::WORKER_OUTPUT,
            None,
            step.data(process.agent.as_str()),
        ) {
            self.inner.bus.publish_live(event);
        }
        if let Some(windows) = ended
            && written.is_ok()
        {
            self.after_turn(&process.agent, &windows).await;
        }
    }

    /// A stop that fold asked for ends `stopped` with `worker.stopped`; any other exit ends `failed` with `worker.failed`. A process the daemon let go of at its own stop leaves the row as it was.
    async fn on_exit(
        &self,
        process: &Process,
        code: Option<i32>,
        detached: bool,
    ) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let id = process.id;
        if detached {
            self.inner
                .store
                .write(async |tx| agents::set_worker_pid(tx, id, None).await)
                .await?;
            return Ok(());
        }
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
                    agents::record_exit(tx, id, WorkerState::Stopped, code, now).await?;
                    stopped_event(&row, code)?
                } else {
                    agents::record_exit(tx, id, WorkerState::Failed, code, now).await?;
                    NewEvent::new(types::WORKER_FAILED, EventActor::fold())?.data(json!({
                        "agent": agent.as_str(),
                        "reason": "exited",
                        "exit_code": code,
                    }))?
                };
                buffer.append(tx, &event, now).await?;
                Ok(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }

    /// Does from the stored state what the events the watcher missed would have done: answers each `ask` question whose messages all have their replies, stops each short-lived worker whose work is done, tells each running task worker about changes requested of it, and serves the resumes and send-backs the limits held back.
    async fn recover(self) {
        self.answer_open_asks().await;
        self.stop_done_workers().await;
        self.notify_running_authors().await;
        self.clone().resume_owed_send_backs().await;
        self.resume_pending().await;
    }
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
        if event.event_type == types::TASK_SENT_BACK
            && let Some(task) = event.task
        {
            tokio::spawn(supervisor.clone().resume_sent_back(task));
        }
        // A freed worker slot or a raised limit can serve a resume that the limits refused earlier.
        if event.event_type == types::WORKER_STOPPED
            || event.event_type == types::WORKER_FAILED
            || event.event_type == types::WORKERS_LIMITS_CHANGED
        {
            tokio::spawn(supervisor.clone().resume_owed_send_backs());
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
