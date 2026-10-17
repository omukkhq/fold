use std::collections::BTreeMap;
use std::fmt::Write as _;

use fold_core::events::types;
use fold_core::{AgentName, EventActor, NewEvent, Role, TaskId, WorkerState, codes};
use fold_store::StoreError;
use fold_store::agents;
use fold_store::tasks::reports::{self, Resolution};
use serde_json::json;

use super::resume::SessionStart;
use super::{Input, Supervisor, lock};
use crate::services::agents::AgentError;
use crate::services::agents::workers::is_paused_refusal;
use crate::services::events::EventBuffer;

/// A sent-back task with the note that sent it back.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SentBack {
    task: TaskId,
    by: String,
    note: String,
}

impl Supervisor {
    /// Hands a sent-back task without a proposal to the task worker that reported it. A running task worker gets the push notice on its own; a reporter that is not a supervised task worker is left alone.
    pub(super) async fn resume_sent_back(self, task: TaskId) {
        let report = match reports::latest_for_task(self.inner.store.read(), task).await {
            Ok(report) => report,
            Err(error) => {
                tracing::warn!(task = %task, code = error.log_code(), "could not read a report to resume its task worker");
                return;
            }
        };
        let Some(report) = report else {
            return;
        };
        let agent = report.worker.agent();
        if let Err(error) = self.resume_reporter(&agent, &[task]).await {
            tracing::warn!(%agent, code = crate::services::agents::code_of(&error), "could not resume the task worker of a sent-back task");
        }
    }

    /// Resumes the task workers of every send-back that no start of theirs has served yet, such as one the worker limits refused.
    pub(super) async fn resume_owed_send_backs(self) {
        let owed = match reports::owed_send_backs(
            self.inner.store.read(),
            &[WorkerState::Stopped, WorkerState::Failed],
        )
        .await
        {
            Ok(owed) => owed,
            Err(error) => {
                tracing::warn!(
                    code = error.log_code(),
                    "could not read the owed send-backs"
                );
                return;
            }
        };
        let mut by_agent: BTreeMap<AgentName, Vec<TaskId>> = BTreeMap::new();
        for owed in owed {
            by_agent.entry(owed.agent).or_default().push(owed.task);
        }
        for (agent, tasks) in by_agent {
            if let Err(error) = self.resume_reporter(&agent, &tasks).await {
                tracing::warn!(%agent, code = crate::services::agents::code_of(&error), "could not resume the task worker of a sent-back task");
            }
        }
    }

    /// The send-backs recorded since each working, idle or starting task worker's process started, as the turn text that tells the task worker of them, by task worker: what a paused task worker missed.
    pub(super) async fn send_backs_owed_to_paused(&self) -> BTreeMap<AgentName, String> {
        let owed = match reports::owed_send_backs(
            self.inner.store.read(),
            &[
                WorkerState::Working,
                WorkerState::Idle,
                WorkerState::Starting,
            ],
        )
        .await
        {
            Ok(owed) => owed,
            Err(error) => {
                tracing::warn!(
                    code = error.log_code(),
                    "could not read the send-backs owed to paused task workers"
                );
                return BTreeMap::new();
            }
        };
        let mut by_agent: BTreeMap<AgentName, Vec<SentBack>> = BTreeMap::new();
        for owed in owed {
            let latest = reports::latest_for_task(self.inner.store.read(), owed.task).await;
            if let Ok(Some(report)) = latest {
                by_agent.entry(owed.agent).or_default().push(SentBack {
                    task: owed.task,
                    by: report.resolved_by.unwrap_or_default(),
                    note: report.send_back_note.unwrap_or_default(),
                });
            }
        }
        by_agent
            .into_iter()
            .map(|(agent, found)| (agent, sections(&found)))
            .collect()
    }

    /// The send-backs that a task worker, caught by a pause before its process read its first turn, still owes on its bound tasks, as turn text: a task whose newest report is a send-back of the task worker's report, which only a new report clears.
    pub(super) async fn send_backs_of_unstarted(
        &self,
        row: &fold_store::agents::WorkerProcessRow,
    ) -> Result<Option<String>, AgentError> {
        if row.role != Role::TaskWorker {
            return Ok(None);
        }
        let mut found = Vec::new();
        for task in &row.tasks {
            let latest = reports::latest_for_task(self.inner.store.read(), *task).await?;
            if let Some(report) = latest
                && report.resolution == Some(Resolution::SentBack)
                && report.worker.agent() == row.agent
            {
                found.push(SentBack {
                    task: *task,
                    by: report.resolved_by.unwrap_or_default(),
                    note: report.send_back_note.unwrap_or_default(),
                });
            }
        }
        Ok((!found.is_empty()).then(|| sections(&found)))
    }

    async fn resume_reporter(&self, agent: &AgentName, tasks: &[TaskId]) -> Result<(), AgentError> {
        // The row is read under the lock, so a second event finds the task worker running and leaves it to its stdin notices.
        let resuming = self.inner.resuming.lock().await;
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let worker = agents::find_worker_process(&mut conn, agent).await?;
        drop(conn);
        let Some(worker) = worker else {
            return Ok(());
        };
        if worker.role != Role::TaskWorker || worker.state.is_running() {
            return Ok(());
        }
        let mut found = Vec::with_capacity(tasks.len());
        for task in tasks {
            let latest = reports::latest_for_task(self.inner.store.read(), *task).await?;
            if let Some(report) = latest
                && report.resolution == Some(Resolution::SentBack)
                && report.worker.agent() == *agent
            {
                found.push(SentBack {
                    task: *task,
                    by: report.resolved_by.unwrap_or_default(),
                    note: report.send_back_note.unwrap_or_default(),
                });
            }
        }
        if found.is_empty() {
            return Ok(());
        }
        let bound: Vec<TaskId> = found.iter().map(|sent| sent.task).collect();
        let turn = resume_turn(&found);
        let watch = match self.resume(agent, turn.clone(), &bound).await {
            Ok(watch) => watch,
            // The send-back stays owed, and the resume serves it.
            Err(error) if is_paused_refusal(&error) => return Ok(()),
            // A start outside this path, such as `resume_worker`, came first; the task worker takes this turn as its next one.
            Err(AgentError::Fold(error)) if error.code() == codes::WORKER_EXISTS => {
                if let Some(run) = lock(&self.inner.running).get(agent) {
                    let _ = run.input.send(Input::Turn(turn));
                }
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        drop(resuming);
        let Some(watch) = watch else {
            return Ok(());
        };
        tokio::spawn(self.clone().restart_lost(
            agent.clone(),
            watch.clone(),
            Some(fresh_instructions(&found)),
        ));
        if watch.outcome().await == SessionStart::Went {
            self.record_sent_back(agent, &found).await?;
        }
        Ok(())
    }

    /// `worker.resumed` for each sent-back task, once the resumed session runs.
    async fn record_sent_back(
        &self,
        agent: &AgentName,
        found: &[SentBack],
    ) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        self.inner
            .store
            .write(async |tx| {
                for sent in found {
                    let event = NewEvent::new(types::WORKER_RESUMED, EventActor::fold())?
                        .task(sent.task)
                        .data(json!({
                            "agent": agent.as_str(),
                            "reason": "sent_back",
                            "resumed": true,
                        }))?;
                    buffer.append(tx, &event, now).await?;
                }
                Ok::<_, AgentError>(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }
}

fn sections(found: &[SentBack]) -> String {
    let mut text = String::new();
    for sent in found {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        let _ = write!(
            text,
            "{} was sent back by {}:\n{}\nFinish the work with the tools the task names, then call report_task for {} again.",
            sent.task,
            sent.by,
            sent.note.trim(),
            sent.task
        );
    }
    text
}

/// The turn a task worker resumed in its own session gets: each sent-back task with its note, then how it ends.
fn resume_turn(found: &[SentBack]) -> String {
    format!(
        "fold resumed you because work you reported needs you again.\n\n{}\n\n{STOP_LINE}",
        sections(found)
    )
}

/// What a task worker whose session is gone is told in its new one, after the role's restart text.
fn fresh_instructions(found: &[SentBack]) -> String {
    format!(
        "You reported the tasks below as finished, and they were sent back.\n\n{}\n\n{STOP_LINE}",
        sections(found)
    )
}

const STOP_LINE: &str = "fold stops you once each of your tasks is reported or has its proposal.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_names_each_task_its_checker_and_the_note() {
        let found = [
            SentBack {
                task: TaskId::from_number(5).unwrap(),
                by: "human:ana".to_owned(),
                note: "  Use the new colours. ".to_owned(),
            },
            SentBack {
                task: TaskId::from_number(6).unwrap(),
                by: "web-lead".to_owned(),
                note: "Add the empty state.".to_owned(),
            },
        ];
        let turn = resume_turn(&found);
        assert!(turn.starts_with("fold resumed you because work you reported needs you again."));
        assert!(turn.contains("T-5 was sent back by human:ana:\nUse the new colours.\n"));
        assert!(turn.contains("call report_task for T-6 again."));
        assert!(turn.ends_with(STOP_LINE));
        let fresh = fresh_instructions(&found[..1]);
        assert!(fresh.contains("T-5 was sent back by human:ana"));
        assert!(!fresh.contains("T-6"));
    }
}
