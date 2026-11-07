use axum::Json;
use fold_vcs::git::Git;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::super::Project;
use super::super::actor::Human;
use super::super::diff::{DiffView, comparison_view, file_view};
use super::super::error::{ApiError, ApiQuery, ErrorEnvelope};
use super::super::workspaces::ChangeSource;
use super::{RecordQuery, RecordResult, record_as};
use crate::daemon::AppState;
use crate::services::projects;
use crate::services::workspaces::WorkspaceService;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(record_agent_changes))
        .routes(routes!(restore_agent_changes))
        .routes(routes!(ignore_agent_changes))
        .routes(routes!(folder_diff))
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

/// Puts the changes that supervised workers made in the project folder into a new stash entry, `stash@{0}`, and brings those paths back to the checkout record's entries, as Put aside on the web app's Needs you item asks; the human's other changes stay, and `fld unstash` brings the worker's change back. For a human, over the socket or over HTTP with the session cookie.
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

/// Lets the changes that supervised workers made in the project folder count as the human's own unrecorded changes, as Keep as mine on the web app's Needs you item asks; `fld record` then records them as any other. For a human, over the socket or over HTTP with the session cookie.
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

/// The file of the project folder whose hunks the answer holds.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FolderDiffQuery {
    /// One changed file, such as `plan.md`: with it the answer holds that file's hunks; without it, the list of changed files.
    pub path: Option<String>,
}

/// Compares the project folder with the revision it holds, calculated on request, so a human sees what a worker's change or a record holds before choosing: the changed files with line counts, or the hunks of one file when `path` is given. The folder is read as `fld record` reads it: ignored and too-large files are left out and a symlink is recorded, not followed. For a human, over the socket or over HTTP with the session cookie.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/folder/diff",
    operation_id = "get_folder_diff",
    params(("slug" = String, Path, description = "The project slug"), FolderDiffQuery),
    responses(
        (status = 200, description = "The file list, or one file's hunks", body = DiffView),
        (status = 400, description = "The path is not a relative, normalised path inside the project folder (`invalid_path`)", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
        (status = 404, description = "The path is not a change of the folder (`path_not_in_comparison`), or the slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "A file of the folder could not be read (`workspace_io`), or an internal error", body = ErrorEnvelope),
    )
)]
pub async fn folder_diff(
    _human: Human,
    Project(handle): Project,
    ApiQuery(query): ApiQuery<FolderDiffQuery>,
) -> Result<Json<DiffView>, ApiError> {
    let (from, to) = projects::folder_sides(&handle).await?;
    let workspaces = WorkspaceService::for_project(&handle);
    let view = match query.path {
        Some(path) => DiffView::File(file_view(
            ChangeSource::Working,
            workspaces.diff_file(from, to, &path).await?,
        )),
        None => DiffView::Files(comparison_view(
            ChangeSource::Working,
            workspaces.compare_states(from, to).await?,
        )),
    };
    Ok(Json(view))
}
