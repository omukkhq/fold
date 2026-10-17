use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use fold_core::roles::{
    AreaProfile, AreaProfileChange, AreaProfileRequest, WorkerLimits, WorkerLimitsChange,
};
use fold_core::{AgentName, AreaName, FoldError, LabelName, ProjectConfig, Role, Timestamp};
use fold_store::agents::{AreaRow, WorkerProcessRow};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::super::Project;
use super::super::actor::Human;
use super::super::error::{ApiError, ApiJson, ErrorEnvelope, fold};
use crate::daemon::AppState;
use crate::services::agents::{PauseState, StartWorker};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_workers, start_worker))
        .routes(routes!(stop_worker))
        .routes(routes!(define_area))
        .routes(routes!(update_area))
        .routes(routes!(set_worker_limits))
        .routes(routes!(pause_project))
        .routes(routes!(resume_project))
}

/// The areas and every worker fold runs or ran.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkerList {
    /// In name order.
    pub areas: Vec<AreaEntry>,
    /// In start order.
    pub workers: Vec<WorkerEntry>,
    /// Each role with the cache time a new worker of the role gets; empty when fold cannot read `.fold/config.toml`.
    pub roles: Vec<RoleEntry>,
    /// The current worker limits; null when fold cannot read `.fold/config.toml`.
    #[schema(required = true)]
    pub limits: Option<LimitsEntry>,
    pub pause: PauseEntry,
}

/// Whether the project's agents are paused. While paused, fold runs no worker process; a worker that was working, idle or starting keeps that state and runs again in its own session at the resume.
#[derive(Debug, Serialize, ToSchema)]
pub struct PauseEntry {
    /// UTC milliseconds of the pause; null while the agents are not paused.
    #[schema(value_type = Option<i64>, required = true)]
    pub paused_since: Option<Timestamp>,
    /// Who paused the agents, such as `human:ana` or the main agent's name; null while they are not paused.
    #[schema(required = true)]
    pub paused_by: Option<String>,
}

impl From<PauseState> for PauseEntry {
    fn from(state: PauseState) -> Self {
        Self {
            paused_since: state.paused_since,
            paused_by: state.paused_by,
        }
    }
}

/// The `[workers]` limits: how many short-lived workers run at once.
#[derive(Debug, Serialize, ToSchema)]
pub struct LimitsEntry {
    /// The most task workers that run at once.
    #[schema(example = 4)]
    pub task_worker: u64,
    /// The most batch reviewers that run at once.
    #[schema(example = 3)]
    pub batch_reviewer: u64,
    /// The most task workers that run at once in one area; null when an area has no limit of its own.
    #[schema(required = true, example = 2)]
    pub max_per_area: Option<u64>,
}

impl From<WorkerLimits> for LimitsEntry {
    fn from(limits: WorkerLimits) -> Self {
        Self {
            task_worker: limits.task_worker,
            batch_reviewer: limits.batch_reviewer,
            max_per_area: limits.max_per_area,
        }
    }
}

/// A change of the worker limits; a limit left out stays as it is.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetWorkerLimitsBody {
    /// The most task workers that run at once, a whole number with no upper bound; 0 lets none start.
    #[serde(default)]
    pub task_worker: Option<u64>,
    /// The most batch reviewers that run at once, a whole number with no upper bound; 0 lets none start.
    #[serde(default)]
    pub batch_reviewer: Option<u64>,
    /// The most task workers that run at once in one area, a whole number with no upper bound; 0 lets none start.
    #[serde(default)]
    pub max_per_area: Option<u64>,
    /// True removes the limit of one area.
    #[serde(default)]
    pub clear_max_per_area: bool,
}

/// A worker role's settings from `.fold/config.toml`.
#[derive(Debug, Serialize, ToSchema)]
pub struct RoleEntry {
    pub role: WorkerRole,
    /// The prompt cache time of the role's workers, `1h` or `5m`.
    #[schema(example = "1h")]
    pub cache_ttl: String,
    /// The model of the role's workers from `[roles.<role>] model`; null when the role sets none and Claude Code picks.
    #[schema(required = true, example = "sonnet")]
    pub model: Option<String>,
}

/// A part of the project that one area lead owns through one task label.
#[derive(Debug, Serialize, ToSchema)]
pub struct AreaEntry {
    #[schema(example = "web")]
    pub name: String,
    #[schema(example = "frontend")]
    pub label: String,
    /// The area lead's agent name; null until the first lead starts.
    #[schema(required = true, example = "web-lead")]
    pub lead: Option<String>,
    /// The instructions every worker of the area starts with; null for none.
    #[schema(required = true, example = "Draw the screens in Pencil.")]
    pub instructions: Option<String>,
    /// The model of the area's lead and task workers; null when the role's model applies.
    #[schema(required = true, example = "opus")]
    pub model: Option<String>,
    /// The MCP servers the area's lead and task workers load; null when the role's servers apply.
    #[schema(required = true, example = json!(["fold", "pencil"]))]
    pub mcp: Option<Vec<String>>,
}

/// The kinds of worker fold runs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRole {
    AreaLead,
    ReviewLead,
    TaskWorker,
    BatchReviewer,
}

impl From<Role> for WorkerRole {
    fn from(role: Role) -> Self {
        match role {
            Role::AreaLead => Self::AreaLead,
            Role::ReviewLead => Self::ReviewLead,
            Role::TaskWorker => Self::TaskWorker,
            Role::BatchReviewer => Self::BatchReviewer,
        }
    }
}

impl From<WorkerRole> for Role {
    fn from(role: WorkerRole) -> Self {
        match role {
            WorkerRole::AreaLead => Self::AreaLead,
            WorkerRole::ReviewLead => Self::ReviewLead,
            WorkerRole::TaskWorker => Self::TaskWorker,
            WorkerRole::BatchReviewer => Self::BatchReviewer,
        }
    }
}

/// `starting` until Claude Code reports its session, `working` during a turn, `idle` between turns, `stopping` while fold ends the process, then `stopped` or `failed`.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkerProcessState {
    Starting,
    Working,
    Idle,
    Stopping,
    Stopped,
    Failed,
}

impl From<fold_core::WorkerState> for WorkerProcessState {
    fn from(state: fold_core::WorkerState) -> Self {
        use fold_core::WorkerState as S;
        match state {
            S::Starting => Self::Starting,
            S::Working => Self::Working,
            S::Idle => Self::Idle,
            S::Stopping => Self::Stopping,
            S::Stopped => Self::Stopped,
            S::Failed => Self::Failed,
        }
    }
}

/// One worker agent and its supervised Claude Code process. The totals add up over every process of the worker.
///
/// The agents route has its own `WorkerEntry`, so the schema takes another name: two schemas of one name leave only one in the OpenAPI document.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = WorkerProcessEntry)]
pub struct WorkerEntry {
    /// The worker's agent name.
    #[schema(example = "web-worker")]
    pub name: String,
    pub role: WorkerRole,
    /// Null for the review lead and batch reviewers.
    #[schema(required = true, example = "web")]
    pub area: Option<String>,
    pub state: WorkerProcessState,
    /// The model Claude Code reported, or the one fold asked for; null for Claude Code's default before the first turn.
    #[schema(required = true)]
    pub model: Option<String>,
    /// The tasks a task worker is bound to.
    pub tasks: Vec<String>,
    /// The proposals a batch reviewer is bound to.
    pub proposals: Vec<String>,
    /// The actor that started the worker, such as `human:tanim` or `main`.
    pub started_by: String,
    /// UTC milliseconds of the newest process start.
    #[schema(value_type = i64)]
    pub started_at: fold_core::Timestamp,
    /// UTC milliseconds; null while the process runs.
    #[schema(value_type = Option<i64>, required = true)]
    pub stopped_at: Option<fold_core::Timestamp>,
    /// Finished turns.
    pub turns: u64,
    /// The cost Claude Code reported, in millionths of a US dollar.
    pub cost_micro_usd: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// The tokens of the newest request: the size of the worker's context.
    pub context_tokens: u64,
    /// The exit code of the last process; a signal counts as 128 plus its number.
    #[schema(required = true)]
    pub exit_code: Option<i64>,
    /// The prompt cache time of the worker's role from `.fold/config.toml`, `1h` or `5m`; null when fold cannot read the file.
    #[schema(required = true, example = "5m")]
    pub cache_ttl: Option<String>,
}

impl WorkerEntry {
    fn new(row: WorkerProcessRow, config: Option<&ProjectConfig>) -> Self {
        let cache_ttl = config.map(|config| config.role(row.role).cache_ttl.as_str().to_owned());
        Self {
            name: row.agent.to_string(),
            role: row.role.into(),
            area: row.area.map(|area| area.to_string()),
            state: row.state.into(),
            model: row.model,
            tasks: row.tasks.iter().map(ToString::to_string).collect(),
            proposals: row.proposals.iter().map(ToString::to_string).collect(),
            started_by: row.started_by,
            started_at: row.started_at,
            stopped_at: row.stopped_at,
            turns: row.turns,
            cost_micro_usd: row.cost_micro_usd,
            cache_read_tokens: row.cache_read_tokens,
            cache_write_tokens: row.cache_write_tokens,
            context_tokens: row.context_tokens,
            exit_code: row.exit_code,
            cache_ttl,
        }
    }
}

impl From<AreaRow> for AreaEntry {
    fn from(area: AreaRow) -> Self {
        Self {
            name: area.name.to_string(),
            label: area.label.to_string(),
            lead: area.lead.map(|lead| lead.to_string()),
            instructions: area.profile.instructions,
            model: area.profile.model,
            mcp: area.profile.mcp,
        }
    }
}

/// A human starts a lead: `area_lead` with its area, or `review_lead`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct StartWorkerBody {
    pub role: WorkerRole,
    /// Leave it out for a name from the role and area, such as `web-lead`.
    #[serde(default)]
    pub name: Option<String>,
    /// The area of an area lead.
    #[serde(default)]
    pub area: Option<String>,
    /// The Claude Code model; leave it out for the role's model from `.fold/config.toml`.
    #[serde(default)]
    pub model: Option<String>,
    /// Added to the worker's first turn.
    #[serde(default)]
    pub instructions: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DefineAreaBody {
    #[schema(example = "web")]
    pub name: String,
    /// The task label whose tasks the area owns.
    #[schema(example = "frontend")]
    pub label: String,
    /// What every worker of the area must know about its kind of work, at most 8000 characters.
    #[serde(default)]
    pub instructions: Option<String>,
    /// The model of the area's lead and task workers; leave it out for the role's model.
    #[serde(default)]
    pub model: Option<String>,
    /// The MCP servers of the area's lead and task workers, by name; leave it out for the role's servers.
    #[serde(default)]
    pub mcp: Option<Vec<String>>,
}

/// A change of an area's profile; a field left out and not cleared stays as it is.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateAreaBody {
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub mcp: Option<Vec<String>>,
    /// True removes the instructions.
    #[serde(default)]
    pub clear_instructions: bool,
    /// True gives the area's workers the role's model again.
    #[serde(default)]
    pub clear_model: bool,
    /// True gives the area's workers the role's MCP servers again.
    #[serde(default)]
    pub clear_mcp: bool,
}

/// The areas and every worker of the project.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/workers",
    operation_id = "list_workers",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The areas and the workers", body = WorkerList),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_workers(project: Project) -> Result<Json<WorkerList>, ApiError> {
    let agents = project.0.agents();
    let areas = agents.list_areas().await?;
    let workers = agents.list_workers().await?;
    let config = agents.worker_config().await;
    let roles = config.as_ref().map_or_else(Vec::new, |config| {
        Role::ALL
            .into_iter()
            .map(|role| {
                let settings = config.role(role);
                RoleEntry {
                    role: role.into(),
                    cache_ttl: settings.cache_ttl.as_str().to_owned(),
                    model: settings.model,
                }
            })
            .collect()
    });
    Ok(Json(WorkerList {
        areas: areas.into_iter().map(AreaEntry::from).collect(),
        workers: workers
            .into_iter()
            .map(|row| WorkerEntry::new(row, config.as_ref()))
            .collect(),
        roles,
        limits: config.as_ref().map(|config| config.workers.limits().into()),
        pause: agents.pause_state().await?.into(),
    }))
}

/// A named human starts an area lead or the review lead, which fold then runs and supervises.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/workers",
    operation_id = "start_worker",
    params(("slug" = String, Path, description = "The project slug")),
    request_body = StartWorkerBody,
    responses(
        (status = 201, description = "The started worker", body = WorkerEntry),
        (status = 400, description = "`invalid_request` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 403, description = "`worker_start_denied`: a human starts only leads", body = ErrorEnvelope),
        (status = 404, description = "`area_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`area_has_lead`, `worker_exists`, `worker_limit_reached`, `project_paused` or `invalid_config`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn start_worker(
    human: Human,
    project: Project,
    ApiJson(body): ApiJson<StartWorkerBody>,
) -> Result<(StatusCode, Json<WorkerEntry>), ApiError> {
    let request = StartWorker {
        role: Some(body.role.into()),
        name: body
            .name
            .as_deref()
            .map(AgentName::parse)
            .transpose()
            .map_err(FoldError::from)?,
        area: body
            .area
            .as_deref()
            .map(AreaName::parse)
            .transpose()
            .map_err(FoldError::from)?,
        model: body.model,
        instructions: body.instructions,
        ..StartWorker::default()
    };
    let agents = project.0.agents();
    let worker = agents.start_worker(&human.actor(), request).await?;
    let config = agents.worker_config().await;
    Ok((
        StatusCode::CREATED,
        Json(WorkerEntry::new(worker, config.as_ref())),
    ))
}

/// A named human stops a worker. Its turn is interrupted and its process ends; it keeps its session.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/workers/{name}/stop",
    operation_id = "stop_worker",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("name" = String, Path, description = "The worker's agent name"),
    ),
    responses(
        (status = 200, description = "The worker after the stop", body = WorkerEntry),
        (status = 400, description = "`invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`worker_process_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn stop_worker(
    human: Human,
    project: Project,
    Path((_slug, name)): Path<(String, String)>,
) -> Result<Json<WorkerEntry>, ApiError> {
    let name = fold(AgentName::parse(&name))?;
    let agents = project.0.agents();
    let worker = agents.stop_worker(&human.actor(), &name).await?;
    let config = agents.worker_config().await;
    Ok(Json(WorkerEntry::new(worker, config.as_ref())))
}

/// A named human defines an area over the tasks of one label.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/areas",
    operation_id = "define_area",
    params(("slug" = String, Path, description = "The project slug")),
    request_body = DefineAreaBody,
    responses(
        (status = 201, description = "The new area", body = AreaEntry),
        (status = 400, description = "`invalid_request`, `invalid_name` or `invalid_label_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`label_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`area_exists`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn define_area(
    human: Human,
    project: Project,
    ApiJson(body): ApiJson<DefineAreaBody>,
) -> Result<(StatusCode, Json<AreaEntry>), ApiError> {
    let name = fold(AreaName::parse(&body.name))?;
    let label = fold(LabelName::parse(&body.label))?;
    let profile = fold(AreaProfile::from_request(
        body.instructions,
        body.model,
        body.mcp,
    ))?;
    let area = project
        .0
        .agents()
        .define_area(&human.actor(), &name, &label, &profile)
        .await?;
    Ok((StatusCode::CREATED, Json(area.into())))
}

/// A named human changes an area's profile. The next start of the area's lead or task workers gets it; running workers keep theirs.
#[utoipa::path(
    patch,
    path = "/api/v1/projects/{slug}/areas/{name}",
    operation_id = "update_area",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("name" = String, Path, description = "The area's name"),
    ),
    request_body = UpdateAreaBody,
    responses(
        (status = 200, description = "The area after the change", body = AreaEntry),
        (status = 400, description = "`invalid_request` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`area_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn update_area(
    human: Human,
    project: Project,
    Path((_slug, name)): Path<(String, String)>,
    ApiJson(body): ApiJson<UpdateAreaBody>,
) -> Result<Json<AreaEntry>, ApiError> {
    let name = fold(AreaName::parse(&name))?;
    let change = fold(AreaProfileChange::from_request(AreaProfileRequest {
        instructions: body.instructions,
        model: body.model,
        mcp: body.mcp,
        clear_instructions: body.clear_instructions,
        clear_model: body.clear_model,
        clear_mcp: body.clear_mcp,
    }))?;
    let area = project
        .0
        .agents()
        .update_area(&human.actor(), &name, &change)
        .await?;
    Ok(Json(area.into()))
}

/// A named human changes the worker limits. The new limits apply to the next start; running workers keep running.
#[utoipa::path(
    patch,
    path = "/api/v1/projects/{slug}/worker-limits",
    operation_id = "set_worker_limits",
    params(("slug" = String, Path, description = "The project slug")),
    request_body = SetWorkerLimitsBody,
    responses(
        (status = 200, description = "The limits after the change", body = LimitsEntry),
        (status = 400, description = "`invalid_request`: no limit, or a limit together with its clear", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`invalid_config`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn set_worker_limits(
    human: Human,
    project: Project,
    ApiJson(body): ApiJson<SetWorkerLimitsBody>,
) -> Result<Json<LimitsEntry>, ApiError> {
    let change = fold(WorkerLimitsChange::from_request(
        body.task_worker,
        body.batch_reviewer,
        body.max_per_area,
        body.clear_max_per_area,
    ))?;
    let changed = project
        .0
        .agents()
        .set_worker_limits(&human.actor(), change)
        .await?;
    Ok(Json(changed.new.into()))
}

/// A named human pauses every agent of the project: each worker's turn is interrupted and its process ends, it keeps its session, and fold starts no worker until the resume. Claims keep their leases; approved proposals still integrate and push. Pausing a paused project changes nothing.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/pause",
    operation_id = "pause_project",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The pause state after the pause", body = PauseEntry),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn pause_project(human: Human, project: Project) -> Result<Json<PauseEntry>, ApiError> {
    let state = project.0.agents().pause_project(&human.actor()).await?;
    Ok(Json(state.into()))
}

/// A named human ends the pause: every unreleased claim gets a full lease, each worker the pause stopped runs again in its own session, and the starts the pause held back run. Resuming a project that is not paused changes nothing.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/resume",
    operation_id = "resume_project",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The pause state after the resume", body = PauseEntry),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn resume_project(human: Human, project: Project) -> Result<Json<PauseEntry>, ApiError> {
    let state = project.0.agents().resume_project(&human.actor()).await?;
    Ok(Json(state.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_and_states_keep_their_wire_names() {
        for role in Role::ALL {
            let wire = WorkerRole::from(role);
            assert_eq!(serde_json::to_value(wire).unwrap(), role.as_str());
            assert_eq!(Role::from(wire), role);
        }
        for state in fold_core::WorkerState::ALL {
            let wire = WorkerProcessState::from(state);
            assert_eq!(serde_json::to_value(wire).unwrap(), state.as_str());
        }
    }
}
