use axum::Json;
use fold_core::Timestamp;
use fold_vcs::git::Git;
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::super::Project;
use super::super::actor::Human;
use super::super::error::{ApiError, ApiQuery, ErrorEnvelope};
use super::{RecordQuery, RecordResult, record_as};
use crate::daemon::AppState;
use crate::services::projects;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_agent_changes))
        .routes(routes!(record_agent_changes))
        .routes(routes!(restore_agent_changes))
        .routes(routes!(ignore_agent_changes))
}

/// A path of the project folder that changed during a tool call of a supervised worker.
#[derive(Debug, Serialize, ToSchema)]
pub struct AgentChangeView {
    pub path: String,
    /// The agents whose tool calls ran when the path changed, in name order.
    #[schema(example = json!(["reviewer-35"]))]
    pub agents: Vec<String>,
    /// UTC milliseconds when fold first saw the change.
    #[schema(value_type = i64)]
    pub since: Timestamp,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentChangeList {
    /// In path order.
    pub changes: Vec<AgentChangeView>,
}

/// The paths that fold placed in a tool call of a supervised worker, which it does not offer as the human's change until a human, or the main agent when the human asks, records, restores or ignores them. A read.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/agent-changes",
    operation_id = "list_agent_changes",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The changes", body = AgentChangeList),
        (status = 404, description = "The slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_agent_changes(
    Project(handle): Project,
) -> Result<Json<AgentChangeList>, ApiError> {
    let changes = projects::agent_changes(&handle.store).await?;
    Ok(Json(AgentChangeList {
        changes: changes
            .into_iter()
            .map(|change| AgentChangeView {
                path: change.path,
                agents: change.agents.iter().map(ToString::to_string).collect(),
                since: change.since,
            })
            .collect(),
    }))
}

/// Records the project folder on main for the signed-in human, as `fld record` does, from the Needs you item of a worker's change: the human keeps the change as their own. It records every unrecorded change of the project folder, the human's own included, not only the worker's files. Without `confirm` a change that empties a tracked file, adds many empty files or holds a worker's change is refused with `record_needs_confirmation`, whose details name the files and the `tree` to confirm. Over the socket and over HTTP with the session cookie.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/agent-changes/record",
    operation_id = "record_agent_changes",
    params(("slug" = String, Path, description = "The project slug"), RecordQuery),
    responses(
        (status = 200, description = "Main holds the human change, or it waits for the main agent", body = RecordResult),
        (status = 400, description = "`confirm` is not a tree hash, the branch or HEAD shows a change fold did not make (`reason` `head_moved`), or files still hold the conflict of `fld unstash`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
        (status = 404, description = "The slug is not registered", body = ErrorEnvelope),
        (status = 409, description = "`record_needs_confirmation`, or main moved during every attempt", body = ErrorEnvelope),
        (status = 502, description = "A Git command failed", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open, or git is not on PATH", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn record_agent_changes(
    human: Human,
    Project(handle): Project,
    ApiQuery(query): ApiQuery<RecordQuery>,
) -> Result<Json<RecordResult>, ApiError> {
    record_as(&handle, &human, &query).await
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentChangesRestored {
    /// The main revision the project folder holds now.
    #[schema(example = "r18")]
    pub revision: String,
    /// The paths that went into `stash@{0}` and back to main's entries, in path order; empty when the folder held them as recorded already.
    pub files: Vec<String>,
    /// The entries on the stash now.
    pub entries: usize,
}

/// Puts the changes that supervised workers made in the project folder into a new stash entry, `stash@{0}`, and brings those paths back to the checkout record's entries, as `fld agent-changes restore` asks; the human's other changes stay, and `fld unstash` brings the worker's change back. For a human, over the socket or over HTTP with the session cookie.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/agent-changes/restore",
    operation_id = "restore_agent_changes",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The worker's change is in the stash", body = AgentChangesRestored),
        (status = 400, description = "The branch or HEAD shows a change fold did not make, the conflict of `fld unstash` waits, or a file changed while fold stashed it", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
        (status = 404, description = "`agent_change_not_found`: no worker's change is open, or the slug is not registered", body = ErrorEnvelope),
        (status = 502, description = "A Git command failed", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn restore_agent_changes(
    human: Human,
    Project(handle): Project,
) -> Result<Json<AgentChangesRestored>, ApiError> {
    let restored =
        projects::restore_agent_changes(&handle, &human.actor(), &Git::default()).await?;
    Ok(Json(AgentChangesRestored {
        revision: restored.revision.to_string(),
        files: restored.files,
        entries: restored.entries,
    }))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AgentChangesIgnored {
    /// The paths that now count as the human's own unrecorded changes, in path order.
    pub files: Vec<String>,
}

/// Lets the changes that supervised workers made in the project folder count as the human's own unrecorded changes, as `fld agent-changes ignore` asks; `fld record` then records them as any other. For a human, over the socket or over HTTP with the session cookie.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/agent-changes/ignore",
    operation_id = "ignore_agent_changes",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The changes count as the human's", body = AgentChangesIgnored),
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
        (status = 404, description = "`agent_change_not_found`: no worker's change is open, or the slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn ignore_agent_changes(
    human: Human,
    Project(handle): Project,
) -> Result<Json<AgentChangesIgnored>, ApiError> {
    let files = projects::ignore_agent_changes(&handle, &human.actor()).await?;
    Ok(Json(AgentChangesIgnored { files }))
}
