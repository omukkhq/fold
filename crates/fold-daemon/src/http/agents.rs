use axum::Json;
use axum::extract::Path;
use fold_core::{AgentName, FoldError};
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::Project;
use super::actor::Human;
use super::error::{ApiError, ErrorEnvelope};
use crate::daemon::AppState;
use crate::services::agents::{AgentService, AgentTags};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(grant_integrator, revoke_integrator))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentTagList {
    pub agent: String,
    /// Every tag of the agent after the change, sorted.
    pub tags: Vec<String>,
}

impl From<AgentTags> for AgentTagList {
    fn from(tags: AgentTags) -> Self {
        Self {
            agent: tags.agent.to_string(),
            tags: tags.tags.iter().map(ToString::to_string).collect(),
        }
    }
}

async fn set_integrator(
    human: &Human,
    project: &Project,
    name: &str,
    on: bool,
) -> Result<Json<AgentTagList>, ApiError> {
    let agent = AgentName::parse(name).map_err(FoldError::from)?;
    let handle = &project.0;
    let service = AgentService::new(
        handle.store.clone(),
        handle.bus.clone(),
        handle.clock.clone(),
    );
    let tags = service.set_integrator(&human.actor(), &agent, on).await?;
    Ok(Json(tags.into()))
}

/// Grants the integrator tag. Only a named human reaches this route; granting a present tag changes nothing.
#[utoipa::path(
    put,
    path = "/api/v1/projects/{slug}/agents/{name}/tags/integrator",
    operation_id = "grant_integrator",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("name" = String, Path, description = "The agent name"),
    ),
    responses(
        (status = 200, description = "The agent's tags after the grant", body = AgentTagList),
        (status = 400, description = "`invalid_name`: the human or agent name breaks the name pattern", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
        (status = 403, description = "`human_required`: the actor is not a human", body = ErrorEnvelope),
        (status = 404, description = "`agent_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn grant_integrator(
    human: Human,
    project: Project,
    Path((_slug, name)): Path<(String, String)>,
) -> Result<Json<AgentTagList>, ApiError> {
    set_integrator(&human, &project, &name, true).await
}

/// Removes the integrator tag. Only a named human reaches this route; removing an absent tag changes nothing.
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{slug}/agents/{name}/tags/integrator",
    operation_id = "revoke_integrator",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("name" = String, Path, description = "The agent name"),
    ),
    responses(
        (status = 200, description = "The agent's tags after the removal", body = AgentTagList),
        (status = 400, description = "`invalid_name`: the human or agent name breaks the name pattern", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
        (status = 403, description = "`human_required`: the actor is not a human", body = ErrorEnvelope),
        (status = 404, description = "`agent_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn revoke_integrator(
    human: Human,
    project: Project,
    Path((_slug, name)): Path<(String, String)>,
) -> Result<Json<AgentTagList>, ApiError> {
    set_integrator(&human, &project, &name, false).await
}
