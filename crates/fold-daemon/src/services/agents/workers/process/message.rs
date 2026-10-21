use std::collections::BTreeMap;
use std::fmt::Write as _;

use fold_core::events::types;
use fold_core::{AgentName, Event, LiveEvent, Role, Sender, WorkerState, codes};
use fold_store::agents::{self, WorkerProcessRow};
use fold_store::messages::{self, OwedMessage};
use fold_store::reviews::checks;
use fold_store::tasks::reports;
use fold_store::{StoreError, WriteTx};
use serde_json::json;

use super::resume::SessionStart;
use super::{Input, Supervisor, lock};
use crate::services::agents::AgentError;
use crate::services::agents::workers::{code_of, is_paused_refusal};
use crate::services::events::EventBuffer;
use crate::services::messages::{forward_in_tx, record_pushed};

/// The states of a task worker that a message starts again.
const ENDED: [WorkerState; 2] = [WorkerState::Stopped, WorkerState::Failed];

impl Supervisor {
    /// Starts each stopped or failed task worker again that has a message to answer from a human, its area's lead or the main agent; see [`Supervisor::answer_messages`]. A paused project leaves the messages owed, and the resume of the pause serves them.
    pub(super) async fn resume_owed_messages(self) {
        let owed = match messages::owed_to_stopped(self.inner.store.read()).await {
            Ok(owed) => owed,
            Err(error) => {
                tracing::warn!(
                    code = error.log_code(),
                    "could not read the messages owed by stopped task workers"
                );
                return;
            }
        };
        let mut by_agent: BTreeMap<AgentName, Vec<OwedMessage>> = BTreeMap::new();
        for owed in owed {
            by_agent.entry(owed.agent.clone()).or_default().push(owed);
        }
        for (agent, owed) in by_agent {
            if let Err(error) = self.answer_messages(&agent, owed).await {
                tracing::warn!(%agent, code = code_of(&error), "could not start a task worker to answer a message");
            }
        }
    }

    /// Resumes the task worker of `agent` in its own session for one turn that names `owed`. A worker whose own resume is owed, such as requested changes the worker limits held back, waits for that resume, which moves its start, and answers once it stops again. When the worker cannot start, because a worker limit refuses it or its session is gone, every message goes on to its area's lead; see [`forward_in_tx`].
    async fn answer_messages(
        &self,
        agent: &AgentName,
        owed: Vec<OwedMessage>,
    ) -> Result<(), AgentError> {
        if !lock(&self.inner.message_starts).insert(agent.clone()) {
            return Ok(());
        }
        let served = self.answer_messages_once(agent, owed).await;
        lock(&self.inner.message_starts).remove(agent);
        served
    }

    async fn answer_messages_once(
        &self,
        agent: &AgentName,
        owed: Vec<OwedMessage>,
    ) -> Result<(), AgentError> {
        // The row is read under the lock, so another resume of the same task worker finds it running.
        let resuming = self.inner.resuming.lock().await;
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let Some(row) = agents::find_worker_process(&mut conn, agent).await? else {
            return Ok(());
        };
        if row.role != Role::TaskWorker || !ENDED.contains(&row.state) {
            return Ok(());
        }
        let directing = directing_agents(&mut conn, &row).await?;
        drop(conn);
        let owed: Vec<OwedMessage> = owed
            .into_iter()
            .filter(|owed| match &owed.sender {
                Sender::Human(_) => true,
                Sender::Worker(worker) => directing.contains(&worker.agent()),
                Sender::Fold => false,
            })
            .collect();
        if owed.is_empty() {
            return Ok(());
        }
        if self.owes_own_resume(agent).await? {
            drop(resuming);
            tokio::spawn(self.clone().resume_pending());
            tokio::spawn(self.clone().resume_owed_send_backs());
            tokio::spawn(self.clone().resume_owed_gate_runs());
            return Ok(());
        }
        let turn = message_turn(&owed);
        lock(&self.inner.answering).insert(agent.clone());
        let watch = match self.resume(agent, turn.clone(), &[]).await {
            Ok(watch) => watch,
            Err(error) => {
                lock(&self.inner.answering).remove(agent);
                return match error {
                    // The messages stay owed, and the resume of the pause serves them.
                    error if is_paused_refusal(&error) => Ok(()),
                    // A start outside this path came first; the task worker takes this turn as its next one.
                    AgentError::Fold(error) if error.code() == codes::WORKER_EXISTS => {
                        if let Some(run) = lock(&self.inner.running).get(agent) {
                            let _ = run.input.send(Input::Turn(turn));
                        }
                        Ok(())
                    }
                    AgentError::Fold(error) if error.code() == codes::WORKER_LIMIT_REACHED => {
                        drop(resuming);
                        self.forward(&row, &owed).await
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
            SessionStart::Lost | SessionStart::Ended => {
                lock(&self.inner.answering).remove(agent);
                self.forward(&row, &owed).await
            }
        }
    }

    /// Whether a resume of the task worker for its own work is owed: requested changes, a conflict, failed checks or a send-back that no start has served.
    async fn owes_own_resume(&self, agent: &AgentName) -> Result<bool, AgentError> {
        let read = self.inner.store.read();
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

    /// The resumed session got its turn about `owed`: the delivery line shows the message waiting for the worker, as for a running worker, until the worker reads its inbox.
    async fn record_told(&self, agent: &AgentName, owed: &OwedMessage) {
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
                tracing::warn!(%agent, code = error.log_code(), "could not record the turn that told a task worker of a message");
            }
        }
    }

    /// Sends each of `owed` on to the area's lead of the task worker of `row`.
    async fn forward(
        &self,
        row: &WorkerProcessRow,
        owed: &[OwedMessage],
    ) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        self.inner
            .store
            .write(async |tx| {
                for owed in owed {
                    if still_owed(tx, owed).await? {
                        forward_in_tx(tx, &mut buffer, owed, row.area.as_ref(), now).await?;
                    }
                }
                Ok::<_, AgentError>(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }

    /// Stops a task worker that fold started to answer a message once it is idle, has read every message sent to it and no bound task waits for its work. It is checked when a turn ends and when the worker sends or acknowledges a message.
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
                tracing::warn!(%agent, code = error.log_code(), "could not check whether a task worker answered its messages");
            }
        }
    }

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
        Ok(agents::tasks_awaiting_proposal(read, &[row.id])
            .await?
            .get(&row.id)
            == Some(&0))
    }

    /// Forgets that the agent's process was started to answer a message, once the process ends.
    pub(super) fn forget_answering(&self, agent: &AgentName) {
        lock(&self.inner.answering).remove(agent);
    }
}

/// The agent whose message, sent or acknowledged, may be the answer of a task worker that fold started to answer a message.
pub(super) fn answering_agent(event: &Event) -> Option<AgentName> {
    if event.event_type != types::MESSAGE_SENT && event.event_type != types::MESSAGE_ACKNOWLEDGED {
        return None;
    }
    AgentName::parse(event.actor.agent()?).ok()
}

/// The agents that direct the task worker of `row`: its area's lead and the main agent.
async fn directing_agents(
    conn: &mut fold_store::SqliteConnection,
    row: &WorkerProcessRow,
) -> Result<Vec<AgentName>, StoreError> {
    let mut directing = Vec::new();
    if let Some(area) = &row.area
        && let Some(lead) = agents::find_area(&mut *conn, area)
            .await?
            .and_then(|area| area.lead)
    {
        directing.push(lead);
    }
    let directory = messages::directory(conn, Vec::new()).await?;
    directing.extend(
        directory
            .agents
            .into_iter()
            .filter(|agent| agent.tags.iter().any(fold_core::Tag::is_main))
            .map(|agent| agent.name),
    );
    Ok(directing)
}

/// Whether `owed` is still unread, untold and not sent on, read again in the forward's own transaction.
async fn still_owed(tx: &mut WriteTx, owed: &OwedMessage) -> Result<bool, StoreError> {
    Ok(messages::owed_to_stopped(tx.conn())
        .await?
        .iter()
        .any(|other| other.agent == owed.agent && other.message == owed.message))
}

/// The turn of a task worker that fold started again to answer `owed`.
fn message_turn(owed: &[OwedMessage]) -> String {
    let mut text = String::from(
        "fold started you again to answer a message sent to you while you were stopped:",
    );
    for owed in owed {
        let _ = write!(text, "\n- {} from {}", owed.message, owed.sender);
        if let Some(task) = owed.task {
            let _ = write!(text, " about {task}");
        }
    }
    let _ = write!(
        text,
        "\n\nRead it with get_inbox, then answer in its thread with send_message and reply_to. Do not start new work: fold stops you again once you have answered."
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_core::{HumanName, MessageId, TaskId, WorkerName};

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
            turn.starts_with("fold started you again to answer"),
            "{turn}"
        );
        assert!(
            turn.contains("\n- m-221 from human:tanim about T-10"),
            "{turn}"
        );
        assert!(turn.contains("\n- m-225 from backend-lead\n"), "{turn}");
        assert!(turn.contains("get_inbox"), "{turn}");
        assert!(
            turn.ends_with("fold stops you again once you have answered."),
            "{turn}"
        );
    }
}
