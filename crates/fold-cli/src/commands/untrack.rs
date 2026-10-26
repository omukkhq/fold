use std::io::Write;
use std::path::{Component, Path, PathBuf};

use clap::Args;
use fold_cli::daemon_client::Response;
use fold_core::{FoldError, codes};
use hyper::Method;
use serde_json::json;

use super::remote::{RemoteError, find_project, refusal, request_project_with};
use crate::output::{self, Item};

#[derive(Args, Debug)]
pub struct UntrackArgs {
    /// Files or directories to stop tracking, relative to the current directory; a directory stands for every tracked file below it
    #[arg(required = true, value_name = "PATH")]
    paths: Vec<PathBuf>,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

/// The most paths the command lists in each group; the rest are counted.
const LISTED_PATHS: usize = 20;

/// Asks the daemon to remove the paths from main in one revision of the human, leaving the files in the project folder, starting the daemon when none runs.
pub fn run_untrack(args: &UntrackArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let cwd = std::env::current_dir().map_err(RemoteError::Cwd)?;
    let start = args.project.clone().unwrap_or_else(|| cwd.clone());
    let root = std::fs::canonicalize(find_project(&start)).map_err(RemoteError::Cwd)?;
    let cwd = std::fs::canonicalize(&cwd).map_err(RemoteError::Cwd)?;
    let paths = args
        .paths
        .iter()
        .map(|path| project_path(&root, &cwd, path))
        .collect::<Result<Vec<_>, _>>()?;
    let body = json!({ "paths": paths });
    let response = request_project_with(
        args.socket.as_deref(),
        args.project.as_deref(),
        Method::POST,
        "untrack",
        Some(&body),
    )?;
    for line in summary(&response)? {
        output::line(out, &line)?;
    }
    Ok(())
}

/// `path`, given relative to `cwd` or absolute, as a path relative to the project folder `root`, with `.` and `..` resolved by name and no file read, so a deleted file can be named.
fn project_path(root: &Path, cwd: &Path, path: &Path) -> Result<String, FoldError> {
    let mut full = PathBuf::new();
    for component in cwd.join(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                full.pop();
            }
            other => full.push(other),
        }
    }
    let outside = || {
        FoldError::new(
            codes::INVALID_PATH,
            format!(
                "{} is not inside the project folder {}",
                path.display(),
                root.display()
            ),
        )
        .with_hint("name a file or directory inside the project folder")
    };
    let relative = full.strip_prefix(root).map_err(|_| outside())?;
    let text = relative.to_str().ok_or_else(outside)?;
    if text.is_empty() {
        return Err(outside());
    }
    Ok(text.to_owned())
}

/// The lines `fld untrack` prints for the daemon's answer, or the error it carries.
fn summary(response: &Response) -> Result<Vec<String>, RemoteError> {
    refusal(response)?;
    let body = &response.body;
    let paths = |key: &str| -> Result<Vec<String>, RemoteError> {
        body[key]
            .as_array()
            .ok_or(RemoteError::Answer)?
            .iter()
            .map(|path| path.as_str().map(output::escape_control))
            .collect::<Option<_>>()
            .ok_or(RemoteError::Answer)
    };
    let revision = body["revision"].as_str().ok_or(RemoteError::Answer)?;
    let files = paths("files")?;
    let unignored = paths("unignored")?;
    let commit = body["commit"]
        .as_str()
        .map(|commit| format!(", Git commit {}", commit.get(..7).unwrap_or(commit)))
        .unwrap_or_default();
    let mut lines = super::stash::finished_unstash_lines(body)?;
    let mut items = Vec::new();
    let mut removed = Item::new(format!(
        "Stopped tracking {} in main as {revision}{commit}",
        output::count(files.len(), "file", "files")
    ));
    list(&mut removed, &files);
    removed.detail("The files stay in the project folder. Older revisions keep them, so a secret that main held still needs to be changed");
    items.push(removed);
    if !unignored.is_empty() {
        let mut again = Item::new(format!(
            "Not ignored: {}, which the next `fld record` records again",
            output::count(unignored.len(), "file", "files")
        ));
        list(&mut again, &unignored);
        again.detail("Next: add them to .foldignore, or move them out of the project folder");
        items.push(again);
    }
    for touched in body["touched"].as_array().ok_or(RemoteError::Answer)? {
        let (Some(task), Some(workspace)) =
            (touched["task"].as_str(), touched["workspace"].as_str())
        else {
            return Err(RemoteError::Answer);
        };
        items.push(Item::new(format!(
            "{task} changes the same files in {workspace}; fold tells its lead and the main agent"
        )));
    }
    lines.extend(output::items(items));
    Ok(lines)
}

/// One detail per path, up to [`LISTED_PATHS`], then a count of the rest.
fn list(item: &mut Item, paths: &[String]) {
    for path in paths.iter().take(LISTED_PATHS) {
        item.detail(path);
    }
    if paths.len() > LISTED_PATHS {
        item.detail(format!("and {} more", paths.len() - LISTED_PATHS));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn lines(body: Value) -> Vec<String> {
        summary(&Response { status: 200, body }).unwrap()
    }

    #[test]
    fn the_removed_files_and_the_ones_record_takes_again_are_listed() {
        assert_eq!(
            lines(json!({
                "revision": "r5", "commit": "0123456789abcdef0123456789abcdef01234567",
                "files": ["config/local.env", "notes.md"], "unignored": ["notes.md"],
                "touched": [{"task": "T-2", "workspace": "ws-3"}], "unstash": null,
            })),
            [
                "Stopped tracking 2 files in main as r5, Git commit 0123456",
                "  ↳ config/local.env",
                "  ↳ notes.md",
                "  ↳ The files stay in the project folder. Older revisions keep them, so a secret that main held still needs to be changed",
                "",
                "Not ignored: 1 file, which the next `fld record` records again",
                "  ↳ notes.md",
                "  ↳ Next: add them to .foldignore, or move them out of the project folder",
                "",
                "T-2 changes the same files in ws-3; fold tells its lead and the main agent",
            ]
        );
        let many: Vec<String> = (0..22).map(|n| format!("gen/{n}.rs")).collect();
        let long = lines(json!({
            "revision": "r6", "commit": null, "files": many, "unignored": [], "touched": [], "unstash": null,
        }));
        assert_eq!(long[0], "Stopped tracking 22 files in main as r6");
        assert_eq!(long[LISTED_PATHS + 1], "  ↳ and 2 more");
        assert_eq!(long.len(), LISTED_PATHS + 3);
    }

    #[test]
    fn a_strange_or_refused_answer_is_an_error() {
        assert!(matches!(
            summary(&Response {
                status: 200,
                body: json!({"revision": "r2", "files": "a"}),
            }),
            Err(RemoteError::Answer)
        ));
        let refused = json!({"error": {"code": "path_not_tracked", "message": "main does not track a.txt", "details": {"hint": "name a file"}}});
        let error = summary(&Response {
            status: 404,
            body: refused,
        })
        .unwrap_err();
        assert_eq!(error.code(), Some("path_not_tracked"));
    }

    #[test]
    fn paths_resolve_against_the_current_directory_inside_the_project() {
        let root = Path::new("/work/demo");
        let cwd = Path::new("/work/demo/src");
        assert_eq!(
            project_path(root, cwd, Path::new("lib.rs")).unwrap(),
            "src/lib.rs"
        );
        assert_eq!(
            project_path(root, cwd, Path::new("../config/./local.env")).unwrap(),
            "config/local.env"
        );
        assert_eq!(
            project_path(root, cwd, Path::new("/work/demo/build/")).unwrap(),
            "build"
        );
        for outside in ["..", "../../other/x", "/etc/passwd"] {
            let error = project_path(root, cwd, Path::new(outside)).unwrap_err();
            assert_eq!(error.code(), codes::INVALID_PATH, "{outside}");
        }
    }
}
