use std::io::Write;
use std::path::PathBuf;

use clap::{Args, Subcommand};
use fold_cli::daemon_client::Response;
use fold_core::config::check_branch;
use hyper::Method;
use serde_json::json;

use super::remote::{RemoteError, refusal, request_project_with};
use crate::output;

#[derive(Args, Debug)]
pub struct GitArgs {
    #[command(subcommand)]
    command: GitCommand,
}

#[derive(Subcommand, Debug)]
enum GitCommand {
    /// Rename the local Git branch that each integration commits fold main to, and its name in .fold/config.toml, together; the commits, the remotes and their branches stay as they are
    Branch(BranchArgs),
}

#[derive(Args, Debug)]
struct BranchArgs {
    /// The new name of the local branch, such as dev
    name: String,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

pub fn run(args: &GitArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    match &args.command {
        GitCommand::Branch(branch) => run_branch(branch, out),
    }
}

/// Asks the daemon to rename the project's local branch, starting the daemon when none runs. The name is checked here first, so a bad name never starts a daemon.
fn run_branch(args: &BranchArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    check_branch(&args.name)?;
    let response = request_project_with(
        args.socket.as_deref(),
        args.project.as_deref(),
        Method::POST,
        "git/branch",
        Some(&json!({"to": args.name})),
    )?;
    output::line(out, &renamed_line(&response)?)?;
    Ok(())
}

/// The line `fld git branch` prints for the daemon's answer, or the error it carries.
fn renamed_line(response: &Response) -> Result<String, RemoteError> {
    refusal(response)?;
    let body = &response.body;
    let text = |key: &str| {
        body[key]
            .as_str()
            .map(output::escape_control)
            .ok_or(RemoteError::Answer)
    };
    let (from, to) = (text("from")?, text("to")?);
    Ok(
        match body["changed"].as_bool().ok_or(RemoteError::Answer)? {
            true => format!(
                "renamed the local branch `{from}` to `{to}`: the next integration commits fold main to `{to}`"
            ),
            false => format!("the local branch is `{to}` already; nothing changed"),
        },
    )
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct Git {
        #[command(flatten)]
        args: GitArgs,
    }

    #[test]
    fn branch_parses_the_name_the_socket_and_the_project() {
        let parsed = Git::try_parse_from(["fld", "branch", "dev", "--project", "app"]).unwrap();
        let GitCommand::Branch(branch) = parsed.args.command;
        assert_eq!(branch.name, "dev");
        assert_eq!(branch.project, Some(PathBuf::from("app")));
        assert!(Git::try_parse_from(["fld", "branch"]).is_err());
        assert!(Git::try_parse_from(["fld"]).is_err());
    }

    #[test]
    fn a_bad_name_is_refused_before_the_daemon_is_asked() {
        let args = BranchArgs {
            name: "-x".to_owned(),
            socket: None,
            project: None,
        };
        let error = run_branch(&args, &mut Vec::new()).unwrap_err();
        assert_eq!(error.code(), Some("invalid_request"));
    }

    #[test]
    fn answers_read_as_one_line_or_the_error_they_carry() {
        let reply = |status, body| Response { status, body };
        assert_eq!(
            renamed_line(&reply(
                200,
                json!({"from": "main", "to": "dev", "changed": true})
            ))
            .unwrap(),
            "renamed the local branch `main` to `dev`: the next integration commits fold main to `dev`"
        );
        assert_eq!(
            renamed_line(&reply(
                200,
                json!({"from": "dev", "to": "dev", "changed": false})
            ))
            .unwrap(),
            "the local branch is `dev` already; nothing changed"
        );
        let refused = json!({"error": {"code": "invalid_request", "message": "the branch dev exists", "details": {"reason": "branch_exists", "hint": "pick another name"}}});
        let error = renamed_line(&reply(400, refused)).unwrap_err();
        assert_eq!(error.code(), Some("invalid_request"));
        assert_eq!(
            error.to_string(),
            "the branch dev exists (pick another name)"
        );
        for bad in [
            json!({"from": "main", "to": "dev"}),
            json!({"to": "dev", "changed": true}),
            json!(null),
        ] {
            assert!(matches!(
                renamed_line(&reply(200, bad)),
                Err(RemoteError::Answer)
            ));
        }
    }
}
