use std::io::{self, Write};

use clap::{Args, Subcommand};
use fold_core::{Clock, FoldError, HostPaths, HumanName, WebTokenId};
use fold_daemon::SystemClock;
use fold_daemon::services::projects::{
    ProjectError, create_web_token_on_host, list_web_tokens_on_host, revoke_web_token_on_host,
};
use thiserror::Error;

use crate::init::host_paths;
use crate::output;
use crate::serve::Dirs;

#[derive(Args, Debug)]
pub struct TokenArgs {
    #[command(subcommand)]
    command: TokenCommand,
}

#[derive(Subcommand, Debug)]
enum TokenCommand {
    /// Create a web sign-in token for a human, who is created when absent, and print it once
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
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error("cannot write the output: {0}")]
    Write(#[from] io::Error),
}

impl TokenError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Fold(error) => Some(error.code()),
            Self::Project(error) => error.code(),
            Self::Runtime(_) | Self::Write(_) => None,
        }
    }
}

/// Reads and writes the registry directly, the way `fld init` does, so every command works while the daemon is down; a running daemon reads `web_tokens` and `web_sessions` on every check, so a new token signs in and a revoked one stops at once.
pub fn run(args: &TokenArgs, dirs: &Dirs, out: &mut impl Write) -> Result<(), TokenError> {
    let host = host_paths(dirs)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(TokenError::Runtime)?;
    runtime.block_on(command(&args.command, &host, &SystemClock, out))
}

async fn command(
    command: &TokenCommand,
    host: &HostPaths,
    clock: &dyn Clock,
    out: &mut impl Write,
) -> Result<(), TokenError> {
    match command {
        TokenCommand::Create(args) => {
            let human = HumanName::parse(&args.human).map_err(FoldError::from)?;
            let token =
                create_web_token_on_host(host, clock, &human, args.label.as_deref()).await?;
            output::line(out, &token)?;
        }
        TokenCommand::List => {
            let tokens = list_web_tokens_on_host(host).await?;
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
            revoke_web_token_on_host(host, clock, id).await?;
            output::line(out, &format!("revoked {id}"))?;
        }
    }
    Ok(())
}
