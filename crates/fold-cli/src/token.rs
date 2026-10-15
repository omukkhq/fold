use std::io::{self, Write};

use clap::{Args, Subcommand};
use fold_core::{FoldError, HumanName};
use fold_daemon::SystemClock;
use fold_daemon::services::projects::{ProjectError, create_web_token_on_host};
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
}

#[derive(Args, Debug)]
struct CreateArgs {
    /// The human the token signs in
    human: String,
    /// A note that tells this token apart from the human's other tokens
    #[arg(long)]
    label: Option<String>,
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

/// Writes the registry directly, the way `fold init` does, so it works while the daemon is down; a running daemon reads `web_tokens` on every check and accepts the token at once.
pub fn run(args: &TokenArgs, dirs: &Dirs, out: &mut impl Write) -> Result<(), TokenError> {
    let TokenCommand::Create(args) = &args.command;
    let human = HumanName::parse(&args.human).map_err(FoldError::from)?;
    let host = host_paths(dirs)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(TokenError::Runtime)?;
    let token = runtime.block_on(create_web_token_on_host(
        &host,
        &SystemClock,
        &human,
        args.label.as_deref(),
    ))?;
    output::line(out, &token)?;
    Ok(())
}
