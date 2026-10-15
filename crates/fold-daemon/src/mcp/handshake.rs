use std::io::Cursor;
use std::path::{Component, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fold_core::{AgentName, FoldError, codes};
use fold_vcs::git::Git;
use rmcp::ServiceExt;
use rmcp::service::ServerInitializeError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

use super::FoldServer;
use super::error::{ErrorBody, from_agent, from_project};
use crate::daemon::AppState;
use crate::services::agents::{AgentService, SessionContext};
use crate::services::events::EventService;
use crate::services::projects;
use crate::services::workspaces::WorkspaceService;
use crate::socket::HandshakeHandler;

/// The value of `fold` in the adapter's line. A daemon answers any other value with `protocol_version_unsupported`.
pub const PROTOCOL_VERSION: u64 = 1;

/// The longest handshake line, newline included.
pub const MAX_LINE: usize = 4096;

pub const LINE_TIMEOUT: Duration = Duration::from_secs(5);

/// The adapter's first line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub fold: u64,
    /// The canonical working directory of the session.
    pub cwd: PathBuf,
    pub pid: u32,
    /// `FOLD_AGENT`; an empty name counts as unset.
    pub agent: Option<String>,
}

/// The daemon's answer line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

impl Answer {
    pub fn accepted(project: &str) -> Self {
        Self {
            ok: true,
            project: Some(project.to_owned()),
            error: None,
        }
    }

    pub fn refused(error: &FoldError) -> Self {
        Self {
            ok: false,
            project: None,
            error: Some(error.into()),
        }
    }

    /// The answer as one line, newline included.
    pub fn line(&self) -> Vec<u8> {
        let mut line = serde_json::to_vec(self).unwrap_or_default();
        line.push(b'\n');
        line
    }
}

/// The handler the socket server runs for a connection that starts with `{`.
pub fn handler(state: AppState) -> HandshakeHandler {
    Arc::new(move |stream: UnixStream| Box::pin(accept(state.clone(), stream)))
}

/// Serves one adapter connection: reads the handshake line, opens the session, answers, serves MCP on the rest of the stream and ends the session when the stream closes.
pub async fn accept<S>(state: AppState, stream: S)
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut read, mut write) = tokio::io::split(stream);
    let opened = match tokio::time::timeout(LINE_TIMEOUT, read_line(&mut read)).await {
        Ok(Ok((line, rest))) => open(&state, &line).await.map(|opened| (opened, rest)),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(invalid("no handshake line arrived within 5 seconds")),
    };
    let ((agents, events, workspaces, session, project), rest) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            if write
                .write_all(&Answer::refused(&error).line())
                .await
                .is_ok()
            {
                let _ = write.shutdown().await;
            }
            return;
        }
    };
    if write
        .write_all(&Answer::accepted(&project).line())
        .await
        .is_ok()
    {
        let server = FoldServer::new(agents.clone(), events, workspaces, session.clone());
        // Bytes the client sent after its line belong to MCP.
        let read = Cursor::new(rest).chain(read);
        match server.serve((read, write)).await {
            Ok(running) => {
                let _ = running.waiting().await;
            }
            Err(error) => tracing::debug!(
                project,
                reason = init_failure(&error),
                "an MCP session ended during initialisation"
            ),
        }
    }
    if let Err(error) = agents.end_session(&session).await {
        tracing::error!(
            project,
            code = from_agent(error).code(),
            "could not end a session"
        );
    }
}

/// The kind of a failed MCP initialisation. The error's own text can quote the client's first message, which may carry content.
fn init_failure(error: &ServerInitializeError) -> &'static str {
    match error {
        ServerInitializeError::ConnectionClosed(_) | ServerInitializeError::Cancelled => "closed",
        ServerInitializeError::ExpectedInitializeRequest(_) => "no initialize request",
        _ => "protocol error",
    }
}

/// The line without its newline, and the bytes read after it.
async fn read_line<R>(read: &mut R) -> Result<(Vec<u8>, Vec<u8>), FoldError>
where
    R: AsyncRead + Unpin,
{
    let mut line = Vec::with_capacity(512);
    let mut chunk = [0_u8; 1024];
    loop {
        let room = (MAX_LINE - line.len()).min(chunk.len());
        if room == 0 {
            return Err(invalid("the handshake line is longer than 4 KiB"));
        }
        let read = read
            .read(&mut chunk[..room])
            .await
            .map_err(|_| invalid("the connection failed during the handshake"))?;
        if read == 0 {
            return Err(invalid(
                "the connection closed before the handshake line ended",
            ));
        }
        let start = line.len();
        line.extend_from_slice(&chunk[..read]);
        if let Some(offset) = line[start..].iter().position(|byte| *byte == b'\n') {
            let rest = line.split_off(start + offset + 1);
            line.pop();
            return Ok((line, rest));
        }
    }
}

/// Checks the version before the shape, so an adapter of another version always learns that the versions differ.
pub fn parse_hello(line: &[u8]) -> Result<Hello, FoldError> {
    let value: Value =
        serde_json::from_slice(line).map_err(|_| invalid("the handshake line is not JSON"))?;
    match value.get("fold").and_then(Value::as_u64) {
        Some(PROTOCOL_VERSION) => {}
        Some(_) => {
            return Err(FoldError::new(
                codes::PROTOCOL_VERSION_UNSUPPORTED,
                format!("this daemon speaks handshake version {PROTOCOL_VERSION}"),
            )
            .with_hint("run the fold binary of the running daemon, or restart the daemon"));
        }
        None => return Err(invalid("the handshake line has no protocol version")),
    }
    let mut hello: Hello = serde_json::from_value(value)
        .map_err(|_| invalid("the handshake line needs fold, cwd, pid and agent"))?;
    let canonical = hello.cwd.is_absolute()
        && hello
            .cwd
            .components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_)));
    if !canonical {
        return Err(invalid("cwd must be a canonical absolute path"));
    }
    if hello.agent.as_deref() == Some("") {
        hello.agent = None;
    }
    Ok(hello)
}

async fn open(
    state: &AppState,
    line: &[u8],
) -> Result<
    (
        AgentService,
        EventService,
        WorkspaceService,
        SessionContext,
        String,
    ),
    FoldError,
> {
    let hello = parse_hello(line)?;
    let agent = hello.agent.as_deref().map(AgentName::parse).transpose()?;
    let handle = projects::open_for_dir(state, &hello.cwd)
        .await
        .map_err(from_project)?;
    let agents = handle.agents();
    let events = EventService::new(handle.store.clone(), handle.bus.clone());
    let config = projects::load_config(&handle.paths)
        .await
        .map_err(from_project)?;
    let workspaces = WorkspaceService::new(
        handle.store.clone(),
        handle.bus.clone(),
        handle.clock.clone(),
        handle.paths.clone(),
        Git::default(),
        config.lease_minutes,
    );
    let mut session = agents.start_session(hello.pid).await.map_err(from_agent)?;
    if let Some(agent) = agent
        && let Err(error) = agents.register_agent(&mut session, &agent, &[]).await
    {
        let _ = agents.end_session(&session).await;
        return Err(from_agent(error));
    }
    Ok((agents, events, workspaces, session, handle.slug.clone()))
}

fn invalid(message: &str) -> FoldError {
    FoldError::new(codes::INVALID_HANDSHAKE, message)
}
