use axum::Json;
use fold_core::{Event, Presence};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::Project;
use super::error::{ApiError, ApiQuery, ErrorEnvelope};
use super::events::DurableEvent;
use super::tasks::AttentionKind;
use crate::daemon::AppState;
use crate::services::agents::{
    AgentAttention, AgentOverview, ClaimSummary, OverviewFilter, ProposalSummary, SessionSummary,
    WorkerOverview, WorkspaceSummary,
};

pub mod questions;
pub mod workers;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_agents))
        .merge(workers::routes())
        .merge(questions::routes())
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AgentsQuery {
    /// How many of each agent's newest events to include; values above 50 give 50, and none are included when it is left out.
    pub events: Option<u32>,
}

/// Every agent of the project in registration order.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentList {
    pub agents: Vec<AgentEntry>,
}

/// One agent with its presence, calculated by the daemon at the time of the request.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentEntry {
    pub name: String,
    /// Sorted.
    pub tags: Vec<String>,
    #[schema(value_type = PresenceState)]
    pub presence: Presence,
    /// UTC milliseconds of the newest call of any of the agent's sessions; null before the first.
    #[schema(value_type = Option<i64>, required = true)]
    pub last_call_at: Option<fold_core::Timestamp>,
    /// How many of the agent's sessions are connected.
    pub sessions: u64,
    /// The connected sessions, oldest first.
    pub open_sessions: Vec<AgentSession>,
    /// The `stalled` and `stale_claim` items of the tasks the agent's workers hold claims on. A `stalled` item shows only for an implementation claim, and a `stale_claim` item only for a claim of this agent that the lease sweeper marked.
    pub attention: Vec<AgentAttentionItem>,
    /// The main worker first, then the children by name.
    pub workers: Vec<WorkerEntry>,
    /// The newest events the agent's workers performed, newest first; empty unless `events` is asked for.
    #[schema(value_type = Vec<DurableEvent>)]
    pub events: Vec<Event>,
    /// The Claude Code process that fold supervises for the agent; null for an agent fold does not run, such as the main agent. A supervised agent runs Claude Code only during a turn, so its presence is offline between turns while this process is idle.
    #[schema(required = true)]
    pub process: Option<AgentProcess>,
}

/// The state of an agent's supervised process.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentProcess {
    pub state: workers::WorkerProcessState,
    /// Why the process failed, such as `exited` or `mcp_unavailable` (its fold MCP server did not connect); null unless `state` is `failed`, and for a process that failed before fold stored the reason.
    #[schema(required = true, example = "mcp_unavailable")]
    pub failed_reason: Option<String>,
    /// Claude Code's error text of the process's latest failed turn or failed exit; null once a turn ends without an error.
    #[schema(required = true, example = "Invalid API key · Please run /login")]
    pub last_error: Option<String>,
    /// The process's turns failed often enough in a row without reaching the model that it cannot work until a human fixes Claude Code; the daemon decides it, and it is false for a stopped process.
    pub model_unreachable: bool,
}

/// A connected session. A session has no public id; one adapter process is one session, so its pid names it.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentSession {
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub started_at: fold_core::Timestamp,
    /// The adapter process.
    pub pid: i64,
    /// The canonical working directory the adapter sent when it connected; null for a session that sent none.
    #[schema(required = true, example = "/home/tanim/ledger")]
    pub cwd: Option<String>,
}

/// One attention item of a task the agent holds a claim on; `kind` is `stalled` or `stale_claim`.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentAttentionItem {
    pub kind: AttentionKind,
    #[schema(example = "T-12")]
    pub task: String,
}

impl From<SessionSummary> for AgentSession {
    fn from(session: SessionSummary) -> Self {
        Self {
            started_at: session.started_at,
            pid: session.pid,
            cwd: session.cwd,
        }
    }
}

impl From<AgentAttention> for AgentAttentionItem {
    fn from(item: AgentAttention) -> Self {
        Self {
            kind: item.kind.into(),
            task: item.task.to_string(),
        }
    }
}

/// `active`: a connected session called in the last two minutes. `idle`: a session is connected and none is active. `offline`: no session is connected.
#[derive(ToSchema)]
#[schema(as = PresenceState)]
pub enum PresenceState {
    #[schema(rename = "active")]
    Active,
    #[schema(rename = "idle")]
    Idle,
    #[schema(rename = "offline")]
    Offline,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkerEntry {
    /// `agent` or `agent/worker`.
    pub name: String,
    /// Null for the main worker.
    #[schema(required = true)]
    pub parent: Option<String>,
    /// UTC milliseconds; null before the first call.
    #[schema(value_type = Option<i64>, required = true)]
    pub last_call_at: Option<fold_core::Timestamp>,
    /// The last tool the worker called, followed by its task id when the call named one, such as `claim_task T-12`.
    #[schema(required = true)]
    pub activity: Option<String>,
    pub claims: Vec<HeldClaim>,
    /// Workspaces in state `active` or `missing`.
    pub workspaces: Vec<WorkerWorkspace>,
    /// Open proposals the worker wrote.
    pub proposals: Vec<AuthoredProposal>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HeldClaim {
    /// For a review claim, the task of the reviewed proposal.
    #[schema(example = "T-12")]
    pub task: String,
    #[schema(value_type = HeldClaimKind)]
    pub kind: &'static str,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub acquired_at: fold_core::Timestamp,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub renewed_at: fold_core::Timestamp,
    /// True once the lease has run out without a renewal.
    pub stale: bool,
}

#[derive(ToSchema)]
#[schema(as = HeldClaimKind)]
pub enum HeldClaimKind {
    #[schema(rename = "implementation")]
    Implementation,
    #[schema(rename = "review")]
    Review,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WorkerWorkspace {
    #[schema(example = "ws-9")]
    pub ws: String,
    #[schema(example = "T-12")]
    pub task: String,
    #[schema(value_type = WorkerWorkspaceState)]
    pub state: &'static str,
    /// The revision the workspace is based on.
    #[schema(example = "r17")]
    pub base: String,
    /// The workspace row's cached count.
    pub changed_files: u32,
    /// UTC milliseconds.
    #[schema(value_type = Option<i64>, required = true)]
    pub last_activity_at: Option<fold_core::Timestamp>,
    #[schema(required = true, example = "ck-88")]
    pub latest_checkpoint: Option<String>,
}

/// A missing workspace's directory is gone from disk, but it still holds the task in progress.
#[derive(ToSchema)]
#[schema(as = WorkerWorkspaceState)]
pub enum WorkerWorkspaceState {
    #[schema(rename = "active")]
    Active,
    #[schema(rename = "missing")]
    Missing,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthoredProposal {
    #[schema(example = "P-3")]
    pub id: String,
    #[schema(example = "T-12")]
    pub task: String,
    pub title: String,
    pub conflicted: bool,
}

impl From<&ClaimSummary> for HeldClaim {
    fn from(claim: &ClaimSummary) -> Self {
        Self {
            task: claim.task.to_string(),
            kind: claim.kind.as_str(),
            acquired_at: claim.acquired_at,
            renewed_at: claim.renewed_at,
            stale: claim.stale,
        }
    }
}

impl From<&WorkspaceSummary> for WorkerWorkspace {
    fn from(workspace: &WorkspaceSummary) -> Self {
        Self {
            ws: workspace.ws.to_string(),
            task: workspace.task.to_string(),
            state: workspace.state.as_str(),
            base: workspace.base.to_string(),
            changed_files: workspace.changed_files,
            last_activity_at: workspace.last_activity_at,
            latest_checkpoint: workspace.latest_checkpoint.map(|id| id.to_string()),
        }
    }
}

impl From<&ProposalSummary> for AuthoredProposal {
    fn from(proposal: &ProposalSummary) -> Self {
        Self {
            id: proposal.id.to_string(),
            task: proposal.task.to_string(),
            title: proposal.title.clone(),
            conflicted: proposal.conflicted,
        }
    }
}

impl From<WorkerOverview> for WorkerEntry {
    fn from(worker: WorkerOverview) -> Self {
        Self {
            name: worker.name.to_string(),
            parent: worker.parent.map(|parent| parent.to_string()),
            last_call_at: worker.last_call_at,
            activity: worker.activity,
            claims: worker.claims.iter().map(HeldClaim::from).collect(),
            workspaces: worker
                .workspaces
                .iter()
                .map(WorkerWorkspace::from)
                .collect(),
            proposals: worker
                .proposals
                .iter()
                .map(AuthoredProposal::from)
                .collect(),
        }
    }
}

impl From<AgentOverview> for AgentEntry {
    fn from(agent: AgentOverview) -> Self {
        Self {
            name: agent.name.to_string(),
            tags: agent.tags.iter().map(ToString::to_string).collect(),
            presence: agent.presence,
            last_call_at: agent.last_call_at,
            sessions: agent.open_session_count,
            open_sessions: agent
                .open_sessions
                .into_iter()
                .map(AgentSession::from)
                .collect(),
            attention: agent
                .attention
                .into_iter()
                .map(AgentAttentionItem::from)
                .collect(),
            workers: agent.workers.into_iter().map(WorkerEntry::from).collect(),
            events: agent.events,
            process: agent.process.map(|process| AgentProcess {
                state: process.state.into(),
                failed_reason: process.failed_reason,
                last_error: process.last_error,
                model_unreachable: process.model_unreachable,
            }),
        }
    }
}

/// Every agent with its presence, workers, claims, live workspaces and open proposals. It reads only; presence is calculated at the time of the request.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/agents",
    operation_id = "list_agents",
    params(
        ("slug" = String, Path, description = "The project slug"),
        AgentsQuery,
    ),
    responses(
        (status = 200, description = "Every agent in registration order", body = AgentList),
        (status = 400, description = "`invalid_request`: `events` is not a whole number", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`invalid_config`: the project's config.toml has a bad lease value", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_agents(
    project: Project,
    ApiQuery(query): ApiQuery<AgentsQuery>,
) -> Result<Json<AgentList>, ApiError> {
    let filter = OverviewFilter {
        agent: None,
        events: query
            .events
            .map_or(0, |events| usize::try_from(events).unwrap_or(usize::MAX)),
    };
    let agents = project.0.agents().overview(&filter).await?;
    Ok(Json(AgentList {
        agents: agents.into_iter().map(AgentEntry::from).collect(),
    }))
}
