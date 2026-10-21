use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args;
use fold_cli::claude_files::{self, ClaudeFileError};
use fold_cli::daemon_client::{ClientConfig, ClientError, DaemonClient, Response};
use fold_core::{FoldError, HostPaths, HumanName, ProjectPaths, codes};
use fold_daemon::SystemClock;
use fold_daemon::services::projects::{
    FolderFiles, ProjectError, RepositorySetup, deleted_project_slug, init_new_project,
    init_project_from_git, root_for_dir_on_host,
};
use fold_vcs::git::Git;
use fold_vcs::git_discovery::Discovery;
use hyper::Method;
use thiserror::Error;

use crate::serve::{Dirs, current_thread_runtime, socket_path};
use crate::{git_identity, output};

/// The `git` program, limited as the user's environment limits how far up Git looks for a repository, so init finds the repository around a folder that `git` in the same shell finds, also when `git` is not on `PATH`.
fn git() -> Git {
    Git::default().with_discovery(Discovery::from_vars(|name| std::env::var_os(name)))
}

/// How long a daemon gets to accept the connection and answer the open request.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Args, Debug, Clone)]
pub struct InitArgs {
    /// A new directory for the project, which fold creates; without it the project is the working directory. With --with-git, also an existing Git repository or fold project. With --refresh, a directory of the project, by default the working directory
    pub(crate) dir: Option<PathBuf>,
    /// The project name; defaults to the directory name
    #[arg(long)]
    pub(crate) name: Option<String>,
    /// Turn Git on: fold writes a commit of every revision into the folder's .git and keeps its branch on main. In a Git repository the commit at HEAD becomes the genesis r0, which needs a clean tree with a branch checked out; in a fold project, and in a new folder, fold creates .git. Git sync stays on
    #[arg(long)]
    pub(crate) with_git: bool,
    /// Unix socket of the running daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    pub(crate) socket: Option<PathBuf>,
    /// Rewrite fold's Claude Code files of an existing project, and remove the entries an earlier fold wrote into .mcp.json and .claude/settings.json, keeping every entry fold does not own, instead of creating a project
    #[arg(long, conflicts_with_all = ["name", "with_git", "socket"])]
    pub(crate) refresh: bool,
    /// The name for the project's own Git config, instead of asking for it; only a new Git repository gets one
    #[arg(long, requires = "with_git", conflicts_with = "refresh")]
    pub(crate) git_name: Option<String>,
    /// The email for the project's own Git config, instead of asking for it; only a new Git repository gets one
    #[arg(long, requires = "with_git", conflicts_with = "refresh")]
    pub(crate) git_email: Option<String>,
}

#[derive(Debug, Error)]
pub enum InitError {
    #[error(transparent)]
    HostPaths(FoldError),
    #[error(transparent)]
    Refused(FoldError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error(transparent)]
    ClaudeFiles(#[from] ClaudeFileError),
    #[error(transparent)]
    Serve(#[from] crate::serve::ServeError),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("{message}")]
    Daemon {
        code: String,
        message: String,
        hint: Option<String>,
    },
    #[error("the daemon answered with a body fold cannot read")]
    Answer,
    #[error("cannot read the directory: {0}")]
    Dir(io::Error),
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error("cannot write the output: {0}")]
    Write(#[from] io::Error),
}

impl InitError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::HostPaths(error) | Self::Refused(error) => Some(error.code()),
            Self::Project(error) => error.code(),
            Self::ClaudeFiles(error) => error.code(),
            Self::Serve(error) => error.code(),
            Self::Daemon { code, .. } => Some(code),
            Self::Client(_) | Self::Answer | Self::Dir(_) | Self::Runtime(_) | Self::Write(_) => {
                None
            }
        }
    }

    /// The hint that names the next step, when a refusal carries one.
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Refused(error) | Self::Project(ProjectError::Fold(error)) => error.hint(),
            Self::ClaudeFiles(error) => error.hint(),
            Self::Daemon { hint, .. } => hint.as_deref(),
            _ => None,
        }
    }
}

/// What the running daemon did with the new project.
#[derive(Debug, PartialEq, Eq)]
enum Registration {
    NoDaemon,
    Opened,
    Refused(String),
}

/// A daemon that refuses or cannot be asked leaves the project initialised and registered, so init still succeeds.
pub fn run(args: &InitArgs, dirs: &Dirs, out: &mut impl Write) -> Result<(), InitError> {
    if args.refresh {
        return refresh(args.dir.as_deref(), dirs, out);
    }
    git_identity::check_given(&given(args)).map_err(InitError::Refused)?;
    let Some(dir) = &args.dir else {
        let cwd = std::env::current_dir().map_err(InitError::Dir)?;
        return start(&cwd, args, dirs, out);
    };
    if args.with_git && (dir.join(".git").exists() || ProjectPaths::new(dir).db().is_file()) {
        return start(dir, args, dirs, out);
    }
    if std::fs::symlink_metadata(dir).is_ok() {
        return Err(InitError::Refused(dir_exists(dir, args.with_git)));
    }
    let initialised = initialise(dir, args, Kind::New, dirs, out);
    if initialised.is_err() {
        // Init created the directory; an empty one would only make the next `fld init DIR` fail.
        let _ = std::fs::remove_dir(dir);
    }
    initialised.map(|_| ())
}

/// How `fld init` takes its folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A folder that is no project and no Git repository; `--with-git` turns Git on in the new project.
    New,
    /// A Git repository with no fold project, whose `HEAD` becomes the genesis.
    FromGit,
}

/// Init in an existing folder: with `--with-git`, a fold project gets Git turned on by its daemon and a Git repository becomes a project from its `HEAD`; anything else is a new project.
fn start(dir: &Path, args: &InitArgs, dirs: &Dirs, out: &mut impl Write) -> Result<(), InitError> {
    if args.with_git {
        let absolute = std::path::absolute(dir).map_err(InitError::Dir)?;
        if let Some(root) = project_root_at_or_above(&absolute) {
            return enable_git(&root, args, dirs, out);
        }
        if dir.join(".git").exists() {
            return initialise(dir, args, Kind::FromGit, dirs, out).map(|_| ());
        }
    }
    initialise(dir, args, Kind::New, dirs, out).map(|_| ())
}

fn given(args: &InitArgs) -> git_identity::Given {
    git_identity::Given {
        name: args.git_name.clone(),
        email: args.git_email.clone(),
    }
}

fn dir_exists(dir: &Path, with_git: bool) -> FoldError {
    let flag = if with_git { " --with-git" } else { "" };
    FoldError::new(
        codes::INVALID_REQUEST,
        format!("{} exists already", dir.display()),
    )
    .with_hint(format!(
        "`cd {} && fld init{flag}`",
        output::shell_quote(dir)
    ))
    .with_details([("reason", serde_json::json!("dir_exists"))])
}

/// [`run`] for the project at `dir`, returning the slug of the new project.
pub(crate) fn initialise(
    dir: &Path,
    args: &InitArgs,
    kind: Kind,
    dirs: &Dirs,
    out: &mut impl Write,
) -> Result<String, InitError> {
    let host = host_paths(dirs).map_err(InitError::HostPaths)?;
    let socket = socket_path(args.socket.as_deref(), dirs);
    let runtime = current_thread_runtime().map_err(InitError::Runtime)?;
    let name = args.name.as_deref();
    if let Ok(Some(socket)) = &socket {
        let deleted = runtime.block_on(deleted_project_slug(&host, dir))?;
        if let Some(slug) = deleted {
            // Asked to open a project whose database was deleted, the daemon closes it and ends its workers, and removes it when its `.fold/` is gone, so the old project is gone before init creates the new one there.
            let _ = runtime.block_on(register(&client(socket.clone()), &slug));
        }
    }
    // The fold name comes first, so its question comes before the Git identity's. The folder's files become the change of this human.
    let human = human_name()?;
    // The question comes before the project exists, because every commit fold writes, `r1`'s first, takes the identity from the repository's config.
    let identity = (args.with_git && kind == Kind::New).then(|| {
        let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
        git_identity::decide(
            &git(),
            git_identity::Plan {
                given: &given(args),
                env: &git_identity::Environment::from_env(),
                interactive,
                dir,
            },
            &mut io::stdin().lock(),
            out,
            &mut io::stderr(),
        )
    });
    let project = runtime.block_on(async {
        if kind == Kind::FromGit {
            return init_project_from_git(&host, dir, name, &human, git(), &SystemClock).await;
        }
        let files: FolderFiles = Box::new(|root: &Path| new_claude_files(root));
        let setup = identity.map(|identity| -> RepositorySetup {
            Box::new(move |root: &Path| {
                git_identity::store_in(&git(), root, &identity, &mut io::stderr());
            })
        });
        init_new_project(
            &host,
            dir,
            name,
            &human,
            git(),
            Some(files),
            setup,
            &SystemClock,
        )
        .await
    })?;
    let root = project.root.display();
    if project.restored {
        output::line(
            out,
            &format!(
                "project {} at {root} is back in fold, with the data its .fold kept",
                project.slug
            ),
        )?;
    } else {
        output::line(
            out,
            &format!("initialised project {} at {root}", project.slug),
        )?;
    }
    if project.replaced {
        output::line(
            out,
            &format!(
                "replaced the registration of the project whose .fold was deleted; the slug {} stays",
                project.slug
            ),
        )?;
    }
    if project.resumed {
        output::line(
            out,
            "finished an init that had stopped before it registered the project",
        )?;
        output::line(
            out,
            &format!(
                "{} keeps the files that the stopped init recorded",
                project.revision
            ),
        )?;
    }
    let imported = &project.imported;
    output::line(
        out,
        &output::revision_summary(&project.revision, imported.files, imported.bytes),
    )?;
    for skipped in &imported.skipped {
        output::line(
            out,
            &output::skipped_line(&skipped.path, skipped.reason.as_str()),
        )?;
    }
    if let Some(commit) = &project.git_commit {
        output::line(out, &format!("{} is Git commit {commit}", project.revision))?;
    }
    if let Some(revision) = &project.gitignore {
        output::line(
            out,
            &format!("{revision} records the .gitignore that keeps .fold/ out of Git"),
        )?;
    }
    if project.git_sync {
        output::line(
            out,
            "Git sync is on: every revision gets its Git commit in .git",
        )?;
        output::line(out, ADD_REMOTE)?;
    }
    write_claude_files(&project.root, dirs, out)?;
    let note = match socket {
        Ok(Some(socket)) => match runtime.block_on(register(&client(socket), &project.slug)) {
            Registration::NoDaemon => None,
            Registration::Opened => Some("opened by the running daemon".to_owned()),
            Registration::Refused(reason) => Some(format!(
                "the running daemon did not open the project ({reason}); it opens the project at its next start"
            )),
        },
        Ok(None) => None,
        Err(error) => Some(format!(
            "{error}; the daemon opens the project at its next start"
        )),
    };
    if let Some(note) = note {
        output::line(out, &note)?;
    }
    Ok(project.slug)
}

/// What init prints while the project has no Git remote.
const ADD_REMOTE: &str = "no Git remote yet: add one with fld remote add <name> <url> [<branch>]";

/// Turns Git on in the fold project at `root` through its daemon, which `fld init --with-git` there starts when none runs, after the Git identity question: fold creates `.git`, records the `.gitignore` copy rule as a revision of yours and writes the commits of every revision.
fn enable_git(
    root: &Path,
    args: &InitArgs,
    dirs: &Dirs,
    out: &mut impl Write,
) -> Result<(), InitError> {
    let host = host_paths(dirs).map_err(InitError::HostPaths)?;
    let human = human_name()?;
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let identity = git_identity::decide(
        &git(),
        git_identity::Plan {
            given: &given(args),
            env: &git_identity::Environment::from_env(),
            interactive,
            dir: root,
        },
        &mut io::stdin().lock(),
        out,
        &mut io::stderr(),
    );
    let config = crate::serve::client_config(args.socket.as_deref(), dirs)?;
    let client = DaemonClient::new(config, Some(human.as_str().to_owned()));
    let runtime = current_thread_runtime().map_err(InitError::Runtime)?;
    let body = serde_json::json!({ "git_name": identity.name, "git_email": identity.email });
    let response = runtime.block_on(async {
        let found =
            fold_daemon::services::projects::registration_for_dir_on_host(&host, root).await?;
        let slug = found.row.slug;
        if found.removed_at.is_some() {
            // The daemon's open route brings a project a person removed back into fold, as `fld claude` does.
            let open = serde_json::json!({ "slug": slug });
            let opened = client
                .request(Method::POST, "/api/v1/projects", Some(&open))
                .await?;
            if !opened.is_success() {
                return Ok::<_, InitError>((slug, false, opened));
            }
        }
        let path = format!("/api/v1/projects/{slug}/git");
        let restored = found.removed_at.is_some();
        Ok((
            slug,
            restored,
            client.request(Method::POST, &path, Some(&body)).await?,
        ))
    })?;
    let (slug, restored, response) = response;
    if restored {
        output::line(out, &format!("project {slug} is back in fold"))?;
    }
    if !response.is_success() {
        let refusal = response.refusal();
        return Err(match (refusal.code, refusal.message) {
            (Some(code), Some(message)) => InitError::Daemon {
                code,
                message,
                hint: refusal.hint,
            },
            _ => InitError::Answer,
        });
    }
    let text = |key: &str| response.body[key].as_str();
    let revision = text("revision").ok_or(InitError::Answer)?;
    let commit = text("commit")
        .map(|commit| format!(", Git commit {}", commit.get(..7).unwrap_or(commit)))
        .unwrap_or_default();
    output::line(
        out,
        &format!("turned Git on in project {slug}: main is {revision}{commit}"),
    )?;
    if let Some(gitignore) = text("gitignore") {
        output::line(
            out,
            &format!("{gitignore} records the .gitignore that keeps .fold/ out of Git"),
        )?;
    }
    output::line(
        out,
        "Git sync is on: every revision gets its Git commit in .git",
    )?;
    let remotes = response.body["remotes"].as_array().map_or(0, Vec::len);
    if remotes == 0 {
        output::line(out, ADD_REMOTE)?;
    }
    Ok(())
}

/// The fold name of the human who runs init, whose change the folder's files become.
fn human_name() -> Result<HumanName, InitError> {
    let refused = || {
        InitError::Refused(
            FoldError::new(
                codes::HUMAN_REQUIRED,
                "cannot tell who you are, and the folder's files become your change",
            )
            .with_hint("set FOLD_HUMAN to your fold name, then run `fld init` again"),
        )
    };
    let name = crate::fold_name::current().ok_or_else(refused)?;
    HumanName::parse(&name).map_err(|_| refused())
}

/// Rewrites the Claude Code files of the registered project that holds `dir`, and makes `.git/info/exclude` the link to `.fold/exclude` again.
fn refresh(dir: Option<&Path>, dirs: &Dirs, out: &mut impl Write) -> Result<(), InitError> {
    let dir = match dir {
        Some(dir) => std::fs::canonicalize(dir),
        None => std::env::current_dir().and_then(std::fs::canonicalize),
    }
    .map_err(InitError::Dir)?;
    let host = host_paths(dirs).map_err(InitError::HostPaths)?;
    let runtime = current_thread_runtime().map_err(InitError::Runtime)?;
    let root = runtime.block_on(root_for_dir_on_host(&host, &dir))?;
    write_claude_files(&root, dirs, out)?;
    let repo = ProjectPaths::new(&root).git_dir();
    if repo.is_dir() {
        fold_vcs::git_remote::exclude_fold_dir(&repo).map_err(InitError::Refused)?;
    }
    output::line(
        out,
        &format!("refreshed the Claude Code files at {}", root.display()),
    )?;
    Ok(())
}

/// Writes the Claude Code files into a new project's folder before init records it, so they are part of `r1`, and returns the files and folders that did not exist before.
fn new_claude_files(root: &Path) -> Result<Vec<PathBuf>, FoldError> {
    // A folder comes before the files below it, so only the topmost new path is recorded.
    let written = [claude_files::instructions_file(root)]
        .into_iter()
        .chain(claude_files::FOLDERS)
        .chain(claude_files::SKILL_FILES);
    let mut created: Vec<PathBuf> = Vec::new();
    for path in written.map(|relative| root.join(relative)) {
        if std::fs::symlink_metadata(&path).is_err()
            && !created.iter().any(|made| path.starts_with(made))
        {
            created.push(path);
        }
    }
    match claude_files::write(root) {
        Ok(()) => Ok(created),
        Err(ClaudeFileError::Malformed(error) | ClaudeFileError::Link(error)) => Err(error),
        Err(ClaudeFileError::Io { source, .. }) => Err(FoldError::new(
            codes::WORKSPACE_IO,
            format!("cannot write the Claude Code files: {}", source.kind()),
        )),
    }
}

/// The warnings never stop the command: the files are written first.
fn write_claude_files(root: &Path, dirs: &Dirs, out: &mut impl Write) -> Result<(), InitError> {
    claude_files::write(root)?;
    let home = dirs
        .home
        .as_deref()
        .map(|home| std::fs::canonicalize(home).unwrap_or_else(|_| home.to_owned()));
    for path in claude_files::shadowing_files(root, home.as_deref()) {
        output::line(out, &claude_files::shadowing_warning(&path))?;
    }
    Ok(())
}

/// Init uses a running daemon and never starts one.
fn client(socket: PathBuf) -> DaemonClient {
    DaemonClient::new(ClientConfig::running(socket), crate::fold_name::current())
}

/// The nearest directory at or above `dir` that holds a project database. The caller decides how `dir` is made absolute, because a symlink in it decides which project is found.
pub(crate) fn project_root_at_or_above(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|candidate| ProjectPaths::new(*candidate).db().is_file())
        .map(Path::to_owned)
}

pub(crate) fn host_paths(dirs: &Dirs) -> Result<HostPaths, FoldError> {
    let data_home = utf8(dirs.data_home.as_deref(), "XDG_DATA_HOME")?;
    let home = utf8(dirs.home.as_deref(), "HOME")?;
    HostPaths::new(data_home, home)
}

/// The daemon finds the registry through the same variables as a `PathBuf`, so a value that is not UTF-8 is refused instead of skipped.
fn utf8<'a>(path: Option<&'a Path>, name: &str) -> Result<Option<&'a str>, FoldError> {
    match path {
        None => Ok(None),
        Some(path) => path.to_str().map(Some).ok_or_else(|| {
            FoldError::new(
                codes::INVALID_HOST_PATHS,
                format!("{name} is not valid UTF-8"),
            )
        }),
    }
}

async fn register(client: &DaemonClient, slug: &str) -> Registration {
    let body = serde_json::json!({ "slug": slug });
    let request = client.request(Method::POST, "/api/v1/projects", Some(&body));
    match tokio::time::timeout(ANSWER_TIMEOUT, request).await {
        Ok(answer) => interpret(answer),
        Err(_) => Registration::Refused("no answer".to_owned()),
    }
}

/// A missing socket file or a refused connection means no daemon runs, which is not an error: the daemon opens every registered project when it starts.
fn interpret(answer: Result<Response, ClientError>) -> Registration {
    match answer {
        Ok(response) if response.is_success() => Registration::Opened,
        Ok(response) => Registration::Refused(
            response
                .refusal()
                .code
                .unwrap_or_else(|| "unknown".to_owned()),
        ),
        Err(ClientError::NoDaemon { .. }) => Registration::NoDaemon,
        Err(ClientError::Body) => Registration::Refused("unknown".to_owned()),
        Err(ClientError::Io { source, .. }) => Registration::Refused(source.kind().to_string()),
        Err(error) => Registration::Refused(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct Wrapper {
        #[command(flatten)]
        args: InitArgs,
    }

    #[test]
    fn init_parses_dir_name_and_socket() {
        let parsed = Wrapper::try_parse_from([
            "fld",
            "~/scratch",
            "--name",
            "Scratch",
            "--socket",
            "/tmp/f.sock",
        ])
        .unwrap();
        assert_eq!(parsed.args.dir.as_deref(), Some(Path::new("~/scratch")));
        assert_eq!(parsed.args.name.as_deref(), Some("Scratch"));
        assert_eq!(
            parsed.args.socket.as_deref(),
            Some(Path::new("/tmp/f.sock"))
        );
        let bare = Wrapper::try_parse_from(["fold", "dir"]).unwrap();
        assert!(bare.args.name.is_none() && bare.args.socket.is_none() && !bare.args.with_git);
        let git = Wrapper::try_parse_from(["fold", "--with-git"]).unwrap();
        assert!(git.args.with_git && git.args.dir.is_none());
        assert!(Wrapper::try_parse_from(["fold", "dir", "--from", "a"]).is_err());
        assert!(Wrapper::try_parse_from(["fold", "dir", "--from-git"]).is_err());
        assert!(Wrapper::try_parse_from(["fold", "dir", "--with-git", "b"]).is_err());
        assert!(
            Wrapper::try_parse_from(["fold"])
                .unwrap()
                .args
                .dir
                .is_none()
        );
        let refresh = Wrapper::try_parse_from(["fold", "--refresh"]).unwrap();
        assert!(refresh.args.refresh && refresh.args.dir.is_none());
        for flag in ["--name", "--socket"] {
            assert!(
                Wrapper::try_parse_from(["fld", "dir", "--refresh", flag, "x"]).is_err(),
                "{flag}"
            );
        }
        assert!(Wrapper::try_parse_from(["fold", "--refresh", "--with-git"]).is_err());
    }

    fn reply(status: u16, body: serde_json::Value) -> Result<Response, ClientError> {
        Ok(Response { status, body })
    }

    #[test]
    fn replies_are_read_by_status_and_error_code() {
        assert_eq!(
            interpret(reply(200, serde_json::json!({}))),
            Registration::Opened
        );
        assert_eq!(
            interpret(reply(201, serde_json::Value::Null)),
            Registration::Opened
        );
        let refused = serde_json::json!({"error": {"code": "project_not_found", "message": "m"}});
        assert_eq!(
            interpret(reply(404, refused)),
            Registration::Refused("project_not_found".to_owned())
        );
        let unknown = Registration::Refused("unknown".to_owned());
        assert_eq!(interpret(reply(500, serde_json::Value::Null)), unknown);
        assert_eq!(interpret(Err(ClientError::Body)), unknown);
        let reset = ClientError::Io {
            context: "connect to the daemon socket",
            source: io::Error::from(io::ErrorKind::PermissionDenied),
        };
        assert_eq!(
            interpret(Err(reset)),
            Registration::Refused("permission denied".to_owned())
        );
        assert_eq!(
            interpret(Err(ClientError::HumanUnknown)),
            Registration::Refused("cannot tell who you are: set FOLD_HUMAN".to_owned())
        );
    }

    fn dirs(runtime: Option<&str>, config: Option<&str>) -> Dirs {
        Dirs {
            home: Some("/nonexistent-home".into()),
            config_home: config.map(Into::into),
            data_home: None,
            runtime_dir: runtime.map(Into::into),
            library_runtime: false,
        }
    }

    #[test]
    fn the_socket_comes_from_the_flag_then_the_file_then_the_runtime_directory() {
        let none = "/nonexistent-config";
        let flag = socket_path(Some(Path::new("/f.sock")), &dirs(None, Some(none))).unwrap();
        assert_eq!(flag.as_deref(), Some(Path::new("/f.sock")));
        let runtime = socket_path(None, &dirs(Some("/run/u"), Some(none))).unwrap();
        assert_eq!(runtime.as_deref(), Some(Path::new("/run/u/fold/fold.sock")));
        assert_eq!(socket_path(None, &dirs(None, Some(none))).unwrap(), None);

        let config = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(config.path().join("fold")).unwrap();
        let file = config.path().join("fold").join("daemon.toml");
        let config_home = config.path().to_str();
        std::fs::write(&file, "socket = \"/from-file.sock\"\n").unwrap();
        let from_file = socket_path(None, &dirs(Some("/run/u"), config_home)).unwrap();
        assert_eq!(from_file.as_deref(), Some(Path::new("/from-file.sock")));
        std::fs::write(&file, "bogus = 1\n").unwrap();
        assert!(socket_path(None, &dirs(Some("/run/u"), config_home)).is_err());
    }

    #[test]
    fn a_missing_home_stops_init_before_it_touches_the_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        let args = InitArgs {
            dir: Some(dir.path().join("p")),
            name: None,
            with_git: false,
            socket: None,
            refresh: false,
            git_name: None,
            git_email: None,
        };
        let mut out = Vec::new();
        let error = run(&args, &Dirs::default(), &mut out).unwrap_err();
        assert_eq!(error.code(), Some("invalid_host_paths"));
        assert_eq!(error.hint(), None);
        assert!(!dir.path().join("p").exists());
        assert!(out.is_empty());
    }

    #[test]
    fn a_named_directory_that_exists_is_refused_with_the_command_that_uses_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let args = InitArgs {
            dir: Some(dir.path().to_owned()),
            name: None,
            with_git: false,
            socket: None,
            refresh: false,
            git_name: None,
            git_email: None,
        };
        let error = run(&args, &Dirs::default(), &mut Vec::new()).unwrap_err();
        assert_eq!(error.code(), Some("invalid_request"));
        assert_eq!(
            error.hint(),
            Some(format!("`cd {} && fld init`", dir.path().display()).as_str())
        );
        assert!(matches!(error, InitError::Refused(_)));
        assert!(!dir.path().join(".fold").exists());
    }

    #[test]
    fn the_hint_quotes_a_directory_name_with_a_space() {
        let dir = tempfile::TempDir::new().unwrap();
        let spaced = dir.path().join("my app");
        std::fs::create_dir(&spaced).unwrap();
        let args = InitArgs {
            dir: Some(spaced.clone()),
            name: None,
            with_git: false,
            socket: None,
            refresh: false,
            git_name: None,
            git_email: None,
        };
        let error = run(&args, &Dirs::default(), &mut Vec::new()).unwrap_err();
        assert_eq!(
            error.hint(),
            Some(format!("`cd '{}' && fld init`", spaced.display()).as_str())
        );
    }

    #[test]
    fn host_paths_refuse_a_variable_that_is_not_utf8() {
        use std::os::unix::ffi::OsStrExt;

        let bad = PathBuf::from(std::ffi::OsStr::from_bytes(b"/data-\xff"));
        let dirs = Dirs {
            home: Some("/home/ana".into()),
            data_home: Some(bad),
            ..Dirs::default()
        };
        let error = host_paths(&dirs).unwrap_err();
        assert_eq!(error.code(), "invalid_host_paths");
        assert!(error.to_string().contains("XDG_DATA_HOME"));
        let fine = Dirs {
            data_home: None,
            ..dirs
        };
        assert_eq!(
            host_paths(&fine).unwrap().registry_db(),
            Path::new("/home/ana/.local/share/fold/registry.db")
        );
    }

    #[test]
    fn errors_without_a_code_say_so() {
        let error = InitError::Runtime(io::Error::other("x"));
        assert_eq!(error.code(), None);
        assert!(
            error
                .to_string()
                .starts_with("cannot start the async runtime")
        );
        assert_eq!(InitError::from(io::Error::other("x")).code(), None);
    }

    #[tokio::test]
    async fn a_socket_path_with_no_listener_is_no_daemon() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("missing.sock");
        assert_eq!(
            register(&client(missing), "s").await,
            Registration::NoDaemon
        );
        let stale = dir.path().join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
        assert_eq!(register(&client(stale), "s").await, Registration::NoDaemon);
    }
}
