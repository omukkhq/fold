use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use fold_core::accounts::AccountStamp;
use fold_core::{AgentName, WorkerState};
use fold_store::StoreError;
use fold_store::agents::{self, ProcessEnd};

use super::stop::stopped_event;
use super::{Supervisor, lock};
use crate::push::StdinSink;
use crate::services::agents::workers::code_of;
use crate::services::agents::{AgentError, accounts};
use crate::services::events::EventBuffer;
use crate::services::projects::load_config;

impl Supervisor {
    /// The account the project names now, `None` for Claude Code's own login. An account that cannot be read or that the host lacks gives `Err`, and then no worker changes its account: a start would fail, while the running process can go on.
    async fn current_account(&self) -> Result<Option<AccountStamp>, ()> {
        let config = load_config(&self.inner.paths).await.map_err(|_| ())?;
        let Some(name) = config.account else {
            return Ok(None);
        };
        let Some(dir) = lock(&self.inner.launcher).accounts.clone() else {
            return Err(());
        };
        match tokio::task::spawn_blocking(move || accounts::stamp(&dir, &name)).await {
            Ok(Ok(Some(stamp))) => Ok(Some(stamp)),
            _ => Err(()),
        }
    }

    /// Lets each running worker whose process started with another account than the project's continue with the project's account: an idle one at once, a working one when its turn ends.
    pub(in crate::services::agents::workers) fn switch_accounts(&self) {
        for agent in self.running_agents() {
            self.switch_soon(&agent);
        }
    }

    /// [`Supervisor::switch_account`] on a task of its own, because a switch waits for the reader of the worker's output to see the process exit.
    pub(super) fn switch_soon(&self, agent: &AgentName) {
        let supervisor = self.clone();
        let agent = agent.clone();
        tokio::spawn(async move {
            if let Err(error) = supervisor.switch_account(&agent).await {
                tracing::warn!(%agent, code = code_of(&error), "could not give a worker the project's account");
            }
        });
    }

    /// Ends the idle process of `agent` that started with another account than the project's, without changing its row, and runs Claude Code again in the same session with `--resume` and the project's account, as a daemon start resumes an idle worker: no turn, unless work waits for a lead. A working worker, one with a turn still queued, one that waits for a person's answer, one that fold started to answer a message and one under another restart keep their process, and the end of their next turn looks again. The notices that come while the process changes wait for the new one. A worker that was stopped meanwhile ends `stopped`; in a paused project the row stays for the resume, which reads the account again.
    async fn switch_account(&self, agent: &AgentName) -> Result<(), AgentError> {
        let Some(run) = lock(&self.inner.running).get(agent).cloned() else {
            return Ok(());
        };
        let Ok(current) = self.current_account().await else {
            return Ok(());
        };
        if run.account == current
            || run.open_turns.load(Ordering::SeqCst) > 0
            || lock(&self.inner.answering).contains(agent)
        {
            return Ok(());
        }
        let mut conn = self
            .inner
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let found = agents::find_worker_process(&mut conn, agent).await?;
        drop(conn);
        let Some(row) = found else {
            return Ok(());
        };
        if row.state != WorkerState::Idle || self.waits_for_answer(&row).await? {
            return Ok(());
        }
        if row.role == fold_core::Role::Main {
            let config = load_config(&self.inner.paths).await?;
            if self.main_refusal(&config).is_some() {
                return Ok(());
            }
        }
        if !lock(&self.inner.refreshing).insert(agent.clone()) {
            return Ok(());
        }
        lock(&self.inner.switching).insert(agent.clone());
        let held: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink: StdinSink = {
            let held = Arc::clone(&held);
            Arc::new(move |text: &str| {
                lock(&held).push(text.to_owned());
                true
            })
        };
        self.inner.push.register_stdin(agent, sink);
        let switched = self.switch_row(agent, &run, &row).await;
        lock(&self.inner.switching).remove(agent);
        lock(&self.inner.refreshing).remove(agent);
        if !matches!(switched, Ok(true)) {
            self.inner.push.clear_stdin(agent);
            return switched.map(|_| ());
        }
        let waiting = std::mem::take(&mut *lock(&held));
        if !waiting.is_empty() {
            self.send_turn(agent, &waiting.join("\n\n"));
        }
        Ok(())
    }

    /// The part of [`Supervisor::switch_account`] after the notices wait: the end of the process, the row's check and the new start. Returns whether the new process runs.
    async fn switch_row(
        &self,
        agent: &AgentName,
        run: &super::Running,
        row: &agents::WorkerProcessRow,
    ) -> Result<bool, AgentError> {
        run.detached.store(true, Ordering::SeqCst);
        self.terminate(run, false).await;
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        let current = self
            .inner
            .store
            .write(async |tx| {
                let Some(current) = agents::find_worker_process(tx.conn(), agent).await? else {
                    return Ok::<_, AgentError>(None);
                };
                if current.id != row.id
                    || current.session_id != row.session_id
                    || agents::pause_state(tx.conn()).await?.is_paused()
                {
                    return Ok(None);
                }
                match current.state {
                    WorkerState::Stopping => {
                        agents::record_exit(tx, current.id, ProcessEnd::Stopped, None, now).await?;
                        buffer
                            .append(tx, &stopped_event(&current, None)?, now)
                            .await?;
                        Ok(None)
                    }
                    WorkerState::Idle | WorkerState::Working => {
                        agents::restart_worker_process(tx, current.id, now).await?;
                        Ok(Some(current))
                    }
                    _ => Ok(None),
                }
            })
            .await?;
        self.inner.bus.publish(buffer);
        let Some(current) = current else {
            return Ok(false);
        };
        let kept = BTreeSet::from([agent.clone()]);
        let turn = self.with_catch_up(&current, None, &kept).await;
        let Some(watch) = self.launch(&current, true, turn).await? else {
            return Ok(false);
        };
        tokio::spawn(self.clone().restart_lost(agent.clone(), watch, None));
        Ok(true)
    }
}
