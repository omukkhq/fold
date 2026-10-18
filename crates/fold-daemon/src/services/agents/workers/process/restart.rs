use fold_core::roles::{StartFacts, first_turn};
use fold_core::{AgentName, WorkerState};
use fold_store::StoreError;
use fold_store::agents::{self, WorkerProcessRow};

use super::{Supervisor, lock, resume};
use crate::services::agents::AgentError;
use crate::services::agents::workers::code_of;

/// The turn a worker that was working gets after a daemon restart resumed its session.
pub const RESTART_TURN: &str = "fold restarted while you were working. Check your claims and your workspace, then continue your work.";

/// The first turn of each worker that the resume of a pause runs again in its own session. Its claims kept their leases through the pause.
pub const PAUSE_TURN: &str = "fold resumed you after a pause of every agent of the project. Your claims kept their leases. Check your inbox, your claims and your workspace, then continue your work.";

impl Supervisor {
    /// Runs again, after a daemon start, every worker that was working or idle when the daemon stopped: `--resume` of its session, with a turn that tells a working one to go on. A worker whose session is gone starts again in a new one, a lead with its latest hand-off note. A worker with an open question waits without a process until a human answers it, and then resumes with the answer as its first turn. A worker left `starting` ends `failed` and one left `stopping` ends `stopped`. A row that cannot be served is logged and the next row is served. Requested changes, conflicts and send-backs that the worker limits held back are then served.
    pub async fn resume_after_restart(&self) -> Result<(), AgentError> {
        if self.is_paused().await? {
            return Ok(());
        }
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let rows = agents::list_worker_processes(&mut conn).await?;
        drop(conn);
        for row in rows {
            if let Err(error) = self.restart_row(&row).await {
                tracing::warn!(agent = %row.agent, code = code_of(&error), "could not serve a worker after the daemon start");
            }
        }
        tokio::spawn(self.clone().resume_pending());
        tokio::spawn(self.clone().resume_owed_send_backs());
        tokio::spawn(self.clone().resume_owed_gate_runs());
        Ok(())
    }

    /// What a daemon start does with one row; see [`Supervisor::resume_after_restart`].
    async fn restart_row(&self, row: &WorkerProcessRow) -> Result<(), AgentError> {
        match row.state {
            WorkerState::Working | WorkerState::Idle => {
                let turn = (row.state == WorkerState::Working).then(|| RESTART_TURN.to_owned());
                self.relaunch(row, turn, None).await?;
            }
            WorkerState::Starting => self.fail(row.id, &row.agent, "daemon_restart").await?,
            WorkerState::Stopping => self.stopped_without_process(row).await?,
            WorkerState::Stopped | WorkerState::Failed => {}
        }
        Ok(())
    }

    /// Runs a worker whose row kept its state without a process again in its own session with `--resume`, after a daemon start or a pause; a worker whose session is gone starts again in a new one, which also gets `lost` after its own note. A worker with an open question waits without a process until a human answers it.
    async fn relaunch(
        &self,
        row: &WorkerProcessRow,
        turn: Option<String>,
        lost: Option<String>,
    ) -> Result<(), AgentError> {
        if self.waits_for_answer(row).await? {
            return Ok(());
        }
        let now = self.inner.clock.now();
        self.inner
            .store
            .write(async |tx| agents::restart_worker_process(tx, row.id, now).await)
            .await?;
        if let Some(watch) = self.launch(row, true, turn).await? {
            tokio::spawn(self.clone().restart_lost(row.agent.clone(), watch, lost));
        }
        Ok(())
    }

    pub(in crate::services::agents::workers) async fn is_paused(&self) -> Result<bool, StoreError> {
        Ok(agents::pause_state(self.inner.store.read())
            .await?
            .is_paused())
    }

    /// Ends a pause that began at `paused_at`: each worker that was working, idle or starting runs again in its own session with [`PAUSE_TURN`], followed by the first turn a starting worker never read, the answer to a question a human answered during the pause, and the requested changes, conflicts and send-backs its task worker missed; a worker left `stopping` ends `stopped`. The resumes, send-backs and fresh starts of lost sessions that the pause held back are then served. A row that cannot be served is logged and the next row is served.
    pub(in crate::services::agents::workers) async fn resume_after_pause(
        &self,
        paused_at: fold_core::Timestamp,
    ) -> Result<(), AgentError> {
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let rows = agents::list_worker_processes(&mut conn).await?;
        drop(conn);
        let mut owed = self.owed_to_running().await;
        let mut sent_back = self.send_backs_owed_to_paused().await;
        for row in rows {
            if self.is_running(&row.agent) {
                continue;
            }
            let served = match row.state {
                WorkerState::Working | WorkerState::Idle | WorkerState::Starting => {
                    let owed_here = owed.remove(&row.agent);
                    let sent_back_here = sent_back.remove(&row.agent);
                    match self
                        .unpaused_turn(&row, paused_at, owed_here, sent_back_here)
                        .await
                    {
                        Ok((parts, first)) => {
                            self.relaunch(&row, Some(parts.join("\n\n")), first).await
                        }
                        Err(error) => Err(error),
                    }
                }
                WorkerState::Stopping => self.stopped_without_process(&row).await,
                WorkerState::Stopped | WorkerState::Failed => Ok(()),
            };
            if let Err(error) = served {
                tracing::warn!(agent = %row.agent, code = code_of(&error), "could not resume a worker after a pause");
            }
        }
        let lost: Vec<(AgentName, Option<String>)> = lock(&self.inner.held_lost).drain().collect();
        for (agent, instructions) in lost {
            tokio::spawn(self.clone().start_lost(agent, instructions));
        }
        tokio::spawn(self.clone().resume_pending());
        tokio::spawn(self.clone().resume_owed_send_backs());
        tokio::spawn(self.clone().resume_owed_gate_runs());
        Ok(())
    }

    /// The parts of the turn that ends a pause for `row`: [`PAUSE_TURN`], then the first turn of a `starting` worker, whose process never read it, the answer to its question that a human gave during the pause, which the worker's repeat of the call then takes, and the work owed to it. A running worker owes `owed` and `sent_back`, recorded since its process started; a `starting` worker owes what its bound proposals and reports still ask for, because the turn that told of it went unread. The first turn also comes back on its own, for a worker whose session turns out to be gone.
    async fn unpaused_turn(
        &self,
        row: &WorkerProcessRow,
        paused_at: fold_core::Timestamp,
        owed: Option<Vec<resume::Found>>,
        sent_back: Option<String>,
    ) -> Result<(Vec<String>, Option<String>), AgentError> {
        let mut parts = vec![PAUSE_TURN.to_owned()];
        let first = if row.state == WorkerState::Starting {
            Some(self.rebuilt_first_turn(row).await?)
        } else {
            None
        };
        if let Some(first) = &first {
            parts.push(first.clone());
        }
        if let Some(question) = self.answered_during_pause(row, paused_at).await?
            && let Some(answer) = self.answer_turn(&question).await?
        {
            self.replay(&question);
            parts.push(answer);
        }
        let (owed, sent_back) = if row.state == WorkerState::Starting {
            let owed = self.owed_to_unstarted(row).await?;
            (
                (!owed.is_empty()).then_some(owed),
                self.send_backs_of_unstarted(row).await?,
            )
        } else {
            (owed, sent_back)
        };
        if let Some(found) = owed {
            parts.push(resume::notice_turn(&found));
        }
        parts.extend(sent_back);
        Ok((parts, first))
    }

    /// The first turn of a start, from the row's role, area and bound tasks and proposals. fold never stores a starter's instructions, so this turn goes without them.
    async fn rebuilt_first_turn(&self, row: &WorkerProcessRow) -> Result<String, AgentError> {
        let label = match &row.area {
            Some(area) => agents::find_area(self.inner.store.read(), area)
                .await?
                .map(|area| area.label),
            None => None,
        };
        Ok(first_turn(&StartFacts {
            role: row.role,
            name: &row.agent,
            area: row.area.as_ref().zip(label.as_ref()),
            tasks: &row.tasks,
            proposals: &row.proposals,
            instructions: None,
        }))
    }
}
