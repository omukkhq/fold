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
        self.fold_with(args, &[])
    }

    fn fold_with(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fold"));
        command
            .args(args)
            .current_dir(self.dir.path())
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_DATA_HOME", self.path("data"))
            .envs(env.iter().copied());
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

/// Answers one request with `201 Created` and returns the request as it arrived.
#[cfg(test)]
async fn accept_one(listener: &tokio::net::UnixListener) -> String {
    let (mut stream, _) = listener.accept().await.unwrap();
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0, "the client closed before its request ended");
        request.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&request);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .map_or(0, |value| value.parse::<usize>().unwrap());
            if body.len() >= length {
                break;
            }
        }
    }
    stream
        .write_all(b"HTTP/1.1 201 Created\r\ncontent-length: 2\r\n\r\n{}")
        .await
        .unwrap();
    String::from_utf8(request).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn init_names_its_human_in_the_request() {
    use std::os::unix::fs::MetadataExt;

    let sandbox = Arc::new(Sandbox::new());
    let socket = sandbox.path("fake.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let uid = std::fs::metadata("/proc/self").unwrap().uid();
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap();
    let account = fold_cli::daemon_client::account_name(&passwd, uid).unwrap();
    for (project, env, human) in [
        (
            "lead-project",
            vec![("FOLD_HUMAN", "lead"), ("USER", "ana")],
            "lead",
        ),
        ("user-project", vec![("USER", "ana")], "ana"),
        ("account-project", vec![], account.as_str()),
    ] {
        let args = [
            "init".to_owned(),
            sandbox.path(project).to_string_lossy().into_owned(),
            "--socket".to_owned(),
            socket.to_string_lossy().into_owned(),
        ];
        let env: Vec<(String, String)> = env
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect();
        let run = {
            let sandbox = sandbox.clone();
            tokio::task::spawn_blocking(move || {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let env: Vec<(&str, &str)> = env
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str()))
                    .collect();
                sandbox.fold_with(&args, &env)
            })
        };
        let request = accept_one(&listener).await;
        let output = run.await.unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(stdout(&output).ends_with("opened by the running daemon\n"));
        assert!(
            request.starts_with("POST /api/v1/projects HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(
            request.contains(&format!("x-fold-human: {human}\r\n")),
            "{request}"
        );
        let slug = project.to_owned();
        assert!(
            request.ends_with(&format!("{{\"slug\":\"{slug}\"}}")),
            "{request}"
        );
    }
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
