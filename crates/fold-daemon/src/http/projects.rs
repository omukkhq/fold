use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::actor::Human;
use super::agents::cost::time_param;
use super::error::{ApiError, ApiJson, ApiQuery, ErrorEnvelope};
use super::{Project, SocketOnly};
use crate::daemon::{AppState, StartedBy};
use crate::services::projects::{
    self, PullOutcome, PushOutcome, RecordOutcome, StashOutcome, UnstashOutcome,
};
use fold_core::{Presence, TaskStatus, Timestamp};
use fold_vcs::git::Git;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list, open))
        .routes(routes!(list_humans))
        .routes(routes!(show_daemon))
        .routes(routes!(show, remove))
        .routes(routes!(restore))
        .routes(routes!(show_git, enable_git))
        .routes(routes!(collect_garbage))
        .routes(routes!(push))
        .routes(routes!(pull))
        .routes(routes!(record))
        .routes(routes!(list_stash, stash))
        .routes(routes!(unstash))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectSummary {
    pub slug: String,
    pub name: String,
    /// The project folder, which holds `.fold/`.
    pub root: String,
    /// False while the project failed to open or has not been opened yet, and for a project a person removed.
    pub available: bool,
    /// UTC milliseconds of the project's last use: when a person last opened it or its newest event happened, whichever is later, and its registration before either. The project switcher lists the most recent first.
    #[schema(value_type = i64)]
    pub last_used_at: Timestamp,
    /// UTC milliseconds of when a person removed the project from fold; null while it is in fold. A removed project keeps its data in its folder's `.fold/`, and the list shows it only when asked to.
    #[schema(value_type = Option<i64>, required = true)]
    pub removed_at: Option<Timestamp>,
    /// What is at the project folder.
    pub folder: ProjectFolderState,
    /// Null for a project that is not available, and for an open project whose store could not be read.
    #[schema(required = true)]
    pub overview: Option<ProjectOverview>,
}

/// What is at a project's folder. `present`: the folder holds `.fold/` with its database. `folder_missing`: the folder is gone, as on a disk that is not mounted; fold opens the project again when it is back. `fold_missing`: the folder is there without `.fold/`; only a removed project is listed so, since fold takes any other off by itself. `database_missing`: `.fold/` holds no database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectFolderState {
    Present,
    FolderMissing,
    FoldMissing,
    DatabaseMissing,
}

impl From<crate::projects::ProjectFolder> for ProjectFolderState {
    fn from(folder: crate::projects::ProjectFolder) -> Self {
        use crate::projects::ProjectFolder;
        match folder {
            ProjectFolder::Present => Self::Present,
            ProjectFolder::RootMissing => Self::FolderMissing,
            ProjectFolder::FoldMissing => Self::FoldMissing,
            ProjectFolder::DatabaseMissing => Self::DatabaseMissing,
        }
    }
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
    /// What needs a person in the project as a whole, beside the attention items of its tasks; empty when nothing does.
    pub attention: Vec<ProjectAttentionKind>,
    /// UTC milliseconds of the pause of the project's agents, while they are paused; null otherwise.
    #[schema(value_type = Option<i64>, required = true)]
    pub paused_since: Option<Timestamp>,
    /// Who paused the agents, such as `human:ana` or the main agent's name, while they are paused; null otherwise.
    #[schema(required = true)]
    pub paused_by: Option<String>,
    pub cost: ProjectCostTotals,
}

/// What the project's supervised workers cost, in millionths of a US dollar, from the values Claude Code reported. The main agent is the user's own Claude Code session, which fold does not supervise, so its cost is in none of them.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectCostTotals {
    pub all_time_micro_usd: u64,
    /// The turns that ended since the start of the day the list was asked for with `today`, else since midnight UTC.
    pub today_micro_usd: u64,
    /// The turns that ended since the start of the week the list was asked for with `week`, else since midnight UTC six days ago.
    pub week_micro_usd: u64,
    /// Every token of every turn, each turn's own from the usage Claude Code reported.
    pub all_time_tokens: u64,
}

/// Why the project, rather than one of its tasks, needs a person. The daemon decides every kind. `uncommitted_changes`: the project folder has changes that no revision records, which the human records with `fld record` or puts aside with `fld stash`; such a change on a path main changed, or the conflict markers of `fld unstash`, keep the whole folder where it is until then. `head_moved`: the Git branch or HEAD shows a change fold did not make, such as another branch checked out, a reset or a stash, so the project folder stops following main until the human brings the branch back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectAttentionKind {
    UncommittedChanges,
    HeadMoved,
}

impl From<projects::ProjectAttention> for ProjectAttentionKind {
    fn from(attention: projects::ProjectAttention) -> Self {
        match attention {
            projects::ProjectAttention::UncommittedChanges => Self::UncommittedChanges,
            projects::ProjectAttention::HeadMoved => Self::HeadMoved,
        }
    }
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
            attention: overview.attention.into_iter().map(Into::into).collect(),
            paused_since: overview.paused_since,
            paused_by: overview.paused_by,
            cost: ProjectCostTotals {
                all_time_micro_usd: overview.cost.all_time,
                today_micro_usd: overview.cost.today,
                week_micro_usd: overview.cost.week,
                all_time_tokens: overview.cost.all_time_tokens,
            },
        }
    }
}

impl From<projects::ProjectEntry> for ProjectSummary {
    fn from(entry: projects::ProjectEntry) -> Self {
        Self {
            slug: entry.slug,
            name: entry.name,
            root: entry.root.to_string_lossy().into_owned(),
            available: entry.available,
            last_used_at: entry.last_used_at,
            removed_at: entry.removed_at,
            folder: entry.folder.into(),
            overview: entry.overview.map(Into::into),
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

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListProjectsQuery {
    /// List the projects a person removed from fold too; they are left out without it.
    #[serde(default)]
    pub include_removed: bool,
    /// UTC milliseconds of the start of the viewer's day, from which each project's cost of today counts; midnight UTC without it.
    pub today: Option<i64>,
    /// UTC milliseconds of the start of the viewer's last seven days, from which each project's cost of the week counts; midnight UTC six days ago without it.
    pub week: Option<i64>,
}

/// Where the viewer's day and week start, for the cost totals of the project an answer shows.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CostDaysQuery {
    /// UTC milliseconds of the start of the viewer's day, from which the project's cost of today counts; midnight UTC without it.
    pub today: Option<i64>,
    /// UTC milliseconds of the start of the viewer's last seven days, from which the project's cost of the week counts; midnight UTC six days ago without it.
    pub week: Option<i64>,
}

fn cost_days(today: Option<i64>, week: Option<i64>) -> Result<projects::CostDays, ApiError> {
    Ok(projects::CostDays {
        today: today.map(time_param).transpose()?,
        week: week.map(time_param).transpose()?,
    })
}

/// Every project in fold and whether the daemon has it open, ordered by slug; with `include_removed`, the projects a person removed too. A project whose `.fold/` was deleted is removed from fold before the list is read, so it is not in the list.
#[utoipa::path(
    get,
    path = "/api/v1/projects",
    operation_id = "list_projects",
    params(ListProjectsQuery),
    responses(
        (status = 200, description = "The registered projects", body = ProjectList),
        (status = 400, description = "`invalid_request`: `include_removed` is not `true` or `false`, or `today` or `week` is not a time", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<ListProjectsQuery>,
) -> Result<Json<ProjectList>, ApiError> {
    let days = cost_days(query.today, query.week)?;
    let projects = projects::list_for(&state, query.include_removed, days)
        .await?
        .into_iter()
        .map(ProjectSummary::from)
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
    let humans = projects::list_humans(&state.registry).await?;
    Ok(Json(HumanList {
        humans: humans.iter().map(ToString::to_string).collect(),
    }))
}

/// What started the daemon process.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DaemonStartedBy {
    /// A person or a command such as `fld serve` or `fld claude`, outside the service.
    Hand,
    /// The systemd user service that `fld setup` installed.
    Systemd,
    /// The launchd agent that `fld setup` installed.
    Launchd,
}

impl From<StartedBy> for DaemonStartedBy {
    fn from(started_by: StartedBy) -> Self {
        match started_by {
            StartedBy::Hand => Self::Hand,
            StartedBy::Systemd => Self::Systemd,
            StartedBy::Launchd => Self::Launchd,
        }
    }
}

/// What the running daemon is, for `fld` and `fld doctor` to report it.
#[derive(Debug, Serialize, ToSchema)]
pub struct DaemonView {
    /// The fold version the daemon runs, such as `0.3.0`.
    pub version: String,
    /// The address and port the HTTP listener is bound to, such as `127.0.0.1:7420`.
    pub listen: String,
    /// The process id of the daemon.
    pub pid: u32,
    /// UTC milliseconds of the daemon's start.
    #[schema(value_type = i64)]
    pub started_at: Timestamp,
    pub started_by: DaemonStartedBy,
    /// The file name of the login shell the daemon ran at its start to read the workers' environment, such as `bash`; null when it knows none.
    #[schema(required = true, example = "bash")]
    pub shell: Option<String>,
    /// UTC milliseconds of the moment the daemon started to run that shell; a shell startup file changed after it holds changes the workers lack until `fld restart`. Null when it ran none.
    #[schema(value_type = Option<i64>, required = true)]
    pub shell_read_at: Option<Timestamp>,
}

/// The daemon's version, process and start, and the address its web interface listens on. Socket only, so a command learns the port the daemon got instead of the one its configuration asked for.
#[utoipa::path(
    get,
    path = "/api/v1/daemon",
    operation_id = "show_daemon",
    responses(
        (status = 200, description = "The running daemon", body = DaemonView),
        (status = 404, description = "Not on the socket", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn show_daemon(_socket: SocketOnly, State(state): State<AppState>) -> Json<DaemonView> {
    let start = state.projects.start();
    let environment = state.projects.environment();
    Json(DaemonView {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        listen: state.listen.to_string(),
        pid: start.pid,
        started_at: start.started_at,
        started_by: start.started_by.into(),
        shell: environment.shell.clone(),
        shell_read_at: environment.shell_read_at,
    })
}

/// One open project. Reading it notes that a person uses the project now, which orders the recent projects of the project switcher.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}",
    operation_id = "show_project",
    params(("slug" = String, Path, description = "The project slug"), CostDaysQuery),
    responses(
        (status = 200, description = "The project", body = ProjectSummary),
        (status = 400, description = "`invalid_request`: `today` or `week` is not a time", body = ErrorEnvelope),
        (status = 404, description = "The slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn show(
    State(state): State<AppState>,
    Project(handle): Project,
    ApiQuery(query): ApiQuery<CostDaysQuery>,
) -> Result<Json<ProjectSummary>, ApiError> {
    let days = cost_days(query.today, query.week)?;
    Ok(Json(projects::show(&state, &handle, days).await?.into()))
}

/// A named human removes the project from fold: the daemon closes it and leaves it out of the project list, and its data stays in the folder's `.fold/`. `fld claude`, `fld new` or `fld init` in the folder, or the restore route, brings it back under its slug. Removing a removed project changes nothing. While a worker of the project runs, the removal is refused.
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{slug}",
    operation_id = "remove_project",
    params(("slug" = String, Path, description = "The project slug"), CostDaysQuery),
    responses(
        (status = 200, description = "The project as the list shows it after the removal", body = ProjectSummary),
        (status = 400, description = "`invalid_request`: `today` or `week` is not a time", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`: the slug is not registered", body = ErrorEnvelope),
        (status = 409, description = "`project_workers_running` with the running agents in `details.workers`: pause the project or stop them first", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn remove(
    human: Human,
    State(state): State<AppState>,
    Path(slug): Path<String>,
    ApiQuery(query): ApiQuery<CostDaysQuery>,
) -> Result<Json<ProjectSummary>, ApiError> {
    let days = cost_days(query.today, query.week)?;
    Ok(Json(
        projects::remove(&state, &human.actor(), &slug, days)
            .await?
            .into(),
    ))
}

/// A named human brings back a project a person removed, while its folder holds the project's `.fold/`: it is in fold again under its slug and the daemon opens it. A project in fold stays as it is.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/restore",
    operation_id = "restore_project",
    params(("slug" = String, Path, description = "The project slug"), CostDaysQuery),
    responses(
        (status = 200, description = "The project as the list shows it after it came back", body = ProjectSummary),
        (status = 400, description = "`invalid_request`: `today` or `week` is not a time", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`: the slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable` with `details.reason` `folder_missing`, `fold_missing` or `database_missing`: the folder does not hold the project, which stays removed", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn restore(
    human: Human,
    State(state): State<AppState>,
    Path(slug): Path<String>,
    ApiQuery(query): ApiQuery<CostDaysQuery>,
) -> Result<Json<ProjectSummary>, ApiError> {
    let days = cost_days(query.today, query.week)?;
    Ok(Json(
        projects::restore(&state, &human.actor(), &slug, days)
            .await?
            .into(),
    ))
}

/// The project's Git repository and the state of its remotes.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectGit {
    /// The project folder, which holds the repository.
    pub repository: String,
    /// The branch every integration is committed to.
    pub branch: String,
    /// Ordered by name; empty when no remote is set.
    pub remotes: Vec<GitRemote>,
}

/// One Git remote of the project.
#[derive(Debug, Serialize, ToSchema)]
pub struct GitRemote {
    pub name: String,
    /// The URL or path, without credentials.
    pub location: String,
    /// The remote branch main is pushed to.
    pub branch: String,
    /// Whether fold pushes after each integration.
    pub push_on_integrate: bool,
    /// The newest push that reached the remote; null before the first.
    #[schema(required = true)]
    pub pushed: Option<GitPushed>,
    /// The failed push after the newest successful one; null when the last push succeeded or none failed.
    #[schema(required = true)]
    pub failed: Option<GitPushFailed>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GitPushed {
    /// The newest revision the push sent.
    #[schema(example = "r17")]
    pub revision: String,
    /// The Git commit of that revision; null while it has none.
    #[schema(required = true)]
    pub commit: Option<String>,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub at: Timestamp,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GitPushFailed {
    /// The error code of the failed push.
    pub code: String,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub at: Timestamp,
}

impl From<projects::GitStatus> for ProjectGit {
    fn from(status: projects::GitStatus) -> Self {
        Self {
            repository: status.repository.to_string_lossy().into_owned(),
            branch: status.branch,
            remotes: status
                .remotes
                .into_iter()
                .map(|remote| GitRemote {
                    name: remote.name,
                    location: remote.location,
                    branch: remote.branch,
                    push_on_integrate: remote.push_on_integrate,
                    pushed: remote.pushed.map(|pushed| GitPushed {
                        revision: pushed.revision.to_string(),
                        commit: pushed.commit,
                        at: pushed.at,
                    }),
                    failed: remote.failed.map(|failed| GitPushFailed {
                        code: failed.code,
                        at: failed.at,
                    }),
                })
                .collect(),
        }
    }
}

/// The Git repository of one open project, its branch and its remotes with the last push of each.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/git",
    operation_id = "show_project_git",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The Git state", body = ProjectGit),
        (status = 404, description = "The slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn show_git(Project(handle): Project) -> Result<Json<ProjectGit>, ApiError> {
    Ok(Json(projects::git_status(&handle).await?.into()))
}

/// Opens a project that was registered after the daemon started, for a `fld` command in its folder; a project a person removed comes back into fold first. Socket only; an open project answers the same again, unless its database was replaced: the daemon then closes it and opens the folder as it is now. A folder that is there without `.fold/`, as after its fold data was deleted, is removed from fold and answers `project_not_found`; a missing folder or a `.fold/` without its database keeps the project and answers `project_unavailable` with `details.reason` `folder_missing` or `database_missing`.
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
#[derive(Debug, Serialize, Deserialize, ToSchema)]
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
    /// Directories of archived or abandoned workspaces that the run removed.
    pub workspaces_removed: u64,
    /// Automatic checkpoints beyond the kept number, and every checkpoint of a workspace closed for longer than `closed_workspace_days`.
    pub checkpoints_pruned: u64,
}

/// Collects the project's garbage: prunes checkpoints, removes the closed workspace directories that are still there and deletes unreferenced objects. Socket only; used by `fld gc`.
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
    /// The first revision of this push; null when up to date.
    #[schema(required = true)]
    pub first: Option<String>,
    /// The newest revision on the remote.
    #[schema(required = true)]
    pub last: Option<String>,
    pub commits: usize,
    /// The commit the remote branch points to; null when up to date.
    #[schema(required = true)]
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
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
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
    /// The fetched tip of the remote branch; null when the remote has no such branch.
    #[schema(required = true)]
    pub remote_commit: Option<String>,
    /// The revision the pull integration made; null unless integrated.
    #[schema(required = true, example = "r18")]
    pub revision: Option<String>,
    /// The sync task that resolves a conflict; null unless conflicted.
    #[schema(required = true, example = "T-12")]
    pub task: Option<String>,
    /// The sync task's workspace; null unless conflicted.
    #[schema(required = true, example = "ws-9")]
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
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
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

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecordState {
    /// The project folder holds no change that is not recorded.
    UpToDate,
    /// The human change merged cleanly into main as a new revision, and the project folder follows main.
    Recorded,
    /// The merge conflicted and main did not move; the main agent prepares the merge with the human.
    Conflicted,
}

/// A file of the project folder that fold leaves out, as an import does.
#[derive(Debug, Serialize, ToSchema)]
pub struct RecordSkipped {
    pub path: String,
    /// Why it is left out, such as `too_large`.
    pub reason: String,
}

/// An open task workspace whose own changes touch a file the human changed.
#[derive(Debug, Serialize, ToSchema)]
pub struct RecordTouched {
    #[schema(example = "T-12")]
    pub task: String,
    #[schema(example = "ws-9")]
    pub workspace: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RecordResult {
    pub outcome: RecordState,
    /// The Git commit of the new revision while Git sync is on; null otherwise.
    #[schema(required = true)]
    pub commit: Option<String>,
    /// Main's head: the new revision of a recorded change, else the head main stays on.
    #[schema(example = "r18")]
    pub revision: String,
    /// The files the human changed, in path order; empty unless recorded.
    pub files: Vec<String>,
    /// In path order; empty unless conflicted.
    pub conflicted: Vec<PullConflict>,
    pub touched: Vec<RecordTouched>,
    pub skipped: Vec<RecordSkipped>,
    /// False when the same conflicted change waited already and nothing was written.
    pub created: bool,
}

impl From<RecordOutcome> for RecordResult {
    fn from(outcome: RecordOutcome) -> Self {
        let touched = |touched: Vec<projects::TouchedTask>| {
            touched
                .into_iter()
                .map(|touched| RecordTouched {
                    task: touched.task.to_string(),
                    workspace: touched.workspace.to_string(),
                })
                .collect()
        };
        let skipped = |skipped: Vec<fold_vcs::scan::Skipped>| {
            skipped
                .into_iter()
                .map(|skipped| RecordSkipped {
                    path: skipped.path.to_string(),
                    reason: skipped.reason.as_str().to_owned(),
                })
                .collect()
        };
        match outcome {
            RecordOutcome::UpToDate { revision } => Self {
                outcome: RecordState::UpToDate,
                commit: None,
                revision: revision.to_string(),
                files: Vec::new(),
                conflicted: Vec::new(),
                touched: Vec::new(),
                skipped: Vec::new(),
                created: false,
            },
            RecordOutcome::Recorded {
                commit,
                revision,
                files,
                touched: found,
                skipped: left,
            } => Self {
                outcome: RecordState::Recorded,
                commit: commit.map(|commit| commit.to_string()),
                revision: revision.to_string(),
                files,
                conflicted: Vec::new(),
                touched: touched(found),
                skipped: skipped(left),
                created: true,
            },
            RecordOutcome::Conflicted {
                head,
                conflicted,
                touched: found,
                skipped: left,
                created,
            } => Self {
                outcome: RecordState::Conflicted,
                commit: None,
                revision: head.to_string(),
                files: Vec::new(),
                conflicted: conflicted
                    .into_iter()
                    .map(|conflict| PullConflict {
                        path: conflict.path,
                        kind: conflict.kind.as_str().to_owned(),
                    })
                    .collect(),
                touched: touched(found),
                skipped: skipped(left),
                created,
            },
        }
    }
}

/// Records the human's changes in the project folder on main, as `fld record` asks: what the folder holds against the entries fold last wrote there is one change of the human's, which a clean merge integrates with no review, and the folder follows main; a conflict leaves main as it is and waits in Needs you while the main agent prepares the merge. While Git sync is on, the human's Git commits on the branch are recorded first, and a project whose branch or HEAD shows a change fold did not make is refused until the human undoes it. Socket only; the human comes from `x-fold-human`, and a process under a worker fold started is refused.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/record",
    operation_id = "record_project",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "Main holds the human change, or it waits for the main agent", body = RecordResult),
        (status = 400, description = "The human name is not valid, the branch or HEAD shows a change fold did not make (`reason` `head_moved`), or files still hold the conflict of `fld unstash` (`reason` `stash_markers`, the files in `paths`)", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, or the slug is not registered", body = ErrorEnvelope),
        (status = 409, description = "Main moved during every attempt, or the config or sync map is invalid", body = ErrorEnvelope),
        (status = 502, description = "A Git command failed", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open, git is not on PATH, or the objects of the change could not be stored", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn record(
    _socket: SocketOnly,
    human: Human,
    Project(handle): Project,
) -> Result<Json<RecordResult>, ApiError> {
    let outcome = projects::record(&handle, &human.actor(), &Git::default()).await?;
    Ok(Json(outcome.into()))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StashState {
    /// The project folder holds no change that is not recorded; nothing was stashed.
    Nothing,
    /// The changes are the newest entry, `stash@{0}`, and the project folder follows main.
    Stashed,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StashResult {
    pub outcome: StashState,
    /// The main revision the project folder holds now.
    #[schema(example = "r18")]
    pub revision: String,
    /// The paths of the new entry, in path order; empty when nothing was stashed.
    pub files: Vec<String>,
    /// The entries on the stack now.
    pub entries: u32,
    /// New files the read of the folder left out, such as one over the size limit: they stay in the folder and are not in the entry.
    pub skipped: Vec<RecordSkipped>,
}

impl From<StashOutcome> for StashResult {
    fn from(outcome: StashOutcome) -> Self {
        let skipped = |skipped: Vec<fold_vcs::scan::Skipped>| {
            skipped
                .into_iter()
                .map(|skipped| RecordSkipped {
                    path: skipped.path.to_string(),
                    reason: skipped.reason.as_str().to_owned(),
                })
                .collect()
        };
        match outcome {
            StashOutcome::Nothing {
                revision,
                entries,
                skipped: left,
            } => Self {
                outcome: StashState::Nothing,
                revision: revision.to_string(),
                files: Vec::new(),
                entries: u32::try_from(entries).unwrap_or(u32::MAX),
                skipped: skipped(left),
            },
            StashOutcome::Stashed {
                revision,
                files,
                entries,
                skipped: left,
            } => Self {
                outcome: StashState::Stashed,
                revision: revision.to_string(),
                files,
                entries: u32::try_from(entries).unwrap_or(u32::MAX),
                skipped: skipped(left),
            },
        }
    }
}

/// Puts the human's changes in the project folder aside, as `fld stash` asks: what the folder holds against the entries fold last wrote there becomes the newest entry of fold's own stash, those paths go back to fold's entries, and the folder follows main. While Git sync is on, the index entries of those paths go back too; no Git stash is made. A conflicted human change from an earlier `fld record` that waits for the main agent is closed with `human_change.withdrawn`, since the entry holds its changes. A project whose branch or HEAD shows a change fold did not make is refused, and so is one whose unstash conflict waits or that holds a file fold tracks and cannot read. Socket only; the human comes from `x-fold-human`.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/stash",
    operation_id = "stash_project",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The changes are stashed, or there were none", body = StashResult),
        (status = 400, description = "The human name is not valid; the branch or HEAD shows a change fold did not make (`reason` `head_moved`); the human's Git commits wait for the main agent (`reason` `git_commits_wait`); an unstash conflict waits, or a stop left an unstash's markers in the files, which keeps its entry as conflicted (`reason` `stash_conflicted`); a file fold tracks is too large or unreadable to stash (`reason` `skipped_tracked`, the files in `paths`); or files changed while fold stashed them (`reason` `changed_during_stash`)", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, or the slug is not registered", body = ErrorEnvelope),
        (status = 409, description = "The config or sync map is invalid", body = ErrorEnvelope),
        (status = 502, description = "A Git command failed", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open, git is not on PATH, or the objects of the entry could not be stored", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn stash(
    _socket: SocketOnly,
    human: Human,
    Project(handle): Project,
) -> Result<Json<StashResult>, ApiError> {
    let outcome = projects::stash(&handle, &human.actor(), &Git::default()).await?;
    Ok(Json(outcome.into()))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnstashState {
    /// The newest entry merged cleanly into the project folder and left the stack; its files are changes that no revision records again.
    Applied,
    /// The merge conflicted: the folder holds conflict markers, or the stash's side beside a file, and the entry stays until `fld record` finds them fixed.
    Conflicted,
}

/// One path where the unstash conflicted with the project folder.
#[derive(Debug, Serialize, ToSchema)]
pub struct UnstashConflictView {
    pub path: String,
    /// `both_modified_text`, `both_modified_binary`, `modify_delete`, `add_add` or `type_change`.
    pub kind: String,
    /// The file or directory beside the path that holds the stash's side of a conflict that takes no markers; null for a text conflict, which holds markers.
    #[schema(required = true, example = "logo.png.fold-stash")]
    pub sidecar: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UnstashResult {
    pub outcome: UnstashState,
    /// The main revision the project folder holds, which the conflict markers name; null when the merge was clean.
    #[schema(required = true, example = "r18")]
    pub revision: Option<String>,
    /// The paths of the entry, in path order.
    pub files: Vec<String>,
    /// In path order; empty unless conflicted.
    pub conflicted: Vec<UnstashConflictView>,
    /// The entries on the stack now.
    pub entries: u32,
}

impl From<UnstashOutcome> for UnstashResult {
    fn from(outcome: UnstashOutcome) -> Self {
        let count = |entries: usize| u32::try_from(entries).unwrap_or(u32::MAX);
        match outcome {
            UnstashOutcome::Applied { files, entries } => Self {
                outcome: UnstashState::Applied,
                revision: None,
                files,
                conflicted: Vec::new(),
                entries: count(entries),
            },
            UnstashOutcome::Conflicted {
                revision,
                files,
                conflicted,
                entries,
            } => Self {
                outcome: UnstashState::Conflicted,
                revision: Some(revision.to_string()),
                files,
                conflicted: conflicted
                    .into_iter()
                    .map(|conflict| UnstashConflictView {
                        path: conflict.path,
                        kind: conflict.kind.as_str().to_owned(),
                        sidecar: conflict.sidecar,
                    })
                    .collect(),
                entries: count(entries),
            },
        }
    }
}

/// Merges the newest entry of the stash into the project folder as it is now, as `fld unstash` asks, with the human's newer unrecorded changes. A clean merge writes the files, which are changes that no revision records again, and removes the entry. A conflict writes conflict markers labelled `main (r<n>)` and `stash@{0}`, or the stash's side as `<path>.fold-stash`, keeps the entry, and holds the folder until `fld record` finds the files fixed. Socket only; the human comes from `x-fold-human`.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/unstash",
    operation_id = "unstash_project",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The entry is applied, or it conflicted and stays", body = UnstashResult),
        (status = 400, description = "The human name is not valid; the stash is empty (`reason` `stash_empty`); an unstash conflict waits (`reason` `stash_conflicted`); files still hold the markers of an earlier unstash (`reason` `stash_markers`), which keeps the entry as conflicted; the folder holds a file fold leaves out, such as an ignored one, where the unstash writes (`reason` `unstash_path_occupied`, the files in `paths`); files changed while fold read them (`reason` `changed_during_unstash`); the branch or HEAD shows a change fold did not make (`reason` `head_moved`); or the human's Git commits wait (`reason` `git_commits_wait`)", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, or the slug is not registered", body = ErrorEnvelope),
        (status = 409, description = "The config or sync map is invalid", body = ErrorEnvelope),
        (status = 502, description = "A Git command failed", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open, or git is not on PATH", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn unstash(
    _socket: SocketOnly,
    human: Human,
    Project(handle): Project,
) -> Result<Json<UnstashResult>, ApiError> {
    let outcome = projects::unstash(&handle, &human.actor(), &Git::default()).await?;
    Ok(Json(outcome.into()))
}

/// One entry of the stash.
#[derive(Debug, Serialize, ToSchema)]
pub struct StashEntryView {
    /// Its name on the stack: `stash@{0}` is the newest.
    #[schema(example = "stash@{0}")]
    pub stash: String,
    /// UTC milliseconds of the stash.
    #[schema(value_type = i64)]
    pub created_at: Timestamp,
    /// The main revision the project folder held then.
    #[schema(example = "r8")]
    pub base: String,
    /// Who stashed, such as `human:ana`.
    pub actor: String,
    /// The paths of the entry, in path order.
    pub files: Vec<String>,
    /// The paths whose unstash conflict waits in the folder; empty otherwise.
    pub conflicted: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StashList {
    /// Newest first.
    pub entries: Vec<StashEntryView>,
}

/// The stash of the project folder, newest entry first, as `fld stash list` shows it. Socket only.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/stash",
    operation_id = "list_project_stash",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The entries", body = StashList),
        (status = 404, description = "Not on the socket, or the slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_stash(
    _socket: SocketOnly,
    Project(handle): Project,
) -> Result<Json<StashList>, ApiError> {
    let entries = projects::stash_list(&handle).await?;
    Ok(Json(StashList {
        entries: entries
            .into_iter()
            .map(|entry| StashEntryView {
                stash: format!("stash@{{{}}}", entry.position),
                created_at: entry.created_at,
                base: entry.base.to_string(),
                actor: entry.actor,
                files: entry.files,
                conflicted: entry.conflicted,
            })
            .collect(),
    }))
}

/// The Git identity for the project's own Git config, as `fld init --with-git` settled it; a value left out is not written.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct EnableGit {
    /// `user.name` for the repository's own config; no control character, `<` or `>`.
    #[schema(required = false, example = "Ana Lima")]
    pub git_name: Option<String>,
    /// `user.email` for the repository's own config; no control character, `<` or `>`.
    #[schema(required = false, example = "ana@example.com")]
    pub git_email: Option<String>,
}

/// What turning Git on did.
#[derive(Debug, Serialize, ToSchema)]
pub struct GitEnabledView {
    /// Main's head.
    #[schema(example = "r4")]
    pub revision: String,
    /// The revision that records the `.gitignore` fold wrote or changed; null when it changed none.
    #[schema(required = true, example = "r4")]
    pub gitignore: Option<String>,
    /// The commit the Git branch points to; null while main holds only the empty genesis.
    #[schema(required = true)]
    pub commit: Option<String>,
    /// The configured remotes, by name.
    pub remotes: Vec<String>,
}

/// Turns Git on in a project whose Git sync is off, as `fld init --with-git` asks in its folder: fold creates `.git` when it is missing, writes the identity into its config, records the `.gitignore` copy rule as the human's revision, turns Git sync on with `git.sync_enabled` and writes the commit of every revision after the empty genesis, with the branch and the index on the newest. Git sync never goes off again. Socket only; the human comes from `x-fold-human`.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/git",
    operation_id = "enable_project_git",
    params(("slug" = String, Path, description = "The project slug")),
    request_body = EnableGit,
    responses(
        (status = 200, description = "Git sync is on", body = GitEnabledView),
        (status = 400, description = "Git sync is on already (`reason` `git_sync_on`), the folder holds a `.git` with commits of its own (`reason` `git_history`) or a `.git` that is no directory (`reason` `git_dir_elsewhere`), or `git_name` or `git_email` cannot go into an author line", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`: the request names no human", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, or the slug is not registered", body = ErrorEnvelope),
        (status = 502, description = "A Git command failed", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open, or git is not on PATH", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn enable_git(
    _socket: SocketOnly,
    human: Human,
    Project(handle): Project,
    ApiJson(body): ApiJson<EnableGit>,
) -> Result<Json<GitEnabledView>, ApiError> {
    let identity = projects::GitIdentity {
        name: body.git_name,
        email: body.git_email,
    };
    let enabled = projects::enable_git(&handle, &human.actor(), &Git::default(), identity).await?;
    Ok(Json(GitEnabledView {
        revision: enabled.head.to_string(),
        gitignore: enabled.gitignore.map(|revision| revision.to_string()),
        commit: enabled.commit.map(|commit| commit.to_string()),
        remotes: enabled.remotes,
    }))
}
