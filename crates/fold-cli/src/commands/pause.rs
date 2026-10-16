use std::io::Write;
use std::path::PathBuf;

use clap::Args;
use fold_cli::daemon_client::Response;

use super::remote::{RemoteError, post_to_project, refusal};
use crate::output;

#[derive(Args, Debug)]
pub struct PauseArgs {
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

/// Asks the daemon to pause every agent of the project, starting the daemon when none runs.
pub fn run_pause(args: &PauseArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let response = post_to_project(args.socket.as_deref(), args.project.as_deref(), "pause")?;
    output::line(out, &summary(&response)?)?;
    Ok(())
}

/// Asks the daemon to resume every agent the pause stopped, starting the daemon when none runs.
pub fn run_resume(args: &PauseArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let response = post_to_project(args.socket.as_deref(), args.project.as_deref(), "resume")?;
    output::line(out, &summary(&response)?)?;
    Ok(())
}

/// The line both commands print for the daemon's answer, or the error it carries.
fn summary(response: &Response) -> Result<String, RemoteError> {
    refusal(response)?;
    let body = &response.body;
    if body["paused_since"].is_null() {
        return Ok("the agents run: fold starts and resumes workers as usual".to_owned());
    }
    let by = body["paused_by"].as_str().ok_or(RemoteError::Answer)?;
    Ok(format!(
        "the agents are paused by {}: every worker is stopped and keeps its session; run fld resume to start them again",
        output::person(by)
    ))
}
