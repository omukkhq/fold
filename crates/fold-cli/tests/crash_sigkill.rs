#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use fold_cli::daemon_client::{ClientConfig, DaemonClient, Response};
use fold_core::{AgentName, IdKind, ProjectPaths, RevisionId, TaskDescription, TaskId, TaskTitle};
use fold_daemon::SystemClock;
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::events::EventBus;
use fold_daemon::services::reviews::ReviewService;
use fold_daemon::services::tasks::{CreateTask, TaskService};
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::Store;
use fold_store::testing::eventually;
use fold_store::workspaces;
use hyper::Method;
use serde_json::{Value, json};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};
use tokio::process::Child;

use crate::common::{Host, daemons};
use crate::invariants::assert_store_consistent;

const WAIT: Duration = Duration::from_secs(20);
const SLUG: &str = "crash";
const HUMAN: &str = "tanim";

/// `fld init` in a folder `crash` with one file, which becomes `r1`, then, in this process, the main agent `frontend` takes `T-1` on the user's word, adds `src/new.rs` in its workspace and opens `P-1`.
async fn project(host: &Host) -> ProjectPaths {
    let root = host.dir.path().join(SLUG);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn x() {}\n").unwrap();
    let init = host.fold(&["init"], &root).output().await.unwrap();
    assert!(init.status.success(), "{init:?}");
    let paths = ProjectPaths::new(root.canonicalize().unwrap());

    let store = Store::open_project(&paths).await.unwrap();
    let bus = EventBus::new();
    let clock = Arc::new(SystemClock);
    let agents = AgentService::new(store.clone(), bus.clone(), clock.clone());
    let mut ctx = agents.start_session(std::process::id()).await.unwrap();
    agents
        .register_agent(
            &mut ctx,
            &AgentName::parse("frontend").unwrap(),
            &[fold_core::Tag::parse(fold_core::MAIN_TAG).unwrap()],
        )
        .await
        .unwrap();
    let author = agents.actor(&ctx, None).await.unwrap();
    let task = TaskId::from_number(1).unwrap();
    TaskService::new(store.clone(), bus.clone(), clock.clone(), 5)
        .create_task(
            &author,
            CreateTask {
                title: TaskTitle::parse("Add the page").unwrap(),
                description: TaskDescription::new("body"),
                labels: Vec::new(),
            },
        )
        .await
        .unwrap();
    let workspaces =
        WorkspaceService::new(store.clone(), bus.clone(), clock.clone(), paths.clone(), 5);
    workspaces.create_workspace(&author, task).await.unwrap();
    std::fs::write(paths.ws_dir().join("t-1/src/new.rs"), "pub fn z() {}\n").unwrap();
    ReviewService::new(
        store.clone(),
        bus,
        clock,
        workspaces,
        Arc::new(tokio::sync::Mutex::new(())),
    )
    .create_proposal(&author, task, "Add the page", None)
    .await
    .unwrap()
    .detail()
    .unwrap();
    store.close().await.unwrap();
    paths
}

/// `fld serve` on the private socket `socket`, with `failpoints` as `FAILPOINTS` when given, once it accepts connections.
async fn serve(host: &Host, socket: &Path, failpoints: Option<&str>) -> Child {
    let mut command = host.fold(&["serve", "--socket", socket.to_str().unwrap()], &host.root);
    command
        .env_remove("FAILPOINTS")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(failpoints) = failpoints {
        command.env("FAILPOINTS", failpoints);
    }
    let child = command.spawn().unwrap();
    eventually(WAIT, async || {
        tokio::net::UnixStream::connect(socket)
            .await
            .map(drop)
            .map_err(|error| error.kind())
    })
    .await;
    child
}

fn client(socket: &Path) -> DaemonClient {
    DaemonClient::new(
        ClientConfig {
            socket: socket.to_path_buf(),
            runtime_dir: None,
            fold_exe: None,
        },
        Some(HUMAN.to_owned()),
    )
}

async fn integrate(socket: PathBuf) -> Response {
    client(&socket)
        .request(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/proposals/P-1/integrate"),
            Some(&json!({"expected_version": 1})),
        )
        .await
        .unwrap()
}

/// The next integration number the counter would hand out.
async fn next_integration(store: &Store) -> i64 {
    sqlx::query_scalar("SELECT COALESCE((SELECT next FROM counters WHERE kind = ?1), 1)")
        .bind(IdKind::Integration.counter_key())
        .fetch_one(store.read())
        .await
        .unwrap()
}

/// Whether another connection holds the writer lock: `BEGIN IMMEDIATE` without a busy wait fails only then.
async fn writer_busy(db: &Path) -> bool {
    let mut probe = SqliteConnectOptions::new()
        .filename(db)
        .busy_timeout(Duration::ZERO)
        .connect()
        .await
        .unwrap();
    let busy = match sqlx::query("BEGIN IMMEDIATE").execute(&mut probe).await {
        Ok(_) => {
            sqlx::query("ROLLBACK").execute(&mut probe).await.unwrap();
            false
        }
        Err(_) => true,
    };
    probe.close().await.unwrap();
    busy
}

async fn head(store: &Store) -> RevisionId {
    workspaces::head(store.read())
        .await
        .unwrap()
        .unwrap()
        .number
}

async fn count(store: &Store, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(store.read())
        .await
        .unwrap()
}

async fn proposal(socket: &Path) -> Value {
    let answer = client(socket)
        .request(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/proposals/P-1"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(answer.status, 200, "{}", answer.body);
    answer.body
}

/// SIGKILL stops a real `fld serve` while it sleeps inside the integration transaction. The restarted daemon shows main at `r1`, the proposal open and the task in Review, the project passes every invariant, and integrating again succeeds.
#[tokio::test]
async fn a_daemon_killed_inside_the_integration_transaction_shows_none_of_it() {
    let host = Host::new().await;
    let paths = project(&host).await;
    let socket = host.dir.path().join("crash.sock");
    let mut daemon = serve(
        &host,
        &socket,
        Some("integration_in_transaction=sleep(600000)"),
    )
    .await;
    let store = Store::open_project(&paths).await.unwrap();
    let before = next_integration(&store).await;

    let request = tokio::spawn(integrate(socket.clone()));
    // The number is allocated in its own write just before the integration transaction opens with `BEGIN IMMEDIATE`, and nothing else writes while the request waits.
    eventually(WAIT, async || {
        let allocated = next_integration(&store).await > before;
        if allocated && writer_busy(&paths.db()).await {
            Ok(())
        } else {
            Err(allocated)
        }
    })
    .await;
    daemon.start_kill().unwrap();
    daemon.wait().await.unwrap();
    request.abort();

    let mut daemon = serve(&host, &socket, None).await;
    assert_eq!(head(&store).await, RevisionId::from_number(1).unwrap());
    // The genesis `r0` and `r1`, the folder's files that init recorded through its own `record` integration.
    assert_eq!(count(&store, "SELECT COUNT(*) FROM revisions").await, 2);
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM integrations WHERE kind <> 'record'"
        )
        .await,
        0
    );
    assert_eq!(count(&store, "SELECT COUNT(*) FROM completions").await, 0);
    let detail = proposal(&socket).await;
    assert_eq!(detail["summary"]["state"], "open");
    assert_eq!(detail["task_status"], "review");
    assert_store_consistent(&store, &paths).await;

    let answer = integrate(socket.clone()).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert_eq!(
        answer.body,
        json!({"proposal": "P-1", "result": "succeeded", "revision": "r2", "run": null})
    );
    assert_eq!(
        count(
            &store,
            "SELECT number FROM integrations WHERE kind = 'proposal' AND result = 'succeeded'"
        )
        .await,
        3
    );
    assert_store_consistent(&store, &paths).await;
    store.close().await.unwrap();
    daemon.start_kill().unwrap();
    daemon.wait().await.unwrap();
    assert!(daemons(&socket).is_empty(), "a fld serve outlived the test");
}
