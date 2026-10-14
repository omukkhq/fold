use std::io;
use std::path::PathBuf;

use clap::Args;
use fold_cli::daemon_client::{self, ClientConfig, ClientError};
use fold_daemon::mcp::handshake::{Answer, Hello, MAX_LINE, PROTOCOL_VERSION};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::serve;

#[derive(Args)]
pub struct McpArgs {
    /// Unix socket path; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum McpError {
    #[error(transparent)]
    Config(#[from] serve::ServeError),
    #[error("cannot find the fold binary: {0}")]
    Exe(io::Error),
    #[error("cannot read the working directory: {0}")]
    Cwd(io::Error),
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("{code}: {message}{}", hint.as_ref().map(|hint| format!(" ({hint})")).unwrap_or_default())]
    Refused {
        code: String,
        message: String,
        hint: Option<String>,
    },
    #[error("the daemon sent no valid handshake answer")]
    NoAnswer,
    #[error("the handshake with the daemon failed: {0}")]
    Io(io::Error),
    #[error("the daemon closed the session; start a new session to reconnect")]
    DaemonClosed,
}

/// Connects this session to the daemon and copies MCP bytes between stdio and the socket until either side closes. One process is one session.
pub fn run(args: &McpArgs) -> Result<(), McpError> {
    let config = client_config(args)?;
    let cwd = std::env::current_dir()
        .and_then(|dir| dir.canonicalize())
        .map_err(McpError::Cwd)?;
    let agent = std::env::var_os("FOLD_AGENT")
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned());
    let hello = Hello {
        fold: PROTOCOL_VERSION,
        cwd,
        pid: std::process::id(),
        agent,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(McpError::Runtime)?;
    let outcome = runtime.block_on(session(&config, &hello));
    // The stdin reader blocks a runtime thread until the next line, which may never come.
    runtime.shutdown_background();
    outcome
}

fn client_config(args: &McpArgs) -> Result<ClientConfig, McpError> {
    let dirs = serve::Dirs::from_env();
    let runtime_dir = serve::runtime_dir(&dirs).ok();
    let socket =
        serve::socket_path(args.socket.as_deref(), &dirs)?.ok_or(ClientError::RuntimeDirMissing)?;
    Ok(ClientConfig {
        socket: std::path::absolute(&socket).map_err(McpError::Cwd)?,
        runtime_dir,
        fold_exe: Some(std::env::current_exe().map_err(McpError::Exe)?),
    })
}

async fn session(config: &ClientConfig, hello: &Hello) -> Result<(), McpError> {
    let stream = daemon_client::connect(config).await?;
    let (read, mut write) = stream.into_split();
    let mut line = serde_json::to_vec(hello).map_err(|_| McpError::NoAnswer)?;
    line.push(b'\n');
    write.write_all(&line).await.map_err(McpError::Io)?;

    let mut read = BufReader::new(read);
    let mut answer = Vec::new();
    (&mut read)
        .take(MAX_LINE as u64)
        .read_until(b'\n', &mut answer)
        .await
        .map_err(McpError::Io)?;
    let answer: Answer = serde_json::from_slice(&answer).map_err(|_| McpError::NoAnswer)?;
    if !answer.ok {
        return Err(match answer.error {
            Some(error) => McpError::Refused {
                code: error.code,
                message: error.message,
                hint: error.hint,
            },
            None => McpError::NoAnswer,
        });
    }

    // These copies carry protocol bytes, never user output.
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    tokio::select! {
        _ = tokio::io::copy(&mut stdin, &mut write) => Ok(()),
        _ = tokio::io::copy(&mut read, &mut stdout) => Err(McpError::DaemonClosed),
    }
}
