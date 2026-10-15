use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use fold_cli::daemon_client::{self, ClientConfig, ClientError, DaemonClient, Response};
use fold_core::config::{GIT_SECTION, check_branch, check_remote};
use fold_core::{FoldError, ProjectConfig, ProjectPaths, codes};
use fold_daemon::services::projects::{ProjectError, slug_for_dir_on_host};
use fold_vcs::git::Git;
use fold_vcs::git_remote::{MIRROR_DIR, prepare_mirror};
use hyper::Method;
use thiserror::Error;
use toml_edit::{DocumentMut, Item, Table, TableLike, value};

use super::export_git::find_project;
use crate::serve::{self, Dirs};
use crate::{init, output};

#[derive(Args, Debug)]
pub struct RemoteArgs {
    #[command(subcommand)]
    command: RemoteCommand,
}

#[derive(Subcommand, Debug)]
enum RemoteCommand {
    /// Link the project to a branch of a remote Git repository and create the mirror fold pushes through
    Set(SetArgs),
}

#[derive(Args, Debug)]
struct SetArgs {
    /// The URL or path of the remote Git repository; your own Git credentials and configuration apply
    url: String,
    /// The remote branch fold syncs with
    #[arg(long, default_value = "main")]
    branch: String,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct PushArgs {
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

/// `fold pull` takes the same options as `fold push`.
pub type PullArgs = PushArgs;

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    Serve(#[from] serve::ServeError),
    #[error(transparent)]
    Init(#[from] init::InitError),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("{message}{}", hint.as_ref().map(|hint| format!(" ({hint})")).unwrap_or_default())]
    Refused {
        code: String,
        message: String,
        hint: Option<String>,
    },
    #[error("the daemon answered with a body fold cannot read")]
    Answer,
    #[error("cannot find the fold binary: {0}")]
    Exe(io::Error),
    #[error("cannot read the project directory: {0}")]
    Cwd(io::Error),
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error("cannot write the output: {0}")]
    Write(#[from] io::Error),
}

impl RemoteError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Fold(error) => Some(error.code()),
            Self::Project(error) => error.code(),
            Self::Serve(error) => error.code(),
            Self::Init(error) => error.code(),
            Self::Refused { code, .. } => Some(code),
            Self::Client(_)
            | Self::Answer
            | Self::Exe(_)
            | Self::Cwd(_)
            | Self::Runtime(_)
            | Self::Write(_) => None,
        }
    }
}

pub fn run_remote(args: &RemoteArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    match &args.command {
        RemoteCommand::Set(args) => set(args, out),
    }
}

/// Writes `remote` and `branch` into `[git]` of `.fold/config.toml`, keeping every other line and comment, after the mirror `.fold/git/` exists and points `origin` at the remote. It changes no database row, so it needs no daemon.
fn set(args: &SetArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let start = args.project.clone().unwrap_or_else(|| PathBuf::from("."));
    let paths = ProjectPaths::new(find_project(&start));
    if !paths.db().is_file() {
        return Err(FoldError::new(
            codes::PROJECT_NOT_FOUND,
            format!("{} is not a fold project", paths.root().display()),
        )
        .with_hint("run the command inside a project directory or pass --project")
        .into());
    }
    check_remote(&args.url)?;
    check_branch(&args.branch)?;
    let remote = absolute_if_local(&args.url)?;
    let config_path = paths.config();
    let text = match std::fs::read_to_string(&config_path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(config_io(&error).into()),
    };
    let edited = with_remote(&text, &remote, &args.branch)?;
    prepare_mirror(&Git::default(), &paths.fold_dir().join(MIRROR_DIR), &remote)?;
    write_config(&config_path, &edited)?;
    output::line(
        out,
        &format!(
            "remote set: fold syncs main with the branch {} of the remote",
            args.branch
        ),
    )?;
    Ok(())
}

/// A local path is made absolute from the working directory, whether or not it exists yet, because Git would resolve a relative one from the mirror. A remote is a URL, as Git reads it, when it holds `://` or a `:` before its first `/` (`host:path` and `<transport>::<address>`); it stays as given.
fn absolute_if_local(url: &str) -> Result<String, FoldError> {
    let colon = url.find(':');
    let is_url = url.contains("://")
        || colon.is_some_and(|colon| url.find('/').is_none_or(|slash| colon < slash));
    if is_url {
        return Ok(url.to_owned());
    }
    let path = Path::new(url);
    let absolute = match std::fs::canonicalize(path) {
        Ok(path) => path,
        Err(_) => std::path::absolute(path).map_err(|error| config_io(&error))?,
    };
    absolute.to_str().map(str::to_owned).ok_or_else(|| {
        FoldError::new(
            codes::INVALID_REQUEST,
            "the absolute path of the remote is not valid UTF-8",
        )
        .with_hint("pass the remote as an absolute path or a URL")
    })
}

/// `text` with `remote` and `branch` in `[git]` and `push_on_integrate` kept, or `false` when absent. The result must be a valid config, so a file that is already invalid is refused before anything is written.
fn with_remote(text: &str, remote: &str, branch: &str) -> Result<String, FoldError> {
    let mut doc: DocumentMut = text.parse().map_err(|_| {
        FoldError::new(codes::INVALID_CONFIG, "config.toml is not valid TOML")
            .with_hint("fix .fold/config.toml, then run fold remote set again")
    })?;
    let edited = match doc.get_mut(GIT_SECTION) {
        Some(item) => {
            let section = item.as_table_like_mut().ok_or_else(|| {
                FoldError::new(codes::INVALID_CONFIG, "config key git must be a table")
                    .with_details([("key", serde_json::json!(GIT_SECTION))])
            })?;
            fill(section, remote, branch);
            doc.to_string()
        }
        // A new section goes after the whole file, so the comments at its end stay where they are.
        None => {
            let mut section = Table::new();
            fill(&mut section, remote, branch);
            let mut added = DocumentMut::new();
            added.insert(GIT_SECTION, Item::Table(section));
            let separator = if text.is_empty() || text.ends_with('\n') {
                ""
            } else {
                "\n"
            };
            format!("{text}{separator}{added}")
        }
    };
    ProjectConfig::from_toml(&edited)?;
    Ok(edited)
}

fn fill(section: &mut dyn TableLike, remote: &str, branch: &str) {
    section.insert("remote", value(remote));
    section.insert("branch", value(branch));
    if !section.contains_key("push_on_integrate") {
        section.insert("push_on_integrate", value(false));
    }
}

/// Written to a new temporary file beside it and renamed over it, so a reader sees the old file or the new one and two writers never share a temporary file.
fn write_config(path: &Path, text: &str) -> Result<(), FoldError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(dir).map_err(|error| config_io(&error))?;
    temporary
        .write_all(text.as_bytes())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| config_io(&error))?;
    temporary
        .persist(path)
        .map_err(|error| config_io(&error.error))?;
    Ok(())
}

fn config_io(error: &io::Error) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_IO,
        format!("cannot read or write config.toml: {}", error.kind()),
    )
}

/// Asks the daemon to push main to the project's Git remote, starting the daemon when none runs.
pub fn run_push(args: &PushArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let response = post_project(args, "push")?;
    output::line(out, &push_summary(response)?)?;
    Ok(())
}

/// Asks the daemon to bring the remote branch into main, starting the daemon when none runs.
pub fn run_pull(args: &PullArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let response = post_project(args, "pull")?;
    for line in pull_summary(response)? {
        output::line(out, &line)?;
    }
    Ok(())
}

/// Sends `POST /api/v1/projects/<slug>/<action>` over the socket as the human in the environment.
fn post_project(args: &PushArgs, action: &str) -> Result<Response, RemoteError> {
    let dirs = Dirs::from_env();
    let host = init::host_paths(&dirs)?;
    let start = args.project.clone().unwrap_or_else(|| PathBuf::from("."));
    let dir = std::fs::canonicalize(&start).map_err(RemoteError::Cwd)?;
    let socket =
        serve::socket_path(args.socket.as_deref(), &dirs)?.ok_or(ClientError::RuntimeDirMissing)?;
    let config = ClientConfig {
        socket: std::path::absolute(&socket).map_err(RemoteError::Cwd)?,
        runtime_dir: serve::runtime_dir(&dirs).ok(),
        fold_exe: Some(std::env::current_exe().map_err(RemoteError::Exe)?),
    };
    let client = DaemonClient::new(config, daemon_client::human_from_env().ok());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(RemoteError::Runtime)?;
    runtime.block_on(async {
        let slug = slug_for_dir_on_host(&host, &dir).await?;
        let path = format!("/api/v1/projects/{slug}/{action}");
        Ok::<_, RemoteError>(client.request(Method::POST, &path, None).await?)
    })
}

/// The error a refusal carries.
fn refusal(response: &Response) -> Result<(), RemoteError> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    let error = &response.body["error"];
    Err(match (error["code"].as_str(), error["message"].as_str()) {
        (Some(code), Some(message)) => RemoteError::Refused {
            code: code.to_owned(),
            message: message.to_owned(),
            hint: error["details"]["hint"].as_str().map(str::to_owned),
        },
        _ => RemoteError::Answer,
    })
}

/// The line `fold push` prints for the daemon's answer, or the error it carries.
fn push_summary(response: Response) -> Result<String, RemoteError> {
    refusal(&response)?;
    let body = &response.body;
    let text = |key: &str| body[key].as_str();
    let branch = text("branch").ok_or(RemoteError::Answer)?;
    let range = || match (text("first"), text("last")) {
        (Some(first), Some(last)) => Ok(output::revision_range(&first, &last)),
        _ => Err(RemoteError::Answer),
    };
    let commits = || {
        body["commits"]
            .as_u64()
            .and_then(|count| usize::try_from(count).ok())
            .map(|count| output::count(count, "commit", "commits"))
            .ok_or(RemoteError::Answer)
    };
    match text("outcome") {
        Some("up_to_date") => Ok(match text("last") {
            Some(last) => format!("the remote branch {branch} is up to date: {last} is pushed"),
            None => format!("the remote branch {branch} is up to date"),
        }),
        Some("pushed") => Ok(format!("pushed {} to {branch}: {}", range()?, commits()?)),
        Some("recorded") => Ok(format!(
            "the remote branch {branch} already held {}: recorded {}",
            range()?,
            commits()?
        )),
        _ => Err(RemoteError::Answer),
    }
}

/// The lines `fold pull` prints for the daemon's answer, or the error it carries: one line, and for a conflict one more per conflicted path.
fn pull_summary(response: Response) -> Result<Vec<String>, RemoteError> {
    refusal(&response)?;
    let body = &response.body;
    let text = |key: &str| body[key].as_str().ok_or(RemoteError::Answer);
    let branch = text("branch")?;
    let short = |commit: &str| commit.get(..7).unwrap_or(commit).to_owned();
    match text("outcome")? {
        "up_to_date" => Ok(vec![match body["remote_commit"].as_str() {
            Some(commit) => format!(
                "main is up to date with {branch}: it holds {}",
                short(commit)
            ),
            None => format!("the remote has no branch {branch}; nothing to pull"),
        }]),
        "integrated" => Ok(vec![format!(
            "pulled {} from {branch} into main as {}",
            short(text("remote_commit")?),
            text("revision")?
        )]),
        "conflicted" => {
            let commit = short(text("remote_commit")?);
            let task = text("task")?;
            let conflicted = body["conflicted"].as_array().ok_or(RemoteError::Answer)?;
            let paths = output::count(conflicted.len(), "path", "paths");
            let mut lines = vec![if body["created"].as_bool().ok_or(RemoteError::Answer)? {
                format!(
                    "{commit} from {branch} conflicts with main in {paths}; main did not move. {task} resolves it in its workspace: an agent claims {task}, resolves the markers and opens a proposal"
                )
            } else {
                format!(
                    "{task} already resolves {commit} from {branch}; nothing was created ({paths} conflicted)"
                )
            }];
            for conflict in conflicted {
                let (Some(path), Some(kind)) =
                    (conflict["path"].as_str(), conflict["kind"].as_str())
                else {
                    return Err(RemoteError::Answer);
                };
                lines.push(format!("  {path} ({kind})"));
            }
            Ok(lines)
        }
        _ => Err(RemoteError::Answer),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use clap::Parser;
    use serde_json::json;

    use super::*;

    #[derive(Parser)]
    struct Remote {
        #[command(flatten)]
        args: RemoteArgs,
    }

    #[derive(Parser)]
    struct Push {
        #[command(flatten)]
        args: PushArgs,
    }

    #[test]
    fn remote_set_parses_the_url_the_branch_and_the_project() {
        let parsed = Remote::try_parse_from(["fold", "set", "../app.git"]).unwrap();
        let RemoteCommand::Set(set) = parsed.args.command;
        assert_eq!(
            (set.url.as_str(), set.branch.as_str()),
            ("../app.git", "main")
        );
        assert!(set.project.is_none());
        let parsed = Remote::try_parse_from([
            "fold",
            "set",
            "git@x:a.git",
            "--branch",
            "trunk",
            "--project",
            "app",
        ])
        .unwrap();
        let RemoteCommand::Set(set) = parsed.args.command;
        assert_eq!(set.branch, "trunk");
        assert_eq!(set.project.as_deref(), Some(Path::new("app")));
        assert!(Remote::try_parse_from(["fold", "set"]).is_err());
        let push = Push::try_parse_from(["fold", "--socket", "/f.sock"]).unwrap();
        assert_eq!(push.args.socket.as_deref(), Some(Path::new("/f.sock")));
    }

    #[test]
    fn the_git_section_is_written_and_everything_else_kept() {
        let text = "# Lease.\nlease_minutes = 7\n# [git]\n";
        let edited = with_remote(text, "/srv/app.git", "main").unwrap();
        assert_eq!(
            edited,
            format!(
                "{text}[git]\nremote = \"/srv/app.git\"\nbranch = \"main\"\npush_on_integrate = false\n"
            )
        );
        let unterminated = with_remote("lease_minutes = 7", "/r", "main").unwrap();
        assert!(unterminated.starts_with("lease_minutes = 7\n[git]\n"));
        let config = ProjectConfig::from_toml(&edited).unwrap();
        assert_eq!(config.lease_minutes, 7);
        assert_eq!(config.git.remote.as_deref(), Some("/srv/app.git"));
        assert!(!config.git.push_on_integrate);
        assert!(edited.contains("push_on_integrate = false"));

        let switched = "[git]\n# Kept.\npush_on_integrate = true\nremote = \"/old\"\n";
        let edited = with_remote(switched, "/new", "trunk").unwrap();
        assert!(edited.contains("# Kept.\npush_on_integrate = true\n"));
        let config = ProjectConfig::from_toml(&edited).unwrap();
        assert_eq!(config.git.remote.as_deref(), Some("/new"));
        assert_eq!(config.git.branch, "trunk");
        assert!(config.git.push_on_integrate);

        let inline = with_remote("git = { push_on_integrate = true }\n", "/r", "main").unwrap();
        let config = ProjectConfig::from_toml(&inline).unwrap();
        assert_eq!(config.git.remote.as_deref(), Some("/r"));
        assert!(config.git.push_on_integrate);

        for bad in ["= x", "git = 1", "lease_minutes = 0"] {
            assert_eq!(
                with_remote(bad, "/r", "main").unwrap_err().code(),
                codes::INVALID_CONFIG,
                "{bad}"
            );
        }
    }

    #[test]
    fn a_local_path_is_made_absolute_and_a_url_stays_as_given() {
        let dir = tempfile::TempDir::new().unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        assert_eq!(
            absolute_if_local(dir.path().to_str().unwrap()).unwrap(),
            canonical.to_str().unwrap()
        );
        let cwd = std::env::current_dir().unwrap();
        for relative in ["../new.git", "new.git", "./a:b"] {
            let absolute = absolute_if_local(relative).unwrap();
            assert!(Path::new(&absolute).is_absolute(), "{relative}");
            assert!(absolute.starts_with(cwd.to_str().unwrap()), "{relative}");
            assert!(
                absolute.ends_with(relative.trim_start_matches("./")),
                "{relative}"
            );
        }
        for url in [
            "https://user:token@example.com/a.git",
            "file:///srv/a.git",
            "git@example.com:a.git",
            "example.com:team/a.git",
            "gcrypt::https://example.com/a.git",
        ] {
            assert_eq!(absolute_if_local(url).unwrap(), url);
        }
    }

    fn reply(status: u16, body: serde_json::Value) -> Response {
        Response { status, body }
    }

    #[test]
    fn push_answers_read_as_one_line_or_the_error_they_carry() {
        let pushed = json!({"outcome": "pushed", "branch": "main", "first": "r1", "last": "r3", "commits": 3, "head": "a"});
        assert_eq!(
            push_summary(reply(200, pushed)).unwrap(),
            "pushed r1..r3 to main: 3 commits"
        );
        let recorded = json!({"outcome": "recorded", "branch": "main", "first": "r2", "last": "r2", "commits": 1});
        assert_eq!(
            push_summary(reply(200, recorded)).unwrap(),
            "the remote branch main already held r2: recorded 1 commit"
        );
        let current =
            json!({"outcome": "up_to_date", "branch": "main", "last": "r3", "commits": 0});
        assert_eq!(
            push_summary(reply(200, current)).unwrap(),
            "the remote branch main is up to date: r3 is pushed"
        );
        let empty = json!({"outcome": "up_to_date", "branch": "trunk", "last": null, "commits": 0});
        assert_eq!(
            push_summary(reply(200, empty)).unwrap(),
            "the remote branch trunk is up to date"
        );
        let refused = json!({"error": {"code": "remote_ahead", "message": "m", "details": {"hint": "run fold pull"}}});
        let error = push_summary(reply(409, refused)).unwrap_err();
        assert_eq!(error.code(), Some("remote_ahead"));
        assert_eq!(error.to_string(), "m (run fold pull)");
        for bad in [
            reply(500, json!(null)),
            reply(200, json!({"outcome": "pushed", "branch": "main"})),
            reply(
                200,
                json!({"outcome": "pushed", "branch": "main", "first": "r1", "last": "r1"}),
            ),
            reply(200, json!({"outcome": "other", "branch": "main"})),
            reply(200, json!({"outcome": "pushed"})),
        ] {
            assert!(matches!(push_summary(bad), Err(RemoteError::Answer)));
        }
    }

    #[test]
    fn pull_answers_read_as_lines_or_the_error_they_carry() {
        let commit = "0123456789abcdef0123456789abcdef01234567";
        let lines = |body| pull_summary(reply(200, body)).unwrap();
        assert_eq!(
            lines(json!({"outcome": "up_to_date", "branch": "main", "remote_commit": commit})),
            ["main is up to date with main: it holds 0123456"]
        );
        assert_eq!(
            lines(json!({"outcome": "up_to_date", "branch": "trunk", "remote_commit": null})),
            ["the remote has no branch trunk; nothing to pull"]
        );
        assert_eq!(
            lines(
                json!({"outcome": "integrated", "branch": "main", "remote_commit": commit, "revision": "r4"})
            ),
            ["pulled 0123456 from main into main as r4"]
        );
        let conflicted = |created: bool| {
            json!({"outcome": "conflicted", "branch": "main", "remote_commit": commit, "task": "T-3", "workspace": "ws-2", "created": created,
                "conflicted": [{"path": "a.txt", "kind": "both_modified_text"}]})
        };
        assert_eq!(
            lines(conflicted(true)),
            [
                "0123456 from main conflicts with main in 1 path; main did not move. T-3 resolves it in its workspace: an agent claims T-3, resolves the markers and opens a proposal",
                "  a.txt (both_modified_text)"
            ]
        );
        assert_eq!(
            lines(conflicted(false))[0],
            "T-3 already resolves 0123456 from main; nothing was created (1 path conflicted)"
        );
        let refused = json!({"error": {"code": "remote_not_set", "message": "m"}});
        let error = pull_summary(reply(409, refused)).unwrap_err();
        assert_eq!(error.code(), Some("remote_not_set"));
        for bad in [
            json!({"outcome": "integrated", "branch": "main", "remote_commit": commit}),
            json!({"outcome": "conflicted", "branch": "main", "remote_commit": commit, "task": "T-3", "created": true}),
            json!({"outcome": "conflicted", "branch": "main", "remote_commit": commit, "task": "T-3", "created": true, "conflicted": [{"path": "a"}]}),
            json!({"outcome": "conflicted", "branch": "main", "remote_commit": commit, "task": "T-3", "conflicted": []}),
            json!({"outcome": "other", "branch": "main"}),
            json!({"outcome": "integrated"}),
        ] {
            assert!(matches!(
                pull_summary(reply(200, bad)),
                Err(RemoteError::Answer)
            ));
        }
    }

    #[test]
    fn errors_without_a_code_say_so() {
        assert_eq!(RemoteError::Answer.code(), None);
        assert_eq!(RemoteError::Runtime(io::Error::other("x")).code(), None);
        assert_eq!(RemoteError::from(ClientError::Body).code(), None);
        assert_eq!(
            RemoteError::from(FoldError::new(codes::GIT_FAILED, "x")).code(),
            Some("git_failed")
        );
    }

    #[test]
    fn set_writes_the_config_and_the_mirror_of_a_project() {
        let dir = tempfile::TempDir::new().unwrap();
        let host = fold_core::HostPaths::new(dir.path().join("data").to_str(), None).unwrap();
        let root = dir.path().join("app");
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fold_daemon::services::projects::init_project(
                &host,
                &root,
                None,
                &fold_core::ManualClock::new(),
            ))
            .unwrap();
        let args = |url: &str, branch: &str, project: PathBuf| SetArgs {
            url: url.to_owned(),
            branch: branch.to_owned(),
            project: Some(project),
        };
        let mut out = Vec::new();
        set(&args("/srv/app.git", "trunk", root.join("main")), &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "remote set: fold syncs main with the branch trunk of the remote\n"
        );
        let text = std::fs::read_to_string(root.join(".fold/config.toml")).unwrap();
        let config = ProjectConfig::from_toml(&text).unwrap();
        assert_eq!(config.git.remote.as_deref(), Some("/srv/app.git"));
        assert_eq!(config.git.branch, "trunk");
        assert!(root.join(".fold/git/HEAD").is_file());

        for (url, branch, code) in [
            ("-x", "main", codes::INVALID_REQUEST),
            ("/srv/app.git", "a b", codes::INVALID_REQUEST),
        ] {
            let error = set(&args(url, branch, root.clone()), &mut Vec::new()).unwrap_err();
            assert_eq!(error.code(), Some(code));
        }
        let error = set(
            &args("/r", "main", dir.path().join("none")),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(error.code(), Some(codes::PROJECT_NOT_FOUND));
        std::fs::write(root.join(".fold/config.toml"), "git = 1\n").unwrap();
        let error = set(&args("/r", "main", root.clone()), &mut Vec::new()).unwrap_err();
        assert_eq!(error.code(), Some(codes::INVALID_CONFIG));
        std::fs::remove_file(root.join(".fold/config.toml")).unwrap();
        set(&args("/r", "main", root.clone()), &mut Vec::new()).unwrap();
        let text = std::fs::read_to_string(root.join(".fold/config.toml")).unwrap();
        assert!(text.starts_with("[git]\nremote = \"/r\"\n"));
        assert_eq!(
            std::fs::read_dir(root.join(".fold"))
                .unwrap()
                .filter(|entry| {
                    let name = entry.as_ref().unwrap().file_name();
                    name.to_string_lossy().starts_with(".tmp")
                })
                .count(),
            0
        );
        let fold_dir = root.join(".fold");
        let mode = |bits| std::fs::Permissions::from_mode(bits);
        std::fs::set_permissions(&fold_dir, mode(0o555)).unwrap();
        let error = set(&args("/s", "main", root.clone()), &mut Vec::new()).unwrap_err();
        std::fs::set_permissions(&fold_dir, mode(0o755)).unwrap();
        assert_eq!(error.code(), Some(codes::WORKSPACE_IO));
        let text = std::fs::read_to_string(root.join(".fold/config.toml")).unwrap();
        assert!(text.contains("remote = \"/r\""));
    }
}
