use std::io;
use std::path::PathBuf;

use clap::Args;
use fold_cli::claude_files::{AGENT_ENV, AGENT_TAGS_ENV, KEEP_OTHERS_ENV};
use fold_cli::daemon_client::{self, ClientConfig, ClientError, HandshakeError, HelloLine};
use fold_cli::mcp_relay::{self, ReconnectPolicy, RelayError};
use fold_core::MAIN_TAG;
use fold_daemon::mcp::OutsideProject;
use fold_daemon::mcp::handshake::{Hello, HelloOptions, PROTOCOL_VERSION, SessionInbox};
use thiserror::Error;

use crate::init::project_root_at_or_above;
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
    #[error(transparent)]
    Relay(#[from] RelayError),
}

/// Connects this session to the daemon and carries MCP between stdio and the socket until Claude Code closes stdin. One process is one session: when the daemon closes the connection, the adapter reconnects as the same agent and keeps stdio open, and it exits only when the reconnect gives up. A session in a folder outside every fold project gets [`OutsideProject`] instead, without a daemon: the user's Claude Code configuration starts `fld mcp` in every folder, and there it serves no tools and writes nothing to stderr.
pub fn run(args: &McpArgs) -> Result<(), McpError> {
    let cwd = std::env::current_dir()
        .and_then(|dir| dir.canonicalize())
        .map_err(McpError::Cwd)?;
    if project_root_at_or_above(&cwd).is_none() {
        let runtime = serve::current_thread_runtime().map_err(McpError::Runtime)?;
        runtime.block_on(OutsideProject::serve(
            tokio::io::stdin(),
            tokio::io::stdout(),
        ));
        runtime.shutdown_background();
        return Ok(());
    }
    let config = serve::client_config(args.socket.as_deref(), &serve::Dirs::from_env())?;
    let agent = std::env::var_os(AGENT_ENV)
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned());
    let hello = Hello {
        fold: PROTOCOL_VERSION,
        cwd,
        pid: std::process::id(),
        agent,
        tags: agent_tags(std::env::var(AGENT_TAGS_ENV).ok().as_deref()),
        inbox: session_inbox(
            std::env::var_os(MESSAGING_SOCKET_ENV),
            std::env::var(MESSAGING_TOKEN_ENV).ok(),
        ),
    };
    let keep_others = keeps_others(&hello.tags, std::env::var_os(KEEP_OTHERS_ENV));
    let hello = HelloLine {
        hello,
        options: HelloOptions {
            keep_others,
            reconnect: false,
        },
    };
    let runtime = serve::current_thread_runtime().map_err(McpError::Runtime)?;
    let outcome = runtime.block_on(session(&config, &hello));
    // The stdin reader blocks a runtime thread until the next line, which may never come.
    runtime.shutdown_background();
    outcome
}

/// Claude Code sets these for its session-start hook; the adapter sends them too whenever Claude Code passed them to it, so a session reaches a restarted daemon even before its next start hook.
const MESSAGING_SOCKET_ENV: &str = "CLAUDE_CODE_MESSAGING_SOCKET";
const MESSAGING_TOKEN_ENV: &str = "CLAUDE_CODE_MESSAGING_TOKEN";

/// The session's inbox when both values are set and not empty.
fn session_inbox(
    socket: Option<std::ffi::OsString>,
    token: Option<String>,
) -> Option<SessionInbox> {
    let socket = socket.filter(|socket| !socket.is_empty())?;
    let token = token.filter(|token| !token.is_empty())?;
    Some(SessionInbox {
        socket: PathBuf::from(socket),
        token,
    })
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

/// `FOLD_KEEP_OTHERS`, which `fld claude --keep-others` sets to `1` on the main session: only `1`, and only with the `main` tag, keeps the agent's other live sessions, so a worker that inherits the variable from a daemon started inside such a session never sends it.
fn keeps_others(tags: &[String], value: Option<std::ffi::OsString>) -> bool {
    tags.iter().any(|tag| tag == MAIN_TAG) && value.is_some_and(|value| value == "1")
}

async fn session(config: &ClientConfig, hello: &HelloLine) -> Result<(), McpError> {
    let stream = daemon_client::connect(config).await?;
    let connection = daemon_client::handshake(stream, hello).await?;
    // The relay carries protocol bytes, never user output.
    mcp_relay::relay(
        tokio::io::stdin(),
        tokio::io::stdout(),
        connection,
        &ClientConfig::running(config.socket.clone()),
        hello,
        &ReconnectPolicy::default(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inbox_needs_both_messaging_values() {
        let socket = || Some(std::ffi::OsString::from("/run/claude/inbox.sock"));
        let token = || Some("0123456789abcdef".to_owned());
        assert_eq!(
            session_inbox(socket(), token()),
            Some(SessionInbox {
                socket: PathBuf::from("/run/claude/inbox.sock"),
                token: "0123456789abcdef".to_owned(),
            })
        );
        assert_eq!(session_inbox(None, token()), None);
        assert_eq!(session_inbox(socket(), None), None);
        assert_eq!(session_inbox(Some("".into()), token()), None);
        assert_eq!(session_inbox(socket(), Some(String::new())), None);
        let shown = format!("{:?}", session_inbox(socket(), token()));
        assert!(!shown.contains("0123456789abcdef"), "{shown}");
    }

    #[test]
    fn keep_others_is_1_on_a_main_session_and_goes_beside_the_hello() {
        let main = ["main".to_owned()];
        assert!(!keeps_others(&main, None));
        assert!(!keeps_others(&main, Some("".into())));
        assert!(!keeps_others(&main, Some("0".into())));
        assert!(!keeps_others(&main, Some("true".into())));
        assert!(keeps_others(&main, Some("1".into())));
        assert!(!keeps_others(&[], Some("1".into())));
        assert!(!keeps_others(&["review".to_owned()], Some("1".into())));
        let hello = Hello {
            fold: PROTOCOL_VERSION,
            cwd: PathBuf::from("/src/app"),
            pid: 7,
            agent: Some("main".to_owned()),
            tags: vec!["main".to_owned()],
            inbox: None,
        };
        let plain = serde_json::to_vec(&HelloLine::from(hello.clone())).unwrap();
        assert!(!String::from_utf8_lossy(&plain).contains("keep_others"));
        let kept = HelloLine {
            hello: hello.clone(),
            options: HelloOptions {
                keep_others: true,
                reconnect: false,
            },
        };
        let line = serde_json::to_vec(&kept).unwrap();
        let (parsed, options) = fold_daemon::mcp::handshake::parse_line(&line).unwrap();
        assert_eq!(parsed, hello);
        assert!(options.keep_others);
    }

    #[test]
    fn the_agent_tags_are_the_comma_separated_words() {
        assert_eq!(agent_tags(None), Vec::<String>::new());
        assert_eq!(agent_tags(Some("")), Vec::<String>::new());
        assert_eq!(agent_tags(Some("main")), ["main"]);
        assert_eq!(agent_tags(Some(" main, ui ,,")), ["main", "ui"]);
    }
}
