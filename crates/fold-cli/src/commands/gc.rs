use std::io::{self, Write};
use std::path::PathBuf;

use clap::Args;
use fold_cli::daemon_client::{ClientConfig, ClientError, DaemonClient, Response};
use fold_daemon::http::projects::{
    GcAction, GcCounts, GcWorker, GcWorkerReason, GcWorkspace, GcWorkspaceReason,
};
use fold_daemon::services::projects::{ProjectError, slug_for_dir_on_host};
use hyper::Method;
use thiserror::Error;

use crate::init::{InitError, host_paths};
use crate::output;
use crate::serve::{Dirs, ServeError, current_thread_runtime, socket_path};

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
    #[error("no daemon listens on {}; start it with fld serve", path.display())]
    NoDaemon { path: PathBuf },
    #[error(transparent)]
    Client(ClientError),
    #[error("{message}")]
    Refused {
        code: String,
        message: String,
        hint: Option<String>,
    },
    #[error("the daemon answered with something that is not a report")]
    Reply,
    #[error("cannot start the async runtime: {0}")]
    Runtime(io::Error),
    #[error("cannot write the output: {0}")]
    Write(io::Error),
}

impl From<ClientError> for GcError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::NoDaemon { path } => Self::NoDaemon { path },
            other => Self::Client(other),
        }
    }
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

    /// The hint that names the next step: the daemon's own for a refusal, else the one a setup or project error carries.
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Refused { hint, .. } => hint.as_deref(),
            Self::Setup(error) => error.hint(),
            Self::Project(ProjectError::Fold(error)) => error.hint(),
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
    let runtime = current_thread_runtime().map_err(GcError::Runtime)?;
    let slug = runtime.block_on(slug_for_dir_on_host(&host, &cwd))?;
    let socket = socket_path(args.socket.as_deref(), dirs)?.ok_or(GcError::NoSocket)?;
    let client = DaemonClient::new(ClientConfig::running(socket), human);
    let body = serde_json::json!({ "dry_run": args.dry_run });
    let path = format!("/api/v1/projects/{slug}/gc");
    let response = runtime.block_on(client.request(Method::POST, &path, Some(&body)))?;
    let counts = interpret(response)?;
    print_counts(out, &slug, args.dry_run, &counts).map_err(GcError::Write)
}

fn interpret(response: Response) -> Result<GcCounts, GcError> {
    if response.is_success() {
        return serde_json::from_value(response.body).map_err(|_| GcError::Reply);
    }
    let refusal = response.refusal();
    match (refusal.code, refusal.message) {
        (Some(code), Some(message)) => Err(GcError::Refused {
            code,
            message,
            hint: refusal.hint,
        }),
        _ => Err(GcError::Reply),
    }
}

fn print_counts(
    out: &mut impl Write,
    slug: &str,
    dry_run: bool,
    counts: &GcCounts,
) -> io::Result<()> {
    let heading = if dry_run {
        format!("garbage collection of project {slug}, dry run: nothing was changed")
    } else {
        format!("garbage collection of project {slug} finished")
    };
    output::line(out, &heading)?;
    let rows = [
        ("objects examined", counts.examined),
        ("objects deleted", counts.deleted),
        ("bytes freed", counts.bytes_freed),
        ("temporary files removed", counts.tmp_removed),
        ("missing objects", counts.missing),
        ("workspaces removed", counts.workspaces_removed),
        ("checkpoints pruned", counts.checkpoints_pruned),
        ("worker folders removed", counts.workers_removed),
        ("worker folder bytes freed", counts.worker_bytes_freed),
    ];
    for (label, value) in rows {
        output::line(out, &format!("{label}: {value}"))?;
    }
    for workspace in &counts.workspaces {
        output::line(out, &workspace_line(workspace, dry_run))?;
    }
    for worker in &counts.workers {
        output::line(out, &worker_line(worker, dry_run))?;
    }
    Ok(())
}

fn action(action: GcAction, dry_run: bool) -> &'static str {
    match (action, dry_run) {
        (GcAction::Remove, true) => "would remove",
        (GcAction::Remove, false) => "removed",
        (GcAction::Keep, _) => "kept",
    }
}

/// One closed workspace as `workspace ws-2 .fold/ws/review-p-1: would remove, it closed more than closed_workspace_days ago`.
fn workspace_line(workspace: &GcWorkspace, dry_run: bool) -> String {
    let owner = workspace.owner.as_deref().unwrap_or("its owner");
    let why = match workspace.reason {
        GcWorkspaceReason::OwnerEnded => {
            format!("{owner} has no running worker process and no open session")
        }
        GcWorkspaceReason::PastWindow => "it closed more than closed_workspace_days ago".to_owned(),
        GcWorkspaceReason::OwnerProcess => {
            format!("{owner} has a worker process that has not ended")
        }
        GcWorkspaceReason::OwnerSession => format!("{owner} has an open session"),
        GcWorkspaceReason::SessionInside => "an open session works in it".to_owned(),
        GcWorkspaceReason::InvalidPath => {
            "its path is no directory directly below .fold/ws".to_owned()
        }
        GcWorkspaceReason::Failed => format!(
            "the removal failed ({}); the next run tries again",
            workspace.code.as_deref().unwrap_or("internal_error")
        ),
    };
    let returned = if workspace.returned {
        " (it came back after fold removed it)"
    } else {
        ""
    };
    format!(
        "workspace {} {}{returned}: {}, {why}",
        workspace.workspace,
        workspace.path,
        action(workspace.action, dry_run)
    )
}

/// One worker folder as `worker folder reviewer-12, 372 bytes: would remove, it can never run again on its own`.
fn worker_line(worker: &GcWorker, dry_run: bool) -> String {
    let why = match worker.reason {
        GcWorkerReason::Finished => "it can never run again on its own",
        GcWorkerReason::Claim => "it holds a claim",
        GcWorkerReason::Session => "an open session uses it",
        GcWorkerReason::Parked => "it is parked",
        GcWorkerReason::Waiting => "fold owes it a turn",
        GcWorkerReason::OwedResume => "fold owes it a resume",
        GcWorkerReason::OwedMessage => "a message waits for it",
        GcWorkerReason::InvalidPath => "its folder is no directory",
        GcWorkerReason::Failed => "the removal failed; the next run tries again",
    };
    format!(
        "worker folder {}, {} bytes: {}, {why}",
        worker.agent,
        worker.bytes,
        action(worker.action, dry_run)
    )
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
            Wrapper::try_parse_from(["fld", "--dry-run", "--socket", "/tmp/f.sock"]).unwrap();
        assert!(parsed.args.dry_run);
        assert_eq!(
            parsed.args.socket.as_deref(),
            Some(Path::new("/tmp/f.sock"))
        );
        let bare = Wrapper::try_parse_from(["fld"]).unwrap();
        assert!(!bare.args.dry_run && bare.args.socket.is_none());
    }

    fn response(status: u16, body: serde_json::Value) -> Response {
        Response { status, body }
    }

    fn report(deleted: u64) -> serde_json::Value {
        json!({
            "dry_run": false, "examined": 5, "deleted": deleted, "bytes_freed": 80,
            "tmp_removed": 0, "missing": 0, "workspaces_removed": 0, "checkpoints_pruned": 1,
            "workers_removed": 1, "worker_bytes_freed": 372,
            "workspaces": [
                {"workspace": "ws-2", "path": ".fold/ws/review-p-1", "owner": "review-lead",
                 "returned": false, "action": "remove", "reason": "past_window", "code": null},
                {"workspace": "ws-9", "path": ".fold/ws/t-5", "owner": "main",
                 "returned": true, "action": "keep", "reason": "owner_session", "code": null},
                {"workspace": "ws-12", "path": ".fold/ws/t-8", "owner": null,
                 "returned": false, "action": "keep", "reason": "failed", "code": "unrecorded_changes"},
            ],
            "workers": [
                {"agent": "reviewer-3", "bytes": 372, "action": "remove", "reason": "finished"},
                {"agent": "web-worker-2", "bytes": 40, "action": "keep", "reason": "claim"},
            ],
        })
    }

    #[test]
    fn only_a_missing_daemon_suggests_starting_one() {
        let none = GcError::from(ClientError::NoDaemon {
            path: "/run/fold.sock".into(),
        });
        assert_eq!(
            none.to_string(),
            "no daemon listens on /run/fold.sock; start it with fld serve"
        );
        let other = GcError::from(ClientError::HumanUnknown);
        assert_eq!(other.to_string(), "cannot tell who you are: set FOLD_HUMAN");
    }

    #[test]
    fn answers_become_counts_or_coded_refusals() {
        let ok = interpret(response(200, report(3))).unwrap();
        assert_eq!(ok.deleted, 3);
        let busy = json!({"error": {"code": "gc_in_progress", "message": "busy", "details": {}}});
        let error = interpret(response(409, busy)).unwrap_err();
        assert_eq!(error.code(), Some("gc_in_progress"));
        assert_eq!(error.to_string(), "busy");
        let partial = json!({"examined": 5, "deleted": 2});
        for broken in [
            response(200, serde_json::Value::Null),
            response(200, partial),
            response(500, json!({})),
        ] {
            assert!(matches!(interpret(broken), Err(GcError::Reply)));
        }
    }

    #[test]
    fn counts_print_one_labelled_number_per_line() {
        let counts = interpret(response(200, report(2))).unwrap();
        let mut out = Vec::new();
        print_counts(&mut out, "demo", true, &counts).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 15);
        assert!(lines[0].contains("dry run"));
        assert_eq!(lines[1], "objects examined: 5");
        assert_eq!(lines[3], "bytes freed: 80");
        assert_eq!(lines[7], "checkpoints pruned: 1");
        assert_eq!(lines[8], "worker folders removed: 1");
        assert_eq!(lines[9], "worker folder bytes freed: 372");
        assert_eq!(
            lines[10],
            "workspace ws-2 .fold/ws/review-p-1: would remove, it closed more than closed_workspace_days ago"
        );
        assert_eq!(
            lines[11],
            "workspace ws-9 .fold/ws/t-5 (it came back after fold removed it): kept, main has an open session"
        );
        assert_eq!(
            lines[12],
            "workspace ws-12 .fold/ws/t-8: kept, the removal failed (unrecorded_changes); the next run tries again"
        );
        assert_eq!(
            lines[13],
            "worker folder reviewer-3, 372 bytes: would remove, it can never run again on its own"
        );
        assert_eq!(
            lines[14],
            "worker folder web-worker-2, 40 bytes: kept, it holds a claim"
        );
        let mut out = Vec::new();
        print_counts(&mut out, "demo", false, &counts).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("ws-2 .fold/ws/review-p-1: removed, it closed"),
            "{text}"
        );
        assert!(text.contains("reviewer-3, 372 bytes: removed, "), "{text}");
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
