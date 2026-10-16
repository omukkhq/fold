use std::sync::atomic::Ordering;

use fold_core::events::types;
use fold_core::roles::{
    HANDOFF_REQUEST, RefreshFacts, SUMMARY_ITEMS, SummaryProposal, SummaryTask, handoff_due,
    handoff_overdue, refresh_turn,
};
use fold_core::{AgentName, AreaName, EventActor, NewEvent, WorkerState};
use fold_store::agents::{self, WorkerProcessRow};
use fold_store::{StoreError, messages};
use serde_json::json;

use super::{Input, Supervisor, lock, stopped_event};
use crate::services::agents::AgentError;
use crate::services::agents::workers::new_session;
use crate::services::events::EventBuffer;
use crate::services::projects::load_config;

/// The context window of the worker's model among the windows a result reported: the entry of `model`, else the smallest one reported, else 0 for unknown.
fn context_window(windows: &[(String, u64)], model: Option<&str>) -> u64 {
    model
        .and_then(|model| windows.iter().find(|(name, _)| name == model))
        .map(|(_, window)| *window)
        .or_else(|| {
            windows
                .iter()
                .map(|(_, window)| *window)
                .filter(|window| *window > 0)
                .min()
        })
        .unwrap_or(0)
}

impl Supervisor {
    /// After each turn of a long-lived worker: a note of its session starts it fresh; otherwise a context past its role's `handoff_at` asks once for a note, and a lead that let [`fold_core::roles::HANDOFF_TURNS`] turns pass since the ask without one starts fresh with the fold summary alone.
    pub(super) async fn after_turn(&self, agent: &AgentName, windows: &[(String, u64)]) {
        if let Err(error) = self.check_handoff(agent, windows).await {
            tracing::warn!(%agent, code = crate::services::agents::code_of(&error), "could not check a worker's context size");
        }
    }

    async fn check_handoff(
        &self,
        agent: &AgentName,
        windows: &[(String, u64)],
    ) -> Result<(), AgentError> {
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
        if !row.role.is_long_lived() || row.state == WorkerState::Stopping {
            return Ok(());
        }
        if agents::session_handoff(&mut *conn, &row.session_id)
            .await?
            .is_some()
        {
            self.refresh_soon(agent);
            return Ok(());
        }
        if let Some(asked) = agents::handoff_asked(&mut *conn, row.id).await? {
            if handoff_overdue(asked, row.turns) {
                self.refresh_soon(agent);
            }
            return Ok(());
        }
        drop(conn);
        let config = load_config(&self.inner.paths).await?;
        let Some(percent) = config.role(row.role).handoff_at else {
            return Ok(());
        };
        let window = context_window(windows, row.model.as_deref());
        if !handoff_due(row.context_tokens, window, percent) {
            return Ok(());
        }
        let now = self.inner.clock.now();
        let asked = self
            .inner
            .store
            .write(async |tx| agents::set_handoff_asked(tx, row.id, row.turns, now).await)
            .await?;
        if asked {
            let input = lock(&self.inner.running)
                .get(agent)
                .map(|run| run.input.clone());
            if let Some(input) = input {
                let _ = input.send(Input::Turn(HANDOFF_REQUEST.to_owned()));
            }
        }
        Ok(())
    }

    /// Starts the lead again in a new session on a task of its own, once at a time per lead. The task never runs inside the reader of the lead's output, because the refresh waits for that reader to see the process exit.
    pub(in crate::services::agents::workers) fn refresh_soon(&self, agent: &AgentName) {
        if !lock(&self.inner.refreshing).insert(agent.clone()) {
            return;
        }
        let supervisor = self.clone();
        let agent = agent.clone();
        tokio::spawn(async move {
            if let Err(error) = supervisor.refresh(&agent).await {
                tracing::warn!(%agent, code = crate::services::agents::code_of(&error), "could not give a worker a fresh context");
            }
            lock(&supervisor.inner.refreshing).remove(&agent);
        });
    }

    /// Ends the lead's process without changing its row, then, in one transaction with `worker.refreshed`, gives the row a new session, and runs Claude Code in that session with a first turn that holds the old session's note, if any, and the fold summary. A lead that was stopped meanwhile ends `stopped` instead.
    async fn refresh(&self, agent: &AgentName) -> Result<(), AgentError> {
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
        drop(conn);
        if !row.role.is_long_lived() || !row.state.is_running() {
            return Ok(());
        }
        let run = lock(&self.inner.running).get(agent).cloned();
        if let Some(run) = run {
            run.detached.store(true, Ordering::SeqCst);
            self.terminate(&run, row.state == WorkerState::Working)
                .await;
        }
        let session = new_session()?;
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        let refreshed = self
            .inner
            .store
            .write(async |tx| {
                let Some(current) = agents::find_worker_process(tx.conn(), agent).await? else {
                    return Ok::<_, AgentError>(None);
                };
                if current.id != row.id
                    || current.session_id != row.session_id
                    || !current.state.is_running()
                {
                    return Ok(None);
                }
                if current.state == WorkerState::Stopping {
                    agents::record_exit(tx, current.id, WorkerState::Stopped, None, now).await?;
                    buffer
                        .append(tx, &stopped_event(&current, None)?, now)
                        .await?;
                    return Ok(None);
                }
                let note = agents::session_handoff(tx.conn(), &current.session_id)
                    .await?
                    .map(|note| note.body);
                agents::refresh_session(tx, current.id, &session, now).await?;
                let event =
                    NewEvent::new(types::WORKER_REFRESHED, EventActor::fold())?.data(json!({
                        "agent": agent.as_str(),
                        "role": current.role.as_str(),
                        "area": current.area.as_ref().map(AreaName::as_str),
                        "old_session": current.session_id,
                        "new_session": session,
                        "context_tokens": current.context_tokens,
                        "handoff": note.is_some(),
                    }))?;
                buffer.append(tx, &event, now).await?;
                let fresh = agents::find_worker_process(tx.conn(), agent)
                    .await?
                    .ok_or(StoreError::Corrupt("worker_processes"))?;
                Ok(Some((fresh, note)))
            })
            .await?;
        self.inner.bus.publish(buffer);
        let Some((fresh, note)) = refreshed else {
            return Ok(());
        };
        let turn = self.refresh_turn(&fresh, note.as_deref()).await?;
        self.launch(&fresh, false, Some(turn)).await?;
        Ok(())
    }

    /// The first turn of the fresh session: the note and the open tasks and proposals of the lead's area (every one for the review lead), with its unread count.
    async fn refresh_turn(
        &self,
        row: &WorkerProcessRow,
        note: Option<&str>,
    ) -> Result<String, AgentError> {
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let area = match &row.area {
            Some(area) => agents::find_area(&mut *conn, area).await?,
            None => None,
        };
        let label_id = area.as_ref().map(|area| area.label_id);
        let limit = SUMMARY_ITEMS as u64;
        let (tasks, task_count) = agents::open_area_tasks(&mut conn, label_id, limit).await?;
        let (proposals, proposal_count) =
            agents::open_area_proposals(&mut conn, label_id, limit).await?;
        let unread = messages::unread_count(&mut *conn, row.agent_id).await?;
        let tasks: Vec<SummaryTask> = tasks
            .into_iter()
            .map(|task| SummaryTask {
                id: task.id,
                status: task.status,
                title: task.title,
            })
            .collect();
        let proposals: Vec<SummaryProposal> = proposals
            .into_iter()
            .map(|proposal| SummaryProposal {
                id: proposal.id,
                task: proposal.task,
                title: proposal.title,
            })
            .collect();
        Ok(refresh_turn(&RefreshFacts {
            role: row.role,
            name: &row.agent,
            area: area.as_ref().map(|area| (&area.name, &area.label)),
            note,
            tasks: &tasks,
            task_count,
            proposals: &proposals,
            proposal_count,
            unread,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::agents::claude::ClaudeMessage;

    #[test]
    fn the_window_is_the_one_of_the_workers_model() {
        let windows = vec![
            ("claude-haiku-5-5".to_owned(), 200_000),
            ("claude-opus-5-5".to_owned(), 1_000_000),
        ];
        assert_eq!(context_window(&windows, Some("claude-opus-5-5")), 1_000_000);
        assert_eq!(context_window(&windows, Some("sonnet")), 200_000);
        assert_eq!(context_window(&windows, None), 200_000);
        assert_eq!(context_window(&[], Some("claude-opus-5-5")), 0);
        assert_eq!(context_window(&[("x".to_owned(), 0)], None), 0);
    }

    #[test]
    fn a_result_reports_the_window_of_each_model() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/claude/result.jsonl");
        let text = std::fs::read_to_string(path).unwrap();
        let result = text
            .lines()
            .filter_map(|line| match ClaudeMessage::parse(line).unwrap() {
                ClaudeMessage::Result(result) => Some(result),
                _ => None,
            })
            .next()
            .unwrap();
        assert_eq!(
            result.model_usage["claude-haiku-5-5"].context_window,
            1_000_000
        );
        let interrupted = ClaudeMessage::parse(
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"s"}"#,
        )
        .unwrap();
        assert!(
            matches!(interrupted, ClaudeMessage::Result(result) if result.model_usage.is_empty())
        );
        assert!(!new_session().unwrap().is_empty());
    }
}
