use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Args, ValueEnum};
use fold_cli::claude_files;
use fold_cli::claude_user::{self, UserFiles};
use fold_cli::daemon_client::{self, ClientError, DaemonClient, Response};
use fold_core::layout::{MCP_COMMAND, session_settings};
use fold_core::transcript::{claude_config_dir, is_session_id};
use fold_core::{AgentName, FoldError, MAIN_TAG, ProjectPaths, codes};
use fold_daemon::services::agents::claude::transcript_path;
use fold_daemon::services::agents::new_session_id;
use fold_daemon::services::projects::{ProjectError, root_for_dir_on_host, slug_for_dir_on_host};
use hyper::Method;
use thiserror::Error;

use crate::claude_setup::Offer;
use crate::init::host_paths;
use crate::serve::{self, Dirs};

#[derive(Args, Debug)]
pub struct ClaudeArgs {
    /// The agent name of the session, which is also its Claude Code name
    #[arg(long, default_value = MAIN_TAG)]
    name: String,
    /// Unix socket of the daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long, env = serve::SOCKET_ENV)]
    socket: Option<PathBuf>,
    /// Arguments for claude itself, after `--`
    #[arg(last = true)]
    claude_args: Vec<OsString>,
}

#[derive(Args, Debug)]
pub struct NewArgs {
    /// What to start; `claude` is the only one and the default
    #[arg(value_enum)]
    target: Option<NewTarget>,
    #[command(flatten)]
    claude: ClaudeArgs,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewTarget {
    /// A new Claude Code session as the main agent
    Claude,
}

/// Which Claude Code session the main agent gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    /// The main session that `fld` started last, or a new one when Claude Code no longer has it.
    Continue,
    New,
}

/// The session arguments of one `claude` run.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Start {
    New(String),
    Resume(String),
}

#[derive(Debug, Error)]
pub enum ClaudeError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Config(#[from] serve::ServeError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error("cannot read the working directory: {0}")]
    Cwd(io::Error),
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("{message}")]
    Refused {
        code: String,
        message: String,
        hint: Option<String>,
    },
    #[error("cannot record the main session in .fold: {0}")]
    Record(io::Error),
    #[error("cannot write the main agent prompt in .fold: {0}")]
    Prompt(io::Error),
    #[error("cannot run claude: {0}")]
    Exec(io::Error),
}

impl ClaudeError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Fold(error) => Some(error.code()),
            Self::Config(error) => error.code(),
            Self::Project(error) => error.code(),
            Self::Refused { code, .. } => Some(code),
            Self::Cwd(_)
            | Self::Runtime(_)
            | Self::Client(_)
            | Self::Record(_)
            | Self::Prompt(_)
            | Self::Exec(_) => None,
        }
    }

    /// The hint that names the next step.
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Project(_) if self.code() == Some(codes::PROJECT_NOT_FOUND) => {
                Some("run `fld init` in the project folder first")
            }
            Self::Fold(error) | Self::Project(ProjectError::Fold(error)) => error.hint(),
            Self::Refused { hint, .. } => hint.as_deref(),
            Self::Exec(error) if error.kind() == io::ErrorKind::NotFound => {
                Some("install Claude Code, so that claude is on your PATH")
            }
            Self::Exec(_)
            | Self::Config(_)
            | Self::Project(_)
            | Self::Cwd(_)
            | Self::Runtime(_)
            | Self::Client(_)
            | Self::Record(_)
            | Self::Prompt(_) => None,
        }
    }
}

/// `fld new` and `fld new claude`: [`run`] with a new session.
pub fn run_new(args: &NewArgs, dirs: &Dirs) -> Result<(), ClaudeError> {
    match args.target {
        None | Some(NewTarget::Claude) => run(&args.claude, Session::New, dirs),
    }
}

/// Starts the daemon when it is not running, then replaces this process with the user's own `claude`, an interactive session in the project root that is the main agent. `FOLD_AGENT` and `--name` give the session its agent name and its messaging socket, and `FOLD_AGENT_TAGS` makes `fld mcp` register it with the `main` tag. The main agent prompt is appended to the session's system prompt on every start and resume, because a system prompt flag holds only for the `claude` run that gets it. fold chooses the id of every new session and records it, so that [`Session::Continue`] resumes the session `fld` started and never one the user started without it, which `claude --continue` could pick. It offers once to add fold to the user's Claude Code configuration, and a session of a user without that setup gets fold's server, permission and hooks as flags; it writes no project settings. It returns only when it could not start `claude`.
pub fn run(args: &ClaudeArgs, session: Session, dirs: &Dirs) -> Result<(), ClaudeError> {
    let agent = AgentName::parse(&args.name).map_err(FoldError::from)?;
    let host = host_paths(dirs)?;
    let cwd = std::env::current_dir()
        .and_then(|dir| dir.canonicalize())
        .map_err(ClaudeError::Cwd)?;
    let config = serve::client_config(args.socket.as_deref(), dirs)?;
    let socket = args.socket.is_some().then(|| config.socket.clone());
    let human = crate::fold_name::current();
    let runtime = serve::current_thread_runtime().map_err(ClaudeError::Runtime)?;
    let root = runtime.block_on(async {
        let root = root_for_dir_on_host(&host, &cwd).await?;
        let slug = slug_for_dir_on_host(&host, &cwd).await?;
        daemon_client::connect(&config).await?;
        if let Some(human) = human {
            let client = DaemonClient::new(config, Some(human));
            let path = format!("/api/v1/projects/{slug}/agents");
            main_is_free(&client.request(Method::GET, &path, None).await?, &agent)?;
        }
        Ok::<_, ClaudeError>(root)
    })?;
    drop(runtime);
    let paths = ProjectPaths::new(&root);
    let prompt = if picks_system_prompt(&args.claude_args) {
        None
    } else {
        Some(prompt_file(&paths).map_err(ClaudeError::Prompt)?)
    };
    let start = if picks_session(&args.claude_args) {
        None
    } else {
        let config_dir = local_config_dir(&root);
        Some(start(
            &paths,
            &agent,
            session,
            config_dir.as_deref(),
            || Ok(new_session_id()?),
        )?)
    };
    let files = UserFiles::from_env();
    let set_up = crate::claude_setup::offer_on_terminal(
        dirs,
        files.as_ref(),
        Offer::Start,
        &mut io::stdout(),
    );
    crate::news::ask_once_on_terminal(dirs);
    let own = if set_up {
        None
    } else {
        OwnSetup::new(&agent, files.as_ref())
    };
    let error = command(
        Path::new("claude"),
        &root,
        &agent,
        start.as_ref(),
        prompt.as_deref(),
        &args.claude_args,
        socket.as_deref(),
        own.as_ref(),
    )
    .exec();
    Err(ClaudeError::Exec(error))
}

/// Whether the user's own arguments for `claude` already choose the session. fold then adds no session argument of its own, which `claude` would refuse or take in place of the user's, and keeps its record as it is.
fn picks_session(extra: &[OsString]) -> bool {
    extra.iter().any(|arg| {
        let arg = arg.to_string_lossy();
        matches!(
            arg.as_ref(),
            "-c" | "--continue" | "-r" | "--resume" | "--session-id"
        ) || arg.starts_with("--resume=")
            || arg.starts_with("--session-id=")
    })
}

/// Whether the user's own arguments for `claude` set a system prompt. fold then appends none of its own, so the user's prompt is the one the session gets.
fn picks_system_prompt(extra: &[OsString]) -> bool {
    const FLAGS: [&str; 4] = [
        "--system-prompt",
        "--system-prompt-file",
        "--append-system-prompt",
        "--append-system-prompt-file",
    ];
    extra.iter().any(|arg| {
        let arg = arg.to_string_lossy();
        let flag = arg.split_once('=').map_or(arg.as_ref(), |(flag, _)| flag);
        FLAGS.contains(&flag)
    })
}

/// The main agent prompt for `claude`, written anew into fold's own file on each start so that it follows the installed `fld`: the project's `.fold/roles/main.md` followed by the shared rules when it has that file, else the shipped prompt. The project's file is never passed itself, because it lacks the shared rules.
fn prompt_file(paths: &ProjectPaths) -> io::Result<PathBuf> {
    let own = paths.main_role_prompt();
    let text = if own.is_file() {
        fold_core::with_shared_rules(&std::fs::read_to_string(&own)?)
    } else {
        fold_core::main_prompt().to_owned()
    };
    let file = paths.main_prompt_file();
    fold_vcs::atomic::write_atomic(&file, text.as_bytes())?;
    Ok(file)
}

/// Resumes the recorded session of `agent` when Claude Code still has it; otherwise records `new_id()` as that session and starts it.
fn start(
    paths: &ProjectPaths,
    agent: &AgentName,
    session: Session,
    config_dir: Option<&Path>,
    new_id: impl FnOnce() -> Result<String, ClaudeError>,
) -> Result<Start, ClaudeError> {
    if session == Session::Continue
        && let Some(id) = recorded(paths, agent)
        && config_dir.is_some_and(|dir| transcript_exists(dir, paths.root(), &id))
    {
        return Ok(Start::Resume(id));
    }
    let id = new_id()?;
    write_record(&paths.main_session(agent), &id).map_err(ClaudeError::Record)?;
    Ok(Start::New(id))
}

/// Writes the record through `write_atomic`, so a reader never sees half an id and two starts of one agent never share a temporary file.
fn write_record(path: &Path, id: &str) -> io::Result<()> {
    fold_vcs::atomic::write_atomic(path, format!("{id}\n").as_bytes())
}

/// Replaces the recorded session of `agent` with `id` when `agent` has a record that names another session. Only `fld claude` and `fld new` create a record, so an agent they never started, such as a worker, keeps having none.
pub(crate) fn follow_record(paths: &ProjectPaths, agent: &AgentName, id: &str) -> io::Result<()> {
    let path = paths.main_session(agent);
    if !is_session_id(id) || !path.is_file() || recorded(paths, agent).as_deref() == Some(id) {
        return Ok(());
    }
    write_record(&path, id)
}

/// The recorded session id of `agent`, when the record holds one.
pub(crate) fn recorded(paths: &ProjectPaths, agent: &AgentName) -> Option<String> {
    let text = std::fs::read_to_string(paths.main_session(agent)).ok()?;
    let id = text.trim();
    is_session_id(id).then(|| id.to_owned())
}

/// Claude Code's own folder for a `claude` that runs in `root` with this process's environment; see [`claude_config_dir`].
pub(crate) fn local_config_dir(root: &Path) -> Option<PathBuf> {
    claude_config_dir(|name| std::env::var_os(name)).map(|dir| root.join(dir))
}

/// Whether Claude Code still has the session `id` of the project at `root`. An interactive `claude --resume` of a session it does not have stops at "No conversation found", and an `exec` cannot recover from that, so fold checks first that the session's transcript file exists. It tests only that the file is there and never opens it, because the transcript holds the user's conversation. Where the file lives is Claude Code's own layout, not a documented interface (see [`transcript_path`]): when the layout changes, the file is not found and a new session starts, which loses the old conversation but never stops the user at an error. A transcript appears only after the first message, so a session the user left empty starts again as a new one.
fn transcript_exists(config_dir: &Path, root: &Path, id: &str) -> bool {
    transcript_path(config_dir, root, id).is_some_and(|path| path.is_file())
}

/// The `claude` run of `fld claude`: its own arguments come after `--name`, the session and the main agent prompt, each `None` when they choose it themselves. `socket` is the socket the user chose with `--socket` or `FOLD_SOCKET`; the session's `fld mcp` and hooks read it from `FOLD_SOCKET`, as they have no flag of their own to carry it.
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is one part of the claude command line"
)]
fn command(
    claude: &Path,
    root: &Path,
    agent: &AgentName,
    start: Option<&Start>,
    prompt: Option<&Path>,
    extra: &[OsString],
    socket: Option<&Path>,
    own: Option<&OwnSetup>,
) -> Command {
    let mut command = Command::new(claude);
    // Before `--name`, which ends the list of `--mcp-config`, so that it never takes a word of the user's own arguments.
    if let Some(own) = own {
        if let Some(mcp) = &own.mcp {
            command.arg("--mcp-config").arg(mcp);
        }
        if let Some(settings) = &own.settings {
            command.arg("--settings").arg(settings);
        }
    }
    command.arg("--name").arg(agent.as_str());
    match start {
        Some(Start::New(id)) => command.arg("--session-id").arg(id),
        Some(Start::Resume(id)) => command.arg("--resume").arg(id),
        None => &mut command,
    };
    if let Some(prompt) = prompt {
        command.arg("--append-system-prompt-file").arg(prompt);
    }
    command
        .args(extra)
        .current_dir(root)
        .env(claude_files::AGENT_ENV, agent.as_str())
        .env(claude_files::AGENT_TAGS_ENV, MAIN_TAG);
    if let Some(socket) = socket {
        command.env(serve::SOCKET_ENV, socket);
    }
    command
}

/// The fold server, the fold tools' permission and fold's hooks as `claude` flags, for a session of a user whose own Claude Code configuration lacks a part of fold's setup, so that the main agent has its tools all the same. The values are JSON text, which Claude Code takes in place of a file. A part the user's configuration holds is not passed again: Claude Code defines no order between a `--mcp-config` server and a configured one of the same name, and settings add nothing to a user's settings that already allow the fold tools and run fold's hooks.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OwnSetup {
    mcp: Option<String>,
    settings: Option<String>,
}

impl OwnSetup {
    fn new(agent: &AgentName, files: Option<&UserFiles>) -> Option<Self> {
        let fold = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("fld"));
        Self::for_binary(
            &fold,
            agent,
            files.is_some_and(claude_user::holds_server),
            files.is_some_and(claude_user::holds_settings),
        )
    }

    /// `None` when the user's configuration holds both the server and the settings.
    fn for_binary(
        fold: &Path,
        agent: &AgentName,
        holds_server: bool,
        holds_settings: bool,
    ) -> Option<Self> {
        let mcp = (!holds_server).then(|| {
            let server = serde_json::json!({
                "command": fold,
                "args": [MCP_COMMAND],
                "env": {(claude_files::AGENT_ENV): agent.as_str()},
            });
            let mut servers = serde_json::Map::new();
            servers.insert(claude_files::MCP_SERVER_NAME.to_owned(), server);
            serde_json::json!({ claude_files::MCP_SERVERS_KEY: servers }).to_string()
        });
        let settings = (!holds_settings).then(|| session_settings().to_string());
        (mcp.is_some() || settings.is_some()).then_some(Self { mcp, settings })
    }
}

/// Refuses before `claude` starts when another agent already has the `main` tag, which the session's adapter would get at its handshake.
fn main_is_free(response: &Response, agent: &AgentName) -> Result<(), ClaudeError> {
    if !response.is_success() {
        let refusal = response.refusal();
        return Err(ClaudeError::Refused {
            code: refusal
                .code
                .unwrap_or_else(|| codes::INTERNAL_ERROR.to_owned()),
            message: refusal
                .message
                .unwrap_or_else(|| "the daemon refused the request".to_owned()),
            hint: refusal.hint,
        });
    }
    let holder = response.body["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| {
            row["tags"]
                .as_array()
                .is_some_and(|tags| tags.iter().any(|tag| tag == MAIN_TAG))
        })
        .and_then(|row| row["name"].as_str());
    match holder {
        Some(holder) if holder != agent.as_str() => Err(ClaudeError::Refused {
            code: codes::MAIN_TAG_DENIED.to_owned(),
            message: format!("agent {holder} is the main agent of this project"),
            hint: Some(format!(
                "run `fld claude --name {holder}` to continue as it"
            )),
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use serde_json::json;

    use super::*;

    #[derive(Parser, Debug)]
    struct Wrapper {
        #[command(flatten)]
        args: ClaudeArgs,
    }

    fn parse(args: &[&str]) -> Result<Wrapper, clap::Error> {
        Wrapper::try_parse_from(std::iter::once("claude").chain(args.iter().copied()))
    }

    #[derive(Parser, Debug)]
    struct NewWrapper {
        #[command(flatten)]
        args: NewArgs,
    }

    fn parse_new(args: &[&str]) -> Result<NewArgs, clap::Error> {
        NewWrapper::try_parse_from(std::iter::once("new").chain(args.iter().copied()))
            .map(|wrapper| wrapper.args)
    }

    const ID: &str = "0f8e2c1a-5b7d-4c3e-9a1f-2d4b6c8e0a13";

    #[test]
    fn fld_new_takes_claude_as_an_optional_target() {
        let bare = parse_new(&[]).unwrap();
        assert_eq!(bare.target, None);
        assert_eq!(bare.claude.name, "main");
        let named = parse_new(&["claude", "--name", "lead", "--", "--model", "opus"]).unwrap();
        assert_eq!(named.target, Some(NewTarget::Claude));
        assert_eq!(named.claude.name, "lead");
        assert_eq!(named.claude.claude_args, ["--model", "opus"]);
        let rest = parse_new(&["--", "claude"]).unwrap();
        assert_eq!(rest.target, None);
        assert_eq!(rest.claude.claude_args, ["claude"]);
        assert!(parse_new(&["codex"]).is_err());
    }

    #[test]
    fn the_socket_comes_from_fold_socket_as_its_session_reads_it() {
        use clap::CommandFactory;
        let command = Wrapper::command();
        let socket = command
            .get_arguments()
            .find(|arg| arg.get_id() == "socket")
            .unwrap();
        assert_eq!(socket.get_env(), Some(std::ffi::OsStr::new("FOLD_SOCKET")));
    }

    #[test]
    fn a_new_session_id_counts_as_one() {
        assert!(is_session_id(&new_session_id().unwrap()));
    }

    #[test]
    fn a_transcript_counts_only_in_the_folder_of_the_root() {
        let config = tempfile::tempdir().unwrap();
        let root = Path::new("/home/ana/my.app/web_2");
        assert!(!transcript_exists(config.path(), root, ID));
        let other = config.path().join("projects/-home-ana-other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(format!("{ID}.jsonl")), "").unwrap();
        assert!(!transcript_exists(config.path(), root, ID));
        let folder = config.path().join("projects/-home-ana-my-app-web-2");
        std::fs::create_dir_all(&folder).unwrap();
        assert!(!transcript_exists(config.path(), root, ID));
        std::fs::write(folder.join(format!("{ID}.jsonl")), "").unwrap();
        assert!(transcript_exists(config.path(), root, ID));
        assert!(!transcript_exists(
            config.path(),
            root,
            &ID.replace('0', "1")
        ));
    }

    #[test]
    fn a_long_root_matches_its_cut_folder_name() {
        let config = tempfile::tempdir().unwrap();
        let root = PathBuf::from(format!("/{}", "a".repeat(250)));
        assert!(!transcript_exists(config.path(), &root, ID));
        let cut = format!("-{}-1x2y3z", "a".repeat(199));
        let folder = config.path().join("projects").join(cut);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(format!("{ID}.jsonl")), "").unwrap();
        assert!(transcript_exists(config.path(), &root, ID));
        assert!(!transcript_exists(
            config.path(),
            &root,
            &ID.replace('0', "1")
        ));
        let other = "11111111-2222-4333-8444-555555555555";
        let uncut = config
            .path()
            .join("projects")
            .join(format!("-{}", "a".repeat(250)));
        std::fs::create_dir_all(&uncut).unwrap();
        std::fs::write(uncut.join(format!("{other}.jsonl")), "").unwrap();
        assert!(
            !transcript_exists(config.path(), &root, other),
            "only the cut name, a dash and the hash"
        );
    }

    #[test]
    fn a_character_outside_utf16s_basic_plane_counts_as_two_dashes() {
        let config = tempfile::tempdir().unwrap();
        let root = Path::new("/home/ana/\u{1f642}app");
        let folder = config.path().join("projects/-home-ana---app");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(format!("{ID}.jsonl")), "").unwrap();
        assert!(transcript_exists(config.path(), root, ID));
    }

    #[test]
    fn the_users_own_session_arguments_replace_folds() {
        let picks =
            |args: &[&str]| picks_session(&args.iter().map(OsString::from).collect::<Vec<_>>());
        for chosen in [
            &["-c"][..],
            &["--continue"],
            &["-r"],
            &["--resume", ID],
            &["--model", "opus", "--resume=x"],
            &["--session-id", ID],
            &["--session-id=x"],
        ] {
            assert!(picks(chosen), "{chosen:?}");
        }
        for other in [&[][..], &["--model", "opus"], &["--resumes"], &["-p", "hi"]] {
            assert!(!picks(other), "{other:?}");
        }
    }

    #[test]
    fn the_users_own_system_prompt_replaces_folds() {
        let picks = |args: &[&str]| {
            picks_system_prompt(&args.iter().map(OsString::from).collect::<Vec<_>>())
        };
        for chosen in [
            &["--system-prompt", "be brief"][..],
            &["--system-prompt=be brief"],
            &["--model", "opus", "--system-prompt-file", "p.md"],
            &["--system-prompt-file=p.md"],
            &["--append-system-prompt", "x"],
            &["--append-system-prompt=x"],
            &["--append-system-prompt-file", "p.md"],
            &["--append-system-prompt-file=p.md"],
        ] {
            assert!(picks(chosen), "{chosen:?}");
        }
        for other in [
            &[][..],
            &["--model", "opus"],
            &["--resume", ID],
            &["--system-prompts"],
            &["--append-system-prompt-files=p.md"],
        ] {
            assert!(!picks(other), "{other:?}");
        }
    }

    #[test]
    fn the_prompt_is_the_projects_own_or_the_shipped_one_written_into_fold() {
        let project = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::new(project.path());
        let shipped = prompt_file(&paths).unwrap();
        assert_eq!(shipped, paths.main_prompt_file());
        assert_eq!(
            std::fs::read_to_string(&shipped).unwrap(),
            fold_core::main_prompt()
        );
        std::fs::write(&shipped, "stale").unwrap();
        prompt_file(&paths).unwrap();
        assert_eq!(
            std::fs::read_to_string(&shipped).unwrap(),
            fold_core::main_prompt(),
            "each start writes the prompt of the installed fld"
        );
        let own = paths.main_role_prompt();
        std::fs::create_dir_all(own.parent().unwrap()).unwrap();
        std::fs::write(&own, "# Our main agent\n").unwrap();
        assert_eq!(prompt_file(&paths).unwrap(), shipped);
        assert_eq!(
            std::fs::read_to_string(&shipped).unwrap(),
            fold_core::with_shared_rules("# Our main agent\n")
        );
        assert_eq!(std::fs::read_to_string(&own).unwrap(), "# Our main agent\n");

        std::fs::remove_dir_all(paths.fold_dir()).unwrap();
        std::fs::write(paths.fold_dir(), "").unwrap();
        assert!(prompt_file(&paths).is_err());
    }

    #[test]
    fn continue_resumes_the_recorded_session_while_claude_has_it() {
        let config = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::new(project.path());
        std::fs::create_dir_all(paths.fold_dir()).unwrap();
        let main = AgentName::parse("main").unwrap();
        let lead = AgentName::parse("lead").unwrap();
        let fresh = |id: &'static str| move || Ok::<_, ClaudeError>(id.to_owned());
        let begin = |agent: &AgentName, session, config: Option<&Path>, new_id| {
            start(&paths, agent, session, config, new_id)
        };
        let continued = begin(&main, Session::Continue, Some(config.path()), fresh(ID));
        assert_eq!(continued.unwrap(), Start::New(ID.to_owned()));
        assert_eq!(
            std::fs::read_to_string(paths.main_session(&main)).unwrap(),
            format!("{ID}\n")
        );
        let other = "11111111-2222-4333-8444-555555555555";
        let empty = begin(&main, Session::Continue, Some(config.path()), fresh(other));
        assert_eq!(
            empty.unwrap(),
            Start::New(other.to_owned()),
            "no transcript yet"
        );

        let name: String = project
            .path()
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let folder = config.path().join("projects").join(name);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(format!("{other}.jsonl")), "").unwrap();
        let resumed = begin(&main, Session::Continue, Some(config.path()), fresh(ID));
        assert_eq!(resumed.unwrap(), Start::Resume(other.to_owned()));
        let own = begin(&lead, Session::Continue, Some(config.path()), fresh(ID));
        assert_eq!(
            own.unwrap(),
            Start::New(ID.to_owned()),
            "another name never resumes the session of main"
        );
        assert_eq!(recorded(&paths, &main).as_deref(), Some(other));
        assert_eq!(recorded(&paths, &lead).as_deref(), Some(ID));
        let no_config = begin(&main, Session::Continue, None, fresh(ID));
        assert_eq!(no_config.unwrap(), Start::New(ID.to_owned()));
        let new = begin(&main, Session::New, Some(config.path()), fresh(other));
        assert_eq!(new.unwrap(), Start::New(other.to_owned()));
        std::fs::write(paths.main_session(&main), "not an id\n").unwrap();
        let bad = begin(&main, Session::Continue, Some(config.path()), fresh(ID));
        assert_eq!(bad.unwrap(), Start::New(ID.to_owned()));

        std::fs::remove_dir_all(paths.fold_dir()).unwrap();
        std::fs::write(paths.fold_dir(), "").unwrap();
        let unrecorded = begin(&main, Session::New, Some(config.path()), fresh(ID));
        assert!(matches!(unrecorded, Err(ClaudeError::Record(_))));
        let failed = start(&paths, &main, Session::New, Some(config.path()), || {
            Err(ClaudeError::Runtime(io::Error::other("x")))
        });
        assert!(matches!(failed, Err(ClaudeError::Runtime(_))));
    }

    #[test]
    fn a_record_follows_the_session_only_for_an_agent_that_has_one() {
        let project = tempfile::tempdir().unwrap();
        let paths = ProjectPaths::new(project.path());
        let main = AgentName::parse("main").unwrap();
        let worker = AgentName::parse("web-worker").unwrap();
        let other = "11111111-2222-4333-8444-555555555555";
        follow_record(&paths, &worker, other).unwrap();
        assert!(
            !paths.main_session(&worker).exists(),
            "a worker gets no record"
        );
        write_record(&paths.main_session(&main), ID).unwrap();
        follow_record(&paths, &main, "not an id").unwrap();
        assert_eq!(recorded(&paths, &main).as_deref(), Some(ID));
        follow_record(&paths, &main, other).unwrap();
        assert_eq!(recorded(&paths, &main).as_deref(), Some(other));
        follow_record(&paths, &main, other).unwrap();
        assert_eq!(
            std::fs::read_to_string(paths.main_session(&main)).unwrap(),
            format!("{other}\n")
        );
    }

    #[test]
    fn a_session_without_the_user_setup_gets_fold_by_flags_before_the_name() {
        let agent = AgentName::parse("main").unwrap();
        let own = OwnSetup::for_binary(Path::new("/bin/fld"), &agent, false, false).unwrap();
        let (own_mcp, own_settings) = (own.mcp.clone().unwrap(), own.settings.clone().unwrap());
        let mcp: serde_json::Value = serde_json::from_str(&own_mcp).unwrap();
        assert_eq!(
            mcp,
            serde_json::json!({"mcpServers": {"fold": {"command": "/bin/fld", "args": ["mcp"], "env": {"FOLD_AGENT": "main"}}}})
        );
        let settings: serde_json::Value = serde_json::from_str(&own_settings).unwrap();
        assert_eq!(
            settings["permissions"]["allow"],
            serde_json::json!(["mcp__fold"])
        );
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"][0]["command"],
            "fld hook stop"
        );
        let command = command(
            Path::new("claude"),
            Path::new("/src/app"),
            &agent,
            None,
            None,
            &[OsString::from("hello")],
            None,
            Some(&own),
        );
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                "--mcp-config",
                own_mcp.as_str(),
                "--settings",
                own_settings.as_str(),
                "--name",
                "main",
                "hello"
            ]
        );
        let server_held = OwnSetup::for_binary(Path::new("/bin/fld"), &agent, true, false).unwrap();
        assert_eq!(server_held.mcp, None);
        assert_eq!(server_held.settings.as_deref(), Some(own_settings.as_str()));
        let settings_held =
            OwnSetup::for_binary(Path::new("/bin/fld"), &agent, false, true).unwrap();
        assert_eq!(settings_held.mcp.as_deref(), Some(own_mcp.as_str()));
        assert_eq!(settings_held.settings, None);
        let args: Vec<_> = super::command(
            Path::new("claude"),
            Path::new("/src/app"),
            &agent,
            None,
            None,
            &[],
            None,
            Some(&settings_held),
        )
        .get_args()
        .map(ToOwned::to_owned)
        .collect();
        assert_eq!(args, ["--mcp-config", own_mcp.as_str(), "--name", "main"]);
        assert_eq!(
            OwnSetup::for_binary(Path::new("/bin/fld"), &agent, true, true),
            None
        );
    }

    #[test]
    fn the_name_defaults_to_main_and_the_rest_goes_to_claude() {
        let parsed = parse(&[]).unwrap().args;
        assert_eq!(parsed.name, "main");
        assert!(parsed.claude_args.is_empty());
        let parsed = parse(&["--name", "lead", "--", "--model", "opus", "--name"])
            .unwrap()
            .args;
        assert_eq!(parsed.name, "lead");
        assert_eq!(parsed.claude_args, ["--model", "opus", "--name"]);
        assert!(parse(&["--model"]).is_err());
    }

    #[test]
    fn claude_runs_in_the_root_with_the_name_and_the_main_tag() {
        let agent = AgentName::parse("main").unwrap();
        let extra = [OsString::from("--model"), OsString::from("opus")];
        let resumed = command(
            Path::new("claude"),
            Path::new("/src/app"),
            &agent,
            Some(&Start::Resume(ID.to_owned())),
            Some(Path::new("/src/app/.fold/sessions/prompt.md")),
            &extra,
            None,
            None,
        );
        let args: Vec<_> = resumed.get_args().collect();
        assert_eq!(
            args,
            [
                "--name",
                "main",
                "--resume",
                ID,
                "--append-system-prompt-file",
                "/src/app/.fold/sessions/prompt.md",
                "--model",
                "opus"
            ]
        );
        let own = command(
            Path::new("claude"),
            Path::new("/src/app"),
            &agent,
            None,
            None,
            &[OsString::from("-c")],
            None,
            None,
        );
        let args: Vec<_> = own.get_args().collect();
        assert_eq!(args, ["--name", "main", "-c"]);
        let command = command(
            Path::new("claude"),
            Path::new("/src/app"),
            &agent,
            Some(&Start::New(ID.to_owned())),
            Some(Path::new("/src/app/.fold/roles/main.md")),
            &extra,
            Some(Path::new("/run/by-hand.sock")),
            None,
        );
        assert_eq!(command.get_program(), "claude");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                "--name",
                "main",
                "--session-id",
                ID,
                "--append-system-prompt-file",
                "/src/app/.fold/roles/main.md",
                "--model",
                "opus"
            ]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/src/app")));
        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.contains(&("FOLD_AGENT".as_ref(), Some("main".as_ref()))));
        assert!(envs.contains(&("FOLD_AGENT_TAGS".as_ref(), Some("main".as_ref()))));
        assert!(envs.contains(&("FOLD_SOCKET".as_ref(), Some("/run/by-hand.sock".as_ref()))));
        assert!(!own.get_envs().any(|(name, _)| name == "FOLD_SOCKET"));
    }

    #[test]
    fn another_main_agent_refuses_the_start_and_the_same_one_continues() {
        let agent = AgentName::parse("main").unwrap();
        let agents = |name: &str| Response {
            status: 200,
            body: json!({"agents": [
                {"name": "backend", "tags": ["review"]},
                {"name": name, "tags": ["main"]},
            ]}),
        };
        assert!(main_is_free(&agents("main"), &agent).is_ok());
        let none = Response {
            status: 200,
            body: json!({"agents": []}),
        };
        assert!(main_is_free(&none, &agent).is_ok());
        let taken = main_is_free(&agents("lead"), &agent).unwrap_err();
        assert_eq!(taken.code(), Some(codes::MAIN_TAG_DENIED));
        assert_eq!(
            taken.to_string(),
            "agent lead is the main agent of this project"
        );
        assert_eq!(
            taken.hint(),
            Some("run `fld claude --name lead` to continue as it")
        );
        let refused = Response {
            status: 404,
            body: json!({"error": {"code": "project_not_found", "message": "gone", "details": {"hint": "fld init"}}}),
        };
        let error = main_is_free(&refused, &agent).unwrap_err();
        assert_eq!(error.code(), Some("project_not_found"));
        assert_eq!(error.hint(), Some("fld init"));
        let bare = Response {
            status: 500,
            body: serde_json::Value::Null,
        };
        let error = main_is_free(&bare, &agent).unwrap_err();
        assert_eq!(error.code(), Some(codes::INTERNAL_ERROR));
        assert_eq!(error.hint(), None);
    }

    #[test]
    fn errors_name_their_code_and_hint() {
        let outside = ClaudeError::Project(ProjectError::Fold(FoldError::new(
            codes::PROJECT_NOT_FOUND,
            "no registered project holds this directory",
        )));
        assert_eq!(outside.code(), Some("project_not_found"));
        assert!(outside.hint().unwrap().contains("fld init"));
        let exec = ClaudeError::Exec(io::Error::from(io::ErrorKind::NotFound));
        assert_eq!(exec.code(), None);
        assert!(exec.hint().unwrap().contains("Claude Code"));
        let denied = ClaudeError::Exec(io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(denied.hint(), None);
        assert_eq!(ClaudeError::Runtime(io::Error::other("x")).hint(), None);
        let record = ClaudeError::Record(io::Error::other("x"));
        assert_eq!((record.code(), record.hint()), (None, None));
        let prompt = ClaudeError::Prompt(io::Error::other("x"));
        assert_eq!((prompt.code(), prompt.hint()), (None, None));
        assert!(prompt.to_string().contains("main agent prompt"));
        let bad = ClaudeError::Fold(FoldError::new(codes::INVALID_NAME, "bad").with_hint("pick"));
        assert_eq!(bad.code(), Some("invalid_name"));
        assert_eq!(bad.hint(), Some("pick"));
    }
}
