use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use fold_core::policy::TagOrigin;
use fold_core::{AgentName, FoldError, Tag, Timestamp, codes};
use fold_store::agents;
use tokio::sync::Notify;

use super::{AgentError, AgentService, SessionContext};

/// The notice a main session gets when a newer `fld claude` takes it over, just before fold closes its connection.
pub const TAKEN_OVER: &str = "fold: a new `fld claude` took over this main session. This session gets no more fold notices, and its fold tools stop working. Continue in the new session.";

/// How many taken-over adapters fold remembers, so their reconnects are refused.
const TAKEN_OVER_KEPT: usize = 64;

/// What identifies one adapter connection of a main session across its reconnects: `fld mcp` repeats the same handshake line from the same process, so the pid and the inbox socket stay the same, and a new `fld claude` has another of both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainConnection {
    /// The kernel's peer pid, else the pid of the handshake line.
    pub pid: u32,
    /// The session's Claude Code inbox from the handshake line.
    pub inbox: Option<PathBuf>,
    /// The handshake's `keep_others`: the connection leaves the live connections of the agent's other sessions open.
    pub keep_others: bool,
    /// The handshake's `reconnect`: the line is a replay of an adapter that was connected before, so it takes nothing over either.
    pub reconnect: bool,
}

impl MainConnection {
    fn takes_over(&self) -> bool {
        !self.keep_others && !self.reconnect
    }
}

/// The live adapter connections of the project's interactive main sessions, by session row id, and the adapters that a newer session took over. Only one Claude Code process works as the main agent unless a handshake asks to keep the others.
#[derive(Debug, Default)]
pub struct MainSessions {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    live: HashMap<i64, Live>,
    /// The (pid, inbox) of each closed connection, newest last.
    taken_over: VecDeque<(u32, Option<PathBuf>)>,
    /// The inbox sockets that a takeover retired, by agent, newest last. Claude Code names its inbox socket after its own process and keeps it across `/clear` and compactions, so the socket identifies a taken-over process when its session-start hook registers again, while the hook's own peer pid is a new process at every run.
    retired: VecDeque<(AgentName, PathBuf)>,
}

#[derive(Debug)]
struct Live {
    agent: AgentName,
    started_at: Timestamp,
    pid: u32,
    inbox: Option<PathBuf>,
    close: Arc<Notify>,
}

impl MainSessions {
    /// Forgets the connection of `session` once it has ended.
    pub fn leave(&self, session: i64) {
        self.lock().live.remove(&session);
    }

    /// How many connections of `agent` are live.
    pub fn count(&self, agent: &AgentName) -> usize {
        self.lock()
            .live
            .values()
            .filter(|live| &live.agent == agent)
            .count()
    }

    /// Whether `socket` is the inbox of a main connection of `agent` that a newer session took over. The inbox of a live connection of `agent` never is: a new `fld claude` whose process got the pid of a taken-over one has the same socket path.
    fn was_inbox_taken_over(&self, agent: &AgentName, socket: &PathBuf) -> bool {
        let state = self.lock();
        if state
            .live
            .values()
            .any(|live| &live.agent == agent && live.inbox.as_ref() == Some(socket))
        {
            return false;
        }
        state
            .retired
            .iter()
            .any(|(old_agent, old_socket)| old_agent == agent && old_socket == socket)
            || state
                .taken_over
                .iter()
                .any(|(_, old_inbox)| old_inbox.as_ref() == Some(socket))
    }

    fn remember_retired(&self, agent: &AgentName, sockets: Vec<PathBuf>) {
        let mut state = self.lock();
        for socket in sockets {
            state.retired.push_back((agent.clone(), socket));
            if state.retired.len() > TAKEN_OVER_KEPT {
                state.retired.pop_front();
            }
        }
    }

    fn was_taken_over(&self, pid: u32, inbox: Option<&PathBuf>) -> bool {
        self.lock()
            .taken_over
            .iter()
            .any(|(old_pid, old_inbox)| *old_pid == pid && old_inbox.as_ref() == inbox)
    }

    /// Counts `session` as live and, when the connection takes over, removes the agent's other live connections and remembers those of another adapter as taken over, in one step, so two handshakes at the same moment never both stay.
    fn enter(
        &self,
        session: i64,
        agent: &AgentName,
        started_at: Timestamp,
        connection: &MainConnection,
    ) -> (Arc<Notify>, Vec<Live>) {
        let close = Arc::new(Notify::new());
        let mut state = self.lock();
        if let Some(inbox) = &connection.inbox {
            state.retired.retain(|(_, socket)| socket != inbox);
        }
        let mut old = Vec::new();
        if connection.takes_over() {
            let ids: Vec<i64> = state
                .live
                .iter()
                .filter(|(id, live)| **id != session && &live.agent == agent)
                .map(|(id, _)| *id)
                .collect();
            for id in ids {
                if let Some(live) = state.live.remove(&id) {
                    // The same adapter that reconnects before fold has ended its earlier session replaces that session; it never takes itself over.
                    if live.pid != connection.pid || live.inbox != connection.inbox {
                        state.taken_over.push_back((live.pid, live.inbox.clone()));
                        if state.taken_over.len() > TAKEN_OVER_KEPT {
                            state.taken_over.pop_front();
                        }
                    }
                    old.push(live);
                }
            }
        }
        state.live.insert(
            session,
            Live {
                agent: agent.clone(),
                started_at,
                pid: connection.pid,
                inbox: connection.inbox.clone(),
                close: Arc::clone(&close),
            },
        );
        (close, old)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn taken_over() -> AgentError {
    FoldError::new(
        codes::MAIN_SESSION_TAKEN_OVER,
        "a new `fld claude` took over this main session",
    )
    .with_hint("continue in the new session, or start this one with `fld claude --keep-others`")
    .into()
}

impl AgentService {
    /// Refuses a push target of `agent` at `socket` with `main_session_taken_over` when `socket` is the inbox of a main session that a newer session took over, so the old Claude Code process gets no notices again after a `/clear` or a compaction.
    pub(super) fn refuse_taken_over_inbox(
        &self,
        agent: &AgentName,
        socket: &PathBuf,
    ) -> Result<(), AgentError> {
        if self.main_sessions.was_inbox_taken_over(agent, socket) {
            return Err(taken_over());
        }
        Ok(())
    }

    /// Refuses the handshake of an adapter whose connection a newer main session took over, with `main_session_taken_over`. `fld mcp` reconnects with the same line when fold closes its connection, and without this refusal it would take the new session over in turn.
    pub fn refuse_taken_over(&self, pid: u32, inbox: Option<&PathBuf>) -> Result<(), AgentError> {
        if self.main_sessions.was_taken_over(pid, inbox) {
            return Err(taken_over());
        }
        Ok(())
    }

    /// Counts the bound session `ctx` as a live main connection when it is an interactive main agent that registered with `tags` from its handshake, and returns the signal that fires when a newer main session takes it over; `None` for every other session. Unless `connection` keeps the others or is a replay, it takes over the agent's other live connections: each old session's inbox sockets registered up to its start, except the new session's own inbox, get [`TAKEN_OVER`] as their last notice and are dropped, and then its signal fires. An old session without an inbox socket gets no text. A connection whose peer is already gone is forgotten the same way, so it never stands in the new session's way.
    pub async fn enter_main_connection(
        &self,
        ctx: &SessionContext,
        tags: &[Tag],
        connection: &MainConnection,
    ) -> Result<Option<Arc<Notify>>, AgentError> {
        let Some(agent) = &ctx.agent else {
            return Ok(None);
        };
        if !self
            .is_interactive_main(ctx, agent, tags, TagOrigin::Handshake)
            .await?
        {
            return Ok(None);
        }
        let started_at = agents::find_session(self.store.read(), ctx.session_id)
            .await?
            .map_or_else(|| self.clock.now(), |session| session.started_at);
        let (close, old) = self
            .main_sessions
            .enter(ctx.session_id, agent, started_at, connection);
        for live in &old {
            let retired = self
                .push
                .retire_registered_by(
                    agent,
                    live.started_at,
                    connection.inbox.as_deref(),
                    TAKEN_OVER,
                )
                .await;
            self.main_sessions.remember_retired(agent, retired);
            live.close.notify_one();
        }
        if !old.is_empty() {
            tracing::info!(agent = %agent, closed = old.len(), "a new main session took over");
        }
        Ok(Some(close))
    }
}
