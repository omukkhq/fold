use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fold_cli::daemon_client;
use fold_core::ManualClock;
use fold_daemon::mcp::handshake::{Hello, PROTOCOL_VERSION};
use fold_daemon::services::agents::{DEV_STUB_DIR, Launcher};
use fold_daemon::{Daemon, DaemonConfig, RunningDaemon};
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::process::Command;

/// How long a test waits for the stub workers' effects.
const WAIT: Duration = Duration::from_secs(60);

/// A private host with a daemon in this process whose workers run `fld dev stub` of the fld under test, with a project `app` whose `.fold/dev-stub/` holds the scripts.
#[cfg(test)]
struct Host {
    /// Removes the daemon's data and the project when the test ends.
    _dir: TempDir,
    daemon: RunningDaemon,
    root: PathBuf,
}

#[cfg(test)]
impl Host {
    async fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let runtime = dir.path().join("run");
        std::fs::create_dir_all(&runtime).unwrap();
        let socket = runtime.join("fold.sock");
        let env =
            |name: &str, path: PathBuf| (name.to_owned(), path.to_string_lossy().into_owned());
        // The workers' `fld mcp` reaches this daemon only, and a missing socket never makes it start one of the user's.
        let launcher = Launcher {
            claude: dir.path().join("no-claude"),
            fold: Some(PathBuf::from(env!("CARGO_BIN_EXE_fld"))),
            env: vec![
                env("FOLD_SOCKET", socket.clone()),
                env("HOME", dir.path().to_owned()),
                env("XDG_DATA_HOME", dir.path().join("data")),
                env("XDG_CONFIG_HOME", dir.path().join("config")),
                env("XDG_RUNTIME_DIR", runtime.clone()),
            ],
            ..Launcher::default()
        };
        let daemon = Daemon::start_with(
            DaemonConfig {
                listen: "127.0.0.1:0".parse().unwrap(),
                socket,
                data_dir: dir.path().join("data").join("fold"),
                runtime_dir: runtime,
                web_dir: None,
                session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
            },
            Arc::new(ManualClock::new()),
            launcher,
        )
        .await
        .unwrap();
        let root = dir.path().join("app");
        let init = Command::new(env!("CARGO_BIN_EXE_fld"))
            .arg("init")
            .arg(&root)
            .arg("--socket")
            .arg(daemon.socket_path())
            .current_dir(dir.path())
            .env_clear()
            .env("HOME", dir.path())
            .env("XDG_DATA_HOME", dir.path().join("data"))
            .env("XDG_CONFIG_HOME", dir.path().join("config"))
            .envs(
                std::env::var_os("LLVM_PROFILE_FILE").map(|profile| ("LLVM_PROFILE_FILE", profile)),
            )
            .output()
            .await
            .unwrap();
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        let root = root.canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".fold").join(DEV_STUB_DIR)).unwrap();
        Self {
            _dir: dir,
            daemon,
            root,
        }
    }

    /// Closes the stub workers and waits for them to exit, so none is killed in the middle of a write.
    async fn stop(self) {
        self.daemon.shutdown().await;
    }

    fn script(&self, agent: &str, script: &Value) {
        let path = self
            .root
            .join(".fold")
            .join(DEV_STUB_DIR)
            .join(format!("{agent}.json"));
        std::fs::write(path, script.to_string()).unwrap();
    }

    /// The project's main agent, as `fld claude` opens it.
    async fn main_agent(&self) -> Session {
        let stream = UnixStream::connect(self.daemon.socket_path())
            .await
            .unwrap();
        let hello = Hello {
            fold: PROTOCOL_VERSION,
            cwd: self.root.clone(),
            pid: std::process::id(),
            agent: Some("main".to_owned()),
            tags: vec!["main".to_owned()],
            inbox: None,
        };
        let (read, write) = daemon_client::handshake(stream, &hello).await.unwrap();
        let session = Session(().serve((read, write)).await.unwrap());
        session.ok("register_agent", json!({"name": "main"})).await;
        session
    }

    fn worker_file(&self, agent: &str, name: &str) -> PathBuf {
        self.root
            .join(".fold")
            .join("workers")
            .join(agent)
            .join(name)
    }
}

#[cfg(test)]
struct Session(RunningService<RoleClient, ()>);

#[cfg(test)]
impl Session {
    async fn call(&self, tool: &str, arguments: Value) -> (bool, Value) {
        let Value::Object(arguments) = arguments else {
            panic!("arguments must be an object");
        };
        let result = self
            .0
            .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
            .await
            .unwrap();
        let text = &result.content[0].as_text().unwrap().text;
        (
            result.is_error == Some(true),
            serde_json::from_str(text).unwrap(),
        )
    }

    async fn ok(&self, tool: &str, arguments: Value) -> Value {
        let (failed, body) = self.call(tool, arguments).await;
        assert!(!failed, "{tool}: {body}");
        body
    }

    /// The worker's row from `list_workers`, once `ready` holds for it.
    async fn worker_when(&self, name: &str, ready: impl Fn(&Value) -> bool) -> Value {
        let started = Instant::now();
        loop {
            let listed = self
                .ok("list_workers", json!({"include_stopped": true}))
                .await;
            let found = listed["workers"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|worker| worker["name"] == name)
                .cloned();
            if let Some(worker) = found.filter(|worker| ready(worker)) {
                return worker;
            }
            assert!(started.elapsed() < WAIT, "{name} never got there: {listed}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[tokio::test]
async fn a_stub_lead_and_its_task_worker_run_their_scripts_through_the_fold_tools() {
    let host = Host::new().await;
    let main = host.main_agent().await;
    main.ok("create_label", json!({"name": "web", "color": "slate"}))
        .await;
    let task = main
        .ok(
            "create_task",
            json!({"title": "Say hello", "description": "Add the greeting.", "labels": ["web"]}),
        )
        .await;
    let task = task["id"].as_str().unwrap().to_owned();
    main.ok("define_area", json!({"name": "web", "label": "web"}))
        .await;

    host.script(
        "web-lead",
        &json!({"turns": [{
            "steps": [{"call": {
                "tool": "start_worker",
                "arguments": {"role": "task_worker", "name": "web-worker", "tasks": [task]},
            }}],
            "reply": "Started web-worker.",
        }]}),
    );
    host.script(
        "web-worker",
        &json!({"turns": [{
            "steps": [
                {"call": {"tool": "create_workspace", "arguments": {"task": task}, "save": {"ws": "/workspace", "dir": "/path"}}},
                {"write": {"path": "${dir}/hello.txt", "text": "hello\n"}},
                {"call": {"tool": "record_changes", "arguments": {"ws": "${ws}", "message": "Add the greeting"}}},
                {"call": {"tool": "create_proposal", "arguments": {"task": task, "title": "Add the greeting", "evidence": "hello.txt says hello."}, "save": {"proposal": "/id"}}},
            ],
            "reply": "Proposed ${proposal}.",
        }]}),
    );
    main.ok(
        "start_worker",
        json!({"role": "area_lead", "area": "web", "name": "web-lead"}),
    )
    .await;

    // The worker stops once its proposal is open, which is the end of its script.
    main.worker_when("web-worker", |worker| worker["state"] == "stopped")
        .await;
    let lead = main
        .worker_when("web-lead", |worker| worker["state"] == "idle")
        .await;
    assert_eq!(lead["role"], "area_lead");
    assert_eq!(lead["turns"], 1);

    let detail = main.ok("get_task", json!({"id": task})).await;
    let proposal = detail["proposal"]["id"].as_str().unwrap().to_owned();
    let proposal = main.ok("get_proposal", json!({"id": proposal})).await;
    assert_eq!(proposal["author"], "web-worker", "{proposal}");
    assert_eq!(proposal["files"][0]["path"], "hello.txt", "{proposal}");

    let log = std::fs::read_to_string(host.worker_file("web-worker", "stub.log")).unwrap();
    assert!(log.contains("turn 0, step 3: done"), "{log}");
    let state: Value = serde_json::from_str(
        &std::fs::read_to_string(host.worker_file("web-worker", "stub-state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["turn"], 1);
    assert!(state["vars"]["ws"].as_str().unwrap().starts_with("ws-"));
    for agent in ["web-lead", "web-worker"] {
        assert!(
            host.worker_file(agent, "stub.log").is_file(),
            "{agent} ran no stub"
        );
    }
    host.stop().await;
}

#[tokio::test]
async fn a_failed_step_fails_the_stub_worker() {
    let host = Host::new().await;
    let main = host.main_agent().await;
    main.ok("create_label", json!({"name": "web", "color": "slate"}))
        .await;
    main.ok("define_area", json!({"name": "web", "label": "web"}))
        .await;
    host.script(
        "web-lead",
        &json!({"turns": [{"steps": [{"call": {"tool": "get_task", "arguments": {"id": "T-99"}, "until": "/proposal"}}]}]}),
    );
    main.ok(
        "start_worker",
        json!({"role": "area_lead", "area": "web", "name": "web-lead"}),
    )
    .await;
    main.worker_when("web-lead", |worker| worker["state"] == "failed")
        .await;
    let log = std::fs::read_to_string(host.worker_file("web-lead", "stub.log")).unwrap();
    assert!(log.contains("get_task failed: task_not_found"), "{log}");
    host.stop().await;
}
