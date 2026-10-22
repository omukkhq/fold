use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use fold_core::text::shorten;
use fold_core::{AgentName, FoldError, TaskId, Timestamp, codes};
use fold_store::agents::{self, ClaimKind};
use fold_store::messages;

use super::overview::OverviewFilter;
use super::{AgentError, AgentService};
use crate::services::tasks::TaskService;

/// The longest context the session-start hook hands to Claude Code, in characters.
pub const MAX_SESSION_CONTEXT: usize = 1500;
/// How many ready tasks the context names.
pub const CONTEXT_READY_TASKS: usize = 5;
/// How many claims the context lists before it only counts the rest.
const CONTEXT_CLAIMS: usize = 10;
/// The longest task title the context repeats, in characters.
const CONTEXT_TITLE: usize = 100;

/// One claim as the context names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextClaim {
    pub task: TaskId,
    pub kind: ClaimKind,
    /// Whole minutes until the lease runs out; zero once it has.
    pub minutes_left: u64,
}

/// What the session-start context says, read at one time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFacts {
    pub project: String,
    pub agent: AgentName,
    /// False for an agent the project does not know yet: the adapter registers it when it connects, which can be after the hook runs.
    pub registered: bool,
    pub unread: u64,
    pub claims: Vec<ContextClaim>,
    /// The first [`CONTEXT_READY_TASKS`] ready tasks, in ready order.
    pub ready: Vec<(TaskId, String)>,
}

impl AgentService {
    /// The text the session-start hook adds to a new Claude Code session of `agent`: who it is, its unread messages, its claims with the minutes left on each lease at the clock's time, and the top ready tasks. Nothing is written.
    pub async fn session_context(
        &self,
        project: &str,
        agent: &AgentName,
        tasks: &TaskService,
    ) -> Result<String, AgentError> {
        let ready = tasks
            .ready()
            .await?
            .into_iter()
            .take(CONTEXT_READY_TASKS)
            .map(|task| (task.id, task.title.as_str().to_owned()))
            .collect();
        let mut facts = SessionFacts {
            project: project.to_owned(),
            agent: agent.clone(),
            registered: false,
            unread: 0,
            claims: Vec::new(),
            ready,
        };
        let Some(row) = agents::find_agent(self.store.read(), agent).await? else {
            return Ok(render(&facts));
        };
        facts.registered = true;
        facts.unread = messages::unread_count(self.store.read(), row.id).await?;
        let lease_minutes = self.lease.minutes().await?;
        let now = self.clock.now();
        let filter = OverviewFilter {
            agent: Some(agent.clone()),
            events: 0,
        };
        for overview in self.overview(&filter).await? {
            for worker in overview.workers {
                for claim in worker.claims {
                    facts.claims.push(ContextClaim {
                        task: claim.task,
                        kind: claim.kind,
                        minutes_left: minutes_left(claim.renewed_at, now, lease_minutes),
                    });
                }
            }
        }
        facts.claims.sort_by_key(|claim| claim.task);
        Ok(render(&facts))
    }
}

/// Rounded down, so a lease is never shown with more time than it has.
fn minutes_left(renewed_at: Timestamp, now: Timestamp, lease_minutes: u64) -> u64 {
    let lease = Duration::from_secs(lease_minutes.saturating_mul(60));
    let elapsed = now.duration_since(renewed_at).unwrap_or_default();
    lease.saturating_sub(elapsed).as_secs() / 60
}

/// The context text, at most [`MAX_SESSION_CONTEXT`] characters.
pub fn render(facts: &SessionFacts) -> String {
    let mut text = String::new();
    let agent = &facts.agent;
    let _ = writeln!(text, "fold project {}.", facts.project);
    if facts.registered {
        let _ = writeln!(text, "You are agent {agent}.");
        let _ = writeln!(
            text,
            "Unread messages: {}.{}",
            facts.unread,
            if facts.unread > 0 {
                " Call get_inbox."
            } else {
                ""
            }
        );
    } else {
        let _ = writeln!(
            text,
            "You are agent {agent}, not registered in this project yet; the fold MCP server registers it when it connects."
        );
    }
    if facts.claims.is_empty() {
        text.push_str("Claims: none.\n");
    } else {
        text.push_str("Claims:\n");
        for claim in facts.claims.iter().take(CONTEXT_CLAIMS) {
            let lease = match claim.minutes_left {
                0 => "lease expired".to_owned(),
                1 => "1 minute left".to_owned(),
                minutes => format!("{minutes} minutes left"),
            };
            let _ = writeln!(text, "- {} {}: {lease}", claim.task, claim.kind.as_str());
        }
        let more = facts.claims.len().saturating_sub(CONTEXT_CLAIMS);
        if more > 0 {
            let _ = writeln!(text, "- and {more} more");
        }
    }
    if facts.ready.is_empty() {
        text.push_str("Ready tasks: none.\n");
    } else {
        text.push_str("Ready tasks:\n");
        for (task, title) in &facts.ready {
            let _ = writeln!(text, "- {task} {}", shorten(title, CONTEXT_TITLE));
        }
    }
    shorten(text.trim_end(), MAX_SESSION_CONTEXT)
}

impl AgentService {
    /// The session-start hook: stores the session's inbox socket when the hook sent one, then builds [`AgentService::session_context`]. For an agent the project does not know yet the socket only waits in memory, because the adapter registers the agent only when it connects, and that registration takes it.
    pub async fn session_start(
        &self,
        project: &str,
        agent: &AgentName,
        tasks: &TaskService,
        push: Option<(PathBuf, &str)>,
    ) -> Result<String, AgentError> {
        if let Some((socket, token)) = push {
            match self.register_push_target(agent, socket, token).await {
                Ok(()) => {}
                Err(AgentError::Fold(error)) if error.code() == codes::AGENT_NOT_REGISTERED => {}
                Err(error) => return Err(error),
            }
        }
        self.session_context(project, agent, tasks).await
    }

    /// The unacknowledged messages of `agent`'s worker, for the stop hook.
    pub async fn unread_of(&self, agent: &AgentName) -> Result<u64, AgentError> {
        let row = agents::find_agent(self.store.read(), agent)
            .await?
            .ok_or_else(|| {
                FoldError::new(
                    codes::AGENT_NOT_REGISTERED,
                    format!("agent {agent} is not registered"),
                )
                .with_hint("call register_agent")
            })?;
        Ok(messages::unread_count(self.store.read(), row.id).await?)
    }
}
