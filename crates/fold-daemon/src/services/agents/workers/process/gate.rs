use fold_core::events::types;
use fold_core::reviews::checks::{Purpose, RunReason, RunState, Stage};
use fold_core::{AgentName, CheckRunId, Event, EventActor, NewEvent, Role, WorkerState, codes};
use fold_store::reviews::checks::{self, CheckRunRow};
use fold_store::{StoreError, agents};
use serde_json::{Value, json};

use super::resume::SessionStart;
use super::{Input, Supervisor, lock};
use crate::services::agents::AgentError;
use crate::services::agents::workers::is_paused_refusal;
use crate::services::events::EventBuffer;

/// The run a `checks.finished` event ends when the checks of a worker's `create_proposal` or `update_proposal` did not pass, so its author's task worker has work again.
pub(super) fn gate_run(event: &Event) -> Option<CheckRunId> {
    if event.event_type != types::CHECKS_FINISHED {
        return None;
    }
    let data = &event.data;
    let text = |key: &str| data.get(key).and_then(Value::as_str);
    let gate = text("stage") == Some(Stage::Proposal.as_str())
        && matches!(
            text("purpose").and_then(Purpose::parse),
            Some(Purpose::CreateProposal | Purpose::UpdateProposal)
        )
        && matches!(
            text("state").and_then(RunState::parse),
            Some(RunState::Failed | RunState::Stopped)
        );
    if !gate {
        return None;
    }
    text("run")?.parse().ok()
}

impl Supervisor {
    /// Resumes the stopped task worker whose proposal command waited for `run` and did not get its version. A running task worker gets the push notice of fold's message on its own.
    pub(super) async fn resume_gate_author(self, run: CheckRunId) {
        if let Err(error) = self.resume_for_run(run).await {
            tracing::warn!(run = %run, code = crate::services::agents::code_of(&error), "could not resume the task worker of a failed check run");
        }
    }

    /// Resumes the task workers of every failed proposal command that no start of theirs has served yet, such as one the worker limits refused.
    pub(super) async fn resume_owed_gate_runs(self) {
        let owed = match checks::owed_gate_runs(
            self.inner.store.read(),
            &[WorkerState::Stopped, WorkerState::Failed],
        )
        .await
        {
            Ok(owed) => owed,
            Err(error) => {
                tracing::warn!(
                    code = error.log_code(),
                    "could not read the owed check runs"
                );
                return;
            }
        };
        for owed in owed {
            if let Err(error) = self.resume_for_run(owed.run).await {
                tracing::warn!(agent = %owed.agent, code = crate::services::agents::code_of(&error), "could not resume the task worker of a failed check run");
            }
        }
    }

    async fn resume_for_run(&self, run: CheckRunId) -> Result<(), AgentError> {
        let read = self.inner.store.read();
        let Some(row) = checks::get_run(read, run).await? else {
            return Ok(());
        };
        let Some(worker) = row.worker.clone() else {
            return Ok(());
        };
        if !matches!(row.state, RunState::Failed | RunState::Stopped) {
            return Ok(());
        }
        let agent = worker.agent();
        // The row is read under the lock, so a second event finds the task worker running and leaves it to its stdin notices.
        let resuming = self.inner.resuming.lock().await;
        let mut conn = read.acquire().await.map_err(StoreError::from)?;
        let process = agents::find_worker_process(&mut conn, &agent).await?;
        drop(conn);
        let Some(process) = process else {
            return Ok(());
        };
        if process.role != Role::TaskWorker || process.state.is_running() {
            return Ok(());
        }
        let failed: Vec<String> = checks::list_results(read, run, false)
            .await?
            .into_iter()
            .filter(|result| result.required && result.outcome.is_failure())
            .map(|result| result.check.unwrap_or_else(|| "setup".to_owned()))
            .collect();
        let turn = gate_turn(&row, &failed);
        let watch = match self.resume(&agent, turn.clone(), &[row.task]).await {
            Ok(watch) => watch,
            // The run stays owed, and the resume of the pause serves it.
            Err(error) if is_paused_refusal(&error) => return Ok(()),
            // A start outside this path came first; the task worker takes this turn as its next one.
            Err(AgentError::Fold(error)) if error.code() == codes::WORKER_EXISTS => {
                if let Some(running) = lock(&self.inner.running).get(&agent) {
                    let _ = running.input.send(Input::Turn(turn));
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
            Some(format!(
                "Your earlier session is gone, so fold started you in a new one.\n\n{turn}"
            )),
        ));
        if watch.outcome().await == SessionStart::Went {
            self.record_gate_resumed(&agent, &row).await?;
        }
        Ok(())
    }

    /// `worker.resumed` for the run, once the resumed session runs.
    async fn record_gate_resumed(
        &self,
        agent: &AgentName,
        run: &CheckRunRow,
    ) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        self.inner
            .store
            .write(async |tx| {
                let mut event =
                    NewEvent::new(types::WORKER_RESUMED, EventActor::fold())?.task(run.task);
                if let Some(proposal) = run.proposal {
                    event = event.proposal(proposal);
                }
                let event = event.data(json!({
                    "agent": agent.as_str(),
                    "reason": "checks",
                    "stage": Stage::Proposal.as_str(),
                    "run": run.id.to_string(),
                    "resumed": true,
                }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, AgentError>(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }
}

/// What a task worker is told when the checks of its proposal command did not pass.
fn gate_turn(run: &CheckRunRow, failed: &[String]) -> String {
    let (made, again) = match run.proposal {
        Some(proposal) => (
            format!("fold added no version to {proposal}"),
            "update_proposal",
        ),
        None => ("fold made no proposal".to_owned(), "create_proposal"),
    };
    let why = if failed.is_empty() {
        run.reason
            .map_or("they did not pass", RunReason::as_str)
            .to_owned()
    } else {
        format!("{} failed", failed.join(", "))
    };
    format!(
        "The checks of {} ({}) did not pass, so {made}: {why}. Read fold's message, or call get_check_run with {} for the logs. Fix the work in your task workspace, call run_checks, then call {again} again.",
        run.task, run.id, run.id
    )
}

#[cfg(test)]
mod tests {
    use fold_core::object::ObjectHash;
    use fold_core::{ProposalId, RevisionId, TaskId, Timestamp, WorkspaceId};

    use super::*;

    fn event(data: Value) -> Event {
        Event {
            seq: 1,
            event_type: types::CHECKS_FINISHED,
            actor: EventActor::fold(),
            task: TaskId::from_number(1),
            proposal: None,
            workspace: None,
            revision: None,
            data,
            created_at: Timestamp::from_millis(0).unwrap(),
        }
    }

    #[test]
    fn a_failed_or_stopped_proposal_command_is_a_gate_run() {
        let failed = event(
            json!({"run": "cr-4", "stage": "proposal", "purpose": "create_proposal", "state": "failed"}),
        );
        assert_eq!(gate_run(&failed), CheckRunId::from_number(4));
        let stopped = event(
            json!({"run": "cr-5", "stage": "proposal", "purpose": "update_proposal", "state": "stopped"}),
        );
        assert_eq!(gate_run(&stopped), CheckRunId::from_number(5));
        for data in [
            json!({"run": "cr-4", "stage": "proposal", "purpose": "check", "state": "failed"}),
            json!({"run": "cr-4", "stage": "proposal", "purpose": "create_proposal", "state": "passed"}),
            json!({"run": "cr-4", "stage": "integration", "purpose": "integrate", "state": "failed"}),
        ] {
            assert_eq!(gate_run(&event(data)), None);
        }
    }

    #[test]
    fn the_turn_names_the_failed_checks_and_the_command_to_call_again() {
        let mut run = CheckRunRow {
            id: CheckRunId::from_number(4).unwrap(),
            stage: Stage::Proposal,
            purpose: Purpose::CreateProposal,
            task: TaskId::from_number(12).unwrap(),
            workspace: WorkspaceId::from_number(3),
            worker: None,
            proposal: None,
            version: None,
            base: RevisionId::from_number(1).unwrap(),
            tree: ObjectHash::from_bytes([1; 32]),
            actor: "backend".to_owned(),
            title: Some("Add".to_owned()),
            evidence: None,
            intent: None,
            state: RunState::Failed,
            reason: Some(RunReason::CheckFailed),
            created_at: Timestamp::from_millis(0).unwrap(),
            finished_at: Some(Timestamp::from_millis(1).unwrap()),
            batch: None,
        };
        let text = gate_turn(&run, &["unit".to_owned()]);
        assert!(
            text.starts_with(
                "The checks of T-12 (cr-4) did not pass, so fold made no proposal: unit failed."
            ),
            "{text}"
        );
        assert!(text.ends_with("then call create_proposal again."), "{text}");
        run.proposal = ProposalId::from_number(3);
        run.reason = Some(RunReason::FilesChanged);
        let text = gate_turn(&run, &[]);
        assert!(
            text.contains("fold added no version to P-3: files_changed."),
            "{text}"
        );
        assert!(text.contains("call update_proposal again"), "{text}");
    }
}
