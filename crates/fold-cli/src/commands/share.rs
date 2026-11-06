use std::io::Write;
use std::path::PathBuf;

use clap::Args;
use fold_cli::daemon_client::Response;
use hyper::Method;
use serde_json::{Value, json};

use super::remote::{RemoteError, find_project, refusal, request_project, request_project_with};
use super::untrack::project_path;
use crate::output::{self, Item};

#[derive(Args, Debug)]
pub struct ShareArgs {
    /// Ignored files or folders to share with every workspace, read-only, relative to the current directory
    #[arg(
        value_name = "PATH",
        required_unless_present_any = ["list", "dismiss"],
        conflicts_with_all = ["list", "dismiss"]
    )]
    paths: Vec<PathBuf>,
    /// List the shared paths and the ignored files workers cannot see
    #[arg(long, conflicts_with = "dismiss")]
    list: bool,
    /// End the Needs you items of shared files that changed while a worker worked, once you checked the files
    #[arg(long)]
    dismiss: bool,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct UnshareArgs {
    /// Shared paths to take off the list, relative to the current directory
    #[arg(required = true, value_name = "PATH")]
    paths: Vec<PathBuf>,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

/// Shares the paths with every workspace through read-only links, or lists the shared paths, starting the daemon when none runs.
pub fn run_share(args: &ShareArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    if args.dismiss {
        let response = request_project_with(
            args.socket.as_deref(),
            args.project.as_deref(),
            Method::POST,
            "shared/dismiss",
            None,
        )?;
        refusal(&response)?;
        let dismissed = response.body["dismissed"]
            .as_u64()
            .ok_or(RemoteError::Answer)?;
        let line = if dismissed == 0 {
            "Nothing to dismiss: no shared file changed while a worker worked".to_owned()
        } else {
            format!(
                "Dismissed {}",
                output::count(
                    usize::try_from(dismissed).unwrap_or(usize::MAX),
                    "change of shared files",
                    "changes of shared files"
                )
            )
        };
        output::line(out, &line)?;
        return Ok(());
    }
    if args.list {
        let response = request_project(
            args.socket.as_deref(),
            args.project.as_deref(),
            Method::GET,
            "shared",
        )?;
        for line in list_lines(&response)? {
            output::line(out, &line)?;
        }
        return Ok(());
    }
    let response = change(
        args.socket.as_ref(),
        args.project.as_ref(),
        &args.paths,
        "share",
    )?;
    refusal(&response)?;
    let added = names(&response.body["added"])?;
    let line = if added.is_empty() {
        "Nothing new to share: every path is shared already".to_owned()
    } else {
        format!(
            "Shared read-only with every workspace: {}. Each workspace links to it at its next create or sync",
            added.join(", ")
        )
    };
    output::line(out, &line)?;
    Ok(())
}

/// Takes the paths off the shared list; each workspace drops its link at its next sync.
pub fn run_unshare(args: &UnshareArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let response = change(
        args.socket.as_ref(),
        args.project.as_ref(),
        &args.paths,
        "unshare",
    )?;
    refusal(&response)?;
    let removed = names(&response.body["removed"])?;
    let line = if removed.is_empty() {
        "Nothing to unshare: no path named is shared".to_owned()
    } else {
        format!(
            "No longer shared: {}. Each workspace drops its link at its next sync",
            removed.join(", ")
        )
    };
    output::line(out, &line)?;
    Ok(())
}

fn change(
    socket: Option<&PathBuf>,
    project: Option<&PathBuf>,
    paths: &[PathBuf],
    action: &str,
) -> Result<Response, RemoteError> {
    let cwd = std::env::current_dir().map_err(RemoteError::Cwd)?;
    let start = project.cloned().unwrap_or_else(|| cwd.clone());
    let root = std::fs::canonicalize(find_project(&start)).map_err(RemoteError::Cwd)?;
    let cwd = std::fs::canonicalize(&cwd).map_err(RemoteError::Cwd)?;
    let paths = paths
        .iter()
        .map(|path| project_path(&root, &cwd, path))
        .collect::<Result<Vec<_>, _>>()?;
    request_project_with(
        socket.map(PathBuf::as_path),
        project.map(PathBuf::as_path),
        Method::POST,
        action,
        Some(&json!({ "paths": paths })),
    )
}

fn names(value: &Value) -> Result<Vec<String>, RemoteError> {
    value
        .as_array()
        .ok_or(RemoteError::Answer)?
        .iter()
        .map(|name| name.as_str().map(output::escape_control))
        .collect::<Option<_>>()
        .ok_or(RemoteError::Answer)
}

/// The lines of `fld share --list`: the shared paths, then the ignored files and folders no worker sees.
fn list_lines(response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let body = &response.body;
    let shared = names(&body["shared"])?;
    let mut items = Vec::new();
    if shared.is_empty() {
        items.push(Item::new("No shared paths"));
    } else {
        let mut item = Item::new(format!(
            "Shared read-only with every workspace: {}",
            output::count(shared.len(), "path", "paths")
        ));
        for path in &shared {
            item.detail(path);
        }
        items.push(item);
    }
    if let Some(note) = unseen_note(body)? {
        items.push(Item::new(note));
    }
    Ok(output::items(items))
}

/// The line that names the ignored files and folders no worker sees, or `None` when there are none.
pub(super) fn unseen_note(body: &Value) -> Result<Option<String>, RemoteError> {
    let unseen = names(&body["unseen"])?;
    let count = body["unseen_count"].as_u64().ok_or(RemoteError::Answer)?;
    if count == 0 {
        return Ok(None);
    }
    let listed = unseen.len() as u64;
    let more = if count > listed {
        format!(" and {} more", count - listed)
    } else {
        String::new()
    };
    Ok(Some(format!(
        "Workers cannot see {}: {}{more}. `fld share <path>` shares one with them, read-only",
        output::count(
            usize::try_from(count).unwrap_or(usize::MAX),
            "ignored path",
            "ignored paths"
        ),
        unseen.join(", "),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unseen_note_names_the_first_paths_and_counts_the_rest() {
        let none = json!({"unseen": [], "unseen_count": 0});
        assert_eq!(unseen_note(&none).unwrap(), None);
        let some = json!({"unseen": [".env", "design/"], "unseen_count": 5});
        assert_eq!(
            unseen_note(&some).unwrap().unwrap(),
            "Workers cannot see 5 ignored paths: .env, design/ and 3 more. `fld share <path>` shares one with them, read-only"
        );
        let one = json!({"unseen": ["notes.md"], "unseen_count": 1});
        assert!(
            unseen_note(&one)
                .unwrap()
                .unwrap()
                .starts_with("Workers cannot see 1 ignored path: notes.md.")
        );
        assert!(unseen_note(&json!({})).is_err());
    }
}
