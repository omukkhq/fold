use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Args;
use fold_cli::daemon_client::{self, ClientConfig, ClientError, DaemonClient, Response};
use fold_core::{AgentName, FoldError, MAIN_TAG, codes};
use fold_daemon::services::projects::{ProjectError, root_for_dir_on_host, slug_for_dir_on_host};
use hyper::Method;
use thiserror::Error;

use crate::init::host_paths;
use crate::serve::{self, Dirs};

#[derive(Args, Debug)]
pub struct ClaudeArgs {
    /// The agent name of the session, which is also its Claude Code name
    #[arg(long, default_value = MAIN_TAG)]
    name: String,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Arguments for claude itself, after `--`
    #[arg(last = true)]
    claude_args: Vec<OsString>,
}

#[derive(Debug, Error)]
pub enum ClaudeError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Config(#[from] serve::ServeError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error("cannot read the working directory: {0}")]
    Cwd(io::Error),
    #[error("cannot find the fld binary: {0}")]
    Exe(io::Error),
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("{message}")]
    Refused {
        code: String,
        message: String,
        hint: Option<String>,
    },
    #[error("cannot run claude: {0}")]
    Exec(io::Error),
}

impl ClaudeError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Fold(error) => Some(error.code()),
            Self::Config(error) => error.code(),
            Self::Project(error) => error.code(),
            Self::Refused { code, .. } => Some(code),
            Self::Cwd(_) | Self::Exe(_) | Self::Runtime(_) | Self::Client(_) | Self::Exec(_) => {
                None
            }
        }
    }

    /// The hint that names the next step.
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Project(_) if self.code() == Some(codes::PROJECT_NOT_FOUND) => {
                Some("run fld claude inside a fold project, or run fld init there first")
            }
            Self::Fold(error) | Self::Project(ProjectError::Fold(error)) => error.hint(),
            Self::Refused { hint, .. } => hint.as_deref(),
            Self::Exec(_) => Some("install Claude Code, so that claude is on your PATH"),
            Self::Config(_)
            | Self::Project(_)
            | Self::Cwd(_)
            | Self::Exe(_)
            | Self::Runtime(_)
            | Self::Client(_) => None,
        }
    }
}

/// Starts the daemon when it is not running, then replaces this process with the user's own `claude`, an interactive session in the project root that is the main agent. `FOLD_AGENT` and `--name` give the session its agent name and its messaging socket, and `FOLD_AGENT_TAGS` makes `fld mcp` register it with the `main` tag. It writes no Claude Code settings. It returns only when it could not start `claude`.
pub fn run(args: &ClaudeArgs, dirs: &Dirs) -> Result<(), ClaudeError> {
    let agent = AgentName::parse(&args.name).map_err(FoldError::from)?;
    let host = host_paths(dirs)?;
    let cwd = std::env::current_dir()
        .and_then(|dir| dir.canonicalize())
        .map_err(ClaudeError::Cwd)?;
    let config = client_config(args.socket.as_deref(), dirs)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(ClaudeError::Runtime)?;
    let root = runtime.block_on(async {
        let root = root_for_dir_on_host(&host, &cwd).await?;
        let slug = slug_for_dir_on_host(&host, &cwd).await?;
        daemon_client::connect(&config).await?;
        if let Ok(human) = daemon_client::human_from_env() {
            let client = DaemonClient::new(config, Some(human));
            let path = format!("/api/v1/projects/{slug}/agents");
            main_is_free(&client.request(Method::GET, &path, None).await?, &agent)?;
        }
        Ok::<_, ClaudeError>(root)
    })?;
    drop(runtime);
    let error = command(Path::new("claude"), &root, &agent, &args.claude_args).exec();
    Err(ClaudeError::Exec(error))
}

/// The `claude` run of `fld claude`: its own arguments come after `--name`.
fn command(claude: &Path, root: &Path, agent: &AgentName, extra: &[OsString]) -> Command {
    let mut command = Command::new(claude);
    command
        .arg("--name")
        .arg(agent.as_str())
        .args(extra)
        .current_dir(root)
        .env("FOLD_AGENT", agent.as_str())
        .env("FOLD_AGENT_TAGS", MAIN_TAG);
    command
}

fn client_config(socket: Option<&Path>, dirs: &Dirs) -> Result<ClientConfig, ClaudeError> {
    let socket = serve::socket_path(socket, dirs)?.ok_or(ClientError::RuntimeDirMissing)?;
    Ok(ClientConfig {
        socket: std::path::absolute(&socket).map_err(ClaudeError::Cwd)?,
        runtime_dir: serve::runtime_dir(dirs).ok(),
        fold_exe: Some(std::env::current_exe().map_err(ClaudeError::Exe)?),
    })
}

/// Refuses before `claude` starts when another agent already has the `main` tag, which the session's adapter would get at its handshake.
fn main_is_free(response: &Response, agent: &AgentName) -> Result<(), ClaudeError> {
    if !(200..300).contains(&response.status) {
        let error = &response.body["error"];
        let text = |key: &str| error[key].as_str().map(str::to_owned);
        return Err(ClaudeError::Refused {
            code: text("code").unwrap_or_else(|| codes::INTERNAL_ERROR.to_owned()),
            message: text("message").unwrap_or_else(|| "the daemon refused the request".to_owned()),
            hint: text("hint"),
        });
    }
    let holder = response.body["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| {
            row["tags"]
                .as_array()
                .is_some_and(|tags| tags.iter().any(|tag| tag == MAIN_TAG))
        })
        .and_then(|row| row["name"].as_str());
    match holder {
        Some(holder) if holder != agent.as_str() => Err(ClaudeError::Refused {
            code: codes::MAIN_TAG_DENIED.to_owned(),
            message: format!("agent {holder} is the main agent of this project"),
            hint: Some(format!("run fld claude --name {holder} to continue as it")),
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use serde_json::json;

    use super::*;

    #[derive(Parser, Debug)]
    struct Wrapper {
        #[command(flatten)]
        args: ClaudeArgs,
    }

    fn parse(args: &[&str]) -> Result<Wrapper, clap::Error> {
        Wrapper::try_parse_from(std::iter::once("claude").chain(args.iter().copied()))
    }

    #[test]
    fn the_name_defaults_to_main_and_the_rest_goes_to_claude() {
        let parsed = parse(&[]).unwrap().args;
        assert_eq!(parsed.name, "main");
        assert!(parsed.claude_args.is_empty());
        let parsed = parse(&["--name", "lead", "--", "--model", "opus", "--name"])
            .unwrap()
            .args;
        assert_eq!(parsed.name, "lead");
        assert_eq!(parsed.claude_args, ["--model", "opus", "--name"]);
        assert!(parse(&["--model"]).is_err());
    }

    #[test]
    fn claude_runs_in_the_root_with_the_name_and_the_main_tag() {
        let agent = AgentName::parse("main").unwrap();
        let command = command(
            Path::new("claude"),
            Path::new("/src/app"),
            &agent,
            &[OsString::from("--model"), OsString::from("opus")],
        );
        assert_eq!(command.get_program(), "claude");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["--name", "main", "--model", "opus"]);
        assert_eq!(command.get_current_dir(), Some(Path::new("/src/app")));
        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.contains(&("FOLD_AGENT".as_ref(), Some("main".as_ref()))));
        assert!(envs.contains(&("FOLD_AGENT_TAGS".as_ref(), Some("main".as_ref()))));
    }

    #[test]
    fn another_main_agent_refuses_the_start_and_the_same_one_continues() {
        let agent = AgentName::parse("main").unwrap();
        let agents = |name: &str| Response {
            status: 200,
            body: json!({"agents": [
                {"name": "backend", "tags": ["review"]},
                {"name": name, "tags": ["main"]},
            ]}),
        };
        assert!(main_is_free(&agents("main"), &agent).is_ok());
        let none = Response {
            status: 200,
            body: json!({"agents": []}),
        };
        assert!(main_is_free(&none, &agent).is_ok());
        let taken = main_is_free(&agents("lead"), &agent).unwrap_err();
        assert_eq!(taken.code(), Some(codes::MAIN_TAG_DENIED));
        assert_eq!(
            taken.to_string(),
            "agent lead is the main agent of this project"
        );
        assert_eq!(
            taken.hint(),
            Some("run fld claude --name lead to continue as it")
        );
        let refused = Response {
            status: 404,
            body: json!({"error": {"code": "project_not_found", "message": "gone", "hint": "fld init"}}),
        };
        let error = main_is_free(&refused, &agent).unwrap_err();
        assert_eq!(error.code(), Some("project_not_found"));
        assert_eq!(error.hint(), Some("fld init"));
        let bare = Response {
            status: 500,
            body: serde_json::Value::Null,
        };
        let error = main_is_free(&bare, &agent).unwrap_err();
        assert_eq!(error.code(), Some(codes::INTERNAL_ERROR));
        assert_eq!(error.hint(), None);
    }

    #[test]
    fn errors_name_their_code_and_hint() {
        let outside = ClaudeError::Project(ProjectError::Fold(FoldError::new(
            codes::PROJECT_NOT_FOUND,
            "no registered project holds this directory",
        )));
        assert_eq!(outside.code(), Some("project_not_found"));
        assert!(outside.hint().unwrap().contains("fld init"));
        let exec = ClaudeError::Exec(io::Error::from(io::ErrorKind::NotFound));
        assert_eq!(exec.code(), None);
        assert!(exec.hint().unwrap().contains("Claude Code"));
        assert_eq!(ClaudeError::Runtime(io::Error::other("x")).hint(), None);
        let bad = ClaudeError::Fold(FoldError::new(codes::INVALID_NAME, "bad").with_hint("pick"));
        assert_eq!(bad.code(), Some("invalid_name"));
        assert_eq!(bad.hint(), Some("pick"));
    }
}
