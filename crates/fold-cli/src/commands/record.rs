use std::io::Write;
use std::path::PathBuf;

use clap::Args;
use fold_cli::daemon_client::Response;

use super::remote::{RemoteError, post_to_project, refusal};
use crate::output;

#[derive(Args, Debug)]
pub struct RecordArgs {
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

/// The most changed or conflicted paths the command lists; the rest are counted.
const LISTED_PATHS: usize = 20;

/// Asks the daemon to record the human's changes in the project folder on main, starting the daemon when none runs.
pub fn run_record(args: &RecordArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let response = post_to_project(args.socket.as_deref(), args.project.as_deref(), "record")?;
    for line in summary(&response)? {
        output::line(out, &line)?;
    }
    Ok(())
}

/// The lines `fld record` prints for the daemon's answer, or the error it carries: one line, then one per listed path, touched workspace and skipped file.
fn summary(response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let body = &response.body;
    let text = |key: &str| body[key].as_str().ok_or(RemoteError::Answer);
    let list = |key: &str| body[key].as_array().ok_or(RemoteError::Answer);
    let short = |commit: &str| commit.get(..7).unwrap_or(commit).to_owned();
    let revision = text("revision")?;
    let mut lines = match text("outcome")? {
        "up_to_date" => {
            return Ok(vec![format!(
                "nothing to record: the project folder has no change that main does not hold (main is {revision})"
            )]);
        }
        "recorded" => {
            let files = list("files")?;
            let mut lines = vec![format!(
                "recorded {} in main as {revision}: {}; the project folder follows main",
                short(text("commit")?),
                output::count(files.len(), "file", "files")
            )];
            push_paths(
                &mut lines,
                files.iter().map(|file| file.as_str()),
                files.len(),
            )?;
            lines
        }
        "conflicted" => {
            let conflicted = list("conflicted")?;
            let paths = output::count(conflicted.len(), "path", "paths");
            let mut lines = vec![if body["created"].as_bool().ok_or(RemoteError::Answer)? {
                format!(
                    "{} conflicts with main {revision} in {paths}; main did not move. Your files stay as they are and the change waits in Needs you; ask the main agent to explain the conflict and prepare the merge for your OK",
                    short(text("commit")?)
                )
            } else {
                format!(
                    "{} waits for the main agent already ({paths} conflicted); nothing new was recorded",
                    short(text("commit")?)
                )
            }];
            for conflict in conflicted.iter().take(LISTED_PATHS) {
                let (Some(path), Some(kind)) =
                    (conflict["path"].as_str(), conflict["kind"].as_str())
                else {
                    return Err(RemoteError::Answer);
                };
                lines.push(format!("  {path} ({kind})"));
            }
            if conflicted.len() > LISTED_PATHS {
                lines.push(format!("  and {} more", conflicted.len() - LISTED_PATHS));
            }
            lines
        }
        _ => return Err(RemoteError::Answer),
    };
    for touched in list("touched")? {
        let (Some(task), Some(workspace)) =
            (touched["task"].as_str(), touched["workspace"].as_str())
        else {
            return Err(RemoteError::Answer);
        };
        lines.push(format!(
            "{task} changes the same files in {workspace}; fold tells its lead and the main agent"
        ));
    }
    for skipped in list("skipped")? {
        let (Some(path), Some(reason)) = (skipped["path"].as_str(), skipped["reason"].as_str())
        else {
            return Err(RemoteError::Answer);
        };
        lines.push(format!(
            "left out {path} ({reason}), as an import leaves it out"
        ));
    }
    Ok(lines)
}

/// One line per path, up to [`LISTED_PATHS`], then a count of the rest.
fn push_paths<'a>(
    lines: &mut Vec<String>,
    paths: impl Iterator<Item = Option<&'a str>>,
    count: usize,
) -> Result<(), RemoteError> {
    for path in paths.take(LISTED_PATHS) {
        lines.push(format!("  {}", path.ok_or(RemoteError::Answer)?));
    }
    if count > LISTED_PATHS {
        lines.push(format!("  and {} more", count - LISTED_PATHS));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn reply(status: u16, body: serde_json::Value) -> Response {
        Response { status, body }
    }

    fn lines(body: serde_json::Value) -> Vec<String> {
        summary(&reply(200, body)).unwrap()
    }

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn each_outcome_reads_as_lines() {
        assert_eq!(
            lines(
                json!({"outcome": "up_to_date", "commit": null, "revision": "r3", "files": [], "conflicted": [], "touched": [], "skipped": [], "created": false})
            ),
            [
                "nothing to record: the project folder has no change that main does not hold (main is r3)"
            ]
        );
        assert_eq!(
            lines(json!({
                "outcome": "recorded", "commit": COMMIT, "revision": "r4", "files": ["README.md", "docs/plan.md"],
                "conflicted": [], "touched": [{"task": "T-2", "workspace": "ws-3"}],
                "skipped": [{"path": "big.bin", "reason": "too_large"}], "created": true,
            })),
            [
                "recorded 0123456 in main as r4: 2 files; the project folder follows main",
                "  README.md",
                "  docs/plan.md",
                "T-2 changes the same files in ws-3; fold tells its lead and the main agent",
                "left out big.bin (too_large), as an import leaves it out",
            ]
        );
        let conflicted = lines(json!({
            "outcome": "conflicted", "commit": COMMIT, "revision": "r5", "files": [],
            "conflicted": [{"path": "a.txt", "kind": "both_modified_text"}], "touched": [], "skipped": [], "created": true,
        }));
        assert!(
            conflicted[0]
                .starts_with("0123456 conflicts with main r5 in 1 path; main did not move.")
        );
        assert!(conflicted[0].contains("main agent"));
        assert_eq!(conflicted[1], "  a.txt (both_modified_text)");
        let waiting = lines(json!({
            "outcome": "conflicted", "commit": COMMIT, "revision": "r5", "files": [],
            "conflicted": [{"path": "a.txt", "kind": "both_modified_text"}], "touched": [], "skipped": [], "created": false,
        }));
        assert_eq!(
            waiting[0],
            "0123456 waits for the main agent already (1 path conflicted); nothing new was recorded"
        );
    }

    #[test]
    fn a_long_list_is_counted_and_a_bad_answer_or_refusal_is_an_error() {
        let files: Vec<String> = (0..25).map(|n| format!("f{n}")).collect();
        let many = lines(json!({
            "outcome": "recorded", "commit": COMMIT, "revision": "r4", "files": files,
            "conflicted": [], "touched": [], "skipped": [], "created": true,
        }));
        assert_eq!(many.len(), 1 + LISTED_PATHS + 1);
        assert_eq!(many.last().unwrap(), "  and 5 more");
        assert!(matches!(
            summary(&reply(200, json!({"outcome": "strange", "revision": "r1"}))),
            Err(RemoteError::Answer)
        ));
        let refused = json!({"error": {"code": "human_required", "message": "only a human records the changes of the project folder", "details": {"hint": "ask a human"}}});
        let error = summary(&reply(403, refused)).unwrap_err();
        assert_eq!(error.code(), Some("human_required"));
    }
}
