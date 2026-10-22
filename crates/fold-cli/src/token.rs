use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Args, Subcommand};
use fold_cli::daemon_client::DaemonClient;
use fold_core::{Clock, FoldError, HumanName, WebTokenId};
use fold_daemon::SystemClock;
use fold_daemon::services::projects::{
    ProjectError, list_web_tokens_on_host, revoke_web_token_on_host,
};
use hyper::Method;
use serde_json::json;
use thiserror::Error;

use crate::commands::remote::{RemoteError, refusal};
use crate::init::host_paths;
use crate::output;
use crate::serve::{self, Dirs, current_thread_runtime};

#[derive(Args, Debug)]
pub struct TokenArgs {
    #[command(subcommand)]
    command: TokenCommand,
}

#[derive(Subcommand, Debug)]
enum TokenCommand {
    /// Create a web sign-in token for a human, who is created when absent, and print it once; the daemon makes it, and starts when needed
    Create(CreateArgs),
    /// List the unrevoked web tokens: id, human, creation and last use, never the token
    List,
    /// Revoke a web token and end every web session made from it
    Revoke(RevokeArgs),
}

#[derive(Args, Debug)]
struct CreateArgs {
    /// The human the token signs in
    human: String,
    /// A note that tells this token apart from the human's other tokens
    #[arg(long)]
    label: Option<String>,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct RevokeArgs {
    /// The token id that `fld token list` shows, such as wt-3
    id: String,
}

#[derive(Debug, Error)]
pub enum TokenError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    Daemon(#[from] RemoteError),
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error("cannot write the output: {0}")]
    Write(#[from] io::Error),
}

impl TokenError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Fold(error) => Some(error.code()),
            Self::Project(error) => error.code(),
            Self::Daemon(error) => error.code(),
            Self::Runtime(_) | Self::Write(_) => None,
        }
    }

    /// The hint that names the next step, when a refusal carries one.
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Fold(error) | Self::Project(ProjectError::Fold(error)) => error.hint(),
            Self::Daemon(error) => error.hint(),
            _ => None,
        }
    }
}

/// `create` goes through the daemon, which refuses a caller in the process tree of a worker or a check, the way it refuses one that names a human; a process that cannot open the registry file can then not make a token either. `list` and `revoke` read and write the registry directly, the way `fld init` does, so they work while the daemon is down; a running daemon reads `web_tokens` and `web_sessions` on every check, so a revoked token stops at once.
pub fn run(args: &TokenArgs, dirs: &Dirs, out: &mut impl Write) -> Result<(), TokenError> {
    let runtime = current_thread_runtime().map_err(TokenError::Runtime)?;
    runtime.block_on(command(&args.command, dirs, &SystemClock, out))
}

/// Sends `POST /api/v1/tokens` over the socket, naming the token's human, and returns the token.
async fn create(args: &CreateArgs, dirs: &Dirs) -> Result<String, TokenError> {
    let human = HumanName::parse(&args.human).map_err(FoldError::from)?;
    let config = serve::client_config(args.socket.as_deref(), dirs).map_err(RemoteError::from)?;
    let client = DaemonClient::new(config, Some(human.to_string()));
    let body = json!({ "human": human.as_str(), "label": args.label });
    let response = client
        .request(Method::POST, "/api/v1/tokens", Some(&body))
        .await
        .map_err(RemoteError::from)?;
    refusal(&response)?;
    Ok(response.body["token"]
        .as_str()
        .ok_or(RemoteError::Answer)?
        .to_owned())
}

async fn command(
    command: &TokenCommand,
    dirs: &Dirs,
    clock: &dyn Clock,
    out: &mut impl Write,
) -> Result<(), TokenError> {
    match command {
        TokenCommand::Create(args) => {
            output::line(out, &create(args, dirs).await?)?;
        }
        TokenCommand::List => {
            let host = host_paths(dirs)?;
            let tokens = list_web_tokens_on_host(&host).await?;
            if tokens.is_empty() {
                output::line(out, "no web tokens")?;
            }
            for token in &tokens {
                output::line(
                    out,
                    &output::web_token_line(
                        &token.id,
                        &token.human,
                        &token.created_at,
                        token.last_used_at.as_ref(),
                        token.label.as_deref(),
                    ),
                )?;
            }
        }
        TokenCommand::Revoke(args) => {
            let id: WebTokenId = args.id.parse().map_err(FoldError::from)?;
            revoke_web_token_on_host(&host_paths(dirs)?, clock, id).await?;
            output::line(out, &format!("revoked {id}"))?;
        }
    }
    Ok(())
}
