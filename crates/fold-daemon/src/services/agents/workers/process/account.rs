use std::collections::BTreeSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use fold_core::accounts::AccountStamp;
use fold_core::{AgentName, WorkerState};
use fold_store::StoreError;
use fold_store::agents::{self, ProcessEnd};

use super::stop::stopped_event;
use super::{Input, Process, Supervisor, lock};
use crate::push::StdinSink;
use crate::services::agents::workers::code_of;
use crate::services::agents::{AgentError, accounts};
use crate::services::events::EventBuffer;
use crate::services::projects::load_config;

impl Supervisor {
    /// The account the project names now, `None` for Claude Code's own login. An account that cannot be read or that the host lacks gives `Err`, and then no worker changes its account: a start would fail, while the running process can go on.
    pub(super) async fn current_account(&self) -> Result<Option<AccountStamp>, ()> {
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

    /// The turn `text` for `process` when it goes to that process now, or `None` when it waits for the process's switch: the process is idle and runs with another account than the project's, as after `fld account update`, so the turn runs on the project's account in the same session instead. A turn that comes while a switch replaces the process waits for the new one. When the account cannot be read, the process keeps its account and takes the turn.
    pub(super) async fn holds_for_switch(&self, process: &Process, text: String) -> Option<String> {
        let agent = &process.agent;
        {
            // The switch removes the worker from `switching` before it takes the turns that waited, so a turn kept under this guard is always among them.
            let switching = lock(&self.inner.switching);
            if switching.contains(agent) {
                lock(&self.inner.switch_turns)
                    .entry(agent.clone())
                    .or_default()
                    .push(text);
                return None;
            }
        }
        if process.open_turns.load(Ordering::SeqCst) > 0
            || lock(&self.inner.refreshing).contains(agent)
        {
            return Some(text);
        }
        let Some(account) = lock(&self.inner.running)
            .get(agent)
            .filter(|run| run.generation == process.generation)
            .map(|run| run.account.clone())
        else {
            return Some(text);
        };
        match self.current_account().await {
            Ok(current) if current != account => {
                let first = {
                    let mut turns = lock(&self.inner.switch_turns);
                    let held = turns.entry(agent.clone()).or_default();
                    held.push(text);
                    held.len() == 1
                };
                if first {
                    self.switch_soon(agent);
                }
                None
            }
            _ => Some(text),
        }
    }

    /// Ends the idle process of `agent` that started with another account than the project's, without changing its row, and runs Claude Code again in the same session with `--resume` and the project's account, as a daemon start resumes an idle worker. Its first turn holds the turns that waited for the switch (see [`Supervisor::holds_for_switch`]) and the notices that came while the process changed; without them it runs no turn, unless work waits for a lead. A working worker, one with a turn still queued, one that waits for a person's answer, one that fold started to answer a message and one under another restart keep their process, which takes the turns that waited, and the end of their next turn looks again. A worker that was stopped meanwhile ends `stopped`; in a paused project the row stays for the resume, which reads the account again.
    async fn switch_account(&self, agent: &AgentName) -> Result<(), AgentError> {
        let (ran, held, result) = match self.switch_process(agent).await {
            // Another restart of the worker is under way: the turns that wait stay for it.
            Ok(None) => return Ok(()),
            Ok(Some((ran, held))) => (ran, held, Ok(())),
            Err(error) => (false, Vec::new(), Err(error)),
        };
        if ran {
            // The waiting turns run at once on the new process: a turn that reached a usage limit runs again on the new account, and a re-run sent to the old process, which ended, is sent again.
            self.forget_rerun(agent);
            self.run_limit_turns_now();
        }
        let mut texts = lock(&self.inner.switch_turns)
            .remove(agent)
            .unwrap_or_default();
        texts.extend(held);
        if !texts.is_empty() {
            let text = texts.join("\n\n");
            if ran {
                self.send_turn(agent, &text);
            } else if let Some(run) = lock(&self.inner.running).get(agent) {
                let _ = run.input.send(Input::KeptTurn(text));
            }
        }
        result
    }

    /// [`Supervisor::switch_account`] up to the turn: whether a new process runs, and the notices held while the process changed; `None` when another restart of the worker is under way, which the turns that wait for a switch go to.
    async fn switch_process(
        &self,
        agent: &AgentName,
    ) -> Result<Option<(bool, Vec<String>)>, AgentError> {
        let none = || Ok(Some((false, Vec::new())));
        let Some(run) = lock(&self.inner.running).get(agent).cloned() else {
            return none();
        };
        let Ok(current) = self.current_account().await else {
            return none();
        };
        if run.account == current
            || run.open_turns.load(Ordering::SeqCst) > 0
            || lock(&self.inner.answering).contains(agent)
        {
            return none();
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
            return none();
        };
        if row.state != WorkerState::Idle || self.waits_for_answer(&row).await? {
            return none();
        }
        if row.role == fold_core::Role::Main {
            let config = load_config(&self.inner.paths).await?;
            if self.main_refusal(&config).is_some() {
                return none();
            }
        }
        if !lock(&self.inner.refreshing).insert(agent.clone()) {
            return Ok(None);
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
        // A turn that reached the process before its notices were held keeps the process, with every notice held since.
        if run.open_turns.load(Ordering::SeqCst) > 0 {
            let input = run.input.clone();
            let live: StdinSink =
                Arc::new(move |text: &str| input.send(Input::Turn(text.to_owned())).is_ok());
            self.inner.push.register_stdin(agent, live);
            lock(&self.inner.switching).remove(agent);
            lock(&self.inner.refreshing).remove(agent);
            return Ok(Some((false, std::mem::take(&mut *lock(&held)))));
        }
        let switched = self.switch_row(agent, &run, &row).await;
        lock(&self.inner.switching).remove(agent);
        lock(&self.inner.refreshing).remove(agent);
        if !matches!(switched, Ok(true)) {
            self.inner.push.clear_stdin(agent);
            return switched.map(|_| Some((false, Vec::new())));
        }
        Ok(Some((true, std::mem::take(&mut *lock(&held)))))
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
