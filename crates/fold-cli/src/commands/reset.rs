use std::io::{BufRead, Write};
use std::path::{Component, Path, PathBuf};

use clap::Args;
use fold_cli::daemon_client::Response;
use fold_core::{FoldError, codes};
use hyper::Method;
use serde_json::{Value, json};

use super::confirm::Consent;
use super::remote::{RemoteError, find_project, refusal, request_project, request_project_with};
use super::stash::{finished_unstash_lines, skipped_reason};
use crate::output;

/// The most paths the commands list; the rest are counted.
const LISTED_PATHS: usize = 20;

#[derive(Args, Debug)]
pub struct ResetArgs {
    /// Reset without the question, as a script needs
    #[arg(long, short = 'y')]
    yes: bool,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct RestoreArgs {
    /// The files or directories to put back, relative to the current directory
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// Restore without the question, as a script needs
    #[arg(long, short = 'y')]
    yes: bool,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

/// Shows what `fld reset` discards, asks as [`Consent`] decides, then asks the daemon to put the project folder back to main, starting the daemon when none runs.
pub fn run_reset(
    args: &ResetArgs,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<(), RemoteError> {
    let (socket, project) = (args.socket.as_deref(), args.project.as_deref());
    let status = request_project(socket, project, Method::GET, "folder")?;
    let preview = preview_lines(&status, None)?;
    let Some(preview) = preview else {
        return print(
            out,
            vec![
                "nothing to reset: the project folder has no change that is not recorded"
                    .to_owned(),
            ],
        );
    };
    print(out, preview)?;
    if !Consent::of(args.yes).ask(
        "reset the project folder? fold stashes these changes first as stash@{0}",
        "fld reset",
        input,
        out,
    )? {
        return print(out, vec!["nothing reset".to_owned()]);
    }
    let response = request_project(socket, project, Method::POST, "folder/reset")?;
    print(out, outcome_lines(&response)?)
}

/// [`run_reset`] for the named paths only, which the command turns into paths of the project folder.
pub fn run_restore(
    args: &RestoreArgs,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<(), RemoteError> {
    let cwd = std::env::current_dir().map_err(RemoteError::Cwd)?;
    let root = find_project(args.project.as_deref().unwrap_or(&cwd));
    let root = std::fs::canonicalize(&root).map_err(RemoteError::Cwd)?;
    let cwd = std::fs::canonicalize(&cwd).map_err(RemoteError::Cwd)?;
    let paths = args
        .paths
        .iter()
        .map(|path| project_path(&root, &cwd, path))
        .collect::<Result<Vec<_>, _>>()?;
    let (socket, project) = (args.socket.as_deref(), args.project.as_deref());
    let status = request_project(socket, project, Method::GET, "folder")?;
    let Some(preview) = preview_lines(&status, Some(&paths))? else {
        return print(
            out,
            vec![format!(
                "nothing to restore: {} holds no change that is not recorded",
                paths.join(", ")
            )],
        );
    };
    print(out, preview)?;
    if !Consent::of(args.yes).ask(
        "restore these files? fold stashes their changes first as stash@{0}",
        "fld restore",
        input,
        out,
    )? {
        return print(out, vec!["nothing restored".to_owned()]);
    }
    let response = request_project_with(
        socket,
        project,
        Method::POST,
        "folder/restore",
        Some(&json!({ "paths": paths })),
    )?;
    print(out, outcome_lines(&response)?)
}

fn print(out: &mut impl Write, lines: Vec<String>) -> Result<(), RemoteError> {
    for line in lines {
        output::line(out, &line)?;
    }
    Ok(())
}

/// `path`, given from `cwd`, as a path of the project folder `root`, without touching the file system, since a deleted file has no path to resolve.
fn project_path(root: &Path, cwd: &Path, path: &Path) -> Result<String, RemoteError> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(part) => parts.push(part.to_owned()),
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    let normal: PathBuf = std::iter::once(std::ffi::OsString::from("/"))
        .chain(parts)
        .collect();
    let outside = || {
        RemoteError::from(
            FoldError::new(
                codes::INVALID_REQUEST,
                format!("{} is not inside the project folder", path.display()),
            )
            .with_hint("name a file or directory of the project folder"),
        )
    };
    let relative = normal.strip_prefix(root).map_err(|_| outside())?;
    if relative.as_os_str().is_empty() {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            "the path is the whole project folder",
        )
        .with_hint("run `fld reset` to put the whole project folder back")
        .into());
    }
    relative.to_str().map(str::to_owned).ok_or_else(outside)
}

fn text<'a>(body: &'a Value, key: &str) -> Result<&'a str, RemoteError> {
    body[key].as_str().ok_or(RemoteError::Answer)
}

fn list<'a>(body: &'a Value, key: &str) -> Result<&'a Vec<Value>, RemoteError> {
    body[key].as_array().ok_or(RemoteError::Answer)
}

fn strings(values: &[Value]) -> Result<Vec<&str>, RemoteError> {
    values
        .iter()
        .map(|value| value.as_str().ok_or(RemoteError::Answer))
        .collect()
}

fn push_paths(lines: &mut Vec<String>, paths: &[&str]) {
    for path in paths.iter().take(LISTED_PATHS) {
        lines.push(format!("  {}", output::escape_control(path)));
    }
    if paths.len() > LISTED_PATHS {
        lines.push(format!("  and {} more", paths.len() - LISTED_PATHS));
    }
}

/// Whether `path` is one of `named` or below one of them.
fn named_by(path: &str, named: &[String]) -> bool {
    named.iter().any(|name| {
        path.strip_prefix(name.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

/// What a reset of the whole folder, or of `named` paths, discards, from `fld status`'s answer: the unrecorded changes and the human change that waits for the main agent. `None` when there is nothing to discard.
fn preview_lines(
    status: &Response,
    named: Option<&[String]>,
) -> Result<Option<Vec<String>>, RemoteError> {
    refusal(status)?;
    let body = &status.body;
    let mut changes = Vec::new();
    for change in list(body, "changes")? {
        let path = text(change, "path")?;
        if named.is_none_or(|named| named_by(path, named)) {
            changes.push(path);
        }
    }
    let waiting = list(body, "blockers")?
        .iter()
        .find(|blocker| blocker["kind"] == "human_change");
    let markers = list(body, "blockers")?
        .iter()
        .find(|blocker| blocker["kind"] == "unstash_conflicted");
    if changes.is_empty() && (named.is_some() || (waiting.is_none() && markers.is_none())) {
        return Ok(None);
    }
    let mut lines = Vec::new();
    if !changes.is_empty() {
        lines.push(format!(
            "these changes go back to main's files: {}",
            output::count(changes.len(), "file", "files")
        ));
        push_paths(&mut lines, &changes);
    }
    if named.is_none() {
        if let Some(blocker) = markers {
            lines.push(format!(
                "the unstash conflict in {} is dropped; stash@{{0}} keeps those files with their markers",
                output::count(list(blocker, "paths")?.len(), "path", "paths")
            ));
        }
        if let Some(blocker) = waiting {
            lines.push(format!(
                "your change that waits for the main agent is dropped, with the merge workspace it made: it conflicts in {}",
                output::count(list(blocker, "paths")?.len(), "path", "paths")
            ));
        }
    }
    Ok(Some(lines))
}

/// The lines of the daemon's answer to a reset or a restore.
fn outcome_lines(response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let body = &response.body;
    let mut lines = finished_unstash_lines(body)?;
    let files = strings(list(body, "files")?)?;
    let revision = text(body, "revision")?;
    let at_head = body["at_head"].as_bool().ok_or(RemoteError::Answer)?;
    let place = if at_head {
        format!("the project folder holds main {revision}")
    } else {
        format!("the project folder holds {revision}; `fld status` says what keeps it from main")
    };
    if body["stashed"].as_bool().ok_or(RemoteError::Answer)? {
        lines.push(format!(
            "put back {} and stashed their changes as stash@{{0}}; {place}. fld unstash brings them back",
            output::count(files.len(), "file", "files")
        ));
        push_paths(&mut lines, &files);
    } else {
        lines.push(format!("nothing to put back; {place}"));
    }
    let unchanged = strings(list(body, "unchanged")?)?;
    if !unchanged.is_empty() {
        lines.push(format!("no change at {}", unchanged.join(", ")));
    }
    let dropped_unstash = list(body, "dropped_unstash")?;
    if !dropped_unstash.is_empty() {
        lines.push(format!(
            "dropped the unstash conflict in {}",
            output::count(dropped_unstash.len(), "path", "paths")
        ));
    }
    let change = &body["dropped_change"];
    if !change.is_null() {
        let conflicted = list(change, "conflicted")?.len();
        lines.push(match change["workspace"].as_str() {
            Some(workspace) => format!(
                "dropped your change that waited for the main agent ({}) and abandoned {workspace}, which held its merge",
                output::count(conflicted, "conflicted path", "conflicted paths")
            ),
            None => format!(
                "dropped your change that waited for the main agent ({})",
                output::count(conflicted, "conflicted path", "conflicted paths")
            ),
        });
    }
    for skipped in list(body, "skipped")? {
        lines.push(format!(
            "{} stays in the folder: {}",
            output::escape_control(text(skipped, "path")?),
            skipped_reason(text(skipped, "reason")?)
        ));
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(body: Value) -> Response {
        Response { status: 200, body }
    }

    #[test]
    fn a_path_is_made_relative_to_the_project_folder() {
        let (root, cwd) = (Path::new("/p"), Path::new("/p/src"));
        assert_eq!(
            project_path(root, cwd, Path::new("a.rs")).unwrap(),
            "src/a.rs"
        );
        assert_eq!(
            project_path(root, cwd, Path::new("../plan.md")).unwrap(),
            "plan.md"
        );
        assert_eq!(
            project_path(root, cwd, Path::new("./x/../y")).unwrap(),
            "src/y"
        );
        assert_eq!(
            project_path(root, cwd, Path::new("/p/docs")).unwrap(),
            "docs"
        );
        assert_eq!(
            project_path(root, cwd, Path::new("../../etc"))
                .unwrap_err()
                .code(),
            Some("invalid_request")
        );
        let whole = project_path(root, cwd, Path::new("..")).unwrap_err();
        assert_eq!(
            whole.hint(),
            Some("run `fld reset` to put the whole project folder back")
        );
    }

    #[test]
    fn the_preview_names_the_changes_the_waiting_change_and_the_markers() {
        let status = reply(json!({
            "changes": [{"path": "plan.md"}, {"path": "src/a.rs"}, {"path": "srcs.md"}],
            "blockers": [
                {"kind": "human_change", "paths": ["plan.md"], "next": "x"},
                {"kind": "unstash_conflicted", "paths": ["a", "b"], "next": "x"},
            ],
        }));
        assert_eq!(
            preview_lines(&status, None).unwrap().unwrap(),
            [
                "these changes go back to main's files: 3 files",
                "  plan.md",
                "  src/a.rs",
                "  srcs.md",
                "the unstash conflict in 2 paths is dropped; stash@{0} keeps those files with their markers",
                "your change that waits for the main agent is dropped, with the merge workspace it made: it conflicts in 1 path",
            ]
        );
        assert_eq!(
            preview_lines(&status, Some(&["src".to_owned()]))
                .unwrap()
                .unwrap(),
            [
                "these changes go back to main's files: 1 file",
                "  src/a.rs"
            ]
        );
        assert_eq!(
            preview_lines(&status, Some(&["docs".to_owned()])).unwrap(),
            None
        );
        let clean = reply(json!({"changes": [], "blockers": []}));
        assert_eq!(preview_lines(&clean, None).unwrap(), None);
        let waiting = reply(
            json!({"changes": [], "blockers": [{"kind": "human_change", "paths": ["a"], "next": "x"}]}),
        );
        assert_eq!(preview_lines(&waiting, None).unwrap().unwrap().len(), 1);
    }

    #[test]
    fn the_outcome_names_the_safety_stash_and_what_was_dropped() {
        let lines = outcome_lines(&reply(json!({
            "revision": "r4", "at_head": true, "files": ["a", "plan.md"], "entries": 1, "stashed": true,
            "unchanged": [], "dropped_unstash": [], "skipped": [], "unstash": null,
            "dropped_change": {"commit": null, "conflicted": ["plan.md"], "workspace": "ws-9"},
        })))
        .unwrap();
        assert_eq!(
            lines,
            [
                "put back 2 files and stashed their changes as stash@{0}; the project folder holds main r4. fld unstash brings them back",
                "  a",
                "  plan.md",
                "dropped your change that waited for the main agent (1 conflicted path) and abandoned ws-9, which held its merge",
            ]
        );
        let lines = outcome_lines(&reply(json!({
            "revision": "r3", "at_head": false, "files": [], "entries": 0, "stashed": false,
            "unchanged": ["docs"], "dropped_unstash": ["x"], "skipped": [{"path": "big.bin", "reason": "too_large"}],
            "unstash": null, "dropped_change": {"commit": null, "conflicted": ["a", "b"], "workspace": null},
        })))
        .unwrap();
        assert_eq!(
            lines,
            [
                "nothing to put back; the project folder holds r3; `fld status` says what keeps it from main",
                "no change at docs",
                "dropped the unstash conflict in 1 path",
                "dropped your change that waited for the main agent (2 conflicted paths)",
                "big.bin stays in the folder: larger than the limit",
            ]
        );
        assert!(matches!(
            outcome_lines(&reply(json!({"revision": "r1"}))),
            Err(RemoteError::Answer)
        ));
    }
}
