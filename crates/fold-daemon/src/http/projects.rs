use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::error::{ApiError, ApiJson, ErrorEnvelope};
use super::{Project, SocketOnly};
use crate::daemon::AppState;
use crate::services::projects;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/projects", get(list).post(open))
        .route("/api/v1/projects/{slug}", get(show))
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
    request_body = OpenProject,
    responses(
        (status = 200, description = "The project is open", body = OpenedProject),
        (status = 400, description = "The body is not valid", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, or the slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
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
