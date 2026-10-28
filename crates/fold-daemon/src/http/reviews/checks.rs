use axum::Json;
use axum::extract::{Path, State};
use fold_core::CheckRunId;
use fold_core::reviews::checks::{Outcome, Purpose, RunReason, RunState, Stage};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::super::Project;
use super::super::error::{ApiError, ErrorEnvelope};
use super::super::tasks::id;
use super::service;
use crate::daemon::AppState;
use crate::services::reviews::checks::{CheckRunView, VersionRun};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_check_run))
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

/// What asked for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckPurpose {
    Check,
    CreateProposal,
    UpdateProposal,
    Integrate,
    ReviewFix,
}

impl From<Purpose> for CheckPurpose {
    fn from(purpose: Purpose) -> Self {
        match purpose {
            Purpose::Check => Self::Check,
            Purpose::CreateProposal => Self::CreateProposal,
            Purpose::UpdateProposal => Self::UpdateProposal,
            Purpose::Integrate => Self::Integrate,
            Purpose::ReviewFix => Self::ReviewFix,
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
