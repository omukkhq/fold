use std::io::Write;
use std::path::PathBuf;

use clap::{Args, Subcommand};
use fold_cli::daemon_client::Response;
use fold_core::Timestamp;
use hyper::Method;
use serde_json::Value;

use super::remote::{RemoteError, refusal, request_project};
use crate::output;

#[derive(Args, Debug)]
pub struct StashArgs {
    #[command(subcommand)]
    command: Option<StashCommand>,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR", global = true)]
    project: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum StashCommand {
    /// List the stash, newest first: stash@{0}, the time, the revision the project folder held and the changed files
    List,
}

#[derive(Args, Debug)]
pub struct UnstashArgs {
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

/// The most paths the commands list for one answer or entry; the rest are counted.
const LISTED_PATHS: usize = 20;

/// Asks the daemon to put the changes of the project folder aside, or lists the stash, starting the daemon when none runs.
pub fn run_stash(args: &StashArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let list = matches!(args.command, Some(StashCommand::List));
    let method = if list { Method::GET } else { Method::POST };
    let response = request_project(
        args.socket.as_deref(),
        args.project.as_deref(),
        method,
        "stash",
    )?;
    let lines = if list {
        list_lines(&response)?
    } else {
        stash_lines(&response)?
    };
    for line in lines {
        output::line(out, &line)?;
    }
    Ok(())
}

/// Asks the daemon to merge the newest entry of the stash into the project folder, starting the daemon when none runs.
pub fn run_unstash(args: &UnstashArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let response = request_project(
        args.socket.as_deref(),
        args.project.as_deref(),
        Method::POST,
        "unstash",
    )?;
    for line in unstash_lines(&response)? {
        output::line(out, &line)?;
    }
    Ok(())
}

fn text<'a>(body: &'a Value, key: &str) -> Result<&'a str, RemoteError> {
    body[key].as_str().ok_or(RemoteError::Answer)
}

fn list<'a>(body: &'a Value, key: &str) -> Result<&'a Vec<Value>, RemoteError> {
    body[key].as_array().ok_or(RemoteError::Answer)
}

fn files_count(files: &[Value]) -> String {
    output::count(files.len(), "file", "files")
}

/// One indented line per path, up to [`LISTED_PATHS`], then a count of the rest.
fn push_paths(lines: &mut Vec<String>, paths: &[Value]) -> Result<(), RemoteError> {
    for path in paths.iter().take(LISTED_PATHS) {
        lines.push(format!("  {}", path.as_str().ok_or(RemoteError::Answer)?));
    }
    if paths.len() > LISTED_PATHS {
        lines.push(format!("  and {} more", paths.len() - LISTED_PATHS));
    }
    Ok(())
}

fn stash_lines(response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let body = &response.body;
    let revision = text(body, "revision")?;
    let mut lines = match text(body, "outcome")? {
        "nothing" => vec![format!(
            "nothing to stash: the project folder has no change that is not recorded (it holds {revision})"
        )],
        "stashed" => {
            let files = list(body, "files")?;
            let mut lines = vec![format!(
                "stashed your changes as stash@{{0}}: {}; the project folder holds main {revision}. fld unstash brings them back",
                files_count(files)
            )];
            push_paths(&mut lines, files)?;
            lines
        }
        _ => return Err(RemoteError::Answer),
    };
    for skipped in list(body, "skipped")? {
        let (path, reason) = (text(skipped, "path")?, text(skipped, "reason")?);
        lines.push(format!(
            "{path} stays in the folder: {}",
            skipped_reason(reason)
        ));
    }
    Ok(lines)
}

/// Why fold leaves a new file out of the stash, as words.
fn skipped_reason(reason: &str) -> &str {
    match reason {
        "too_large" => "larger than the limit",
        "special" => "not a file, a directory or a symlink",
        "unreadable" => "not readable",
        other => other,
    }
}

fn unstash_lines(response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let body = &response.body;
    let files = list(body, "files")?;
    match text(body, "outcome")? {
        "applied" => {
            let mut lines = vec![format!(
                "applied stash@{{0}} to the project folder: {}, which are your changes that no revision records again; fld record records them",
                files_count(files)
            )];
            push_paths(&mut lines, files)?;
            Ok(lines)
        }
        "conflicted" => {
            let conflicted = list(body, "conflicted")?;
            let mut lines = vec![format!(
                "stash@{{0}} conflicts with the project folder ({}) in {}. fold wrote conflict markers and keeps the entry; the project folder does not follow main until you fix the files and run fld record",
                text(body, "revision")?,
                output::count(conflicted.len(), "path", "paths")
            )];
            for conflict in conflicted.iter().take(LISTED_PATHS) {
                let (path, kind) = (text(conflict, "path")?, text(conflict, "kind")?);
                lines.push(match conflict["sidecar"].as_str() {
                    Some(sidecar) => format!(
                        "  {path} ({kind}): the stash's side is in {sidecar}; delete it when the file is as you want"
                    ),
                    None => format!("  {path} ({kind})"),
                });
            }
            if conflicted.len() > LISTED_PATHS {
                lines.push(format!("  and {} more", conflicted.len() - LISTED_PATHS));
            }
            Ok(lines)
        }
        _ => Err(RemoteError::Answer),
    }
}

fn list_lines(response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let entries = list(&response.body, "entries")?;
    if entries.is_empty() {
        return Ok(vec!["the stash is empty".to_owned()]);
    }
    let mut lines = Vec::new();
    for entry in entries {
        let at = entry["created_at"]
            .as_i64()
            .and_then(|millis| Timestamp::from_millis(millis).ok())
            .ok_or(RemoteError::Answer)?;
        let files = list(entry, "files")?;
        let conflicted = list(entry, "conflicted")?;
        let waiting = if conflicted.is_empty() {
            String::new()
        } else {
            format!(
                ", its unstash conflicts in {}: fix them, then run fld record",
                output::count(conflicted.len(), "path", "paths")
            )
        };
        lines.push(format!(
            "{} {at} base {} by {}: {}{waiting}",
            text(entry, "stash")?,
            text(entry, "base")?,
            output::person(text(entry, "actor")?),
            files_count(files)
        ));
        push_paths(&mut lines, files)?;
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn reply(body: Value) -> Response {
        Response { status: 200, body }
    }

    #[test]
    fn a_stash_reads_as_lines() {
        assert_eq!(
            stash_lines(&reply(
                json!({"outcome": "nothing", "revision": "r3", "files": [], "entries": 0, "skipped": []})
            ))
            .unwrap(),
            [
                "nothing to stash: the project folder has no change that is not recorded (it holds r3)"
            ]
        );
        assert_eq!(
            stash_lines(&reply(json!({"outcome": "stashed", "revision": "r4", "files": ["a.txt", "b/c.md"], "entries": 1, "skipped": [{"path": "video.mp4", "reason": "too_large"}, {"path": "pipe", "reason": "special"}, {"path": "secret", "reason": "unreadable"}, {"path": "odd", "reason": "other"}]}))).unwrap(),
            [
                "stashed your changes as stash@{0}: 2 files; the project folder holds main r4. fld unstash brings them back",
                "  a.txt",
                "  b/c.md",
                "video.mp4 stays in the folder: larger than the limit",
                "pipe stays in the folder: not a file, a directory or a symlink",
                "secret stays in the folder: not readable",
                "odd stays in the folder: other",
            ]
        );
        assert!(matches!(
            stash_lines(&reply(json!({"outcome": "strange", "revision": "r1"}))),
            Err(RemoteError::Answer)
        ));
    }

    #[test]
    fn an_unstash_reads_as_lines_and_names_each_conflict() {
        let files: Vec<String> = (0..22).map(|n| format!("f{n}")).collect();
        let applied = unstash_lines(&reply(json!({
            "outcome": "applied", "revision": null, "files": files, "conflicted": [], "entries": 0,
        })))
        .unwrap();
        assert_eq!(
            applied[0],
            "applied stash@{0} to the project folder: 22 files, which are your changes that no revision records again; fld record records them"
        );
        assert_eq!(applied.len(), 1 + LISTED_PATHS + 1);
        assert_eq!(applied.last().unwrap(), "  and 2 more");
        let conflicted = unstash_lines(&reply(json!({
            "outcome": "conflicted", "revision": "r8", "files": ["a.txt", "logo.png"], "entries": 1,
            "conflicted": [
                {"path": "a.txt", "kind": "both_modified_text", "sidecar": null},
                {"path": "logo.png", "kind": "both_modified_binary", "sidecar": "logo.png.fold-stash"},
            ],
        })))
        .unwrap();
        assert_eq!(
            conflicted,
            [
                "stash@{0} conflicts with the project folder (r8) in 2 paths. fold wrote conflict markers and keeps the entry; the project folder does not follow main until you fix the files and run fld record",
                "  a.txt (both_modified_text)",
                "  logo.png (both_modified_binary): the stash's side is in logo.png.fold-stash; delete it when the file is as you want",
            ]
        );
        let refused = Response {
            status: 400,
            body: json!({"error": {"code": "invalid_request", "message": "the stash is empty", "details": {"hint": "fld stash puts the changes of the project folder aside", "reason": "stash_empty"}}}),
        };
        assert_eq!(
            unstash_lines(&refused).unwrap_err().code(),
            Some("invalid_request")
        );
    }

    #[test]
    fn the_list_shows_each_entry_newest_first_with_its_files() {
        assert_eq!(
            list_lines(&reply(json!({"entries": []}))).unwrap(),
            ["the stash is empty"]
        );
        let lines = list_lines(&reply(json!({"entries": [
            {"stash": "stash@{0}", "created_at": 1_000, "base": "r8", "actor": "human:ana", "files": ["a.txt"], "conflicted": ["a.txt"]},
            {"stash": "stash@{1}", "created_at": 0, "base": "r5", "actor": "human:ana", "files": ["b.txt", "c.txt"], "conflicted": []},
        ]})))
        .unwrap();
        assert_eq!(
            lines,
            [
                "stash@{0} 1970-01-01T00:00:01.000Z base r8 by @ana: 1 file, its unstash conflicts in 1 path: fix them, then run fld record",
                "  a.txt",
                "stash@{1} 1970-01-01T00:00:00.000Z base r5 by @ana: 2 files",
                "  b.txt",
                "  c.txt",
            ]
        );
        assert!(matches!(
            list_lines(&reply(json!({"entries": [{"stash": "stash@{0}"}]}))),
            Err(RemoteError::Answer)
        ));
    }
}
