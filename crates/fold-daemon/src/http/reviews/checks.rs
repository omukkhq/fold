use axum::Json;
use axum::extract::{Path, State};
use fold_core::reviews::checks::{Outcome, RunReason, RunState, Stage};
use fold_core::{CheckRunId, TaskId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::super::Project;
use super::super::actor::Human;
use super::super::error::{ApiError, ApiJson, ErrorEnvelope};
use super::super::tasks::id;
use super::service;
use crate::daemon::AppState;
pub use crate::live::CheckPurpose;
use crate::services::reviews::checks::{CancelTarget, CheckRunView, VersionRun};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_check_run))
        .routes(routes!(cancel_check_runs))
}

/// When a check runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckStage {
    /// Before fold makes a proposal version, in the worker's task workspace.
    Proposal,
    /// On the merged tree before an integration reaches main, in fold's check directory.
    Integration,
}

impl From<Stage> for CheckStage {
    fn from(stage: Stage) -> Self {
        match stage {
            Stage::Proposal => Self::Proposal,
            Stage::Integration => Self::Integration,
        }
    }
}

/// The run in progress that makes a proposal's next version when its checks pass. While it goes, the current version is neither approved nor integrated (`update_checks_running`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct NextVersionRun {
    #[schema(example = "cr-4")]
    pub run: String,
    /// `update_proposal` for the author's update, `review_fix` for a reviewer's fix in a review checkout.
    pub purpose: CheckPurpose,
}

impl From<VersionRun> for NextVersionRun {
    fn from(run: VersionRun) -> Self {
        Self {
            run: run.run.to_string(),
            purpose: run.purpose.into(),
        }
    }
}

/// Where a run stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckRunState {
    Running,
    Passed,
    Failed,
    /// The run ended without a verdict, as when the daemon stopped during it.
    Stopped,
}

impl From<RunState> for CheckRunState {
    fn from(state: RunState) -> Self {
        match state {
            RunState::Running => Self::Running,
            RunState::Passed => Self::Passed,
            RunState::Failed => Self::Failed,
            RunState::Stopped => Self::Stopped,
        }
    }
}

/// Why a run failed or stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckRunReason {
    CheckFailed,
    SetupFailed,
    ChecksFileInvalid,
    /// The checks or an edit changed recorded files during the run.
    FilesChanged,
    WorkspaceChanged,
    Refused,
    DaemonStopped,
    Superseded,
    /// fold itself could not run the checks; the integration can be asked again once the fault is fixed.
    FoldFailed,
    /// A signal that fold did not send ended a command, such as the kernel's OOM killer; no later command of the run started.
    Killed,
    /// A command waited longer than `[runs] build_slot_wait` for a slot of the machine's build gate; no later command of the run started, and the code was not at fault.
    NoBuildSlot,
    /// A worker, a lead, the main agent or a human cancelled the run; `checks.finished` names who.
    Cancelled,
}

impl From<RunReason> for CheckRunReason {
    fn from(reason: RunReason) -> Self {
        match reason {
            RunReason::CheckFailed => Self::CheckFailed,
            RunReason::SetupFailed => Self::SetupFailed,
            RunReason::ChecksFileInvalid => Self::ChecksFileInvalid,
            RunReason::FilesChanged => Self::FilesChanged,
            RunReason::WorkspaceChanged => Self::WorkspaceChanged,
            RunReason::Refused => Self::Refused,
            RunReason::DaemonStopped => Self::DaemonStopped,
            RunReason::Superseded => Self::Superseded,
            RunReason::FoldFailed => Self::FoldFailed,
            RunReason::Killed => Self::Killed,
            RunReason::NoBuildSlot => Self::NoBuildSlot,
            RunReason::Cancelled => Self::Cancelled,
        }
    }
}

/// What one command did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    Passed,
    Failed,
    TimedOut,
    /// Its `files` pattern matched no changed path.
    Skipped,
    /// The setup failed first.
    NotRun,
}

impl From<Outcome> for CheckOutcome {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Passed => Self::Passed,
            Outcome::Failed => Self::Failed,
            Outcome::TimedOut => Self::TimedOut,
            Outcome::Skipped => Self::Skipped,
            Outcome::NotRun => Self::NotRun,
        }
    }
}

/// One command of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CheckResult {
    /// The check's name, or `setup` for the setup command.
    pub name: String,
    pub setup: bool,
    /// A required command that fails fails the run; one that is not required only shows.
    pub required: bool,
    pub outcome: CheckOutcome,
    #[schema(required = true)]
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// The size of the command's whole output.
    pub log_bytes: u64,
    /// The stored end of the output, as text. Empty on a proposal view, which leaves the logs out.
    pub log: String,
    /// The stored log is the end of a longer output.
    pub truncated: bool,
}

/// One run of a project's checks by fold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CheckRun {
    #[schema(example = "cr-4")]
    pub id: String,
    pub stage: CheckStage,
    pub purpose: CheckPurpose,
    #[schema(example = "T-12")]
    pub task: String,
    /// The version an integration run checks.
    #[schema(required = true)]
    pub version: Option<u32>,
    /// The revision the checked tree is based on: the workspace's base, or main's head for an integration.
    #[schema(example = "r17")]
    pub base: String,
    pub state: CheckRunState,
    #[schema(required = true)]
    pub reason: Option<CheckRunReason>,
    /// UTC milliseconds.
    pub created_at: i64,
    #[schema(required = true)]
    pub finished_at: Option<i64>,
    /// The review batch whose members an integration run checked together; the run shows on each member's page.
    #[schema(required = true, example = "rb-2")]
    pub batch: Option<String>,
    /// In run order: the setup first, then each check of the stage.
    pub results: Vec<CheckResult>,
    /// UTC milliseconds since the running run holds its permit and waits for free memory before its commands start. Absent while it does not wait.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_for_memory_since: Option<i64>,
    /// UTC milliseconds since a command of the running run waits for a slot of the machine's build gate; its timeout starts once it has one. Absent while it does not wait.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_for_build_slot_since: Option<i64>,
    /// The public name of the actor that cancelled the run, such as `human:ana` or `daemon-lead`, for a run that ended with reason `cancelled`; null otherwise.
    #[schema(example = "human:ana")]
    pub cancelled_by: Option<String>,
}

impl From<CheckRunView> for CheckRun {
    fn from(view: CheckRunView) -> Self {
        let run = view.run;
        Self {
            id: run.id.to_string(),
            stage: run.stage.into(),
            purpose: run.purpose.into(),
            task: run.task.to_string(),
            version: run.version,
            base: run.base.to_string(),
            state: run.state.into(),
            reason: run.reason.map(CheckRunReason::from),
            created_at: run.created_at.as_millis(),
            finished_at: run.finished_at.map(|at| at.as_millis()),
            batch: run.batch.map(|batch| batch.to_string()),
            waiting_for_memory_since: view.waiting_for_memory.map(|since| since.as_millis()),
            waiting_for_build_slot_since: view
                .waiting_for_build_slot
                .map(|since| since.as_millis()),
            cancelled_by: view.cancelled_by,
            results: view
                .results
                .into_iter()
                .map(|result| CheckResult {
                    setup: result.check.is_none(),
                    name: result.check.unwrap_or_else(|| "setup".to_owned()),
                    required: result.required,
                    outcome: result.outcome.into(),
                    exit_code: result.exit_code,
                    duration_ms: result.duration_ms,
                    log_bytes: result.log_bytes,
                    log: String::from_utf8_lossy(&result.log).into_owned(),
                    truncated: result.truncated,
                })
                .collect(),
        }
    }
}

/// One check run with the stored end of each command's output.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/check-runs/{id}",
    operation_id = "get_check_run",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The check run id, such as cr-4"),
    ),
    responses(
        (status = 200, description = "The run", body = CheckRun),
        (status = 400, description = "`invalid_id`", body = ErrorEnvelope),
        (status = 404, description = "`check_run_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn get_check_run(
    State(state): State<AppState>,
    project: Project,
    Path((_slug, run)): Path<(String, String)>,
) -> Result<Json<CheckRun>, ApiError> {
    let run: CheckRunId = id(&run)?;
    let view = service(&state, &project).get_check_run(run, true).await?;
    Ok(Json(view.into()))
}

/// The running check runs to cancel: give exactly one of `run`, `task` and `all`.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CancelCheckRunsBody {
    /// One running run, such as cr-4.
    #[serde(default)]
    #[schema(example = "cr-4")]
    pub run: Option<String>,
    /// A task, such as T-12: every running run of its task workspace and review checkouts.
    #[serde(default)]
    #[schema(example = "T-12")]
    pub task: Option<String>,
    /// True for every running run of the project.
    #[serde(default)]
    pub all: bool,
}

/// The runs a cancel ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CancelledCheckRuns {
    /// Each run that ended as failed with reason `cancelled`, oldest first; empty when the task or the project had no running run.
    #[schema(example = json!(["cr-4"]))]
    pub cancelled: Vec<String>,
}

/// Cancels running check runs: the running command's process group ends, no later command starts, and each run ends as failed with reason `cancelled`. Integration runs are not cancelled: a run id of one is `invalid_request` with `details.reason` `integration_run`, and `task` and `all` leave them out.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/check-runs/cancel",
    operation_id = "cancel_check_runs",
    params(
        ("slug" = String, Path, description = "The project slug"),
    ),
    request_body = CancelCheckRunsBody,
    responses(
        (status = 200, description = "The cancelled runs", body = CancelledCheckRuns),
        (status = 400, description = "`invalid_request`: not exactly one of `run`, `task` and `all`, or `details.reason` `integration_run`; or `invalid_id`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`check_run_not_found`, `task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`check_run_ended`: the run ended before the cancel", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn cancel_check_runs(
    State(state): State<AppState>,
    human: Human,
    project: Project,
    ApiJson(body): ApiJson<CancelCheckRunsBody>,
) -> Result<Json<CancelledCheckRuns>, ApiError> {
    let target = match (body.run.as_deref(), body.task.as_deref(), body.all) {
        (Some(run), None, false) => CancelTarget::Run(id::<CheckRunId>(run)?),
        (None, Some(task), false) => CancelTarget::Task(id::<TaskId>(task)?),
        (None, None, true) => CancelTarget::All,
        _ => {
            return Err(ApiError::invalid_request(
                "give exactly one of run, task and all",
            ));
        }
    };
    let cancelled = service(&state, &project)
        .cancel_check_runs(&human.actor(), target)
        .await?;
    Ok(Json(CancelledCheckRuns {
        cancelled: cancelled.iter().map(ToString::to_string).collect(),
    }))
}
