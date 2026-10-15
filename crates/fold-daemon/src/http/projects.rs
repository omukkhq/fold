use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::actor::Human;
use super::error::{ApiError, ApiJson, ApiQuery, ErrorEnvelope};
use super::{Project, SocketOnly};
use crate::daemon::AppState;
use crate::services::projects::{self, PullOutcome, PushOutcome};
use fold_core::{Presence, TaskStatus, Timestamp};
use fold_vcs::git::Git;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, open))
        .routes(routes!(list_humans))
        .routes(routes!(show))
        .routes(routes!(collect_garbage))
        .routes(routes!(push))
        .routes(routes!(pull))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectSummary {
    pub slug: String,
    pub name: String,
    /// False while the project failed to open or has not been opened yet.
    pub available: bool,
    /// Null for a project that is not available, and for an open project whose store could not be read.
    #[schema(required = true)]
    pub overview: Option<ProjectOverview>,
}

/// What the project list shows of an open project.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectOverview {
    /// Unarchived tasks per board column.
    pub counts: BoardCounts,
    /// Agents with a connected session, by name.
    pub agents: Vec<ProjectAgent>,
    /// The revision `main` names, such as `r17`; null before `r1` exists.
    #[schema(required = true, example = "r17")]
    pub head: Option<String>,
    /// UTC milliseconds of the newest stored event; null when nothing happened yet.
    #[schema(value_type = Option<i64>, required = true)]
    pub last_change_at: Option<Timestamp>,
}

/// The number of tasks in each board column.
#[derive(Debug, Default, Serialize, ToSchema)]
pub struct BoardCounts {
    pub backlog: u32,
    pub todo: u32,
    pub in_progress: u32,
    pub review: u32,
    pub done: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectAgent {
    pub name: String,
    #[schema(value_type = super::agents::PresenceState)]
    pub presence: Presence,
}

impl From<projects::ProjectOverview> for ProjectOverview {
    fn from(overview: projects::ProjectOverview) -> Self {
        let mut counts = BoardCounts::default();
        for (status, count) in overview.counts {
            match status {
                TaskStatus::Backlog => counts.backlog = count,
                TaskStatus::Todo => counts.todo = count,
                TaskStatus::InProgress => counts.in_progress = count,
                TaskStatus::Review => counts.review = count,
                TaskStatus::Done => counts.done = count,
            }
        }
        Self {
            counts,
            agents: overview
                .agents
                .into_iter()
                .map(|(name, presence)| ProjectAgent {
                    name: name.to_string(),
                    presence,
                })
                .collect(),
            head: overview.head.map(|head| head.to_string()),
            last_change_at: overview.last_change_at,
        }
    }
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
            overview: entry.overview.map(Into::into),
        })
        .collect();
    Ok(Json(ProjectList { projects }))
}

/// The people who use fold on this host.
#[derive(Debug, Serialize, ToSchema)]
pub struct HumanList {
    /// Human names without the `human:` prefix, ordered by name.
    pub humans: Vec<String>,
}

/// Every human the host registry knows, so the web interface can tell a person from a file when text mentions `@name`.
#[utoipa::path(
    get,
    path = "/api/v1/humans",
    operation_id = "list_humans",
    responses(
        (status = 200, description = "The known humans", body = HumanList),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_humans(State(state): State<AppState>) -> Result<Json<HumanList>, ApiError> {
    let humans = fold_store::projects::list_humans(state.registry.read()).await?;
    Ok(Json(HumanList {
        humans: humans.iter().map(ToString::to_string).collect(),
    }))
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
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn show(Project(handle): Project) -> Result<Json<ProjectSummary>, ApiError> {
    Ok(Json(ProjectSummary {
        slug: handle.slug.clone(),
        name: handle.name.clone(),
        available: true,
        overview: Some(projects::overview(&handle).await?.into()),
    }))
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

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PushState {
    /// Every main revision was on the remote already; nothing was written.
    UpToDate,
    /// New commits were sent to the remote branch.
    Pushed,
}

/// Which remote a push or pull uses.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RemoteQuery {
    /// The remote's name; left out, the only remote.
    #[serde(default)]
    pub remote: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PushResult {
    pub outcome: PushState,
    /// The remote's name.
    pub remote: String,
    /// The remote branch.
    pub branch: String,
    /// The first revision of this push; absent when up to date.
    pub first: Option<String>,
    /// The newest revision on the remote.
    pub last: Option<String>,
    pub commits: usize,
    /// The commit the remote branch points to; absent when up to date.
    pub head: Option<String>,
}

impl From<PushOutcome> for PushResult {
    fn from(outcome: PushOutcome) -> Self {
        match outcome {
            PushOutcome::UpToDate {
                remote,
                branch,
                last,
            } => Self {
                outcome: PushState::UpToDate,
                remote,
                branch,
                first: None,
                last: last.map(|last| last.to_string()),
                commits: 0,
                head: None,
            },
            PushOutcome::Synced {
                remote,
                branch,
                first,
                last,
                commits,
                head,
            } => Self {
                outcome: PushState::Pushed,
                remote,
                branch,
                first: Some(first.to_string()),
                last: Some(last.to_string()),
                commits,
                head: Some(head.to_string()),
            },
        }
    }
}

/// Pushes the main revisions not yet on the Git remote of the project, without force. Socket only; the human comes from `x-fold-human`.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/push",
    operation_id = "push_project",
    params(("slug" = String, Path, description = "The project slug"), RemoteQuery),
    responses(
        (status = 200, description = "The remote branch holds main", body = PushResult),
        (status = 400, description = "The human name or the query is not valid, or the project has several remotes and the query names none", body = ErrorEnvelope),
        (status = 401, description = "The request names no human", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, or the slug is not registered", body = ErrorEnvelope),
        (status = 409, description = "No remote is set, the remote is ahead or diverged, or the config or sync map is invalid", body = ErrorEnvelope),
        (status = 502, description = "A Git command failed", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open, or git is not on PATH", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn push(
    _socket: SocketOnly,
    human: Human,
    Project(handle): Project,
    ApiQuery(query): ApiQuery<RemoteQuery>,
) -> Result<Json<PushResult>, ApiError> {
    let outcome = projects::push(
        &handle,
        &human.actor(),
        &Git::default(),
        query.remote.as_deref(),
    )
    .await?;
    Ok(Json(outcome.into()))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PullState {
    /// Main holds the remote branch already, or the remote has no such branch.
    UpToDate,
    /// The remote commit merged cleanly into main as a new revision.
    Integrated,
    /// The merge conflicted and main did not move; the sync task resolves it.
    Conflicted,
}

/// One path whose merge conflicted.
#[derive(Debug, Serialize, ToSchema)]
pub struct PullConflict {
    pub path: String,
    /// `both_modified_text`, `both_modified_binary`, `modify_delete`, `add_add` or `type_change`.
    pub kind: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PullResult {
    pub outcome: PullState,
    /// The remote's name.
    pub remote: String,
    /// The remote branch.
    pub branch: String,
    /// The fetched tip of the remote branch; absent when the remote has no such branch.
    pub remote_commit: Option<String>,
    /// The revision the pull integration made; absent unless integrated.
    #[schema(example = "r18")]
    pub revision: Option<String>,
    /// The sync task that resolves a conflict; absent unless conflicted.
    #[schema(example = "T-12")]
    pub task: Option<String>,
    /// The sync task's workspace; absent unless conflicted.
    #[schema(example = "ws-9")]
    pub workspace: Option<String>,
    /// In path order; empty unless conflicted.
    pub conflicted: Vec<PullConflict>,
    /// False when an open sync task for the same commit existed already and nothing was created.
    pub created: bool,
}

impl From<PullOutcome> for PullResult {
    fn from(outcome: PullOutcome) -> Self {
        match outcome {
            PullOutcome::UpToDate {
                remote,
                branch,
                remote_commit,
            } => Self {
                outcome: PullState::UpToDate,
                remote,
                branch,
                remote_commit: remote_commit.map(|commit| commit.to_string()),
                revision: None,
                task: None,
                workspace: None,
                conflicted: Vec::new(),
                created: false,
            },
            PullOutcome::Integrated {
                remote,
                branch,
                remote_commit,
                revision,
            } => Self {
                outcome: PullState::Integrated,
                remote,
                branch,
                remote_commit: Some(remote_commit.to_string()),
                revision: Some(revision.to_string()),
                task: None,
                workspace: None,
                conflicted: Vec::new(),
                created: false,
            },
            PullOutcome::Conflicted {
                remote,
                branch,
                remote_commit,
                task,
                workspace,
                conflicted,
                created,
            } => Self {
                outcome: PullState::Conflicted,
                remote,
                branch,
                remote_commit: Some(remote_commit.to_string()),
                revision: None,
                task: Some(task.to_string()),
                workspace: workspace.map(|workspace| workspace.to_string()),
                conflicted: conflicted
                    .into_iter()
                    .map(|conflict| PullConflict {
                        path: conflict.path,
                        kind: conflict.kind.as_str().to_owned(),
                    })
                    .collect(),
                created,
            },
        }
    }
}

/// Brings the remote branch into main: a clean merge integrates, a conflict creates a sync task with a workspace that holds the merge. Socket only; the human comes from `x-fold-human`.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/pull",
    operation_id = "pull_project",
    params(("slug" = String, Path, description = "The project slug"), RemoteQuery),
    responses(
        (status = 200, description = "Main holds the remote branch, or a sync task resolves the conflict", body = PullResult),
        (status = 400, description = "The human name or the query is not valid, or the project has several remotes and the query names none", body = ErrorEnvelope),
        (status = 401, description = "The request names no human", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, or the slug is not registered", body = ErrorEnvelope),
        (status = 409, description = "No remote is set, main moved during every attempt, the mirror diverged, or the config or sync map is invalid", body = ErrorEnvelope),
        (status = 502, description = "A Git command failed", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open, git is not on PATH, or the objects of the integration could not be stored", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn pull(
    _socket: SocketOnly,
    human: Human,
    Project(handle): Project,
    ApiQuery(query): ApiQuery<RemoteQuery>,
) -> Result<Json<PullResult>, ApiError> {
    let outcome = projects::pull(
        &handle,
        &human.actor(),
        &Git::default(),
        query.remote.as_deref(),
    )
    .await?;
    Ok(Json(outcome.into()))
}
