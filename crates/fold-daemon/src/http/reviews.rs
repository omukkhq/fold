use axum::Json;
use axum::extract::{Path, State};
use fold_core::{FoldError, HumanName, TaskId};
use fold_vcs::git::Git;
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::Project;
use super::actor::Human;
use super::error::{ApiError, ApiJson, ErrorEnvelope};
use super::tasks::{HumanReviewMark, TaskView, id};
use crate::daemon::AppState;
use crate::services::reviews::ReviewService;
use crate::services::workspaces::WorkspaceService;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(set_human_review, clear_human_review))
}

fn service(state: &AppState, project: &Project) -> ReviewService {
    let handle = &project.0;
    ReviewService::new(
        handle.store.clone(),
        handle.bus.clone(),
        handle.clock.clone(),
        WorkspaceService::for_project(handle, Git::default()),
    )
    .with_registry(state.registry.clone())
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct HumanReviewBody {
    /// The one human whose approval counts; left out or null, any human may approve. It must be a human who has used the CLI or the web interface.
    #[serde(default)]
    pub reviewer: Option<String>,
    /// Why a human must look; not empty, at most 500 characters.
    pub reason: String,
}

/// Turns on the human review mark, or changes the reviewer and the reason. The mark never changes the task's status; a proposal of a marked task waits for a human approval of its current version.
#[utoipa::path(
    put,
    path = "/api/v1/projects/{slug}/tasks/{id}/human-review",
    operation_id = "set_human_review",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    request_body = HumanReviewBody,
    responses(
        (status = 200, description = "The mark as it now stands", body = HumanReviewMark),
        (status = 400, description = "`invalid_request` (`details.field` names the field), `invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found`, `unknown_human`: the reviewer is not a known human, or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`task_closed`: the task is Done or archived", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn set_human_review(
    State(state): State<AppState>,
    human: Human,
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
    ApiJson(body): ApiJson<HumanReviewBody>,
) -> Result<Json<HumanReviewMark>, ApiError> {
    let task: TaskId = id(&task)?;
    let reviewer = body
        .reviewer
        .as_deref()
        .map(HumanName::parse)
        .transpose()
        .map_err(FoldError::from)?;
    let mark = service(&state, &project)
        .require_human_review(&human.actor(), task, reviewer, &body.reason)
        .await?;
    Ok(Json(mark.into()))
}

/// Turns the human review mark off. Clearing a task without a mark changes nothing.
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{slug}/tasks/{id}/human-review",
    operation_id = "clear_human_review",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    responses(
        (status = 200, description = "The task without a mark", body = TaskView),
        (status = 400, description = "`invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`task_closed`: the task is Done or archived, or `invalid_config`: the task has a claim and the project configuration that sets its lease is invalid", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn clear_human_review(
    State(state): State<AppState>,
    human: Human,
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
) -> Result<Json<TaskView>, ApiError> {
    let task: TaskId = id(&task)?;
    service(&state, &project)
        .clear_human_review(&human.actor(), task)
        .await?;
    Ok(Json(project.0.tasks.get_view(task).await?.into()))
}
