use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args;
use fold_core::{FoldError, HostPaths, codes};
use fold_daemon::SystemClock;
use fold_daemon::services::projects::{ProjectError, init_project};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use crate::output;
use crate::serve::{Dirs, FileConfig, ServeError, read_file_config};

/// How long a daemon gets to accept the connection and answer the open request.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Args, Debug)]
pub struct InitArgs {
    /// The project directory; created when it does not exist
    dir: PathBuf,
    /// The project name; defaults to the directory name
    #[arg(long)]
    name: Option<String>,
    /// Unix socket of the running daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum InitError {
    #[error(transparent)]
    HostPaths(FoldError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error("cannot write the output: {0}")]
    Write(#[from] io::Error),
}

impl InitError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::HostPaths(error) => Some(error.code()),
            Self::Project(error) => error.code(),
            Self::Runtime(_) | Self::Write(_) => None,
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
    let host = host_paths(dirs)?;
    let socket = socket_path(args.socket.as_deref(), dirs);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(InitError::Runtime)?;
    let project = runtime.block_on(init_project(
        &host,
        &args.dir,
        args.name.as_deref(),
        &SystemClock,
    ))?;
    let root = project.root.display();
    output::line(
        out,
        &format!("initialised project {} at {root}", project.slug),
    )?;
    let note = match socket {
        Ok(Some(socket)) => match runtime.block_on(register(&socket, &project.slug)) {
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
    Ok(())
}

fn host_paths(dirs: &Dirs) -> Result<HostPaths, InitError> {
    let data_home = utf8(dirs.data_home.as_deref(), "XDG_DATA_HOME")?;
    let home = utf8(dirs.home.as_deref(), "HOME")?;
    HostPaths::new(data_home, home).map_err(InitError::HostPaths)
}

/// The daemon finds the registry through the same variables as a `PathBuf`, so a value that is not UTF-8 is refused instead of skipped.
fn utf8<'a>(path: Option<&'a Path>, name: &str) -> Result<Option<&'a str>, InitError> {
    match path {
        None => Ok(None),
        Some(path) => path.to_str().map(Some).ok_or_else(|| {
            InitError::HostPaths(FoldError::new(
                codes::INVALID_HOST_PATHS,
                format!("{name} is not valid UTF-8"),
            ))
        }),
    }
}

/// The flag wins over `daemon.toml`, which wins over the runtime directory. `None` means no daemon can be listening.
fn socket_path(flag: Option<&Path>, dirs: &Dirs) -> Result<Option<PathBuf>, ServeError> {
    if let Some(flag) = flag {
        return Ok(Some(flag.to_owned()));
    }
    let file = match dirs.config_file() {
        Ok(path) => read_file_config(&path)?,
        Err(_) => FileConfig::default(),
    };
    Ok(file.socket.or_else(|| {
        dirs.runtime_dir
            .as_ref()
            .map(|runtime| runtime.join("fold").join("fold.sock"))
    }))
}

async fn register(socket: &Path, slug: &str) -> Registration {
    tokio::time::timeout(ANSWER_TIMEOUT, ask_daemon(socket, slug))
        .await
        .unwrap_or_else(|_| Registration::Refused("no answer".to_owned()))
}

/// A missing socket file or a refused connection means no daemon runs, which is not an error: the daemon opens every registered project when it starts.
async fn ask_daemon(socket: &Path, slug: &str) -> Registration {
    let stream = match UnixStream::connect(socket).await {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Registration::NoDaemon;
        }
        Err(error) => return Registration::Refused(error.kind().to_string()),
    };
    let body = serde_json::json!({ "slug": slug }).to_string();
    match post_projects(stream, &body).await {
        Ok(reply) => interpret(&reply),
        Err(error) => Registration::Refused(error.kind().to_string()),
    }
}

async fn post_projects(mut stream: UnixStream, body: &str) -> io::Result<Vec<u8>> {
    let request = format!(
        "POST /api/v1/projects HTTP/1.1\r\nHost: fold\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await?;
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).await?;
    Ok(reply)
}

fn interpret(reply: &[u8]) -> Registration {
    let text = String::from_utf8_lossy(reply);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok());
    if status.is_some_and(|status| (200..300).contains(&status)) {
        return Registration::Opened;
    }
    let code = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| value["error"]["code"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned());
    Registration::Refused(code)
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
            "fold",
            "~/scratch",
            "--name",
            "Scratch",
            "--socket",
            "/tmp/f.sock",
        ])
        .unwrap();
        assert_eq!(parsed.args.dir, Path::new("~/scratch"));
        assert_eq!(parsed.args.name.as_deref(), Some("Scratch"));
        assert_eq!(
            parsed.args.socket.as_deref(),
            Some(Path::new("/tmp/f.sock"))
        );
        let bare = Wrapper::try_parse_from(["fold", "dir"]).unwrap();
        assert!(bare.args.name.is_none() && bare.args.socket.is_none());
        assert!(Wrapper::try_parse_from(["fold"]).is_err());
    }

    #[test]
    fn replies_are_read_by_status_and_error_code() {
        let ok = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}";
        assert_eq!(interpret(ok), Registration::Opened);
        let created = b"HTTP/1.1 201 Created\r\n\r\n{}";
        assert_eq!(interpret(created), Registration::Opened);
        let refused = b"HTTP/1.1 404 Not Found\r\n\r\n{\"error\":{\"code\":\"project_not_found\",\"message\":\"m\",\"details\":{}}}";
        assert_eq!(
            interpret(refused),
            Registration::Refused("project_not_found".to_owned())
        );
        assert_eq!(
            interpret(b"garbage"),
            Registration::Refused("unknown".to_owned())
        );
    }

    fn dirs(runtime: Option<&str>, config: Option<&str>) -> Dirs {
        Dirs {
            home: Some("/nonexistent-home".into()),
            config_home: config.map(Into::into),
            data_home: None,
            runtime_dir: runtime.map(Into::into),
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
            dir: dir.path().join("p"),
            name: None,
            socket: None,
        };
        let mut out = Vec::new();
        let error = run(&args, &Dirs::default(), &mut out).unwrap_err();
        assert_eq!(error.code(), Some("invalid_host_paths"));
        assert!(!dir.path().join("p").exists());
        assert!(out.is_empty());
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
        assert_eq!(error.code(), Some("invalid_host_paths"));
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
        assert_eq!(register(&missing, "s").await, Registration::NoDaemon);
        let stale = dir.path().join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
        assert_eq!(register(&stale, "s").await, Registration::NoDaemon);
    }
}
