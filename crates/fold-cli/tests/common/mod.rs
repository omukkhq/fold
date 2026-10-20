#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::Stdio;

use fold_core::{Clock, ManualClock, ProjectPaths};
use fold_store::projects::{self, ProjectRow};
use fold_store::{Store, StoreError};
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};

pub type Client = RunningService<RoleClient, ()>;

/// Writes `stdin` to the child and closes it. A hook may exit before it reads its input, as `fld hook` does without `FOLD_AGENT`, so a broken pipe is accepted; any other write error fails the test.
pub async fn feed(child: &mut Child, stdin: &str) {
    let mut input = child.stdin.take().unwrap();
    if let Err(error) = input.write_all(stdin.as_bytes()).await {
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::BrokenPipe,
            "writing the hook's stdin failed: {error}"
        );
    }
}

/// A private host: HOME and the XDG directories in one temporary directory, and one registered project, `app`. `daemon.toml` makes any daemon started here listen on a free port.
pub struct Host {
    pub dir: TempDir,
    pub root: PathBuf,
}

impl Host {
    pub async fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let host = Self {
            root: PathBuf::new(),
            dir,
        };
        std::fs::create_dir_all(host.config_home().join("fold")).unwrap();
        std::fs::write(
            host.config_home().join("fold").join("daemon.toml"),
            "listen = \"127.0.0.1:0\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(host.runtime_base()).unwrap();
        let paths = ProjectPaths::new(host.dir.path().join("app"));
        Store::open_project(&paths)
            .await
            .unwrap()
            .close()
            .await
            .unwrap();
        let root = paths.root().canonicalize().unwrap();
        let registry = Store::open_registry(&host.data_home().join("fold").join("registry.db"))
            .await
            .unwrap();
        let row = ProjectRow {
            slug: "app".to_owned(),
            name: "App".to_owned(),
            root: root.to_string_lossy().into_owned(),
        };
        let now = ManualClock::new().now();
        registry
            .write(async |tx| {
                projects::insert_project(tx, &row, now).await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
        registry.close().await.unwrap();
        Self { root, ..host }
    }

    pub fn config_home(&self) -> PathBuf {
        self.dir.path().join("config")
    }

    pub fn data_home(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    pub fn runtime_base(&self) -> PathBuf {
        self.dir.path().join("run")
    }

    /// The socket a daemon started without `--socket` uses.
    pub fn default_socket(&self) -> PathBuf {
        self.runtime_base().join("fold").join("fold.sock")
    }

    /// `fld <args>` with this host's environment, run in `cwd`.
    pub fn fold(&self, args: &[&str], cwd: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
        command
            .args(args)
            .current_dir(cwd)
            .env("HOME", self.dir.path())
            .env("XDG_CONFIG_HOME", self.config_home())
            .env("XDG_DATA_HOME", self.data_home())
            .env("XDG_RUNTIME_DIR", self.runtime_base())
            // A daemon started here reads this shell's environment at its start, never the user's own shell.
            .env("SHELL", "/bin/sh")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("FOLD_AGENT")
            .env_remove("FOLD_HUMAN")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command
    }
}

/// A `fld mcp` process driven by an rmcp client over its stdio. Cancelling the client closes the adapter's stdin, which ends the session.
pub async fn adapter(mut command: Command) -> (Client, Child) {
    let mut child = command.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let client = ().serve((stdout, stdin)).await.unwrap();
    (client, child)
}

pub async fn list_agents(client: &Client) -> Value {
    let result = client
        .call_tool(CallToolRequestParams::new("list_agents"))
        .await
        .unwrap();
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

/// The process table with each process's arguments. sysinfo reads it on every platform; macOS has no `/proc`.
fn processes() -> sysinfo::System {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, UpdateKind};
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    system
}

/// The pids of the `fld serve` processes that use `socket`.
pub fn daemons(socket: &Path) -> Vec<i32> {
    let socket = socket.as_os_str();
    processes()
        .processes()
        .iter()
        // On Linux sysinfo lists every thread of a process as well.
        .filter(|(_, process)| process.thread_kind().is_none())
        .filter(|(_, process)| {
            let args = process.cmd();
            args.iter().any(|arg| arg == "serve") && args.iter().any(|arg| arg == socket)
        })
        .filter_map(|(pid, _)| i32::try_from(pid.as_u32()).ok())
        .collect()
}

/// Stops a detached daemon with SIGTERM, so it shuts down cleanly, and waits until it is gone.
pub struct DaemonGuard(pub Vec<i32>);

impl DaemonGuard {
    pub async fn stop(mut self) {
        let pids = std::mem::take(&mut self.0);
        terminate(&pids);
        fold_store::testing::eventually(std::time::Duration::from_secs(10), async || {
            let table = processes();
            let alive: Vec<i32> = pids
                .iter()
                .copied()
                .filter(|pid| {
                    u32::try_from(*pid)
                        .ok()
                        .and_then(|pid| table.process(sysinfo::Pid::from_u32(pid)))
                        .is_some_and(|process| process.status() != sysinfo::ProcessStatus::Zombie)
                })
                .collect();
            if alive.is_empty() { Ok(()) } else { Err(alive) }
        })
        .await;
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        terminate(&self.0);
    }
}

fn terminate(pids: &[i32]) {
    for pid in pids {
        if let Some(pid) = rustix::process::Pid::from_raw(*pid) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
    }
}

/// The files `fld init` writes into a new project's folder that are there now, and their bytes as `r1` counts them. The `.gitignore` always counts, with the bytes it holds after init, so the folder's own `.gitignore` is not part of the folder's file count. An empty `.foldignore` counts as a file of no bytes.
pub fn init_files(project: &Path) -> (usize, u64) {
    let written = [
        "AGENTS.md",
        ".claude/skills/fold/SKILL.md",
        ".foldignore",
        ".gitignore",
    ];
    let mut found = (0, 0);
    for relative in written {
        let path = project.join(relative);
        let bytes = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta.len(),
            Err(_) => continue,
        };
        found = (found.0 + 1, found.1 + bytes);
    }
    found
}

/// The `r1:` line `fld init` prints for a folder whose own files are `files` with `bytes`, with what init writes beside them.
pub fn r1_line(project: &Path, files: usize, bytes: u64) -> String {
    let (made, made_bytes) = init_files(project);
    format!("r1: {} files, {} bytes", files + made, bytes + made_bytes)
}
