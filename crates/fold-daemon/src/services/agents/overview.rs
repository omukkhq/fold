use std::collections::HashMap;

use fold_core::claims::is_stale;
use fold_core::{
    AgentName, CheckpointId, Event, FoldError, Presence, ProposalId, RevisionId, Tag, TaskId,
    Timestamp, WorkerName, WorkspaceId, codes, presence_of,
};
use fold_store::agents::{self, ClaimKind};
use fold_store::workspaces::WorkspaceState;
use fold_store::{StoreError, events};
use serde_json::json;

use super::presence::facts;
use super::{AgentError, AgentService};

/// The most events one agent of the overview carries.
pub const MAX_OVERVIEW_EVENTS: usize = 50;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverviewFilter {
    /// Only this agent; every agent when absent.
    pub agent: Option<AgentName>,
    /// How many of each agent's newest events to include, at most [`MAX_OVERVIEW_EVENTS`].
    pub events: usize,
}

/// What one agent and its workers are doing at the time of the read.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentOverview {
    pub name: AgentName,
    pub tags: Vec<Tag>,
    pub presence: Presence,
    /// The newest call of any of the agent's sessions.
    pub last_call_at: Option<Timestamp>,
    /// How many of the agent's sessions are connected.
    pub open_sessions: u64,
    /// The main worker first, then the children by name.
    pub workers: Vec<WorkerOverview>,
    /// The newest events the agent's workers performed, newest first; empty unless the filter asks for events.
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerOverview {
    pub name: WorkerName,
    pub parent: Option<WorkerName>,
    pub last_call_at: Option<Timestamp>,
    /// The worker's last tool, followed by its task id when the call named one.
    pub activity: Option<String>,
    pub claims: Vec<ClaimSummary>,
    pub workspaces: Vec<WorkspaceSummary>,
    pub proposals: Vec<ProposalSummary>,
}

/// A claim the worker holds. `stale` is decided by the claim lease alone; a worker that holds a claim but makes no edits is stalled, which is a separate flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimSummary {
    pub task: TaskId,
    pub kind: ClaimKind,
    pub acquired_at: Timestamp,
    pub renewed_at: Timestamp,
    pub stale: bool,
}

/// A task workspace in state `active` or `missing` that the worker owns, read from the workspace row's cached counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceSummary {
    pub ws: WorkspaceId,
    pub task: TaskId,
    pub state: WorkspaceState,
    pub base: RevisionId,
    pub changed_files: u32,
    pub last_activity_at: Option<Timestamp>,
    pub latest_checkpoint: Option<CheckpointId>,
}

/// An open proposal the worker wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalSummary {
    pub id: ProposalId,
    pub task: TaskId,
    pub title: String,
    pub conflicted: bool,
}

impl AgentService {
    /// Every agent in registration order, or the one agent of the filter, with presence calculated at the clock's time. All reads share one read transaction, so they see one state, and nothing is written.
    pub async fn overview(
        &self,
        filter: &OverviewFilter,
    ) -> Result<Vec<AgentOverview>, AgentError> {
        let lease_minutes = self.lease_minutes().await?;
        let now = self.clock.now();
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        let mut rows = agents::list_agents(&mut *tx).await?;
        if let Some(name) = &filter.agent {
            rows.retain(|row| &row.name == name);
            if rows.is_empty() {
                return Err(FoldError::new(
                    codes::AGENT_NOT_FOUND,
                    format!("agent {name} does not exist"),
                )
                .with_details([("agent", json!(name.as_str()))])
                .with_hint("call list_agents to see the agents")
                .into());
            }
        }
        let tags = agents::all_tags(&mut *tx).await?;
        let mut workers = agents::all_workers(&mut *tx).await?;
        let mut sessions = agents::sessions_by_agent(&mut tx).await?;
        let claims = agents::held_claims(&mut *tx).await?;
        let workspaces = agents::owned_live_workspaces(&mut *tx).await?;
        let proposals = agents::open_proposals(&mut *tx).await?;
        let limit = filter.events.min(MAX_OVERVIEW_EVENTS);
        let mut events_of = HashMap::new();
        if limit > 0 {
            for row in &rows {
                let events = events::newest_by_agent(&mut *tx, &row.name, limit).await?;
                events_of.insert(row.id, events);
            }
        }
        tx.commit().await.map_err(StoreError::from)?;

        let mut tags_of: HashMap<i64, Vec<Tag>> = HashMap::new();
        for (agent_id, tag) in tags {
            tags_of.entry(agent_id).or_default().push(tag);
        }
        let mut claims_of: HashMap<i64, Vec<ClaimSummary>> = HashMap::new();
        for claim in claims {
            claims_of
                .entry(claim.worker_id)
                .or_default()
                .push(ClaimSummary {
                    task: claim.task,
                    kind: claim.kind,
                    acquired_at: claim.acquired_at,
                    renewed_at: claim.renewed_at,
                    stale: is_stale(claim.renewed_at, now, lease_minutes),
                });
        }
        let mut workspaces_of: HashMap<i64, Vec<WorkspaceSummary>> = HashMap::new();
        for workspace in workspaces {
            workspaces_of
                .entry(workspace.worker_id)
                .or_default()
                .push(WorkspaceSummary {
                    ws: workspace.id,
                    task: workspace.task,
                    state: workspace.state,
                    base: workspace.base,
                    changed_files: workspace.changed_files,
                    last_activity_at: workspace.last_activity_at,
                    latest_checkpoint: workspace.latest_checkpoint,
                });
        }
        let mut proposals_of: HashMap<i64, Vec<ProposalSummary>> = HashMap::new();
        for proposal in proposals {
            proposals_of
                .entry(proposal.author_worker_id)
                .or_default()
                .push(ProposalSummary {
                    id: proposal.id,
                    task: proposal.task,
                    title: proposal.title,
                    conflicted: proposal.conflicted,
                });
        }
        workers.sort_by(|a, b| (!a.name.is_main(), &a.name).cmp(&(!b.name.is_main(), &b.name)));
        let mut workers_of: HashMap<i64, Vec<WorkerOverview>> = HashMap::new();
        for worker in workers {
            workers_of
                .entry(worker.agent_id)
                .or_default()
                .push(WorkerOverview {
                    parent: worker.name.parent(),
                    last_call_at: worker.last_call_at,
                    activity: worker.activity,
                    claims: claims_of.remove(&worker.id).unwrap_or_default(),
                    workspaces: workspaces_of.remove(&worker.id).unwrap_or_default(),
                    proposals: proposals_of.remove(&worker.id).unwrap_or_default(),
                    name: worker.name,
                });
        }

        Ok(rows
            .into_iter()
            .map(|row| {
                let sessions = sessions.remove(&row.id).unwrap_or_default();
                let facts = facts(&sessions.open);
                AgentOverview {
                    presence: presence_of(now, &facts),
                    tags: tags_of.remove(&row.id).unwrap_or_default(),
                    last_call_at: sessions.newest_call,
                    open_sessions: u64::try_from(facts.len()).unwrap_or(u64::MAX),
                    workers: workers_of.remove(&row.id).unwrap_or_default(),
                    events: events_of.remove(&row.id).unwrap_or_default(),
                    name: row.name,
                }
            })
            .collect())
    }
}
