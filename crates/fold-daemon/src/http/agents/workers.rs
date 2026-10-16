use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use fold_core::{AgentName, AreaName, FoldError, LabelName, ProjectConfig, Role};
use fold_store::agents::{AreaRow, WorkerProcessRow};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::super::Project;
use super::super::actor::Human;
use super::super::error::{ApiError, ApiJson, ErrorEnvelope};
use crate::daemon::AppState;
use crate::services::agents::StartWorker;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_workers, start_worker))
        .routes(routes!(stop_worker))
        .routes(routes!(define_area))
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
}

/// The kinds of worker fold runs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRole {
    AreaLead,
    ReviewLead,
    TaskCoder,
    BatchReviewer,
}

impl From<Role> for WorkerRole {
    fn from(role: Role) -> Self {
        match role {
            Role::AreaLead => Self::AreaLead,
            Role::ReviewLead => Self::ReviewLead,
            Role::TaskCoder => Self::TaskCoder,
            Role::BatchReviewer => Self::BatchReviewer,
        }
    }
}

impl From<WorkerRole> for Role {
    fn from(role: WorkerRole) -> Self {
        match role {
            WorkerRole::AreaLead => Self::AreaLead,
            WorkerRole::ReviewLead => Self::ReviewLead,
            WorkerRole::TaskCoder => Self::TaskCoder,
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
    #[schema(example = "web-coder")]
    pub name: String,
    pub role: WorkerRole,
    /// Null for the review lead and batch reviewers.
    #[schema(required = true, example = "web")]
    pub area: Option<String>,
    pub state: WorkerProcessState,
    /// The model Claude Code reported, or the one fold asked for; null for Claude Code's default before the first turn.
    #[schema(required = true)]
    pub model: Option<String>,
    /// The tasks a task coder is bound to.
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
}

fn fold<T, E: Into<FoldError>>(result: Result<T, E>) -> Result<T, ApiError> {
    result.map_err(|error| ApiError::from(error.into()))
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
        (status = 409, description = "`area_has_lead`, `worker_exists`, `worker_limit_reached` or `invalid_config`", body = ErrorEnvelope),
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
    let area = project
        .0
        .agents()
        .define_area(&human.actor(), &name, &label)
        .await?;
    Ok((StatusCode::CREATED, Json(area.into())))
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
