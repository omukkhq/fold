#![cfg(test)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fold_core::{
    Actor, AgentActor, AgentName, HostPaths, ManualClock, TaskDescription, TaskTitle, WorkerName,
    WorkspaceId,
};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::router;
use fold_daemon::jobs::QuietCheckpoints;
use fold_daemon::projects::{DatabaseFile, ProjectHandle, Projects, Slot};
use fold_daemon::services::projects::{
    self as project_service, DEFAULT_SESSION_LIFETIME, init_project,
};
use fold_daemon::services::tasks::CreateTask;
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::Store;
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

/// A host whose registry holds the project `app`, open in a daemon state with agent `frontend`, task `T-1` and its live workspace.
struct Running {
    _dir: TempDir,
    host: HostPaths,
    clock: ManualClock,
    root: PathBuf,
    state: AppState,
    handle: Arc<ProjectHandle>,
    workspace: WorkspaceId,
}

fn frontend() -> Actor {
    let worker = WorkerName::parse("frontend").unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

impl Running {
    async fn start() -> Self {
        let dir = TempDir::new().unwrap();
        let host = HostPaths::new(dir.path().join("data").to_str(), None).unwrap();
        let clock = ManualClock::new();
        let root = dir.path().join("app");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        let root = init_project(&host, &root, None, &clock).await.unwrap().root;
        let registry = Store::open_registry(&host.registry_db()).await.unwrap();
        let state: AppState = Arc::new(DaemonState {
            projects: Projects::default().with_registry(registry.clone()),
            registry,
            clock: Arc::new(clock.clone()),
            assets: AssetSource::Embedded,
            listen: "127.0.0.1".parse().unwrap(),
            session_lifetime: DEFAULT_SESSION_LIFETIME,
        });
        let handle = project_service::open(&state, "app").await.unwrap();
        let agents = handle.agents();
        let mut session = agents.start_session(1).await.unwrap();
        agents
            .register_agent(&mut session, &AgentName::parse("frontend").unwrap(), &[])
            .await
            .unwrap();
        let workspace = Self::claim(&handle).await.unwrap();
        Self {
            _dir: dir,
            host,
            clock,
            root,
            state,
            handle,
            workspace,
        }
    }

    /// Creates a task and the workspace of `frontend` for it.
    async fn claim(handle: &ProjectHandle) -> Result<WorkspaceId, String> {
        let task = handle
            .tasks
            .create_task(
                &frontend(),
                CreateTask {
                    title: TaskTitle::parse("task").unwrap(),
                    description: TaskDescription::new("body"),
                    labels: Vec::new(),
                },
            )
            .await
            .map_err(|error| format!("{error:?}"))?;
        let created = WorkspaceService::for_project(handle)
            .create_workspace(&frontend(), task.id)
            .await
            .map_err(|error| format!("{error:?}"))?;
        Ok(created.workspace.id)
    }

    fn fold_dir(&self) -> PathBuf {
        self.root.join(".fold")
    }

    /// Deletes what `fld init` made in the folder: `.fold/` and the repository.
    fn delete_fold_data(&self) {
        std::fs::remove_dir_all(self.fold_dir()).unwrap();
        std::fs::remove_dir_all(self.root.join(".git")).unwrap();
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .uri(format!("/api/v1/projects/app{path}"))
            .body(Body::empty())
            .unwrap();
        let response = router(self.state.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn close(self) {
        for handle in self.state.projects.handles() {
            handle.bus.close();
            handle.stop_tasks(Duration::from_secs(5)).await;
            handle.store.close().await.unwrap();
        }
        self.state.registry.close().await.unwrap();
    }
}

fn closed(handle: &ProjectHandle) -> bool {
    *handle.bus.closing().borrow() && handle.store.read().is_closed()
}

/// Every file and directory below `dir`.
fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(at) = pending.pop() {
        let Ok(read) = std::fs::read_dir(&at) else {
            continue;
        };
        for entry in read {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path.clone());
            }
            found.push(path);
        }
    }
    found.sort();
    found
}

#[tokio::test]
async fn a_request_closes_a_project_whose_fold_was_deleted_and_its_handle_writes_nothing_there() {
    let running = Running::start().await;
    let handle = running.handle.clone();
    let before: Vec<PathBuf> = entries(&running.root)
        .into_iter()
        .filter(|path| {
            !path.starts_with(running.fold_dir()) && !path.starts_with(running.root.join(".git"))
        })
        .collect();
    running.delete_fold_data();
    assert_eq!(handle.database_file().await, DatabaseFile::Missing);

    // Until the daemon closes it, every writer of the handle leaves the deleted `.fold/` deleted.
    let workspaces = WorkspaceService::for_project(&handle);
    assert!(
        workspaces
            .record_checkpoint(&frontend(), running.workspace, Some("kept"))
            .await
            .is_err()
    );
    assert!(Running::claim(&handle).await.is_err());
    assert_eq!(QuietCheckpoints::default().run(&handle).await, 0);
    assert!(project_service::gc(&handle, false).await.is_err());
    running.clock.advance(Duration::from_secs(600)).unwrap();
    handle.live.tick();
    handle.live.detach(running.workspace).await;
    assert!(!running.fold_dir().exists());

    let (status, body) = running.get("/tasks").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "project_unavailable");
    assert!(closed(&handle));
    assert!(matches!(
        running.state.projects.slot("app"),
        Some(Slot::Unavailable)
    ));
    assert_eq!(running.state.projects.close_stale().await, 0);
    assert!(
        !running.fold_dir().exists(),
        "the close writes nothing there"
    );
    assert_eq!(entries(&running.root), before);

    let project = init_project(&running.host, &running.root, None, &running.clock)
        .await
        .unwrap();
    assert!(project.replaced);
    let new = project_service::open(&running.state, "app").await.unwrap();
    assert!(!Arc::ptr_eq(&handle, &new));
    assert_eq!(new.database_file().await, DatabaseFile::Opened);
    running.close().await;
}

#[tokio::test]
async fn the_live_job_sweep_closes_only_a_project_whose_database_is_gone() {
    let running = Running::start().await;
    let handle = running.handle.clone();
    assert_eq!(running.state.projects.close_stale().await, 0);
    assert!(!closed(&handle));
    assert!(matches!(
        running.state.projects.slot("app"),
        Some(Slot::Open(_))
    ));

    running.delete_fold_data();
    assert_eq!(running.state.projects.close_stale().await, 1);
    assert!(closed(&handle));
    assert!(running.state.projects.handles().is_empty());
    assert!(!running.fold_dir().exists());
    running.close().await;
}
