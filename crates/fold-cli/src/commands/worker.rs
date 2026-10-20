use std::io::Write;
use std::path::PathBuf;

use clap::{Args, Subcommand};
use fold_cli::daemon_client::Response;
use fold_core::{AgentName, FoldError};
use hyper::Method;
use serde_json::json;

use super::remote::{RemoteError, refusal, request_project_with};
use crate::output;

#[derive(Args, Debug)]
pub struct WorkerArgs {
    #[command(subcommand)]
    command: WorkerCommand,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR", global = true)]
    project: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum WorkerCommand {
    /// Start a worker of any role again in a new Claude Code session, for example when its session is broken: fold ends its process when it runs, and it keeps its name, claims and workers
    Restart(RestartArgs),
}

#[derive(Args, Debug)]
struct RestartArgs {
    /// The worker's agent name, such as web-lead; its old session id stays in its previous sessions
    name: String,
    /// Text for the first turn of the new session; fold does not store it
    #[arg(long)]
    instructions: Option<String>,
}

/// Asks the daemon for the worker's new session, starting the daemon when none runs.
pub fn run(args: &WorkerArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    match &args.command {
        WorkerCommand::Restart(restart) => {
            // An agent name is only a-z, 0-9, - and _, so the parsed name is a path segment as it is.
            let name = AgentName::parse(&restart.name).map_err(FoldError::from)?;
            let body = json!({ "instructions": restart.instructions });
            let response = request_project_with(
                args.socket.as_deref(),
                args.project.as_deref(),
                Method::POST,
                &format!("workers/{name}/restart"),
                Some(&body),
            )?;
            output::line(out, &restart_summary(&response)?)?;
        }
    }
    Ok(())
}

/// The line `fld worker restart` prints for the daemon's answer, or the error it carries.
fn restart_summary(response: &Response) -> Result<String, RemoteError> {
    refusal(response)?;
    let body = &response.body;
    let name = body["name"].as_str().ok_or(RemoteError::Answer)?;
    let session = body["session_id"].as_str().ok_or(RemoteError::Answer)?;
    let old = body["previous_sessions"]
        .as_array()
        .and_then(|sessions| sessions.last())
        .and_then(|last| last["session_id"].as_str())
        .ok_or(RemoteError::Answer)?;
    Ok(format!(
        "{name} starts again in session {session}; its earlier session was {old}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn response(status: u16, body: Value) -> Response {
        Response { status, body }
    }

    #[test]
    fn the_line_names_the_new_session_and_the_one_before_it() {
        let answer = response(
            200,
            json!({
                "name": "web-lead",
                "session_id": "s-3",
                "previous_sessions": [
                    {"session_id": "s-1", "reason": "handoff", "ended_at": 1},
                    {"session_id": "s-2", "reason": "requested", "ended_at": 2}
                ]
            }),
        );
        assert_eq!(
            restart_summary(&answer).unwrap(),
            "web-lead starts again in session s-3; its earlier session was s-2"
        );
        let without = response(200, json!({"name": "web-lead", "session_id": "s-3"}));
        assert!(matches!(
            restart_summary(&without),
            Err(RemoteError::Answer)
        ));
        let refused = response(
            409,
            json!({"error": {"code": "project_paused", "message": "the agents of this project are paused"}}),
        );
        assert!(matches!(
            restart_summary(&refused),
            Err(RemoteError::Refused { code, .. }) if code == "project_paused"
        ));
    }
}
