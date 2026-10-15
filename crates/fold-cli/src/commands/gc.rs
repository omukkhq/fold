use std::io::{self, Write};
use std::path::PathBuf;

use clap::Args;
use fold_cli::daemon_client::{ClientConfig, ClientError, DaemonClient, Response};
use fold_daemon::services::projects::{ProjectError, slug_for_dir_on_host};
use hyper::Method;
use serde_json::Value;
use thiserror::Error;

use crate::init::{InitError, host_paths};
use crate::output;
use crate::serve::{Dirs, ServeError, socket_path};

/// The report fields in the order they are printed, with their labels.
const COUNTS: [(&str, &str); 7] = [
    ("examined", "objects examined"),
    ("deleted", "objects deleted"),
    ("bytes_freed", "bytes freed"),
    ("tmp_removed", "temporary files removed"),
    ("missing", "missing objects"),
    ("workspaces_removed", "workspaces removed"),
    ("checkpoints_pruned", "checkpoints pruned"),
];

#[derive(Args, Debug)]
pub struct GcArgs {
    /// Count what a run would remove and change nothing
    #[arg(long)]
    dry_run: bool,
    /// Unix socket of the running daemon; overrides daemon.toml (default $XDG_RUNTIME_DIR/fold/fold.sock)
    #[arg(long)]
    socket: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum GcError {
    #[error("cannot read the working directory: {0}")]
    WorkingDir(io::Error),
    #[error(transparent)]
    Setup(#[from] InitError),
    #[error(transparent)]
    Config(#[from] ServeError),
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error("no daemon socket is known: set XDG_RUNTIME_DIR or pass --socket")]
    NoSocket,
    #[error("{0}; start it with fold serve")]
    Client(#[from] ClientError),
    #[error("{message}")]
    Refused { code: String, message: String },
    #[error("the daemon answered with something that is not a report")]
    Reply,
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error("cannot write the output: {0}")]
    Write(io::Error),
}

impl GcError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Setup(error) => error.code(),
            Self::Config(error) => error.code(),
            Self::Project(error) => error.code(),
            Self::Refused { code, .. } => Some(code),
            _ => None,
        }
    }
}

/// Runs garbage collection of the project that holds `cwd` through a running daemon, which it never starts, and prints the counts. `human` names the caller in the request.
pub fn run(
    args: &GcArgs,
    dirs: &Dirs,
    cwd: io::Result<PathBuf>,
    human: Option<String>,
    out: &mut impl Write,
) -> Result<(), GcError> {
    let cwd = cwd
        .and_then(std::fs::canonicalize)
        .map_err(GcError::WorkingDir)?;
    let host = host_paths(dirs).map_err(InitError::HostPaths)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(GcError::Runtime)?;
    let slug = runtime.block_on(slug_for_dir_on_host(&host, &cwd))?;
    let socket = socket_path(args.socket.as_deref(), dirs)?.ok_or(GcError::NoSocket)?;
    let client = DaemonClient::new(
        ClientConfig {
            socket,
            runtime_dir: None,
            fold_exe: None,
        },
        human,
    );
    let body = serde_json::json!({ "dry_run": args.dry_run });
    let path = format!("/api/v1/projects/{slug}/gc");
    let response = runtime.block_on(client.request(Method::POST, &path, Some(&body)))?;
    let counts = interpret(response)?;
    print_counts(out, &slug, args.dry_run, &counts).map_err(GcError::Write)
}

fn interpret(response: Response) -> Result<Value, GcError> {
    if (200..300).contains(&response.status) && response.body.is_object() {
        return Ok(response.body);
    }
    let error = &response.body["error"];
    match (error["code"].as_str(), error["message"].as_str()) {
        (Some(code), Some(message)) => Err(GcError::Refused {
            code: code.to_owned(),
            message: message.to_owned(),
        }),
        _ => Err(GcError::Reply),
    }
}

fn print_counts(out: &mut impl Write, slug: &str, dry_run: bool, counts: &Value) -> io::Result<()> {
    let heading = if dry_run {
        format!("garbage collection of project {slug}, dry run: nothing was changed")
    } else {
        format!("garbage collection of project {slug} finished")
    };
    output::line(out, &heading)?;
    for (field, label) in COUNTS {
        let value = counts[field].as_u64().unwrap_or(0);
        output::line(out, &format!("{label}: {value}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use clap::Parser;
    use serde_json::json;

    use super::*;

    #[derive(Parser)]
    struct Wrapper {
        #[command(flatten)]
        args: GcArgs,
    }

    #[test]
    fn parses_dry_run_and_socket() {
        let parsed =
            Wrapper::try_parse_from(["fold", "--dry-run", "--socket", "/tmp/f.sock"]).unwrap();
        assert!(parsed.args.dry_run);
        assert_eq!(
            parsed.args.socket.as_deref(),
            Some(Path::new("/tmp/f.sock"))
        );
        let bare = Wrapper::try_parse_from(["fold"]).unwrap();
        assert!(!bare.args.dry_run && bare.args.socket.is_none());
    }

    fn response(status: u16, body: Value) -> Response {
        Response { status, body }
    }

    #[test]
    fn answers_become_counts_or_coded_refusals() {
        let ok = interpret(response(200, json!({"deleted": 3}))).unwrap();
        assert_eq!(ok["deleted"], 3);
        let busy = json!({"error": {"code": "gc_in_progress", "message": "busy", "details": {}}});
        let error = interpret(response(409, busy)).unwrap_err();
        assert_eq!(error.code(), Some("gc_in_progress"));
        assert_eq!(error.to_string(), "busy");
        for broken in [response(200, Value::Null), response(500, json!({}))] {
            assert!(matches!(interpret(broken), Err(GcError::Reply)));
        }
    }

    #[test]
    fn counts_print_one_labelled_number_per_line() {
        let counts = json!({"examined": 5, "deleted": 2, "bytes_freed": 80});
        let mut out = Vec::new();
        print_counts(&mut out, "demo", true, &counts).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 8);
        assert!(lines[0].contains("dry run"));
        assert_eq!(lines[1], "objects examined: 5");
        assert_eq!(lines[3], "bytes freed: 80");
        assert_eq!(lines[7], "checkpoints pruned: 0");
    }

    fn args(socket: Option<&Path>) -> GcArgs {
        GcArgs {
            dry_run: false,
            socket: socket.map(Path::to_owned),
        }
    }

    #[test]
    fn refusals_before_the_daemon_answers() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut out = Vec::new();
        let dirs = Dirs {
            data_home: Some(dir.path().join("data")),
            ..Dirs::default()
        };
        let human = || Some("ana".to_owned());
        let outside = run(
            &args(None),
            &dirs,
            Ok(dir.path().to_owned()),
            human(),
            &mut out,
        );
        assert_eq!(outside.unwrap_err().code(), Some("project_not_found"));
        let lost = run(
            &args(None),
            &dirs,
            Err(io::Error::other("gone")),
            human(),
            &mut out,
        );
        assert!(matches!(lost, Err(GcError::WorkingDir(_))));
        let no_home = run(
            &args(None),
            &Dirs::default(),
            Ok(dir.path().to_owned()),
            human(),
            &mut out,
        );
        assert_eq!(no_home.unwrap_err().code(), Some("invalid_host_paths"));
        assert!(out.is_empty());
    }
}
