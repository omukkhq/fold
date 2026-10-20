use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use fold_cli::daemon_client::{ClientError, DaemonClient, Response};
use fold_core::config::{
    GIT_SECTION, REMOTES_SECTION, UNNAMED_REMOTE, check_branch, check_remote, check_remote_name,
};
use fold_core::{FoldError, GitConfig, ProjectConfig, ProjectPaths, codes};
use fold_daemon::services::projects::{ProjectError, public_location, slug_for_dir_on_host};
use fold_vcs::atomic::EditLock;
use fold_vcs::git::Git;
use fold_vcs::git_remote::{default_branch, prepare_mirror, project_repository, remove_remote};
use hyper::Method;
use thiserror::Error;
use toml_edit::{DocumentMut, Item, Table, value};

use crate::serve::{self, Dirs};
use crate::{init, output};

/// The branch a remote without a default branch is synced on.
const DEFAULT_BRANCH: &str = "main";

#[derive(Args, Debug)]
pub struct RemoteArgs {
    #[command(subcommand)]
    command: Option<RemoteCommand>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR", global = true)]
    project: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum RemoteCommand {
    /// Add a remote Git repository that fld push and fld pull sync main with; a name that exists already is refused
    Add(AddArgs),
    /// Delete a remote and the refs fetched from it
    Remove(RemoveArgs),
    /// List the remotes: name, branch and URL; also what `fld remote` with no subcommand prints
    List,
}

#[derive(Args, Debug)]
struct AddArgs {
    /// The remote's name, such as origin
    name: String,
    /// The URL or path of the remote Git repository; your own Git credentials and configuration apply
    url: String,
    /// The remote branch fold syncs main with; defaults to the branch the remote's HEAD names, or main when the remote has none, as a new empty repository does
    branch: Option<String>,
}

#[derive(Args, Debug)]
struct RemoveArgs {
    /// The remote's name
    name: String,
}

#[derive(Args, Debug)]
pub struct PushArgs {
    /// The remote's name; it can be left out when the project has exactly one remote
    remote: Option<String>,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
    /// The project directory or a directory inside it; defaults to the current directory
    #[arg(long, value_name = "DIR")]
    project: Option<PathBuf>,
}

/// `fld pull` takes the same arguments as `fld push`.
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
            Self::Client(_) | Self::Answer | Self::Cwd(_) | Self::Runtime(_) | Self::Write(_) => {
                None
            }
        }
    }

    /// The hint of a local refusal. A refusal by the daemon carries its hint in its message.
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Fold(error) | Self::Project(ProjectError::Fold(error)) => error.hint(),
            Self::Init(error) => error.hint(),
            _ => None,
        }
    }
}

pub fn run_remote(args: &RemoteArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let paths = project_paths(args.project.as_deref())?;
    match &args.command {
        Some(RemoteCommand::Add(add_args)) => add(&paths, add_args, &Git::default(), out),
        Some(RemoteCommand::Remove(remove_args)) => remove(&paths, &remove_args.name, out),
        Some(RemoteCommand::List) | None => list(&paths, out),
    }
}

/// The nearest directory at or above `start` that holds a project database, or `start` itself when none does, so the error names the directory the user gave.
pub(crate) fn find_project(start: &Path) -> PathBuf {
    let absolute = std::path::absolute(start).unwrap_or_else(|_| start.to_owned());
    init::project_root_at_or_above(&absolute).unwrap_or(absolute)
}

fn project_paths(project: Option<&Path>) -> Result<ProjectPaths, FoldError> {
    let paths = ProjectPaths::new(find_project(project.unwrap_or_else(|| Path::new("."))));
    if !paths.db().is_file() {
        return Err(FoldError::new(
            codes::PROJECT_NOT_FOUND,
            format!("{} is not a fold project", paths.root().display()),
        )
        .with_hint("run the command inside a project directory or pass --project"));
    }
    Ok(paths)
}

/// Writes a new `[remotes.<name>]` into `.fold/config.toml`, keeping every other line and comment, after the project's Git repository points its remote `<name>` at the URL. Without a branch, the branch the remote's `HEAD` names is used; a remote without one, such as a new empty repository or a local path that a push creates, gets [`DEFAULT_BRANCH`]. A name the config holds already is refused before the config or the repository changes. It changes no database row, so it needs no daemon.
fn add(
    paths: &ProjectPaths,
    args: &AddArgs,
    git: &Git,
    out: &mut impl Write,
) -> Result<(), RemoteError> {
    check_remote_name(&args.name)?;
    check_remote(&args.url)?;
    // Checked again under the lock below; this first check spares the remote's default branch lookup.
    refuse_existing(&read_config(&paths.config())?, &args.name)?;
    let url = absolute_if_local(&args.url)?;
    if std::fs::symlink_metadata(paths.git_dir())
        .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
    {
        return Err(git_sync_off().into());
    }
    let mirror = project_repository(paths)?;
    let branch = match &args.branch {
        Some(branch) => {
            check_branch(branch)?;
            branch.clone()
        }
        None => {
            let local = Path::new(&url);
            let head = if local.is_absolute() && !local.exists() {
                None
            } else {
                default_branch(git, &mirror, &url)?
            };
            head.unwrap_or_else(|| DEFAULT_BRANCH.to_owned())
        }
    };
    let _lock = lock_config(&paths.config())?;
    let text = read_config(&paths.config())?;
    let edited = with_remote(&text, &args.name, &url, &branch)?;
    prepare_mirror(git, &mirror, &args.name, &url)?;
    write_config(&paths.config(), &edited)?;
    output::line(
        out,
        &format!(
            "remote {} added: fold syncs main with its branch {branch}",
            args.name
        ),
    )?;
    Ok(())
}

/// A project without `.git` is a plain folder whose Git sync is off, and a remote needs Git.
fn git_sync_off() -> FoldError {
    FoldError::new(
        codes::INVALID_REQUEST,
        "Git sync is off in this project, so it has no Git repository for a remote",
    )
    .with_hint(
        "run `fld init --with-git` in the project folder to turn Git on, then `fld remote add` again",
    )
    .with_details([("reason", serde_json::json!("git_sync_off"))])
}

/// Refuses a remote `name` that the config `text` names already, as `git remote add` does, so an add never changes a remote; an invalid config is refused too.
fn refuse_existing(text: &str, name: &str) -> Result<(), FoldError> {
    if !ProjectConfig::from_toml(text)?.remotes.contains_key(name) {
        return Ok(());
    }
    Err(FoldError::new(
        codes::INVALID_REQUEST,
        format!("the project has a Git remote named {name} already"),
    )
    .with_hint(format!(
        "run `fld remote remove {name}`, then `fld remote add {name} <url>` again"
    ))
    .with_details([("reason", serde_json::json!("remote_exists"))]))
}

/// Deletes `[remotes.<name>]` from the config and the remote with its fetched refs from the project's Git repository.
fn remove(paths: &ProjectPaths, name: &str, out: &mut impl Write) -> Result<(), RemoteError> {
    check_remote_name(name)?;
    let _lock = lock_config(&paths.config())?;
    let text = read_config(&paths.config())?;
    let edited = without_remote(&text, name)?;
    remove_remote(&Git::default(), &paths.git_dir(), name)?;
    write_config(&paths.config(), &edited)?;
    output::line(out, &format!("remote {name} removed"))?;
    Ok(())
}

/// One line per remote, in name order: its name, branch and URL without the credentials it may hold. The only remote is marked, because push and pull use it when no name is given.
fn list(paths: &ProjectPaths, out: &mut impl Write) -> Result<(), RemoteError> {
    let config = ProjectConfig::from_toml(&read_config(&paths.config())?)?;
    if config.remotes.is_empty() {
        output::line(out, "no remotes: run fld remote add <name> <url>")?;
        return Ok(());
    }
    let only = config.remotes.len() == 1;
    for (name, remote) in &config.remotes {
        let mark = if only { " (the only remote)" } else { "" };
        output::line(
            out,
            &format!(
                "{name}\t{}\t{}{mark}",
                remote.branch,
                public_location(&remote.url)
            ),
        )?;
    }
    Ok(())
}

fn read_config(path: &Path) -> Result<String, FoldError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(config_io(&error)),
    }
}

/// A local path is made absolute from the working directory, whether or not it exists yet, because Git would resolve a relative one from the project's repository. A remote is a URL, as Git reads it, when it holds `://` or a `:` before its first `/` (`host:path` and `<transport>::<address>`); it stays as given.
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

fn parse_config(text: &str) -> Result<DocumentMut, FoldError> {
    text.parse().map_err(|_| {
        FoldError::new(codes::INVALID_CONFIG, "config.toml is not valid TOML")
            .with_hint("fix .fold/config.toml, then run the command again")
    })
}

/// Moves `remote` and `push_on_integrate` of `[git]` into `[remotes.origin]`, as [`ProjectConfig::from_toml`] reads them, so the file names every remote in its own section once fold edits it.
fn lift_unnamed_remote(doc: &mut DocumentMut) -> Result<(), FoldError> {
    let Some(git) = doc.get_mut(GIT_SECTION).and_then(Item::as_table_like_mut) else {
        return Ok(());
    };
    if !git.contains_key("remote") {
        return Ok(());
    }
    let branch = git
        .get("branch")
        .cloned()
        .unwrap_or_else(|| value(GitConfig::default().branch));
    let url = git.remove("remote").unwrap_or_default();
    let push_on_integrate = git.remove("push_on_integrate");
    let remotes = remotes_table(doc)?;
    let mut section = Table::new();
    section.insert("url", url);
    section.insert("branch", branch);
    section.insert(
        "push_on_integrate",
        push_on_integrate.unwrap_or_else(|| value(false)),
    );
    remotes.insert(UNNAMED_REMOTE, Item::Table(section));
    Ok(())
}

fn remotes_table(doc: &mut DocumentMut) -> Result<&mut Table, FoldError> {
    let item = doc.entry(REMOTES_SECTION).or_insert_with(|| {
        let mut table = Table::new();
        table.set_implicit(true);
        Item::Table(table)
    });
    item.as_table_mut().ok_or_else(|| {
        FoldError::new(codes::INVALID_CONFIG, "config key remotes must be a table")
            .with_details([("key", serde_json::json!(REMOTES_SECTION))])
    })
}

/// `text` with a new `[remotes.<name>]` holding `url` and `branch`, and `push_on_integrate` off; a name the config holds already is `remote_exists`. The input and the result must both be valid configs, so a file that fold-core refuses, such as one that names the origin twice, is refused before anything is written.
fn with_remote(text: &str, name: &str, url: &str, branch: &str) -> Result<String, FoldError> {
    refuse_existing(text, name)?;
    let mut doc = parse_config(text)?;
    lift_unnamed_remote(&mut doc)?;
    let mut section = Table::new();
    section.insert("url", value(url));
    section.insert("branch", value(branch));
    section.insert("push_on_integrate", value(false));
    let edited = if doc.contains_key(REMOTES_SECTION) {
        remotes_table(&mut doc)?.insert(name, Item::Table(section));
        doc.to_string()
    } else {
        // A new section goes after the whole file, so the comments at its end stay where they are.
        let mut added = DocumentMut::new();
        remotes_table(&mut added)?.insert(name, Item::Table(section));
        let separator = if text.is_empty() || text.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        format!("{text}{separator}{added}")
    };
    ProjectConfig::from_toml(&edited)?;
    Ok(edited)
}

/// `text` without `[remotes.<name>]`; a name the config does not hold is `remote_not_set`.
fn without_remote(text: &str, name: &str) -> Result<String, FoldError> {
    let mut doc = parse_config(text)?;
    ProjectConfig::from_toml(text)?;
    lift_unnamed_remote(&mut doc)?;
    let removed = remotes_table(&mut doc)?.remove(name);
    if removed.is_none() {
        return Err(FoldError::new(
            codes::REMOTE_NOT_SET,
            format!("the project has no Git remote named {name}"),
        )
        .with_hint("run `fld remote list` to see the remotes"));
    }
    if remotes_table(&mut doc)?.is_empty() {
        doc.remove(REMOTES_SECTION);
    }
    Ok(doc.to_string())
}

/// Written through `write_atomic`, which keeps the file's permission bits.
fn write_config(path: &Path, text: &str) -> Result<(), FoldError> {
    fold_vcs::atomic::write_atomic(path, text.as_bytes()).map_err(|error| config_io(&error))
}

/// Holds the config's edit lock, which the daemon takes for its own rewrites, until the guard drops.
fn lock_config(path: &Path) -> Result<EditLock, FoldError> {
    EditLock::acquire(path).map_err(|error| config_io(&error))
}

fn config_io(error: &io::Error) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_IO,
        format!("cannot read or write config.toml: {}", error.kind()),
    )
}

/// Asks the daemon to push main to the named remote, or the only one, starting the daemon when none runs.
pub fn run_push(args: &PushArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let response = post_project(args, "push")?;
    output::line(out, &push_summary(response)?)?;
    Ok(())
}

/// Asks the daemon to bring the named remote's branch, or the only remote's, into main, starting the daemon when none runs.
pub fn run_pull(args: &PullArgs, out: &mut impl Write) -> Result<(), RemoteError> {
    let response = post_project(args, "pull")?;
    for line in pull_summary(response)? {
        output::line(out, &line)?;
    }
    Ok(())
}

/// Sends `POST /api/v1/projects/<slug>/<action>[?remote=<name>]` over the socket as the human in the environment. Without a name the daemon takes the only remote, after it checks that Git sync is on.
fn post_project(args: &PushArgs, action: &str) -> Result<Response, RemoteError> {
    let query = match &args.remote {
        Some(name) => {
            check_remote_name(name)?;
            format!("?remote={name}")
        }
        None => String::new(),
    };
    post_to_project(
        args.socket.as_deref(),
        args.project.as_deref(),
        &format!("{action}{query}"),
    )
}

/// Sends `POST /api/v1/projects/<slug>/<action>` for the project at or above `project`, the current directory by default, over the socket as the human in the environment, starting the daemon when none runs.
pub(crate) fn post_to_project(
    socket: Option<&Path>,
    project: Option<&Path>,
    action: &str,
) -> Result<Response, RemoteError> {
    request_project(socket, project, Method::POST, action)
}

/// [`post_to_project`] with any method.
pub(crate) fn request_project(
    socket: Option<&Path>,
    project: Option<&Path>,
    method: Method,
    action: &str,
) -> Result<Response, RemoteError> {
    request_project_with(socket, project, method, action, None)
}

/// [`request_project`] with a JSON body.
pub(crate) fn request_project_with(
    socket: Option<&Path>,
    project: Option<&Path>,
    method: Method,
    action: &str,
    body: Option<&serde_json::Value>,
) -> Result<Response, RemoteError> {
    let dirs = Dirs::from_env();
    let host = init::host_paths(&dirs)?;
    let start = project.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let dir = std::fs::canonicalize(&start).map_err(RemoteError::Cwd)?;
    let config = serve::client_config(socket, &dirs)?;
    let client = DaemonClient::new(config, crate::fold_name::current());
    let runtime = serve::current_thread_runtime().map_err(RemoteError::Runtime)?;
    runtime.block_on(async {
        let slug = slug_for_dir_on_host(&host, &dir).await?;
        let path = format!("/api/v1/projects/{slug}/{action}");
        Ok::<_, RemoteError>(client.request(method, &path, body).await?)
    })
}

/// The error a refusal carries.
pub(crate) fn refusal(response: &Response) -> Result<(), RemoteError> {
    if response.is_success() {
        return Ok(());
    }
    let refusal = response.refusal();
    Err(match (refusal.code, refusal.message) {
        (Some(code), Some(message)) => RemoteError::Refused {
            code,
            message,
            hint: refusal.hint,
        },
        _ => RemoteError::Answer,
    })
}

/// The line `fld push` prints for the daemon's answer, or the error it carries.
fn push_summary(response: Response) -> Result<String, RemoteError> {
    refusal(&response)?;
    let body = &response.body;
    let text = |key: &str| body[key].as_str();
    let remote = text("remote").ok_or(RemoteError::Answer)?;
    let branch = format!("{remote}/{}", text("branch").ok_or(RemoteError::Answer)?);
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
            Some(last) => format!("{branch} is up to date: {last} is pushed"),
            None => format!("{branch} is up to date"),
        }),
        Some("pushed") => Ok(format!("pushed {} to {branch}: {}", range()?, commits()?)),
        _ => Err(RemoteError::Answer),
    }
}

/// The lines `fld pull` prints for the daemon's answer, or the error it carries: one line, and for a conflict one more per conflicted path.
fn pull_summary(response: Response) -> Result<Vec<String>, RemoteError> {
    refusal(&response)?;
    let body = &response.body;
    let text = |key: &str| body[key].as_str().ok_or(RemoteError::Answer);
    let branch = format!("{}/{}", text("remote")?, text("branch")?);
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
    use std::process::Command;

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
    fn remote_commands_parse_the_name_the_url_and_the_branch() {
        let parsed = Remote::try_parse_from(["fld", "add", "origin", "../app.git"]).unwrap();
        let Some(RemoteCommand::Add(add)) = parsed.args.command else {
            panic!("expected add");
        };
        assert_eq!(
            (add.name.as_str(), add.url.as_str(), add.branch),
            ("origin", "../app.git", None)
        );
        let parsed = Remote::try_parse_from([
            "fld",
            "add",
            "backup",
            "git@x:a.git",
            "trunk",
            "--project",
            "app",
        ])
        .unwrap();
        assert_eq!(parsed.args.project.as_deref(), Some(Path::new("app")));
        let Some(RemoteCommand::Add(add)) = parsed.args.command else {
            panic!("expected add");
        };
        assert_eq!(add.branch.as_deref(), Some("trunk"));
        let parsed = Remote::try_parse_from(["fld", "remove", "origin"]).unwrap();
        assert!(
            matches!(parsed.args.command, Some(RemoteCommand::Remove(ref remove)) if remove.name == "origin")
        );
        assert!(matches!(
            Remote::try_parse_from(["fld", "list"])
                .unwrap()
                .args
                .command,
            Some(RemoteCommand::List)
        ));
        assert!(
            Remote::try_parse_from(["fld"])
                .unwrap()
                .args
                .command
                .is_none()
        );
        assert!(Remote::try_parse_from(["fld", "add", "origin"]).is_err());
        assert!(Remote::try_parse_from(["fld", "set", "origin", "/r.git"]).is_err());
        let push = Push::try_parse_from(["fld", "backup", "--project", "app"]).unwrap();
        assert_eq!(push.args.remote.as_deref(), Some("backup"));
        assert!(Push::try_parse_from(["fld"]).unwrap().args.remote.is_none());
    }

    #[test]
    fn a_remote_section_is_written_and_removed_and_everything_else_kept() {
        let text = "# Mine.\nlease_minutes = 7\n";
        let edited = with_remote(text, "origin", "/srv/app.git", "main").unwrap();
        assert!(edited.starts_with(text));
        let config = ProjectConfig::from_toml(&edited).unwrap();
        assert_eq!(config.lease_minutes, 7);
        assert_eq!(config.remotes["origin"].url, "/srv/app.git");
        assert!(!config.remotes["origin"].push_on_integrate);
        let held =
            "[remotes.origin]\nurl = \"/old\"\nbranch = \"main\"\npush_on_integrate = true\n";
        let exists = with_remote(held, "origin", "/new", "trunk").unwrap_err();
        assert_eq!(exists.code(), codes::INVALID_REQUEST);
        assert_eq!(exists.details().unwrap()["reason"], "remote_exists");
        assert_eq!(
            exists.hint(),
            Some("run `fld remote remove origin`, then `fld remote add origin <url>` again")
        );
        let edited = held;
        let config = ProjectConfig::from_toml(edited).unwrap();
        let two = with_remote(edited, "backup", "/b", "main").unwrap();
        assert_eq!(ProjectConfig::from_toml(&two).unwrap().remotes.len(), 2);
        let one = without_remote(&two, "backup").unwrap();
        assert_eq!(ProjectConfig::from_toml(&one).unwrap(), config);
        let none = without_remote(&one, "origin").unwrap();
        assert!(!none.contains("remotes"));
        assert_eq!(
            without_remote(&none, "origin").unwrap_err().code(),
            codes::REMOTE_NOT_SET
        );
        for bad in ["= x", "remotes = 3\n", "lease_minutes = 0\n"] {
            assert_eq!(
                with_remote(bad, "origin", "/r", "main").unwrap_err().code(),
                codes::INVALID_CONFIG,
                "{bad}"
            );
        }
        assert_eq!(
            without_remote("lease_minutes = 0\n", "origin")
                .unwrap_err()
                .code(),
            codes::INVALID_CONFIG
        );
    }

    #[test]
    fn the_unnamed_remote_of_the_git_section_becomes_its_own_section_when_fold_edits() {
        let unnamed =
            "# Mine.\n[git]\nremote = \"/old.git\"\nbranch = \"trunk\"\npush_on_integrate = true\n";
        let kept = ProjectConfig::from_toml(unnamed).unwrap().remotes;
        let added = with_remote(unnamed, "backup", "/b.git", "main").unwrap();
        assert!(!added.contains("remote = "));
        assert!(added.starts_with("# Mine.\n[git]\nbranch = \"trunk\"\n"));
        let config = ProjectConfig::from_toml(&added).unwrap();
        assert_eq!(config.git.branch, "trunk");
        assert_eq!(config.remotes["origin"], kept["origin"]);
        assert!(config.remotes["origin"].push_on_integrate);
        assert_eq!(config.remotes["backup"].url, "/b.git");
        assert_eq!(
            with_remote(unnamed, "origin", "/new.git", "main")
                .unwrap_err()
                .details()
                .unwrap()["reason"],
            "remote_exists"
        );
        let removed = without_remote("[git]\nremote = \"/old.git\"\n", "origin").unwrap();
        let config = ProjectConfig::from_toml(&removed).unwrap();
        assert!(config.remotes.is_empty());
        assert!(!removed.contains("remote"));
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

    fn project_with_config(text: &str) -> (tempfile::TempDir, ProjectPaths) {
        let dir = tempfile::TempDir::new().unwrap();
        let paths = ProjectPaths::new(dir.path());
        std::fs::create_dir_all(paths.config().parent().unwrap()).unwrap();
        std::fs::write(paths.config(), text).unwrap();
        (dir, paths)
    }

    #[test]
    fn credentials_never_reach_the_list() {
        let (_dir, paths) = project_with_config(
            "[remotes.origin]\nurl = \"https://user:token@example.com/a.git?private_token=abc\"\nbranch = \"main\"\n",
        );
        let mut out = Vec::new();
        list(&paths, &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("https://example.com/a.git"), "{out}");
        assert!(!out.contains("token") && !out.contains("abc"), "{out}");
    }

    #[test]
    fn a_config_that_names_the_origin_twice_is_refused_and_left_as_it_is() {
        let twice =
            "[git]\nremote = \"/a.git\"\n\n[remotes.origin]\nurl = \"/b.git\"\nbranch = \"main\"\n";
        assert_eq!(
            with_remote(twice, "backup", "/c.git", "main")
                .unwrap_err()
                .code(),
            codes::INVALID_CONFIG
        );
        let (_plain_dir, plain_paths) = project_with_config("");
        let plain = AddArgs {
            name: "backup".to_owned(),
            url: "/c.git".to_owned(),
            branch: None,
        };
        let RemoteError::Fold(off) =
            add(&plain_paths, &plain, &Git::default(), &mut Vec::new()).unwrap_err()
        else {
            panic!("a fold error");
        };
        assert_eq!(off.details().unwrap()["reason"], "git_sync_off");
        assert!(off.hint().unwrap().contains("fld init --with-git"));
        let (dir, paths) = project_with_config(twice);
        git(dir.path(), &["init", "--quiet"]);
        let mut out = Vec::new();
        let args = AddArgs {
            name: "backup".to_owned(),
            url: "/c.git".to_owned(),
            branch: Some("main".to_owned()),
        };
        assert_eq!(
            add(&paths, &args, &Git::default(), &mut out)
                .unwrap_err()
                .code(),
            Some(codes::INVALID_CONFIG)
        );
        assert_eq!(std::fs::read_to_string(paths.config()).unwrap(), twice);
    }

    #[test]
    fn a_remote_change_waits_for_a_config_edit_in_progress_and_keeps_it() {
        let both = "[remotes.origin]\nurl = \"/a.git\"\nbranch = \"main\"\n\n[remotes.backup]\nurl = \"/b.git\"\nbranch = \"main\"\n";
        let (_dir, paths) = project_with_config(both);
        let held = EditLock::acquire(&paths.config()).unwrap();
        let removing = {
            let paths = paths.clone();
            std::thread::spawn(move || remove(&paths, "backup", &mut Vec::new()).is_ok())
        };
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert_eq!(std::fs::read_to_string(paths.config()).unwrap(), both);
        let edited = format!("lease_minutes = 9\n\n{both}");
        fold_vcs::atomic::write_atomic(&paths.config(), edited.as_bytes()).unwrap();
        drop(held);
        assert!(removing.join().unwrap());
        let text = std::fs::read_to_string(paths.config()).unwrap();
        assert!(text.contains("lease_minutes = 9"), "{text}");
        assert!(!text.contains("backup"), "{text}");
    }

    #[test]
    fn a_written_config_keeps_its_permission_bits() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, paths) = project_with_config("lease_minutes = 7\n");
        std::fs::set_permissions(paths.config(), std::fs::Permissions::from_mode(0o640)).unwrap();
        write_config(&paths.config(), "lease_minutes = 8\n").unwrap();
        let mode = std::fs::metadata(paths.config())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o640);
    }

    fn reply(status: u16, body: serde_json::Value) -> Response {
        Response { status, body }
    }

    #[test]
    fn push_answers_read_as_one_line_or_the_error_they_carry() {
        let pushed = json!({"outcome": "pushed", "remote": "origin", "branch": "main", "first": "r1", "last": "r3", "commits": 3, "head": "a"});
        assert_eq!(
            push_summary(reply(200, pushed)).unwrap(),
            "pushed r1..r3 to origin/main: 3 commits"
        );
        let current = json!({"outcome": "up_to_date", "remote": "origin", "branch": "main", "last": "r3", "commits": 0});
        assert_eq!(
            push_summary(reply(200, current)).unwrap(),
            "origin/main is up to date: r3 is pushed"
        );
        let empty = json!({"outcome": "up_to_date", "remote": "backup", "branch": "trunk", "last": null, "commits": 0});
        assert_eq!(
            push_summary(reply(200, empty)).unwrap(),
            "backup/trunk is up to date"
        );
        let refused = json!({"error": {"code": "remote_ahead", "message": "m", "details": {"hint": "run fld pull"}}});
        let error = push_summary(reply(409, refused)).unwrap_err();
        assert_eq!(error.code(), Some("remote_ahead"));
        assert_eq!(error.to_string(), "m (run fld pull)");
        for bad in [
            reply(500, json!(null)),
            reply(
                200,
                json!({"outcome": "pushed", "remote": "origin", "branch": "main"}),
            ),
            reply(
                200,
                json!({"outcome": "pushed", "remote": "origin", "branch": "main", "first": "r1", "last": "r1"}),
            ),
            reply(200, json!({"outcome": "pushed", "branch": "main"})),
            reply(
                200,
                json!({"outcome": "other", "remote": "origin", "branch": "main"}),
            ),
        ] {
            assert!(push_summary(bad).is_err());
        }
    }

    #[test]
    fn pull_answers_read_as_lines_or_the_error_they_carry() {
        let commit = "0123456789abcdef0123456789abcdef01234567";
        let lines = |body| pull_summary(reply(200, body)).unwrap();
        assert_eq!(
            lines(
                json!({"outcome": "up_to_date", "remote": "origin", "branch": "main", "remote_commit": commit})
            ),
            ["main is up to date with origin/main: it holds 0123456"]
        );
        assert_eq!(
            lines(
                json!({"outcome": "up_to_date", "remote": "origin", "branch": "trunk", "remote_commit": null})
            ),
            ["the remote has no branch origin/trunk; nothing to pull"]
        );
        assert_eq!(
            lines(
                json!({"outcome": "integrated", "remote": "origin", "branch": "main", "remote_commit": commit, "revision": "r4"})
            ),
            ["pulled 0123456 from origin/main into main as r4"]
        );
        let conflicted = |created: bool| {
            json!({"outcome": "conflicted", "remote": "origin", "branch": "main", "remote_commit": commit, "task": "T-3", "workspace": "ws-2", "created": created,
                "conflicted": [{"path": "a.txt", "kind": "both_modified_text"}]})
        };
        assert_eq!(
            lines(conflicted(true)),
            [
                "0123456 from origin/main conflicts with main in 1 path; main did not move. T-3 resolves it in its workspace: an agent claims T-3, resolves the markers and opens a proposal",
                "  a.txt (both_modified_text)"
            ]
        );
        assert_eq!(
            lines(conflicted(false))[0],
            "T-3 already resolves 0123456 from origin/main; nothing was created (1 path conflicted)"
        );
        let refused = json!({"error": {"code": "remote_not_set", "message": "m"}});
        let error = pull_summary(reply(409, refused)).unwrap_err();
        assert_eq!(error.code(), Some("remote_not_set"));
        for bad in [
            json!({"outcome": "integrated", "remote": "origin", "branch": "main", "remote_commit": commit}),
            json!({"outcome": "conflicted", "remote": "origin", "branch": "main", "remote_commit": commit, "task": "T-3", "created": true}),
            json!({"outcome": "conflicted", "remote": "origin", "branch": "main", "remote_commit": commit, "task": "T-3", "created": true, "conflicted": [{"path": "a"}]}),
            json!({"outcome": "conflicted", "remote": "origin", "branch": "main", "remote_commit": commit, "task": "T-3", "conflicted": []}),
            json!({"outcome": "other", "remote": "origin", "branch": "main"}),
            json!({"outcome": "integrated", "branch": "main"}),
        ] {
            assert!(matches!(
                pull_summary(reply(200, bad)),
                Err(RemoteError::Answer)
            ));
        }
    }

    #[test]
    fn a_local_refusal_names_its_hint() {
        let with_hint = FoldError::new(codes::GIT_FAILED, "x").with_hint("run git status");
        assert_eq!(
            RemoteError::from(with_hint.clone()).hint(),
            Some("run git status")
        );
        assert_eq!(
            RemoteError::from(ProjectError::Fold(with_hint)).hint(),
            Some("run git status")
        );
        assert_eq!(RemoteError::Answer.hint(), None);
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
    fn the_project_is_found_above_the_start_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("app");
        std::fs::create_dir_all(root.join(".fold")).unwrap();
        std::fs::write(root.join(".fold/fold.db"), "").unwrap();
        std::fs::create_dir_all(root.join(".fold/ws/one/src")).unwrap();
        assert_eq!(find_project(&root.join(".fold/ws/one/src")), root);
        assert_eq!(find_project(&root), root);
        let outside = dir.path().join("elsewhere");
        assert_eq!(find_project(&outside), outside);
        assert_eq!(
            project_paths(Some(&outside)).unwrap_err().code(),
            codes::PROJECT_NOT_FOUND
        );
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn text(out: Vec<u8>) -> String {
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn add_list_and_remove_change_the_config_and_the_mirror_of_a_project() {
        let dir = tempfile::TempDir::new().unwrap();
        let host = fold_core::HostPaths::new(dir.path().join("data").to_str(), None).unwrap();
        let root = dir.path().join("app");
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fold_daemon::services::projects::init_new_project(
                &host,
                &root,
                None,
                &fold_core::HumanName::parse("ana").unwrap(),
                Git::default(),
                None,
                Some(Box::new(|_: &Path| {})),
                &fold_core::ManualClock::new(),
            ))
            .unwrap();
        let paths = project_paths(Some(&root)).unwrap();
        // A repository whose HEAD names `trunk` gives the default branch.
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        git(&source, &["init", "--quiet", "--initial-branch=trunk"]);
        std::fs::write(source.join("a.txt"), "a").unwrap();
        git(&source, &["add", "a.txt"]);
        git(&source, &["commit", "--quiet", "-m", "a"]);
        let args = |name: &str, url: &str, branch: Option<&str>| AddArgs {
            name: name.to_owned(),
            url: url.to_owned(),
            branch: branch.map(str::to_owned),
        };
        let gitbin = Git::default();
        let mut out = Vec::new();
        list(&paths, &mut out).unwrap();
        assert_eq!(text(out), "no remotes: run fld remote add <name> <url>\n");
        let mut out = Vec::new();
        add(
            &paths,
            &args("origin", source.to_str().unwrap(), None),
            &gitbin,
            &mut out,
        )
        .unwrap();
        assert_eq!(
            text(out),
            "remote origin added: fold syncs main with its branch trunk\n"
        );
        let mut out = Vec::new();
        list(&paths, &mut out).unwrap();
        let canonical = source.canonicalize().unwrap();
        assert_eq!(
            text(out),
            format!("origin\ttrunk\t{} (the only remote)\n", canonical.display())
        );
        let url = "https://user:token@example.com/a.git";
        add(
            &paths,
            &args("backup", url, Some("main")),
            &gitbin,
            &mut Vec::new(),
        )
        .unwrap();
        let mut out = Vec::new();
        list(&paths, &mut out).unwrap();
        let listed = text(out);
        assert!(
            listed.contains("backup\tmain\thttps://example.com/a.git\n"),
            "{listed}"
        );
        assert!(!listed.contains("token") && !listed.contains("only"));
        assert!(root.join(".git/HEAD").is_file());

        // A missing local path or an empty repository has no default branch, so main is synced on `main`.
        for (name, missing) in [
            ("absent", dir.path().join("absent")),
            ("empty", {
                let empty = dir.path().join("empty.git");
                std::fs::create_dir(&empty).unwrap();
                git(
                    &empty,
                    &["init", "--quiet", "--bare", "--initial-branch=trunk"],
                );
                empty
            }),
        ] {
            let mut out = Vec::new();
            add(
                &paths,
                &args(name, missing.to_str().unwrap(), None),
                &gitbin,
                &mut out,
            )
            .unwrap();
            assert_eq!(
                text(out),
                format!("remote {name} added: fold syncs main with its branch main\n")
            );
            remove(&paths, name, &mut Vec::new()).unwrap();
        }
        // A second add of a name changes neither the config nor the repository.
        let config_before = std::fs::read_to_string(paths.config()).unwrap();
        let git_before = std::fs::read_to_string(root.join(".git/config")).unwrap();
        let error = add(
            &paths,
            &args("backup", "/elsewhere.git", Some("release")),
            &gitbin,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(error.code(), Some(codes::INVALID_REQUEST));
        assert_eq!(
            error.hint(),
            Some("run `fld remote remove backup`, then `fld remote add backup <url>` again")
        );
        assert_eq!(
            std::fs::read_to_string(paths.config()).unwrap(),
            config_before
        );
        assert_eq!(
            std::fs::read_to_string(root.join(".git/config")).unwrap(),
            git_before
        );
        for (name, url, branch) in [
            ("-x", "/r", "main"),
            ("o", "-x", "main"),
            ("o", "/r", "a b"),
        ] {
            let error = add(
                &paths,
                &args(name, url, Some(branch)),
                &gitbin,
                &mut Vec::new(),
            )
            .unwrap_err();
            assert_eq!(error.code(), Some(codes::INVALID_REQUEST));
        }

        let mut out = Vec::new();
        remove(&paths, "backup", &mut out).unwrap();
        assert_eq!(text(out), "remote backup removed\n");
        let config =
            ProjectConfig::from_toml(&std::fs::read_to_string(paths.config()).unwrap()).unwrap();
        assert_eq!(config.remotes.keys().collect::<Vec<_>>(), ["origin"]);
        assert_eq!(
            remove(&paths, "backup", &mut Vec::new())
                .unwrap_err()
                .code(),
            Some(codes::REMOTE_NOT_SET)
        );
        assert_eq!(
            remove(&paths, "-x", &mut Vec::new()).unwrap_err().code(),
            Some(codes::INVALID_REQUEST)
        );

        std::fs::write(paths.config(), "remotes = 1\n").unwrap();
        let error = add(
            &paths,
            &args("o", "/r", Some("main")),
            &gitbin,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(error.code(), Some(codes::INVALID_CONFIG));
        assert_eq!(
            list(&paths, &mut Vec::new()).unwrap_err().code(),
            Some(codes::INVALID_CONFIG)
        );
        std::fs::remove_file(paths.config()).unwrap();
        add(
            &paths,
            &args("o", "/r", Some("main")),
            &gitbin,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_dir(root.join(".fold"))
                .unwrap()
                .filter(|entry| entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp"))
                .count(),
            0
        );
        let fold_dir = root.join(".fold");
        let mode = |bits| std::fs::Permissions::from_mode(bits);
        std::fs::set_permissions(&fold_dir, mode(0o555)).unwrap();
        let error = add(
            &paths,
            &args("s", "/s", Some("main")),
            &gitbin,
            &mut Vec::new(),
        )
        .unwrap_err();
        std::fs::set_permissions(&fold_dir, mode(0o755)).unwrap();
        assert_eq!(error.code(), Some(codes::WORKSPACE_IO));
    }
}
