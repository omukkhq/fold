use std::sync::atomic::Ordering;

use fold_core::{AgentName, EventActor, Role, WorkerState};
use fold_store::agents::{self, WorkerProcessRow};
use fold_store::reviews::{self, ProposalState};
use fold_store::{StoreError, messages};

use super::stop::Park;
use super::{Supervisor, lock};
use crate::services::messages::asked_proposals;
use crate::services::reviews::held_open_batches;

impl Supervisor {
    /// Parks a reviewer whose turn ended while it waits for a person: every bound open proposal without a verdict has an open ask of its own to the humans, and it holds no open batch with a member, has no unread message and no user turn waiting in its stdin. It parks itself, so only a person's message, the answer in the ask's thread among them, resumes it in its own session; meanwhile it never stalls and fold sends its review lead no notice of the versions it leaves.
    pub(super) async fn park_if_asking(&self, agent: &AgentName) {
        match self.asking_park(agent).await {
            Ok(Some((park, id))) => {
                // The stop waits for the exit that this process's reader records, so it runs in a task of its own.
                let supervisor = self.clone();
                let agent = agent.clone();
                tokio::spawn(async move {
                    // A turn that came since the row was read, such as the answer itself, keeps the reviewer running.
                    let still = |current: &WorkerProcessRow| {
                        current.id == id
                            && may_park(current)
                            && !supervisor.has_open_turn(&current.agent)
                    };
                    if let Err(error) = supervisor.stop_and_park_when(&agent, &park, still).await {
                        tracing::warn!(%agent, code = crate::services::agents::code_of(&error), "could not park a reviewer that waits for a person");
                    }
                });
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(%agent, code = error.log_code(), "could not check whether a reviewer waits for a person");
            }
        }
    }

    /// The park of [`Supervisor::park_if_asking`] for `agent`, with the row id it read, when it waits for a person.
    async fn asking_park(&self, agent: &AgentName) -> Result<Option<(Park, i64)>, StoreError> {
        if self.has_open_turn(agent) {
            return Ok(None);
        }
        let read = self.inner.store.read();
        let mut conn = read.acquire().await.map_err(StoreError::from)?;
        let Some(row) = agents::find_worker_process(&mut conn, agent).await? else {
            return Ok(None);
        };
        drop(conn);
        if !may_park(&row) || messages::undelivered_count(read, agent).await? > 0 {
            return Ok(None);
        }
        let mut waiting = Vec::new();
        for proposal in &row.proposals {
            let open = reviews::get_proposal(read, *proposal)
                .await?
                .is_some_and(|found| {
                    found.state == ProposalState::Open && found.latest_verdict.is_none()
                });
            if open {
                waiting.push(*proposal);
            }
        }
        if waiting.is_empty() {
            return Ok(None);
        }
        let asked = asked_proposals(&self.inner.store, agent).await?;
        if !waiting.iter().all(|proposal| asked.contains(proposal)) {
            return Ok(None);
        }
        let mut conn = read.acquire().await.map_err(StoreError::from)?;
        let holds_batch = held_open_batches(&mut conn)
            .await?
            .iter()
            .any(|batch| batch.holder.belongs_to(agent));
        if holds_batch {
            return Ok(None);
        }
        let listed = waiting
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        Ok(Some((
            Park {
                actor: EventActor::fold(),
                by: agent.clone(),
                wait_for: format!("a person's answer to its question about {listed}"),
            },
            row.id,
        )))
    }

    /// Whether a user turn written to the agent's stdin has no result yet.
    fn has_open_turn(&self, agent: &AgentName) -> bool {
        lock(&self.inner.running)
            .get(agent)
            .is_some_and(|run| run.open_turns.load(Ordering::SeqCst) > 0)
    }
}

/// A reviewer that is idle, with no turn that waits for a cause outside it and no park yet.
fn may_park(row: &WorkerProcessRow) -> bool {
    row.role == Role::Reviewer
        && row.state == WorkerState::Idle
        && row.wait.is_none()
        && row.park.is_none()
}
