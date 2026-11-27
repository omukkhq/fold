use std::io::Write;
use std::path::PathBuf;

use clap::{ArgGroup, Args, Subcommand};
use fold_cli::daemon_client::Response;
use hyper::Method;
use serde_json::{Value, json};

use super::remote::{RemoteError, refusal, request_project_with};
use crate::output;

#[derive(Args, Debug)]
pub struct ChecksArgs {
    #[command(subcommand)]
    command: ChecksCommand,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR", global = true)]
    project: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum ChecksCommand {
    /// Cancel running check runs: one run, every running run of a task, or every running run of the project. The running command stops, no later command starts, and each run fails with reason cancelled; integration runs are not cancelled
    Cancel(CancelArgs),
}

#[derive(Args, Debug)]
#[command(group(ArgGroup::new("target").required(true).args(["run", "task", "all"])))]
struct CancelArgs {
    /// The running check run, such as cr-4
    run: Option<String>,
    /// Cancel every running run of this task, such as T-12
    #[arg(long, value_name = "T-ID")]
    task: Option<String>,
    /// Cancel every running run of the project
    #[arg(long)]
    all: bool,
}

impl CancelArgs {
    /// The body of the daemon's cancel route; clap lets exactly one of the three through.
    fn body(&self) -> Value {
        match (&self.run, &self.task) {
            (Some(run), _) => json!({ "run": run }),
            (None, Some(task)) => json!({ "task": task }),
            (None, None) => json!({ "all": self.all }),
        }
    }
}

/// Asks the daemon to cancel the runs, starting the daemon when none runs, and prints each run it cancelled.
pub fn run(args: &ChecksArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    match &args.command {
        ChecksCommand::Cancel(cancel) => {
            let response = request_project_with(
                args.socket.as_deref(),
                args.project.as_deref(),
                Method::POST,
                "check-runs/cancel",
                Some(&cancel.body()),
            )?;
            for line in cancel_lines(&response)? {
                output::line(out, &line)?;
            }
        }
    }
    Ok(())
}

/// The lines `fld checks cancel` prints for the daemon's answer, or the error it carries.
fn cancel_lines(response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let runs = response.body["cancelled"]
        .as_array()
        .ok_or(RemoteError::Answer)?
        .iter()
        .map(|run| run.as_str().map(|run| format!("{run} cancelled")))
        .collect::<Option<Vec<String>>>()
        .ok_or(RemoteError::Answer)?;
    if runs.is_empty() {
        return Ok(vec!["no check run was running".to_owned()]);
    }
    Ok(runs)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser, Debug)]
    struct Cli {
        #[command(flatten)]
        checks: ChecksArgs,
    }

    fn parse(args: &[&str]) -> Result<CancelArgs, clap::Error> {
        let cli = Cli::try_parse_from(std::iter::once("fld").chain(args.iter().copied()))?;
        let ChecksCommand::Cancel(cancel) = cli.checks.command;
        Ok(cancel)
    }

    fn response(status: u16, body: Value) -> Response {
        Response { status, body }
    }

    #[test]
    fn each_form_sends_its_one_target() {
        assert_eq!(
            parse(&["cancel", "cr-4"]).unwrap().body(),
            json!({"run": "cr-4"})
        );
        assert_eq!(
            parse(&["cancel", "--task", "T-12"]).unwrap().body(),
            json!({"task": "T-12"})
        );
        assert_eq!(
            parse(&["cancel", "--all"]).unwrap().body(),
            json!({"all": true})
        );
    }

    #[test]
    fn the_command_needs_exactly_one_target() {
        for args in [
            &["cancel"][..],
            &["cancel", "cr-4", "--all"],
            &["cancel", "--task", "T-1", "--all"],
            &["cancel", "cr-4", "--task", "T-1"],
        ] {
            let error = parse(args).unwrap_err();
            assert!(
                matches!(
                    error.kind(),
                    clap::error::ErrorKind::MissingRequiredArgument
                        | clap::error::ErrorKind::ArgumentConflict
                ),
                "{args:?}: {error}"
            );
        }
    }

    #[test]
    fn the_lines_name_each_cancelled_run_or_say_none_ran() {
        let two = response(200, json!({"cancelled": ["cr-4", "cr-7"]}));
        assert_eq!(
            cancel_lines(&two).unwrap(),
            ["cr-4 cancelled", "cr-7 cancelled"]
        );
        let none = response(200, json!({"cancelled": []}));
        assert_eq!(cancel_lines(&none).unwrap(), ["no check run was running"]);
        for body in [json!({}), json!({"cancelled": [4]})] {
            assert!(matches!(
                cancel_lines(&response(200, body)),
                Err(RemoteError::Answer)
            ));
        }
        let refused = response(
            404,
            json!({"error": {"code": "check_run_not_found", "message": "no cr-9", "details": {"hint": "call get_check_run"}}}),
        );
        assert!(matches!(
            cancel_lines(&refused),
            Err(RemoteError::Refused { code, hint: Some(_), .. }) if code == "check_run_not_found"
        ));
    }
}
