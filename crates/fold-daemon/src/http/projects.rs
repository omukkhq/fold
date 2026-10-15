use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::error::{ApiError, ApiJson, ErrorEnvelope};
use super::{Project, SocketOnly};
use crate::daemon::AppState;
use crate::services::projects;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, open))
        .routes(routes!(show))
        .routes(routes!(collect_garbage))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectSummary {
    pub slug: String,
    pub name: String,
    /// False while the project failed to open or has not been opened yet.
    pub available: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectList {
    pub projects: Vec<ProjectSummary>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct OpenProject {
    /// The slug of a project already in the registry.
    pub slug: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OpenedProject {
    pub slug: String,
    pub name: String,
}

/// Every registered project and whether the daemon has it open.
#[utoipa::path(
    get,
    path = "/api/v1/projects",
    operation_id = "list_projects",
    responses(
        (status = 200, description = "The registered projects", body = ProjectList),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list(State(state): State<AppState>) -> Result<Json<ProjectList>, ApiError> {
    let projects = projects::list(&state)
        .await?
        .into_iter()
        .map(|entry| ProjectSummary {
            slug: entry.slug,
            name: entry.name,
            available: entry.available,
        })
        .collect();
    Ok(Json(ProjectList { projects }))
}

/// One open project.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}",
    operation_id = "show_project",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The project", body = ProjectSummary),
        (status = 404, description = "The slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
    )
)]
pub async fn show(Project(handle): Project) -> Json<ProjectSummary> {
    Json(ProjectSummary {
        slug: handle.slug.clone(),
        name: handle.name.clone(),
        available: true,
    })
}

/// Opens a project that was registered after the daemon started. Socket only; an open project answers the same again.
#[utoipa::path(
    post,
    path = "/api/v1/projects",
    operation_id = "open_project",
    request_body = OpenProject,
    responses(
        (status = 200, description = "The project is open", body = OpenedProject),
        (status = 400, description = "The body is not valid", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, or the slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn open(
    _socket: SocketOnly,
    State(state): State<AppState>,
    ApiJson(request): ApiJson<OpenProject>,
) -> Result<Json<OpenedProject>, ApiError> {
    let handle = projects::open(&state, &request.slug).await?;
    Ok(Json(OpenedProject {
        slug: handle.slug.clone(),
        name: handle.name.clone(),
    }))
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct GcRequest {
    /// Count what a run would remove and change nothing.
    #[serde(default)]
    pub dry_run: bool,
}

/// The counts of one garbage collection run.
#[derive(Debug, Serialize, ToSchema)]
pub struct GcCounts {
    /// True when nothing was changed and the counts are what a run would do.
    pub dry_run: bool,
    /// Objects in the store when the sweep listed them.
    pub examined: u64,
    /// Unreferenced objects older than the grace period.
    pub deleted: u64,
    /// Disk bytes of the deleted objects.
    pub bytes_freed: u64,
    /// Abandoned scratch files of the object store.
    pub tmp_removed: u64,
    /// Objects a row reaches that the store does not hold.
    pub missing: u64,
    /// Directories of archived or abandoned workspaces past their retention.
    pub workspaces_removed: u64,
    /// Automatic checkpoints beyond the kept number, and every checkpoint of a removed workspace.
    pub checkpoints_pruned: u64,
}

/// Collects the project's garbage: prunes checkpoints, removes closed workspace directories past their retention and deletes unreferenced objects. Socket only; used by `fold gc`.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/gc",
    operation_id = "run_gc",
    params(("slug" = String, Path, description = "The project slug")),
    request_body = GcRequest,
    responses(
        (status = 200, description = "The run finished", body = GcCounts),
        (status = 400, description = "The body is not valid", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, or the slug is not registered", body = ErrorEnvelope),
        (status = 409, description = "A run of this project is in progress (`gc_in_progress`)", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn collect_garbage(
    _socket: SocketOnly,
    Project(handle): Project,
    ApiJson(request): ApiJson<GcRequest>,
) -> Result<Json<GcCounts>, ApiError> {
    let report = projects::gc(&handle, request.dry_run).await?;
    Ok(Json(GcCounts {
        dry_run: request.dry_run,
        examined: report.examined,
        deleted: report.deleted,
        bytes_freed: report.bytes_freed,
        tmp_removed: report.tmp_removed,
        missing: u64::try_from(report.missing.len()).unwrap_or(u64::MAX),
        workspaces_removed: report.workspaces_removed,
        checkpoints_pruned: report.checkpoints_pruned,
    }))
}
