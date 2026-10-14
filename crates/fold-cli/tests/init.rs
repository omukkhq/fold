use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

use fold_core::ManualClock;
use fold_daemon::{Daemon, DaemonConfig};
use fold_store::Store;
use serde_json::Value;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

#[cfg(test)]
struct Sandbox {
    dir: TempDir,
}

#[cfg(test)]
impl Sandbox {
    fn new() -> Self {
        Self {
            dir: TempDir::new().unwrap(),
        }
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(name)
    }

    /// The environment holds no real home, so no run touches the user's registry or a real daemon.
    fn fold(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fold"));
        command
            .args(args)
            .current_dir(self.dir.path())
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_DATA_HOME", self.path("data"));
        // Coverage runs name the profile file through this variable. `env_clear` drops every variable set before it, so this comes after.
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command.output().unwrap()
    }

    fn registry_path(&self) -> std::path::PathBuf {
        self.path("data").join("fold").join("registry.db")
    }
}

#[cfg(test)]
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[cfg(test)]
async fn get_projects(socket: &Path) -> Value {
    let mut stream = UnixStream::connect(socket).await.unwrap();
    stream
        .write_all(b"GET /api/v1/projects HTTP/1.1\r\nHost: fold\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).await.unwrap();
    let (_, body) = reply.split_once("\r\n\r\n").unwrap();
    serde_json::from_str(body).unwrap()
}

#[cfg(test)]
async fn start_daemon(sandbox: &Sandbox, data: &str) -> fold_daemon::RunningDaemon {
    let config = DaemonConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        socket: sandbox.path("run").join("fold.sock"),
        data_dir: sandbox.path(data).join("fold"),
        runtime_dir: sandbox.path("run"),
    };
    Daemon::start(config, Arc::new(ManualClock::new()))
        .await
        .unwrap()
}

#[cfg(test)]
async fn fold_blocking(sandbox: Arc<Sandbox>, args: Vec<String>) -> Output {
    tokio::task::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        sandbox.fold(&args)
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn init_registers_with_a_running_daemon() {
    let sandbox = Arc::new(Sandbox::new());
    let daemon = start_daemon(&sandbox, "data").await;
    let socket = daemon.socket_path().to_owned();
    assert_eq!(
        get_projects(&socket).await["projects"],
        serde_json::json!([])
    );

    let project = sandbox.path("scratch");
    let output = fold_blocking(
        sandbox.clone(),
        vec![
            "init".into(),
            project.to_string_lossy().into_owned(),
            "--socket".into(),
            socket.to_string_lossy().into_owned(),
        ],
    )
    .await;
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    let canonical = project.canonicalize().unwrap();
    assert!(text.starts_with(&format!(
        "initialised project scratch at {}\n",
        canonical.display()
    )));
    assert!(text.ends_with("opened by the running daemon\n"));

    let listed = get_projects(&socket).await;
    assert_eq!(listed["projects"][0]["slug"], "scratch");
    assert_eq!(listed["projects"][0]["available"], true);
    daemon.shutdown().await;
}

#[test]
fn init_without_a_daemon_skips_the_call() {
    let sandbox = Sandbox::new();
    let socket = sandbox.path("run").join("fold.sock");
    let project = sandbox.path("scratch");
    let output = sandbox.fold(&[
        "init",
        project.to_str().unwrap(),
        "--socket",
        socket.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        format!(
            "initialised project scratch at {}\nr1: 0 files, 0 bytes\n",
            project.canonicalize().unwrap().display()
        )
    );
    assert!(!socket.exists());
    assert!(project.join(".fold").join("fold.db").is_file());

    // With no socket flag, no daemon.toml and no runtime directory there is nothing to ask.
    let second = sandbox.path("second");
    let output = sandbox.fold(&["init", second.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    assert!(!stdout(&output).contains("daemon"));
}

#[tokio::test(flavor = "multi_thread")]
async fn init_keeps_the_project_when_the_daemon_refuses() {
    let sandbox = Arc::new(Sandbox::new());
    // The daemon reads another registry, so it does not know the slug the command sends.
    let daemon = start_daemon(&sandbox, "other-data").await;
    let socket = daemon.socket_path().to_owned();
    let project = sandbox.path("scratch");
    let output = fold_blocking(
        sandbox.clone(),
        vec![
            "init".into(),
            project.to_string_lossy().into_owned(),
            "--socket".into(),
            socket.to_string_lossy().into_owned(),
        ],
    )
    .await;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = stdout(&output);
    assert!(text.contains("project_not_found"), "{text}");
    assert!(text.contains("opens the project at its next start"));
    assert!(project.join(".fold").join("fold.db").is_file());

    let registry = Store::open_registry(&sandbox.registry_path())
        .await
        .unwrap();
    let rows = fold_store::projects::list_projects(registry.read())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].slug, "scratch");
    registry.close().await.unwrap();
    daemon.shutdown().await;
}

#[test]
fn a_second_init_prints_project_exists_and_keeps_the_first_project() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("scratch");
    let first = sandbox.fold(&["init", project.to_str().unwrap()]);
    assert!(first.status.success());
    let db = project.join(".fold").join("fold.db");
    let before = std::fs::metadata(&db).unwrap().len();

    let second = sandbox.fold(&["init", project.to_str().unwrap()]);
    assert_eq!(second.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&second.stderr).contains("project_exists"));
    assert!(second.stdout.is_empty());
    assert!(db.is_file());
    assert_eq!(std::fs::metadata(&db).unwrap().len(), before);
}

#[test]
fn name_sets_the_slug_and_no_ignore_files_are_written() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("dir");
    let output = sandbox.fold(&["init", project.to_str().unwrap(), "--name", "Big Shop"]);
    assert!(stdout(&output).starts_with("initialised project big-shop at "));
    assert!(!project.join(".gitignore").exists());
    assert!(!project.join(".foldignore").exists());
}
