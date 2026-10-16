use std::io;
use std::path::PathBuf;

use clap::Args;
use fold_cli::claude_files::{AGENT_ENV, AGENT_TAGS_ENV};
use fold_cli::daemon_client::{self, ClientConfig, ClientError, HandshakeError};
use fold_daemon::mcp::handshake::{Hello, PROTOCOL_VERSION};
use thiserror::Error;

use crate::serve;

#[derive(Args)]
pub struct McpArgs {
    /// Unix socket path; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long, env = serve::SOCKET_ENV)]
    socket: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum McpError {
    #[error(transparent)]
    Config(#[from] serve::ServeError),
    #[error("cannot read the working directory: {0}")]
    Cwd(io::Error),
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    Handshake(#[from] HandshakeError),
    #[error("the connection to the daemon failed: {0}")]
    Copy(io::Error),
    #[error("the daemon closed the session; start a new session to reconnect")]
    DaemonClosed,
}

/// Connects this session to the daemon and copies MCP bytes between stdio and the socket until either side closes. One process is one session.
pub fn run(args: &McpArgs) -> Result<(), McpError> {
    let config = serve::client_config(args.socket.as_deref(), &serve::Dirs::from_env())?;
    let cwd = std::env::current_dir()
        .and_then(|dir| dir.canonicalize())
        .map_err(McpError::Cwd)?;
    let agent = std::env::var_os(AGENT_ENV)
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned());
    let hello = Hello {
        fold: PROTOCOL_VERSION,
        cwd,
        pid: std::process::id(),
        agent,
        tags: agent_tags(std::env::var(AGENT_TAGS_ENV).ok().as_deref()),
    };
    let runtime = serve::current_thread_runtime().map_err(McpError::Runtime)?;
    let outcome = runtime.block_on(session(&config, &hello));
    // The stdin reader blocks a runtime thread until the next line, which may never come.
    runtime.shutdown_background();
    outcome
}

/// The comma-separated tags of `FOLD_AGENT_TAGS`, without blanks. The daemon checks each one.
fn agent_tags(value: Option<&str>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_owned)
        .collect()
}

async fn session(config: &ClientConfig, hello: &Hello) -> Result<(), McpError> {
    let stream = daemon_client::connect(config).await?;
    let (mut read, mut write) = daemon_client::handshake(stream, hello).await?;

    // These copies carry protocol bytes, never user output.
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    tokio::select! {
        copied = tokio::io::copy(&mut stdin, &mut write) => session_ended(copied),
        copied = tokio::io::copy(&mut read, &mut stdout) => Err(daemon_ended(copied)),
    }
}

/// The session ends cleanly when Claude Code closes stdin. A write that fails because the daemon went away is the daemon closing the session.
fn session_ended(copied: io::Result<u64>) -> Result<(), McpError> {
    match copied {
        Ok(_) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
            ) =>
        {
            Err(McpError::DaemonClosed)
        }
        Err(error) => Err(McpError::Copy(error)),
    }
}

/// The daemon's side ends with its end of stream, or with the error that stopped the copy.
fn daemon_ended(copied: io::Result<u64>) -> McpError {
    match copied {
        Ok(_) => McpError::DaemonClosed,
        Err(error) => McpError::Copy(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_error_is_reported_as_itself() {
        assert!(session_ended(Ok(3)).is_ok());
        let gone = io::Error::from(io::ErrorKind::BrokenPipe);
        assert!(matches!(
            session_ended(Err(gone)),
            Err(McpError::DaemonClosed)
        ));
        let other = io::Error::from(io::ErrorKind::PermissionDenied);
        assert!(matches!(session_ended(Err(other)), Err(McpError::Copy(_))));
        assert!(matches!(daemon_ended(Ok(0)), McpError::DaemonClosed));
        let error = daemon_ended(Err(io::Error::other("stdout closed")));
        assert_eq!(
            error.to_string(),
            "the connection to the daemon failed: stdout closed"
        );
    }

    #[test]
    fn the_agent_tags_are_the_comma_separated_words() {
        assert_eq!(agent_tags(None), Vec::<String>::new());
        assert_eq!(agent_tags(Some("")), Vec::<String>::new());
        assert_eq!(agent_tags(Some("main")), ["main"]);
        assert_eq!(agent_tags(Some(" main, ui ,,")), ["main", "ui"]);
    }
}
