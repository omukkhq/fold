use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Subcommand};
use fold_cli::daemon_client::{self, ClientConfig, ClientError, DaemonClient};
use fold_core::codes;
use hyper::Method;
use serde_json::{Value, json};

use crate::output;
use crate::serve::{self, Dirs};

/// How long `fold hook session-start` waits for a daemon it starts.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// How long either hook waits for the daemon's answer, and `fold hook stop` for the socket.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(2);
/// Claude Code's hook input is a small JSON object; anything longer is refused unread.
const MAX_INPUT: u64 = 1 << 20;

#[derive(Args, Debug)]
pub struct HookArgs {
    #[command(subcommand)]
    command: HookCommand,
}

#[derive(Subcommand, Debug)]
enum HookCommand {
    /// Claude Code SessionStart hook: give the session its fold context, starting the daemon when needed
    SessionStart(HookOptions),
    /// Claude Code Stop hook: keep the turn going once while the agent has unread fold messages
    Stop(HookOptions),
}

#[derive(Args, Debug)]
struct HookOptions {
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
}

/// A hook never fails the session: every failure writes one line naming its code to stderr and nothing to stdout, and the exit code stays 0.
pub fn run(args: &HookArgs, out: &mut impl Write) {
    let Some(agent) = std::env::var_os("FOLD_AGENT")
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return;
    };
    let (name, outcome) = match &args.command {
        HookCommand::SessionStart(options) => ("session-start", session_start(options, &agent)),
        HookCommand::Stop(options) => ("stop", stop(options, &agent)),
    };
    let written = match outcome {
        Ok(Some(answer)) => output::line(out, &answer.to_string()),
        Ok(None) => Ok(()),
        Err(code) => output::line(
            &mut io::stderr().lock(),
            &format!("fold hook {name}: {code}"),
        ),
    };
    // Claude Code may close the pipe first; the session goes on either way.
    let _ = written;
}

/// Claude Code's JSON from stdin. The text is never echoed, because it can hold the session's paths and messages.
fn input() -> Result<Value, &'static str> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take(MAX_INPUT)
        .read_to_end(&mut bytes)
        .map_err(|_| codes::INVALID_REQUEST)?;
    match serde_json::from_slice(&bytes) {
        Ok(value @ Value::Object(_)) => Ok(value),
        _ => Err(codes::INVALID_REQUEST),
    }
}

/// The daemon resolves the project from the canonical path, as the adapter handshake does.
fn cwd(input: &Value) -> Result<String, &'static str> {
    let cwd = input["cwd"].as_str().ok_or(codes::INVALID_REQUEST)?;
    let cwd = std::fs::canonicalize(cwd).map_err(|_| codes::INVALID_REQUEST)?;
    cwd.into_os_string()
        .into_string()
        .map_err(|_| codes::INVALID_REQUEST)
}

fn session_start(options: &HookOptions, agent: &str) -> Result<Option<Value>, String> {
    let input = input()?;
    let mut body = json!({"cwd": cwd(&input)?, "agent": agent});
    let messaging = |name| std::env::var(name).ok().filter(|value| !value.is_empty());
    if let (Some(socket), Some(token)) = (
        messaging("CLAUDE_CODE_MESSAGING_SOCKET"),
        messaging("CLAUDE_CODE_MESSAGING_TOKEN"),
    ) {
        body["socket"] = json!(socket);
        body["token"] = json!(token);
    }
    let dirs = Dirs::from_env();
    let starting = ClientConfig {
        socket: socket(options, &dirs)?,
        runtime_dir: serve::runtime_dir(&dirs).ok(),
        fold_exe: std::env::current_exe().ok(),
    };
    let answer = exchange(
        starting,
        START_TIMEOUT,
        "/api/v1/hooks/session-start",
        &body,
    )?;
    let context = answer["context"].as_str().ok_or(codes::INTERNAL_ERROR)?;
    Ok(Some(json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": context,
        }
    })))
}

fn stop(options: &HookOptions, agent: &str) -> Result<Option<Value>, String> {
    let input = input()?;
    if input["stop_hook_active"].as_bool() == Some(true) {
        return Ok(None);
    }
    let body = json!({"cwd": cwd(&input)?, "agent": agent});
    let config = ClientConfig {
        socket: socket(options, &Dirs::from_env())?,
        runtime_dir: None,
        fold_exe: None,
    };
    let answer = exchange(config, ANSWER_TIMEOUT, "/api/v1/hooks/stop", &body)?;
    let unread = answer["unread"].as_u64().ok_or(codes::INTERNAL_ERROR)?;
    if unread == 0 {
        return Ok(None);
    }
    // A Stop hook keeps the turn going only through a block decision; Claude Code then sets `stop_hook_active` on the next stop.
    Ok(Some(json!({
        "decision": "block",
        "reason": format!("{unread} unread fold messages. Call get_inbox."),
    })))
}

fn socket(options: &HookOptions, dirs: &Dirs) -> Result<PathBuf, String> {
    let socket = serve::socket_path(options.socket.as_deref(), dirs)
        .map_err(|error| error.code().unwrap_or(codes::INVALID_CONFIG).to_owned())?
        .ok_or(codes::RUNTIME_DIR_MISSING)?;
    std::path::absolute(&socket).map_err(|_| codes::INVALID_REQUEST.to_owned())
}

/// Connects first within `connect_timeout`, starting the daemon when `config` allows it, then posts `body` over a connection of its own. The runtime is dropped without waiting, so a daemon start that outlives its timeout never holds the hook.
fn exchange(
    config: ClientConfig,
    connect_timeout: Duration,
    path: &str,
    body: &Value,
) -> Result<Value, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| codes::INTERNAL_ERROR)?;
    let outcome = runtime.block_on(async {
        match tokio::time::timeout(connect_timeout, daemon_client::connect(&config)).await {
            Ok(Ok(_stream)) => {}
            Ok(Err(error)) => return Err(client_code(&error)),
            Err(_) => return Err(codes::DAEMON_UNREACHABLE),
        }
        let running = ClientConfig {
            fold_exe: None,
            ..config
        };
        let client = DaemonClient::new(running, daemon_client::human_from_env().ok());
        match tokio::time::timeout(
            ANSWER_TIMEOUT,
            client.request(Method::POST, path, Some(body)),
        )
        .await
        {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => Err(client_code(&error)),
            Err(_) => Err(codes::DAEMON_UNREACHABLE),
        }
    });
    runtime.shutdown_background();
    let response = outcome?;
    if (200..300).contains(&response.status) {
        return Ok(response.body);
    }
    Err(response.body["error"]["code"]
        .as_str()
        .unwrap_or(codes::INTERNAL_ERROR)
        .to_owned())
}

fn client_code(error: &ClientError) -> &'static str {
    match error {
        ClientError::HumanUnknown => codes::HUMAN_NAME_REQUIRED,
        ClientError::HumanInvalid => codes::INVALID_NAME,
        ClientError::RuntimeDirMissing => codes::RUNTIME_DIR_MISSING,
        ClientError::NoDaemon { .. }
        | ClientError::StartTimeout { .. }
        | ClientError::StartFailed
        | ClientError::Io { .. }
        | ClientError::Http(_)
        | ClientError::Body => codes::DAEMON_UNREACHABLE,
    }
}
