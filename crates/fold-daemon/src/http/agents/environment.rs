use axum::Json;
use axum::extract::State;
use serde::Serialize;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::daemon::AppState;
use crate::http::SocketOnly;
use crate::http::error::{ApiError, ErrorEnvelope};
use crate::services::agents::environment::{EnvSource, ShellProblem, ask_claude};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(worker_environment))
}

/// Where the workers' environment came from.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkerEnvironmentSource {
    /// The user's login shell, read once at the daemon's start.
    Shell,
    /// The daemon's own environment, which the service of `fld setup` gives it.
    Setup,
    /// The daemon's own environment, from whatever started it.
    Daemon,
}

/// Why the daemon did not take the login shell's environment.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShellProblemKind {
    /// No login shell is known: neither `SHELL` nor the password file names one.
    NoShell,
    /// The shell could not start, failed or printed no environment.
    Failed,
    /// The shell did not finish within the time limit.
    TimedOut,
    /// The shell's `PATH` holds no `claude`.
    NoClaude,
}

/// The environment in which the daemon starts Claude Code for its workers, as it found it at its start, with the answers of that `claude`. It names paths and variable names, never a variable's value.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkerEnvironmentView {
    pub source: WorkerEnvironmentSource,
    /// Why the login shell's environment was not used; null when it was.
    pub shell_problem: Option<ShellProblemKind>,
    /// The absolute path of the `claude` that workers run; null when the `PATH` of the workers' environment holds none.
    pub claude: Option<String>,
    /// The absolute path of the `git` on that `PATH`; null when it holds none.
    pub git: Option<String>,
    /// The version that `claude --version` printed, such as `2.1.301`; null when it printed none fold can read.
    pub version: Option<String>,
    /// Whether `claude auth status` says Claude Code is signed in; null when it gave no answer fold can read.
    pub signed_in: Option<bool>,
    /// The sorted names of the variables of the workers' environment that decide how Claude Code signs in, such as `HOME`, `CLAUDE_CONFIG_DIR` and `ANTHROPIC_API_KEY`.
    pub sign_in_variables: Vec<String>,
}

/// The environment of the workers and the answers of their `claude`, which the daemon runs with `--version` and `auth status`, never with a prompt. Socket only, so the paths of the user's home never reach the network.
#[utoipa::path(
    get,
    path = "/api/v1/worker-environment",
    operation_id = "worker_environment",
    responses(
        (status = 200, description = "The workers' environment", body = WorkerEnvironmentView),
        (status = 404, description = "Not on the socket", body = ErrorEnvelope),
    )
)]
pub async fn worker_environment(
    _socket: SocketOnly,
    State(state): State<AppState>,
) -> Result<Json<WorkerEnvironmentView>, ApiError> {
    let environment = state.projects.environment();
    let answers = match environment.claude {
        Some(_) => ask_claude(state.projects.launcher()).await,
        None => Default::default(),
    };
    let path = |path: &Option<std::path::PathBuf>| {
        path.as_ref()
            .map(|path| path.to_string_lossy().into_owned())
    };
    Ok(Json(WorkerEnvironmentView {
        source: match environment.source {
            EnvSource::Shell => WorkerEnvironmentSource::Shell,
            EnvSource::Setup => WorkerEnvironmentSource::Setup,
            EnvSource::Daemon => WorkerEnvironmentSource::Daemon,
        },
        shell_problem: environment.shell_problem.map(|problem| match problem {
            ShellProblem::NoShell => ShellProblemKind::NoShell,
            ShellProblem::Failed => ShellProblemKind::Failed,
            ShellProblem::TimedOut => ShellProblemKind::TimedOut,
            ShellProblem::NoClaude => ShellProblemKind::NoClaude,
        }),
        claude: path(&environment.claude),
        git: path(&environment.git),
        version: answers.version.map(|version| version.to_string()),
        signed_in: answers.signed_in,
        sign_in_variables: environment.sign_in.clone(),
    }))
}
