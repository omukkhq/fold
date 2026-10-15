use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::Args;
use fold_core::ProjectPaths;
use fold_daemon::services::workspaces::{GitExport, WorkspaceError, WorkspaceService};
use fold_vcs::git::Git;
use thiserror::Error;

use crate::output;

#[derive(Args, Debug)]
pub struct ExportGitArgs {
    /// The Git repository that receives the commits; created as a bare repository when the path does not exist or is an empty directory
    repo: PathBuf,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum ExportGitError {
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error("cannot write the output: {0}")]
    Write(#[from] io::Error),
}

impl ExportGitError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Workspace(error) => error.code(),
            Self::Runtime(_) | Self::Write(_) => None,
        }
    }
}

pub fn run(args: &ExportGitArgs, out: &mut impl Write) -> Result<(), ExportGitError> {
    let start = args.project.clone().unwrap_or_else(|| PathBuf::from("."));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(ExportGitError::Runtime)?;
    let exported = runtime.block_on(async {
        let service =
            WorkspaceService::open_local(ProjectPaths::new(find_project(&start)), Git::default())
                .await?;
        let exported = service.export_git(&args.repo).await;
        service.close().await;
        exported
    })?;
    let repo = args.repo.display();
    let text = match exported {
        GitExport::UpToDate { last: Some(last) } => {
            format!("main is up to date in {repo}: {last} is exported")
        }
        GitExport::UpToDate { last: None } => format!("main is up to date in {repo}"),
        GitExport::Exported {
            first,
            last,
            commits,
            head,
        } => format!(
            "exported {} to {repo}: {}, main at {head}",
            output::revision_range(&first, &last),
            output::count(commits, "commit", "commits")
        ),
    };
    output::line(out, &text)?;
    Ok(())
}

/// The nearest directory at or above `start` that holds a project database, or `start` itself when none does, so the error names the directory the user gave.
fn find_project(start: &Path) -> PathBuf {
    let absolute = std::path::absolute(start).unwrap_or_else(|_| start.to_owned());
    absolute
        .ancestors()
        .find(|dir| ProjectPaths::new(*dir).db().is_file())
        .map_or(absolute.clone(), Path::to_owned)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct Wrapper {
        #[command(flatten)]
        args: ExportGitArgs,
    }

    #[test]
    fn export_git_parses_the_repository_and_the_project() {
        let parsed = Wrapper::try_parse_from(["fold", "../out", "--project", "app"]).unwrap();
        assert_eq!(parsed.args.repo, Path::new("../out"));
        assert_eq!(parsed.args.project.as_deref(), Some(Path::new("app")));
        assert!(Wrapper::try_parse_from(["fold"]).is_err());
    }

    #[test]
    fn the_project_is_found_above_the_start_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir_all(root.join(".fold")).unwrap();
        std::fs::write(root.join(".fold/fold.db"), "").unwrap();
        std::fs::create_dir_all(root.join("ws/one/src")).unwrap();
        assert_eq!(find_project(&root.join("ws/one/src")), root);
        assert_eq!(find_project(&root), root);
        let outside = dir.path().join("elsewhere");
        assert_eq!(find_project(&outside), outside);
    }

    #[test]
    fn a_run_exports_then_reports_up_to_date_and_names_a_missing_project() {
        let dir = tempfile::TempDir::new().unwrap();
        let host = fold_core::HostPaths::new(dir.path().join("data").to_str(), None).unwrap();
        let root = dir.path().join("app");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime
            .block_on(fold_daemon::services::projects::init_project(
                &host,
                &root,
                None,
                &fold_core::ManualClock::new(),
            ))
            .unwrap();
        let repo = dir.path().join("out");
        let args = ExportGitArgs {
            repo: repo.clone(),
            project: Some(root.join("main")),
        };
        let mut out = Vec::new();
        run(&args, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.starts_with(&format!(
                "exported r1 to {}: 1 commit, main at ",
                repo.display()
            )),
            "{text}"
        );
        let mut out = Vec::new();
        run(&args, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("main is up to date in {}: r1 is exported\n", repo.display())
        );
        let missing = ExportGitArgs {
            repo,
            project: Some(dir.path().join("none")),
        };
        let error = run(&missing, &mut Vec::new()).unwrap_err();
        assert_eq!(error.code(), Some("project_not_found"));
    }

    #[test]
    fn errors_without_a_code_say_so() {
        assert_eq!(ExportGitError::Runtime(io::Error::other("x")).code(), None);
        assert_eq!(ExportGitError::from(io::Error::other("x")).code(), None);
    }
}
