use std::io::Write;
use std::path::PathBuf;

use clap::{Args, Subcommand};
use fold_cli::daemon_client::Response;
use hyper::Method;
use serde_json::Value;

use super::remote::{RemoteError, refusal, request_project};
use crate::output;

#[derive(Args, Debug)]
pub struct AgentChangesArgs {
    #[command(subcommand)]
    command: Option<AgentChangesCommand>,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR", global = true)]
    project: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum AgentChangesCommand {
    /// Put the files workers changed into a new stash entry and bring them back to main's version; your own changes stay, and fld unstash brings the files back
    Restore,
    /// Let the files workers changed count as your own changes, which fld record records
    Ignore,
}

/// The most paths the command lists; the rest are counted.
const LISTED_PATHS: usize = 20;

/// Lists the files that supervised workers changed in the project folder, or restores or ignores them, starting the daemon when none runs.
pub fn run(args: &AgentChangesArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let (method, action) = match args.command {
        None => (Method::GET, "agent-changes"),
        Some(AgentChangesCommand::Restore) => (Method::POST, "agent-changes/restore"),
        Some(AgentChangesCommand::Ignore) => (Method::POST, "agent-changes/ignore"),
    };
    let response = request_project(
        args.socket.as_deref(),
        args.project.as_deref(),
        method,
        action,
    )?;
    let lines = match args.command {
        None => list_lines(&response)?,
        Some(AgentChangesCommand::Restore) => restore_lines(&response)?,
        Some(AgentChangesCommand::Ignore) => ignore_lines(&response)?,
    };
    for line in lines {
        output::line(out, &line)?;
    }
    Ok(())
}

fn strings(value: &Value) -> Result<Vec<&str>, RemoteError> {
    value
        .as_array()
        .ok_or(RemoteError::Answer)?
        .iter()
        .map(|item| item.as_str().ok_or(RemoteError::Answer))
        .collect()
}

fn push_paths(lines: &mut Vec<String>, paths: &[String]) {
    for path in paths.iter().take(LISTED_PATHS) {
        lines.push(format!("  {path}"));
    }
    if paths.len() > LISTED_PATHS {
        lines.push(format!("  and {} more", paths.len() - LISTED_PATHS));
    }
}

fn list_lines(response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let changes = response.body["changes"]
        .as_array()
        .ok_or(RemoteError::Answer)?;
    if changes.is_empty() {
        return Ok(vec![
            "no file of the project folder changed while a worker worked".to_owned(),
        ]);
    }
    let mut lines = vec![format!(
        "{} changed in the project folder while a worker worked; fold does not offer them as your change:",
        output::count(changes.len(), "file", "files")
    )];
    let named: Vec<String> = changes
        .iter()
        .map(|change| {
            let path = change["path"].as_str().ok_or(RemoteError::Answer)?;
            let agents = strings(&change["agents"])?;
            Ok(format!("{path} ({})", agents.join(", ")))
        })
        .collect::<Result<_, RemoteError>>()?;
    push_paths(&mut lines, &named);
    lines.push("run fld record to keep them as your change, fld agent-changes restore to put them aside in the stash, or fld agent-changes ignore to treat them as your own".to_owned());
    Ok(lines)
}

fn restore_lines(response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let body = &response.body;
    let revision = body["revision"].as_str().ok_or(RemoteError::Answer)?;
    let files: Vec<String> = strings(&body["files"])?
        .into_iter()
        .map(str::to_owned)
        .collect();
    if files.is_empty() {
        return Ok(vec![format!(
            "nothing to restore: the project folder holds main's version of those files (it holds {revision})"
        )]);
    }
    let mut lines = vec![format!(
        "put {} that workers changed aside as stash@{{0}}; the project folder holds main {revision} there again and keeps your own changes. fld unstash brings them back",
        output::count(files.len(), "file", "files")
    )];
    push_paths(&mut lines, &files);
    Ok(lines)
}

fn ignore_lines(response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let files: Vec<String> = strings(&response.body["files"])?
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut lines = vec![format!(
        "{} that workers changed count as your own changes now; fld record records them",
        output::count(files.len(), "file", "files")
    )];
    push_paths(&mut lines, &files);
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn reply(status: u16, body: Value) -> Response {
        Response { status, body }
    }

    #[test]
    fn the_list_names_each_file_with_its_agents() {
        let lines = list_lines(&reply(
            200,
            json!({"changes": [
                {"path": "plan.md", "agents": ["reviewer-35"], "since": 1},
                {"path": "x", "agents": ["backend", "reviewer-35"], "since": 1},
            ]}),
        ))
        .unwrap();
        assert_eq!(lines[1], "  plan.md (reviewer-35)");
        assert_eq!(lines[2], "  x (backend, reviewer-35)");
        assert!(lines[0].starts_with("2 files changed"));
        assert!(lines[3].contains("fld agent-changes restore"));
        assert_eq!(
            list_lines(&reply(200, json!({"changes": []}))).unwrap(),
            ["no file of the project folder changed while a worker worked"]
        );
    }

    #[test]
    fn restore_and_ignore_name_the_files_and_a_refusal_is_an_error() {
        let files: Vec<String> = (0..22).map(|n| format!("f{n}")).collect();
        let restored = restore_lines(&reply(
            200,
            json!({"revision": "r4", "files": files, "entries": 2}),
        ))
        .unwrap();
        assert!(restored[0].starts_with("put 22 files that workers changed aside as stash@{0}"));
        assert_eq!(restored.last().unwrap(), "  and 2 more");
        let nothing = restore_lines(&reply(
            200,
            json!({"revision": "r4", "files": [], "entries": 1}),
        ))
        .unwrap();
        assert!(nothing[0].starts_with("nothing to restore"));
        let ignored = ignore_lines(&reply(200, json!({"files": ["plan.md"]}))).unwrap();
        assert_eq!(
            ignored,
            [
                "1 file that workers changed count as your own changes now; fld record records them",
                "  plan.md"
            ]
        );
        let refused =
            json!({"error": {"code": "agent_change_not_found", "message": "none", "details": {}}});
        assert_eq!(
            ignore_lines(&reply(404, refused)).unwrap_err().code(),
            Some("agent_change_not_found")
        );
        assert!(matches!(
            list_lines(&reply(200, json!({}))),
            Err(RemoteError::Answer)
        ));
    }
}
