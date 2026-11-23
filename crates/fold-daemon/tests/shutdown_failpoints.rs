#![cfg(test)]

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fold_core::{Clock, ManualClock};
use fold_daemon::jobs::reconcile_workspaces;
use fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME;
use fold_daemon::services::workspaces::{Reconciled, WorkspaceService};
use fold_daemon::{Daemon, DaemonConfig, RunningDaemon};
use fold_store::projects::{self, ProjectRow};
use fold_store::testing::{TestProject, eventually};
use fold_store::{Store, StoreError, workspaces};
use tempfile::TempDir;

use crate::daemon_support::{Opened, agent};

const SLUG: &str = "demo";

/// A held step is let go after this limit, which the failpoint `shutdown_limit_ms` gives every step.
const LIMIT_MS: u64 = 300;

/// Longer than every step at [`LIMIT_MS`] together with the jobs' and drains' own limits, and far shorter than the 45 s of the real limits.
const STOP_BOUND: Duration = Duration::from_secs(20);

const WAIT: Duration = Duration::from_secs(10);

/// Every log line of fold's own crates.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl Logs {
    fn install() -> Self {
        use tracing_subscriber::layer::SubscriberExt as _;
        let logs = Self::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(move || writer.clone()),
            )
            .with(
                tracing_subscriber::filter::Targets::new()
                    .with_target("fold_daemon", tracing::Level::INFO),
            );
        // nextest runs each test in its own process, so the global subscriber is this test's.
        tracing::subscriber::set_global_default(subscriber).unwrap();
        logs
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl io::Write for Logs {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Host {
    _dir: TempDir,
    config: DaemonConfig,
}

impl Host {
    async fn with_project(root: &Path, clock: &ManualClock) -> Self {
        let dir = TempDir::new().unwrap();
        let config = DaemonConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            socket: dir.path().join("run").join("fold.sock"),
            data_dir: dir.path().join("data"),
            runtime_dir: dir.path().join("run"),
            web_dir: None,
            session_lifetime: DEFAULT_SESSION_LIFETIME,
        };
        let registry = Store::open_registry(&config.registry_path()).await.unwrap();
        let row = ProjectRow {
            slug: SLUG.to_owned(),
            name: "Demo".to_owned(),
            root: root.to_string_lossy().into_owned(),
        };
        let now = clock.now();
        registry
            .write(async |tx| {
                projects::insert_project(tx, &row, now).await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
        registry.close().await.unwrap();
        Self { _dir: dir, config }
    }

    async fn start(&self, clock: &ManualClock) -> RunningDaemon {
        Daemon::start(self.config.clone(), Arc::new(clock.clone()))
            .await
            .unwrap()
    }
}

/// Holds `step` of the stop, which never ends, and checks that the daemon lets it go after its limit, logs the step with the project's slug when it belongs to one, and still logs `daemon stopped`.
async fn a_held_step_is_let_go(step: &str, of_project: bool) {
    let logs = Logs::install();
    let scenario = fail::FailScenario::setup();
    let project = TestProject::new().await;
    let host = Host::with_project(project.paths().root(), project.clock()).await;
    let daemon = host.start(project.clock()).await;
    fail::cfg("shutdown_limit_ms", &format!("return({LIMIT_MS})")).unwrap();
    fail::cfg("shutdown_hold", &format!("return({step})")).unwrap();
    let started = Instant::now();
    tokio::time::timeout(STOP_BOUND, daemon.shutdown())
        .await
        .expect("the daemon stopped within the bound");
    let took = started.elapsed();
    scenario.teardown();
    assert!(
        took >= Duration::from_millis(LIMIT_MS),
        "the stop waited for the held step: {took:?}"
    );
    let text = logs.text();
    let over = text
        .lines()
        .find(|line| line.contains("went over its time limit") && line.contains(step))
        .unwrap_or_else(|| panic!("no warning names {step}: {text}"));
    assert!(over.contains("WARN"), "{over}");
    assert!(over.contains(&format!("step=\"{step}\"")), "{over}");
    assert!(over.contains(&format!("limit_ms={LIMIT_MS}")), "{over}");
    if of_project {
        assert!(over.contains(&format!("project=\"{SLUG}\"")), "{over}");
    }
    assert!(text.contains("daemon stopped"), "{text}");
}

#[tokio::test]
async fn a_worker_stop_that_does_not_end_is_let_go() {
    a_held_step_is_let_go("worker_stops", true).await;
}

#[tokio::test]
async fn a_socket_drain_that_does_not_return_is_let_go() {
    a_held_step_is_let_go("socket", false).await;
}

#[tokio::test]
async fn a_check_run_that_does_not_end_is_let_go() {
    a_held_step_is_let_go("checks", true).await;
}

#[tokio::test]
async fn a_live_detach_that_blocks_is_let_go() {
    a_held_step_is_let_go("live_detach", true).await;
}

#[tokio::test]
async fn an_end_of_sessions_that_blocks_is_let_go() {
    a_held_step_is_let_go("close_sessions", true).await;
}

#[tokio::test]
async fn a_database_close_that_blocks_is_let_go() {
    a_held_step_is_let_go("store_close", true).await;
}

#[tokio::test]
async fn a_registry_close_that_blocks_is_let_go() {
    a_held_step_is_let_go("registry_close", false).await;
}

async fn base_of(store: &Store, id: fold_core::WorkspaceId) -> fold_core::RevisionId {
    workspaces::get_workspace(store.read(), id)
        .await
        .unwrap()
        .unwrap()
        .base
}

/// A stop that comes while a sync has written the merged files and not yet committed its rows. The stop does not wait for the sync; the ended process takes the sync with it. The next start shows none of the sync in the rows: the base is the old one and the sync's checkpoint marks the unfinished write, and the live state is attached with it. The agent's next sync then does all of it.
#[tokio::test]
async fn a_stop_during_a_sync_leaves_all_or_none_of_it_at_the_next_start() {
    let scenario = fail::FailScenario::setup();
    let fx = Opened::new().await;
    let (id, dir) = fx.create(1).await;
    let readme = dir.join("README.md");
    let lib = dir.join("src/lib.rs");
    std::fs::write(&readme, "# mine\n").unwrap();
    let main = fx
        .move_main(&[("src/lib.rs", b"pub fn theirs() {}\n")])
        .await;
    let old_base = base_of(&fx.handle.store, id).await;
    for live in fx.handle.live.all() {
        fx.handle.live.detach(live.view.workspace).await;
    }
    fx.handle.store.close().await.unwrap();
    let clock = fx.project.clock().clone();
    let host = Host::with_project(fx.project.paths().root(), &clock).await;

    let daemon = host.start(&clock).await;
    let handle = Arc::clone(&daemon.state().projects.handles()[0]);
    fail::cfg("workspace_sync_hold", "return(after_write)").unwrap();
    let sync = tokio::spawn({
        let service = WorkspaceService::for_project(&handle);
        async move { service.sync_workspace(&agent("frontend"), id).await }
    });
    eventually(WAIT, async || {
        let text = std::fs::read_to_string(&lib).unwrap_or_default();
        if text == "pub fn theirs() {}\n" {
            Ok(())
        } else {
            Err(text)
        }
    })
    .await;
    let mark = workspaces::sync_checkpoint(handle.store.read(), id)
        .await
        .unwrap()
        .expect("the sync marked its checkpoint before it wrote");
    drop(handle);
    tokio::time::timeout(STOP_BOUND, daemon.shutdown())
        .await
        .expect("the stop does not wait for the held sync");
    // The process ends here, and the held sync with it.
    sync.abort();
    assert!(sync.await.unwrap_err().is_cancelled());
    fail::remove("workspace_sync_hold");

    let daemon = host.start(&clock).await;
    let handle = Arc::clone(&daemon.state().projects.handles()[0]);
    let service = WorkspaceService::for_project(&handle);
    assert_eq!(
        base_of(&handle.store, id).await,
        old_base,
        "the rows hold none of the sync"
    );
    assert_eq!(
        workspaces::sync_checkpoint(handle.store.read(), id)
            .await
            .unwrap(),
        Some(mark)
    );
    let done = reconcile_workspaces(&handle).await;
    let this = done.iter().find(|done| done.workspace == id).unwrap();
    assert_eq!(this.outcome, Reconciled::Attached);
    assert_eq!(this.unfinished, Some(mark));
    let live = handle.live.get(id).expect("the workspace is live");
    assert_eq!(live.view.base, old_base);

    let synced = service
        .sync_workspace(&agent("frontend"), id)
        .await
        .unwrap();
    assert_eq!(synced.base, main);
    assert!(synced.conflicted.is_empty(), "{:?}", synced.conflicted);
    assert_eq!(std::fs::read_to_string(&readme).unwrap(), "# mine\n");
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        "pub fn theirs() {}\n"
    );
    assert_eq!(
        workspaces::sync_checkpoint(handle.store.read(), id)
            .await
            .unwrap(),
        None
    );
    let live = eventually(WAIT, async || match handle.live.get(id) {
        Some(live) if live.view.base == main => Ok(live),
        other => Err(other.map(|live| live.view.base)),
    })
    .await;
    assert!(
        live.view.files.iter().any(|file| file.path == "README.md"),
        "{:?}",
        live.view.files
    );
    drop(handle);
    daemon.shutdown().await;
    scenario.teardown();
}

/// A stand-in for Claude Code that ignores its closed input, SIGINT and SIGTERM, and starts a command of its own in its process group, as a tool call would.
const STUBBORN: &str = r#"#!/bin/sh
trap '' INT TERM
sleep 600 &
printf '%s\n' "$$" > "$FOLD_TEST_DIR/pid.$$.tmp" && mv "$FOLD_TEST_DIR/pid.$$.tmp" "$FOLD_TEST_DIR/pid.$$"
while :; do sleep 1; done
"#;

/// The processes of group `group` that have not ended; a zombie has ended and only waits to be reaped.
fn alive_in_group(group: u32) -> Vec<u32> {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| {
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                return false;
            };
            let Some((_, rest)) = stat.rsplit_once(')') else {
                return false;
            };
            let fields: Vec<&str> = rest.split_whitespace().collect();
            fields.first().is_some_and(|state| *state != "Z")
                && fields.get(2).and_then(|pgrp| pgrp.parse::<u32>().ok()) == Some(group)
        })
        .collect()
}

/// A worker that never ends, as its stop steps take longer than the limit: the stop lets it go after the limit, sends SIGKILL to its process group, so the command it started ends too, and still logs `daemon stopped`.
#[tokio::test]
async fn a_worker_process_that_does_not_end_is_killed_with_its_group() {
    use std::os::unix::fs::PermissionsExt;

    use fold_core::WorkerState;
    use fold_daemon::SystemClock;
    use fold_daemon::services::agents::{Launcher, StopTimeouts};

    use crate::workers_support::FakeClaude;

    let logs = Logs::install();
    let scenario = fail::FailScenario::setup();
    let fake = FakeClaude::new("init");
    let mut harness =
        crate::worker_resume::task_worker_for(&fake, &["Draw the page"], WorkerState::Working)
            .await;
    harness.stop().await;
    let host = Host::with_project(Path::new(harness.root()), harness.project.clock()).await;
    let dir = TempDir::new().unwrap();
    let marks = dir.path().join("marks");
    std::fs::create_dir_all(&marks).unwrap();
    let program = dir.path().join("claude");
    std::fs::write(&program, STUBBORN).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let launcher = Launcher {
        claude: program,
        fold: fake.launcher.fold.clone(),
        env: vec![(
            "FOLD_TEST_DIR".to_owned(),
            marks.to_string_lossy().into_owned(),
        )],
        ..Launcher::default()
    };
    let daemon = Daemon::start_with(host.config.clone(), Arc::new(SystemClock), launcher)
        .await
        .unwrap();
    let groups: Vec<u32> = eventually(WAIT, async || {
        let pids: Vec<u32> = std::fs::read_dir(&marks)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("pid.") && !name.ends_with(".tmp")
            })
            .filter_map(|entry| {
                std::fs::read_to_string(entry.path())
                    .ok()?
                    .trim()
                    .parse()
                    .ok()
            })
            .collect();
        if pids.is_empty() { Err(0) } else { Ok(pids) }
    })
    .await;
    for handle in daemon.state().projects.handles() {
        handle.workers.set_stop_timeouts(StopTimeouts {
            interrupt: Duration::from_secs(600),
            sigint: Duration::from_secs(600),
            sigterm: Duration::from_secs(600),
        });
    }
    for group in &groups {
        assert!(
            alive_in_group(*group).len() >= 2,
            "the worker and its command run"
        );
    }
    fail::cfg("shutdown_limit_ms", &format!("return({LIMIT_MS})")).unwrap();
    tokio::time::timeout(STOP_BOUND, daemon.shutdown())
        .await
        .expect("the daemon stopped within the bound");
    scenario.teardown();
    for group in &groups {
        eventually(WAIT, async || {
            let alive = alive_in_group(*group);
            if alive.is_empty() { Ok(()) } else { Err(alive) }
        })
        .await;
    }
    let text = logs.text();
    assert!(
        text.lines()
            .any(|line| line.contains("went over its time limit")
                && line.contains("step=\"worker_stops\"")
                && line.contains(&format!("project=\"{SLUG}\""))),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|line| line.contains("sent SIGKILL")
                && line.contains(&format!("project=\"{SLUG}\""))),
        "{text}"
    );
    assert!(text.contains("daemon stopped"), "{text}");
}
