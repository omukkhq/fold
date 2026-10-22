use std::collections::HashSet;

use fold_core::agents::push::{is_pushed, notice_for};
use fold_core::events::types;
use fold_core::roles::{Role, StartFacts, first_turn};
use fold_core::{
    AgentName, Event, EventActor, FoldError, MAIN_TAG, NewEvent, ProjectConfig, Tag, WorkerName,
    codes,
};
use fold_store::agents::{self, NewWorkerProcess, ProcessEnd, WorkerProcessRow};
use fold_store::{StoreError, messages};
use serde_json::json;

use super::message::message_turn;
use super::stop::stopped_event;
use super::{Supervisor, lock};
use crate::services::agents::AgentError;
use crate::services::agents::workers::{
    code_of, is_paused_refusal, new_session, refuse_while_paused,
};
use crate::services::events::EventBuffer;
use crate::services::projects::load_config;

/// Who may run the main agent now: the interactive sessions of the agent that holds the `main` tag, by session row id, and whether a start of the supervised main is under way. One lock covers both, so an interactive session and a supervised process never both pass.
#[derive(Debug, Default)]
pub(super) struct MainGate {
    interactive: HashSet<i64>,
    starting: bool,
}

/// Holds [`MainGate::starting`] while a launch of the supervised main puts its process in place.
pub(super) struct MainStart {
    supervisor: Supervisor,
}

impl Drop for MainStart {
    fn drop(&mut self) {
        lock(&self.supervisor.inner.main_gate).starting = false;
    }
}

/// The refusal of a supervised main while the project's setting does not let fold run it.
fn not_supervised_main() -> AgentError {
    FoldError::new(
        codes::MAIN_NOT_SUPERVISED,
        "the project's setting does not let fold run the main agent",
    )
    .with_hint("set supervised = true under [main] in .fold/config.toml, or run the main agent in a terminal with `fld claude`")
    .into()
}

/// The refusal of a supervised main while the user's own session is the main agent.
fn interactive_main() -> AgentError {
    FoldError::new(
        codes::MAIN_INTERACTIVE,
        "the main agent runs in an interactive `fld claude` session of this project",
    )
    .with_hint("fold gives it its work there; end that session to let fold run the main agent")
    .into()
}

/// Whether `error` refuses a start of the supervised main for its setting or an interactive session; the work that asked for the start waits.
pub(in crate::services::agents::workers) fn is_main_refusal(error: &AgentError) -> bool {
    matches!(error, AgentError::Fold(error) if error.code() == codes::MAIN_NOT_SUPERVISED || error.code() == codes::MAIN_INTERACTIVE)
}

/// The turn of a supervised main that fold starts for what it would have told it.
fn notice_turn(notices: &[String]) -> String {
    let mut text = String::from("fold started you for work that waits for you:");
    for notice in notices {
        text.push_str("\n- ");
        text.push_str(notice);
    }
    text.push_str("\n\nHandle one item per turn.");
    text
}

impl Supervisor {
    /// Why fold may not run the supervised main now, with the project's `config`: the setting is off, or an interactive session is the main agent.
    pub(in crate::services::agents::workers) fn main_refusal(
        &self,
        config: &ProjectConfig,
    ) -> Option<AgentError> {
        if !config.main.supervised {
            return Some(not_supervised_main());
        }
        if !lock(&self.inner.main_gate).interactive.is_empty() {
            return Some(interactive_main());
        }
        None
    }

    /// [`Supervisor::main_refusal`], and when it allows the start, the hold that keeps an interactive session out until the launch has its process in place.
    pub(super) fn claim_main_start(&self, config: &ProjectConfig) -> Result<MainStart, AgentError> {
        if !config.main.supervised {
            return Err(not_supervised_main());
        }
        let mut gate = lock(&self.inner.main_gate);
        if !gate.interactive.is_empty() {
            return Err(interactive_main());
        }
        gate.starting = true;
        Ok(MainStart {
            supervisor: self.clone(),
        })
    }

    /// Counts the session `session` of `agent`, which asked for the `main` tag or named the supervised main from outside every worker's process tree, as an interactive main agent. While fold runs or starts the supervised main, such a session is refused with `main_supervised`.
    pub fn enter_interactive_main(&self, session: i64, agent: &AgentName) -> Result<(), FoldError> {
        let mut gate = lock(&self.inner.main_gate);
        if gate.starting || self.is_running(agent) {
            return Err(FoldError::new(
                codes::MAIN_SUPERVISED,
                format!("fold runs the main agent {agent} of this project as a supervised worker"),
            )
            .with_details([("agent", json!(agent.as_str()))])
            .with_hint("talk to it in the fold web app; to run it in a terminal, set supervised = false under [main] in .fold/config.toml and stop it"));
        }
        gate.interactive.insert(session);
        Ok(())
    }

    /// Forgets the interactive main session `session` once it ends. When it was the last one, the supervised main may run again, so fold starts it when the setting says so (see [`Supervisor::catch_up_main`]).
    pub fn leave_interactive_main(&self, session: i64) {
        let last = {
            let mut gate = lock(&self.inner.main_gate);
            gate.interactive.remove(&session) && gate.interactive.is_empty()
        };
        if last {
            tokio::spawn(self.clone().catch_up_main());
        }
    }

    /// Forgets every interactive main session, when every session ends.
    pub fn clear_interactive_main(&self) {
        lock(&self.inner.main_gate).interactive.clear();
    }

    /// The row of the supervised main, if fold ever started one.
    async fn main_row(&self) -> Result<Option<WorkerProcessRow>, StoreError> {
        let mut conn = self.inner.store.read().acquire().await?;
        Ok(agents::list_worker_processes(&mut conn)
            .await?
            .into_iter()
            .find(|row| row.role == Role::Main))
    }

    /// Whether a human stopped the supervised main of `row`, so fold starts it for nothing until a human starts it again.
    pub(super) async fn is_held(&self, row: &WorkerProcessRow) -> Result<bool, StoreError> {
        if row.role != Role::Main {
            return Ok(false);
        }
        agents::worker_held(self.inner.store.read(), row.id).await
    }

    /// The agent that is or would be the supervised main: the agent that holds the `main` tag, else `main`.
    async fn main_agent(&self) -> Result<AgentName, AgentError> {
        let read = self.inner.store.read();
        for (holder, tag) in agents::all_tags(read).await? {
            if tag.is_main()
                && let Some(name) = agents::agent_name(read, holder).await?
            {
                return Ok(name);
            }
        }
        Ok(AgentName::parse(MAIN_TAG).map_err(FoldError::from)?)
    }

    /// Starts or resumes the supervised main for an event that sends it a notice, while its process does not run: a request, a ready task of `main`, a review it must act on, or a message when fold never started it yet; a message to a stopped main starts it through the messages it owes. The notice texts are its turn. A paused project, a setting that is off and an interactive main session start nothing.
    pub(super) async fn wake_main(self, event: std::sync::Arc<Event>) {
        // A pause stops the supervised main too, and the resume of a pause starts it with what waits for it.
        if !is_pushed(&event)
            || event.event_type == types::PROJECT_PAUSED
            || event.event_type == types::PROJECT_RESUMED
        {
            return;
        }
        let woken = self.notices_for_main(&event).await;
        let notices = match woken {
            Ok(Some(notices)) if !notices.is_empty() => notices,
            Ok(_) => return,
            Err(error) => {
                tracing::warn!(
                    seq = event.seq,
                    code = code_of(&error),
                    "could not check the work of the supervised main"
                );
                return;
            }
        };
        if let Err(error) = self.ensure_main(Some(notice_turn(&notices))).await {
            tracing::warn!(
                code = code_of(&error),
                "could not start the supervised main"
            );
        }
    }

    /// The notices `event` sends the supervised main, `None` when the setting is off, its process runs or its row waits for the messages it owes. The setting is read first, so a project with it off reads no row for an event.
    async fn notices_for_main(&self, event: &Event) -> Result<Option<Vec<String>>, AgentError> {
        let config = load_config(&self.inner.paths).await?;
        if !config.main.supervised {
            return Ok(None);
        }
        let agent = self.main_agent().await?;
        if self.is_running(&agent) {
            return Ok(None);
        }
        let row = self.main_row().await?;
        if event.event_type == types::MESSAGE_SENT && row.is_some() {
            return Ok(None);
        }
        let mut ctx = crate::push::context(&self.inner.store, event).await?;
        if ctx.main_agents.is_empty() {
            ctx.main_agents.insert(agent.clone());
        }
        Ok(Some(
            notice_for(event, &ctx)
                .into_iter()
                .filter(|notice| notice.agent == agent)
                .map(|notice| notice.text)
                .collect(),
        ))
    }

    /// After `worker.stopped` of the supervised main, such as fold's stop once it answered its messages: work can have reached its process as a notice while it answered, so the stop is followed by [`Supervisor::catch_up_main`]. A human's stop holds the row, and the catch-up starts nothing.
    pub(super) async fn catch_up_after_stop(self, event: std::sync::Arc<Event>) {
        let Some(agent) = event
            .data
            .get("agent")
            .and_then(serde_json::Value::as_str)
            .and_then(|agent| AgentName::parse(agent).ok())
        else {
            return;
        };
        let main = async {
            let mut conn = self.inner.store.read().acquire().await?;
            agents::find_worker_process(&mut conn, &agent).await
        };
        match main.await {
            Ok(Some(row)) if row.role == Role::Main => self.catch_up_main().await,
            Ok(_) => {}
            Err(error) => tracing::warn!(
                code = error.log_code(),
                "could not read the row of a stopped worker"
            ),
        }
    }

    /// After a daemon start, the resume of a pause or fold's stop of the main: when the setting lets fold run the main agent, its process does not run and no human's stop holds it, starts it with the work that waits for it, the first time even when nothing waits, so the humans see it on the Agents screen.
    pub(super) async fn catch_up_main(self) {
        let caught = async {
            let config = load_config(&self.inner.paths).await?;
            if !config.main.supervised {
                return Ok(());
            }
            let turn = match self.main_row().await? {
                Some(row) if self.is_running(&row.agent) => return Ok(()),
                Some(row) if self.is_held(&row).await? => return Ok(()),
                Some(row) => match self.catch_up(&row, &Default::default()).await {
                    Some(turn) => Some(turn),
                    None => return Ok(()),
                },
                // The agent can hold messages from the time an interactive session was the main agent.
                None => {
                    let agent = self.main_agent().await?;
                    let id = agents::find_agent(self.inner.store.read(), &agent)
                        .await?
                        .map(|found| found.id);
                    self.main_waiting(id, &Default::default()).await?
                }
            };
            self.ensure_main(turn).await
        };
        if let Err(error) = caught.await {
            tracing::warn!(
                code = code_of(&error),
                "could not start the supervised main"
            );
        }
    }

    /// Runs the supervised main with `turn`: resumes its stopped or failed row in its own session, or creates its row and starts it in a new session. One start at a time: a turn that comes while one is under way is sent to the started process after its first turn. A start that a pause, the setting or an interactive session refuses starts nothing.
    pub(super) async fn ensure_main(&self, turn: Option<String>) -> Result<(), AgentError> {
        {
            let mut waking = lock(&self.inner.main_waking);
            if let Some(pending) = waking.as_mut() {
                pending.extend(turn);
                return Ok(());
            }
            *waking = Some(Vec::new());
        }
        let started = self.ensure_main_once(turn).await;
        let pending = lock(&self.inner.main_waking).take().unwrap_or_default();
        if !pending.is_empty()
            && let Ok(Some(row)) = self.main_row().await
        {
            self.send_turn(&row.agent, &pending.join("\n\n"));
        }
        match started {
            Err(error) if is_paused_refusal(&error) || is_main_refusal(&error) => Ok(()),
            other => other,
        }
    }

    /// Under the lock of the starts for messages, so a start for the messages the main owes and this one start it once, and the second turn goes to the process the first one started.
    async fn ensure_main_once(&self, turn: Option<String>) -> Result<(), AgentError> {
        let _resuming = self.inner.resuming.lock().await;
        let Some(row) = self.main_row().await? else {
            return self.start_main(turn).await;
        };
        if self.is_running(&row.agent) || row.state.is_running() {
            if let Some(turn) = turn {
                self.send_turn(&row.agent, &turn);
            }
            return Ok(());
        }
        if self.is_held(&row).await? {
            return Ok(());
        }
        // The start for these messages finds the main running and leaves them, so this start tells of them.
        let owed = messages::owed_to_stopped(self.inner.store.read(), Some(&row.agent)).await?;
        // A start for messages alone also names the other work that waits, so the main does not stop after its answer with that work untold.
        let turn = match turn {
            Some(turn) => Some(turn),
            None if !owed.is_empty() => self.main_waiting(None, &Default::default()).await?,
            None => None,
        };
        // Only a start for messages alone stops the main once it has answered, as the message turn says; with other work it stays, as the long-lived main it is.
        let answering = turn.is_none() && !owed.is_empty();
        let parts: Vec<String> = turn
            .into_iter()
            .chain((!owed.is_empty()).then(|| message_turn(&owed)))
            .collect();
        if parts.is_empty() {
            return Ok(());
        }
        if answering {
            lock(&self.inner.answering).insert(row.agent.clone());
        }
        match self.resume(&row.agent, parts.join("\n\n"), &[]).await {
            Ok(watch) => {
                for owed in &owed {
                    self.record_told(&row.agent, owed).await;
                }
                if let Some(watch) = watch {
                    tokio::spawn(self.clone().restart_lost(row.agent.clone(), watch, None));
                }
                Ok(())
            }
            Err(error) => {
                lock(&self.inner.answering).remove(&row.agent);
                Err(error)
            }
        }
    }

    /// Creates the supervised main and starts it in a new session: the agent that holds the `main` tag, else the agent `main` with the tag, gets a worker process of role `main`, in one transaction with `worker.started`.
    async fn start_main(&self, turn: Option<String>) -> Result<(), AgentError> {
        let config = load_config(&self.inner.paths).await?;
        if let Some(refusal) = self.main_refusal(&config) {
            return Err(refusal);
        }
        let model = config.role(Role::Main).model;
        let session = new_session()?;
        let agent = self.main_agent().await?;
        let by = EventActor::fold();
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        let main_tag = Tag::parse(MAIN_TAG).map_err(FoldError::from)?;
        let row = self
            .inner
            .store
            .write(async |tx| {
                refuse_while_paused(tx).await?;
                let agent_id = match agents::find_agent(tx.conn(), &agent).await? {
                    Some(found) => {
                        if agents::find_worker_process(tx.conn(), &agent)
                            .await?
                            .is_some()
                        {
                            return Err(FoldError::new(
                                codes::WORKER_EXISTS,
                                format!("agent {agent} is a worker of another role"),
                            )
                            .with_details([("agent", json!(agent.as_str()))])
                            .with_hint(
                                "give the main tag to no worker; fold makes its own main agent",
                            )
                            .into());
                        }
                        found.id
                    }
                    None => {
                        let id = agents::insert_agent(tx, &agent, now).await?;
                        let main = WorkerName::main(&agent);
                        agents::insert_worker(tx, id, &main, None, now).await?;
                        for (kind, data) in [
                            (types::AGENT_REGISTERED, json!({"agent": agent.as_str()})),
                            (
                                types::WORKER_REGISTERED,
                                json!({"agent": agent.as_str(), "worker": main.as_str()}),
                            ),
                        ] {
                            let event =
                                NewEvent::new(kind, EventActor::worker(&main))?.data(data)?;
                            buffer.append(tx, &event, now).await?;
                        }
                        id
                    }
                };
                agents::add_tag(tx, agent_id, &main_tag, None, now).await?;
                agents::insert_worker_process(
                    tx,
                    &NewWorkerProcess {
                        agent_id,
                        role: Role::Main,
                        area_id: None,
                        model: model.as_deref(),
                        session_id: &session,
                        started_by: by.as_str(),
                        now,
                    },
                )
                .await?;
                let event = NewEvent::new(types::WORKER_STARTED, by.clone())?.data(json!({
                    "agent": agent.as_str(),
                    "role": Role::Main.as_str(),
                    "area": null,
                    "tasks": [],
                    "proposals": [],
                    "model": model,
                }))?;
                buffer.append(tx, &event, now).await?;
                agents::find_worker_process(tx.conn(), &agent)
                    .await?
                    .ok_or(StoreError::Corrupt("worker_processes"))
                    .map_err(AgentError::from)
            })
            .await?;
        self.inner.bus.publish(buffer);
        let first = first_turn(&StartFacts {
            role: Role::Main,
            name: &row.agent,
            area: None,
            tasks: &[],
            proposals: &[],
            batch: None,
            instructions: None,
        });
        let turn = match turn {
            Some(turn) => format!("{first}\n\n{turn}"),
            None => first,
        };
        self.launch(&row, false, Some(turn)).await?;
        Ok(())
    }

    /// Ends the row of a supervised main that the setting or an interactive session keeps from running again, which a daemon stop or a pause left running without a process, so it does not look as if it runs.
    pub(super) async fn end_unallowed_main(
        &self,
        row: &WorkerProcessRow,
    ) -> Result<(), AgentError> {
        let now = self.inner.clock.now();
        let mut buffer = EventBuffer::new();
        self.inner
            .store
            .write(async |tx| {
                let current = agents::find_worker_process(tx.conn(), &row.agent).await?;
                if !current
                    .is_some_and(|current| current.id == row.id && current.state.is_running())
                {
                    return Ok(());
                }
                agents::record_exit(tx, row.id, ProcessEnd::Stopped, None, now).await?;
                agents::close_open_questions(tx, row.id, now).await?;
                buffer.append(tx, &stopped_event(row, None)?, now).await?;
                Ok::<_, AgentError>(())
            })
            .await?;
        self.inner.bus.publish(buffer);
        Ok(())
    }
}
