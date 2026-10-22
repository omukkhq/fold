use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::atomic::Ordering;

use fold_core::events::types;
use fold_core::roles::check_limits;
use fold_core::{AgentName, Event, LiveEvent, Role, WorkerState, codes};
use fold_store::agents::{self, WorkerProcessRow};
use fold_store::messages::{self, OwedMessage};
use fold_store::reviews::checks;
use fold_store::tasks::reports;
use fold_store::{StoreError, WriteTx};
use serde_json::json;

use super::main::is_main_refusal;
use super::resume::SessionStart;
use super::{Input, Supervisor, lock};
use crate::services::agents::AgentError;
use crate::services::agents::workers::{code_of, is_paused_refusal};
use crate::services::events::EventBuffer;
use crate::services::messages::{forward_in_tx, record_pushed};
use crate::services::projects::load_config;

/// The states of a worker that a message starts again.
const ENDED: [WorkerState; 2] = [WorkerState::Stopped, WorkerState::Failed];

/// The failure reason of a worker whose session a start for a message found gone.
const SESSION_LOST: &str = "session_lost";

impl Supervisor {
    /// Starts each stopped or failed worker again that has a message to answer; see [`Supervisor::answer_messages`]. A paused project leaves the messages owed, and the resume of the pause serves them.
    pub(super) async fn resume_owed_messages(self) {
        let owed = match messages::owed_to_stopped(self.inner.store.read(), None).await {
            Ok(owed) => owed,
            Err(error) => {
                tracing::warn!(
                    code = error.log_code(),
                    "could not read the messages owed by stopped workers"
                );
                return;
            }
        };
        let agents: BTreeSet<AgentName> = owed.into_iter().map(|owed| owed.agent).collect();
        for agent in agents {
            if let Err(error) = self.answer_messages(&agent).await {
                tracing::warn!(%agent, code = code_of(&error), "could not start a worker to answer a message");
            }
        }
    }

    /// Resumes the worker of `agent` in its own session for one turn that names the messages it owes. A worker whose own resume is owed, such as requested changes to a task worker or a new version to a reviewer, waits for that resume, which moves its start, and answers once it stops again: a start for a message first would move the start too and leave the owed work untold. When the worker cannot start, because a worker limit refuses it or the owed resume, because its session is gone or because its process ends before it runs, every message goes on to a lead; see [`forward_in_tx`].
    async fn answer_messages(&self, agent: &AgentName) -> Result<(), AgentError> {
        if !lock(&self.inner.message_starts).insert(agent.clone()) {
            return Ok(());
        }
        let served = self.answer_messages_once(agent).await;
        lock(&self.inner.message_starts).remove(agent);
        served
    }

    async fn answer_messages_once(&self, agent: &AgentName) -> Result<(), AgentError> {
        // The row and the messages are read under the lock, so another start of the same worker finds it running and the messages it served told.
        let resuming = self.inner.resuming.lock().await;
        let read = self.inner.store.read();
        let mut conn = read.acquire().await.map_err(StoreError::from)?;
        let Some(row) = agents::find_worker_process(&mut conn, agent).await? else {
            return Ok(());
        };
        drop(conn);
        if !ENDED.contains(&row.state) {
            return Ok(());
        }
        let owed = messages::owed_to_stopped(read, Some(agent)).await?;
        if owed.is_empty() {
            return Ok(());
        }
        if row.state == WorkerState::Failed && row.failed_reason.as_deref() == Some(SESSION_LOST) {
            drop(resuming);
            return self.forward(&row, &owed, false).await;
        }
        if self.owes_own_resume(&row).await? {
            drop(resuming);
            if self.limits_refuse(&row).await? {
                return self.forward(&row, &owed, false).await;
            }
            tokio::spawn(self.clone().resume_pending());
            tokio::spawn(self.clone().resume_owed_send_backs());
            tokio::spawn(self.clone().resume_owed_gate_runs());
            tokio::spawn(self.clone().resume_owed_reviewers(None));
            return Ok(());
        }
        let turn = message_turn(&owed);
        lock(&self.inner.answering).insert(agent.clone());
        let watch = match self.resume(agent, turn.clone(), &[]).await {
            Ok(watch) => watch,
            Err(error) => {
                lock(&self.inner.answering).remove(agent);
                return match error {
                    // The messages stay owed: the resume of the pause serves them, and a supervised main that may not run reads them in the session that runs it.
                    error if is_paused_refusal(&error) || is_main_refusal(&error) => Ok(()),
                    // A start outside this path came first; the worker takes this turn as its next one. A process that fold let go of takes no turn, so the messages stay owed.
                    AgentError::Fold(error) if error.code() == codes::WORKER_EXISTS => {
                        let input = lock(&self.inner.running)
                            .get(agent)
                            .filter(|run| !run.detached.load(Ordering::SeqCst))
                            .map(|run| run.input.clone());
                        if let Some(input) = input
                            && input.send(Input::Turn(turn)).is_ok()
                        {
                            drop(resuming);
                            for owed in &owed {
                                self.record_told(agent, owed).await;
                            }
                        }
                        Ok(())
                    }
                    AgentError::Fold(error) if error.code() == codes::WORKER_LIMIT_REACHED => {
                        drop(resuming);
                        self.forward(&row, &owed, false).await
                    }
                    error => Err(error),
                };
            }
        };
        drop(resuming);
        let outcome = match watch {
            Some(watch) => watch.outcome().await,
            None => SessionStart::Ended,
        };
        match outcome {
            SessionStart::Went => {
                for owed in &owed {
                    self.record_told(agent, owed).await;
                }
                Ok(())
            }
            // A new session would not know the work the sender asks about.
            SessionStart::Lost => {
                lock(&self.inner.answering).remove(agent);
                self.forward(&row, &owed, true).await
            }
            // A pause that ended the process leaves the messages owed for its resume.
            SessionStart::Ended => {
                lock(&self.inner.answering).remove(agent);
                if self.is_paused().await? {
                    return Ok(());
                }
                self.forward(&row, &owed, false).await
            }
        }
    }

    /// Whether a resume of the worker of `row` for its own work is owed: for a task worker requested changes, a conflict, failed checks or a send-back that no start has served, for a reviewer a new version of a proposal whose review claim it holds (see [`agents::reviews_owed_to_ended_reviewers`]).
    async fn owes_own_resume(&self, row: &WorkerProcessRow) -> Result<bool, AgentError> {
        let read = self.inner.store.read();
        let agent = &row.agent;
        if row.role == Role::Reviewer {
            return Ok(agents::reviews_owed_to_ended_reviewers(read, None)
                .await?
                .iter()
                .any(|owed| owed.agent == *agent));
        }
        if row.role != Role::TaskWorker {
            return Ok(false);
        }
        Ok(agents::pending_resumes(read, &ENDED)
            .await?
            .iter()
            .any(|owed| owed.agent == *agent && (owed.changes || owed.conflict || owed.checks))
            || reports::owed_send_backs(read, &ENDED)
                .await?
                .iter()
                .any(|owed| owed.agent == *agent)
            || checks::owed_gate_runs(read, &ENDED)
                .await?
                .iter()
                .any(|owed| owed.agent == *agent))
    }

    /// Whether the worker limits refuse a start of the worker of `row` now.
    async fn limits_refuse(&self, row: &WorkerProcessRow) -> Result<bool, AgentError> {
        let limits = load_config(&self.inner.paths).await?.workers.limits();
        let (running, in_area) =
            agents::running_counts(self.inner.store.read(), row.role, row.area_id).await?;
        Ok(check_limits(
            limits,
            row.role,
            running,
            row.area_id
                .and(row.area.as_ref())
                .map(|area| (area, in_area)),
        )
        .is_err())
    }

    /// A session got its turn about `owed`: the delivery line shows the message waiting for the worker, as for a running worker, until the worker reads its inbox.
    pub(super) async fn record_told(&self, agent: &AgentName, owed: &OwedMessage) {
        let now = self.inner.clock.now();
        match record_pushed(&self.inner.store, agent, owed.message, now).await {
            Ok(0) => {}
            Ok(_) => {
                let event = LiveEvent::new(
                    types::MESSAGE_PUSHED,
                    None,
                    json!({
                        "message": owed.message.to_string(),
                        "thread": owed.thread.to_string(),
                        "agent": agent.as_str(),
                    }),
                );
                match event {
                    Ok(event) => self.inner.bus.publish_live(event),
                    Err(error) => {
                        tracing::error!(%agent, %error, "could not build message.pushed");
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%agent, code = error.log_code(), "could not record the turn that told a worker of a message");
            }
        }
    }

    /// Sends each of `owed` on to the lead of the worker of `row`; with `lost`, also records that the worker's session is gone.
    async fn forward(
        &self,
        row: &WorkerProcessRow,
        owed: &[OwedMessage],
        lost: bool,
    ) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        self.inner
            .store
            .write(async |tx| {
                if lost {
                    agents::mark_session_lost(tx, row.id, SESSION_LOST).await?;
                }
                for owed in owed {
                    if still_owed(tx, owed).await? {
                        forward_in_tx(tx, &mut buffer, owed, row, now).await?;
                    }
                }
                Ok::<_, AgentError>(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }

    /// Stops a worker that fold started to answer a message once it is idle, has read every message sent to it and nothing bound to it waits for its work. It is checked when a turn ends and when the worker sends or acknowledges a message.
    pub(super) async fn stop_if_answered(&self, agent: &AgentName) {
        if !lock(&self.inner.answering).contains(agent) {
            return;
        }
        match self.answered(agent).await {
            Ok(true) => {
                lock(&self.inner.answering).remove(agent);
                self.spawn_stop(agent.clone());
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%agent, code = error.log_code(), "could not check whether a worker answered its messages");
            }
        }
    }

    /// Whether the worker of `agent` is idle, has read its inbox, and has no bound task that waits for its proposal or report and no bound proposal that waits for its verdict.
    async fn answered(&self, agent: &AgentName) -> Result<bool, StoreError> {
        let read = self.inner.store.read();
        let mut conn = read.acquire().await.map_err(StoreError::from)?;
        let Some(row) = agents::find_worker_process(&mut conn, agent).await? else {
            return Ok(false);
        };
        drop(conn);
        if row.state != WorkerState::Idle || messages::undelivered_count(read, agent).await? > 0 {
            return Ok(false);
        }
        let waiting = match row.role {
            Role::TaskWorker => agents::tasks_awaiting_proposal(read, &[row.id]).await?,
            Role::Reviewer => agents::proposals_without_verdict(read, &[row.id]).await?,
            Role::AreaLead | Role::ReviewLead | Role::Main => return Ok(true),
        };
        Ok(waiting.get(&row.id) == Some(&0))
    }

    /// Forgets that the agent's process was started to answer a message, once the process ends.
    pub(super) fn forget_answering(&self, agent: &AgentName) {
        lock(&self.inner.answering).remove(agent);
    }
}

/// The agent whose message, sent or acknowledged, may be the answer of a worker that fold started to answer a message.
pub(super) fn answering_agent(event: &Event) -> Option<AgentName> {
    if event.event_type != types::MESSAGE_SENT && event.event_type != types::MESSAGE_ACKNOWLEDGED {
        return None;
    }
    AgentName::parse(event.actor.agent()?).ok()
}

/// Whether `owed` is still unread, untold and not settled, read again in the forward's own transaction.
async fn still_owed(tx: &mut WriteTx, owed: &OwedMessage) -> Result<bool, StoreError> {
    Ok(messages::owed_to_stopped(tx.conn(), Some(&owed.agent))
        .await?
        .iter()
        .any(|other| other.message == owed.message))
}

/// The turn of a worker that fold started again for `owed`. A worker can have stopped while its work waited for the answer, such as one that asked a human, so it goes on with that work.
pub(super) fn message_turn(owed: &[OwedMessage]) -> String {
    let mut text =
        String::from("fold started you again for a message sent to you while you were stopped:");
    for owed in owed {
        let _ = write!(text, "\n- {} from {}", owed.message, owed.sender);
        if let Some(task) = owed.task {
            let _ = write!(text, " about {task}");
        }
    }
    let _ = write!(
        text,
        "\n\nRead it with get_inbox. If your bound task or review still waits for your work, continue it. Else answer in its thread with send_message and reply_to, and end your turn: fold stops you again once you have answered."
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_core::{HumanName, MessageId, Sender, TaskId, WorkerName};

    #[test]
    fn the_turn_names_each_message_its_sender_and_task_and_how_it_ends() {
        let owed = [
            OwedMessage {
                agent: AgentName::parse("backend-worker-10").unwrap(),
                message: MessageId::from_number(221).unwrap(),
                thread: MessageId::from_number(221).unwrap(),
                task: TaskId::from_number(10),
                sender: Sender::Human(HumanName::parse("tanim").unwrap()),
            },
            OwedMessage {
                agent: AgentName::parse("backend-worker-10").unwrap(),
                message: MessageId::from_number(225).unwrap(),
                thread: MessageId::from_number(221).unwrap(),
                task: None,
                sender: Sender::Worker(WorkerName::parse("backend-lead").unwrap()),
            },
        ];
        let turn = message_turn(&owed);
        assert!(
            turn.starts_with("fold started you again for a message"),
            "{turn}"
        );
        assert!(
            turn.contains("\n- m-221 from human:tanim about T-10"),
            "{turn}"
        );
        assert!(turn.contains("\n- m-225 from backend-lead\n"), "{turn}");
        assert!(turn.contains("get_inbox"), "{turn}");
        assert!(
            turn.contains("If your bound task or review still waits for your work, continue it."),
            "{turn}"
        );
        assert!(!turn.contains("Do not start new work"), "{turn}");
        assert!(
            turn.ends_with("fold stops you again once you have answered."),
            "{turn}"
        );
    }
}
