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
        let root = init_project(&host, &root, None, &crate::common::ana(), &clock)
            .await
            .unwrap()
            .root;
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
        crate::common::roles::lead_work_areas(&handle.store).await;
        let task = handle
            .tasks
            .create_task(
                &frontend(),
                CreateTask {
                    title: TaskTitle::parse("task").unwrap(),
                    description: TaskDescription::new("body"),
                    labels: crate::common::roles::work_labels(),
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

    /// Deletes what `fld init` made in the folder: `.fold/`.
    fn delete_fold_data(&self) {
        std::fs::remove_dir_all(self.fold_dir()).unwrap();
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

impl Running {
    async fn registered(&self) -> bool {
        fold_store::projects::find_project(self.state.registry.read(), "app")
            .await
            .unwrap()
            .is_some()
    }

    /// The project is out of fold: its row is gone, its handle closed, its slot empty, and nothing was written into the deleted `.fold/`.
    async fn assert_removed(&self, handle: &ProjectHandle) {
        assert!(!self.registered().await, "the registry row went");
        assert!(closed(handle));
        assert!(self.state.projects.slot("app").is_none());
        assert!(!self.fold_dir().exists());
    }
}

#[tokio::test]
async fn a_request_removes_a_project_whose_fold_was_deleted_and_its_handle_writes_nothing_there() {
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
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "project_not_found");
    assert!(
        body["error"]["details"]["hint"]
            .as_str()
            .unwrap()
            .contains("run `fld init` in")
    );
    running.assert_removed(&handle).await;
    let (status, body) = running.get("").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "project_not_found");
    assert_eq!(
        entries(&running.root),
        before,
        "the close writes nothing there"
    );
    // A session that still holds the closed project learns that it is unavailable, not an internal error.
    let refused: fold_core::FoldError = handle
        .tasks
        .create_task(
            &frontend(),
            CreateTask {
                title: TaskTitle::parse("late").unwrap(),
                description: TaskDescription::new("body"),
                labels: Vec::new(),
            },
        )
        .await
        .unwrap_err()
        .into();
    assert_eq!(refused.code(), "project_unavailable");

    let project = init_project(
        &running.host,
        &running.root,
        None,
        &crate::common::ana(),
        &running.clock,
    )
    .await
    .unwrap();
    assert!(!project.replaced, "the row was gone");
    assert_eq!(project.slug, "app");
    let new = project_service::open(&running.state, "app").await.unwrap();
    assert!(!Arc::ptr_eq(&handle, &new));
    assert_eq!(new.database_file().await, DatabaseFile::Opened);
    running.close().await;
}

/// The places that look for a deleted `.fold/` other than a request, each on a fresh daemon state.
#[derive(Debug, Clone, Copy)]
enum CheckPoint {
    List,
    Open,
    OpenForDir,
    Sweep,
    UnavailableRequest,
}

#[tokio::test]
async fn every_check_point_removes_a_project_whose_fold_was_deleted() {
    for point in [
        CheckPoint::List,
        CheckPoint::Open,
        CheckPoint::OpenForDir,
        CheckPoint::Sweep,
        CheckPoint::UnavailableRequest,
    ] {
        let running = Running::start().await;
        let handle = running.handle.clone();
        running.delete_fold_data();
        match point {
            CheckPoint::List => {
                assert!(
                    project_service::list(&running.state)
                        .await
                        .unwrap()
                        .is_empty()
                );
            }
            CheckPoint::Open => {
                let error = project_service::open(&running.state, "app")
                    .await
                    .unwrap_err();
                assert_eq!(error.code(), Some("project_not_found"), "{point:?}");
            }
            CheckPoint::OpenForDir => {
                let error = project_service::open_for_dir(&running.state, &running.root)
                    .await
                    .unwrap_err();
                assert_eq!(error.code(), Some("project_not_found"), "{point:?}");
            }
            CheckPoint::Sweep => {
                assert_eq!(
                    project_service::sweep_deleted(&running.state)
                        .await
                        .unwrap(),
                    1
                );
                // The first answer after the live job removed the project names the way back, the next one is a plain not-found.
                let (status, body) = running.get("").await;
                assert_eq!(status, StatusCode::NOT_FOUND);
                assert!(
                    body["error"]["details"]["hint"]
                        .as_str()
                        .unwrap()
                        .contains("run `fld init` in")
                );
                let (_, body) = running.get("").await;
                assert_eq!(body["error"]["code"], "project_not_found");
                assert!(body["error"]["details"]["hint"].is_null(), "{body}");
            }
            CheckPoint::UnavailableRequest => {
                // A replaced database leaves the project unavailable; once that file goes too, the next request removes it.
                std::fs::create_dir_all(running.fold_dir()).unwrap();
                std::fs::write(running.fold_dir().join("fold.db"), b"").unwrap();
                let (status, _) = running.get("").await;
                assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
                assert!(closed(&handle) && running.registered().await);
                assert!(matches!(
                    running.state.projects.slot("app"),
                    Some(Slot::Unavailable)
                ));
                std::fs::remove_dir_all(running.fold_dir()).unwrap();
                let (status, _) = running.get("").await;
                assert_eq!(status, StatusCode::NOT_FOUND);
            }
        }
        running.assert_removed(&handle).await;
        running.close().await;
    }
}

#[tokio::test]
async fn a_project_that_keeps_its_fold_is_never_removed() {
    let running = Running::start().await;
    let handle = running.handle.clone();
    assert_eq!(
        project_service::sweep_deleted(&running.state)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        project_service::list(&running.state).await.unwrap().len(),
        1
    );
    assert_eq!(running.get("").await.0, StatusCode::OK);
    std::fs::remove_file(running.root.join(".foldignore")).unwrap();
    assert_eq!(
        project_service::sweep_deleted(&running.state)
            .await
            .unwrap(),
        0
    );
    assert!(!closed(&handle) && running.registered().await);
    assert!(matches!(
        running.state.projects.slot("app"),
        Some(Slot::Open(_))
    ));
    running.close().await;
}

#[tokio::test]
async fn a_fold_dir_that_cannot_be_read_is_never_removed() {
    use std::os::unix::fs::PermissionsExt;
    let running = Running::start().await;
    let handle = running.handle.clone();
    let mode = |bits| std::fs::Permissions::from_mode(bits);
    std::fs::set_permissions(running.fold_dir(), mode(0o000)).unwrap();
    // A root user reads the directory anyway, and the check has nothing to prove then.
    let denied = std::fs::metadata(running.fold_dir().join("fold.db")).is_err();
    let swept = project_service::sweep_deleted(&running.state).await;
    let listed = project_service::list(&running.state).await;
    let (status, _) = running.get("").await;
    std::fs::set_permissions(running.fold_dir(), mode(0o755)).unwrap();
    if denied {
        assert_eq!(swept.unwrap(), 0);
        assert_eq!(listed.unwrap().len(), 1);
        assert_ne!(status, StatusCode::NOT_FOUND);
        assert!(!closed(&handle) && running.registered().await);
    }
    running.close().await;
}

#[tokio::test]
async fn without_a_daemon_the_host_registry_loses_only_the_projects_whose_fold_was_deleted() {
    let dir = TempDir::new().unwrap();
    let host = HostPaths::new(dir.path().join("data").to_str(), None).unwrap();
    assert!(
        project_service::remove_deleted_on_host(&host)
            .await
            .unwrap()
            .is_empty()
    );
    let clock = ManualClock::new();
    for name in ["kept", "gone", "away", "nodb"] {
        let root = dir.path().join(name);
        std::fs::create_dir_all(&root).unwrap();
        init_project(&host, &root, None, &crate::common::ana(), &clock)
            .await
            .unwrap();
    }
    std::fs::remove_dir_all(dir.path().join("gone/.fold")).unwrap();
    std::fs::rename(dir.path().join("away"), dir.path().join("away.moved")).unwrap();
    std::fs::remove_file(dir.path().join("nodb/.fold/fold.db")).unwrap();
    assert_eq!(
        project_service::remove_deleted_on_host(&host)
            .await
            .unwrap(),
        ["gone"]
    );
    let registry = Store::open_registry(&host.registry_db()).await.unwrap();
    let slugs: Vec<String> = fold_store::projects::list_projects(registry.read())
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.slug)
        .collect();
    registry.close().await.unwrap();
    assert_eq!(slugs, ["away", "kept", "nodb"]);
}

async fn stop_handles(state: &AppState) {
    for handle in state.projects.handles() {
        handle.bus.close();
        handle.stop_tasks(Duration::from_secs(5)).await;
        handle.store.close().await.unwrap();
    }
}

#[tokio::test]
async fn a_missing_folder_keeps_the_project_and_it_opens_again_when_the_folder_is_back() {
    let running = Running::start().await;
    let handle = running.handle.clone();
    let away = running.root.with_extension("away");
    std::fs::rename(&running.root, &away).unwrap();

    assert_eq!(
        project_service::sweep_deleted(&running.state)
            .await
            .unwrap(),
        1
    );
    assert!(closed(&handle) && running.registered().await);
    assert!(running.state.projects.is_away("app"));
    let (status, body) = running.get("").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "project_unavailable");
    assert_eq!(body["error"]["details"]["reason"], "folder_missing");
    let listed = project_service::list(&running.state).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].available);
    assert_eq!(
        project_service::sweep_deleted(&running.state)
            .await
            .unwrap(),
        0,
        "nothing changes while the folder is missing"
    );
    assert!(!running.root.exists(), "nothing makes the folder again");

    std::fs::rename(&away, &running.root).unwrap();
    assert_eq!(
        project_service::sweep_deleted(&running.state)
            .await
            .unwrap(),
        1
    );
    let Some(Slot::Open(again)) = running.state.projects.slot("app") else {
        panic!("the project opened again");
    };
    assert!(!Arc::ptr_eq(&handle, &again));
    assert!(!running.state.projects.is_away("app"));
    assert_eq!(running.get("").await.0, StatusCode::OK);
    running.close().await;
}

#[tokio::test]
async fn a_folder_missing_at_the_start_opens_once_it_is_back() {
    let running = Running::start().await;
    stop_handles(&running.state).await;
    let registry = running.state.registry.clone();
    let state: AppState = Arc::new(DaemonState {
        projects: Projects::default().with_registry(registry.clone()),
        registry: registry.clone(),
        clock: Arc::new(running.clock.clone()),
        assets: AssetSource::Embedded,
        listen: "127.0.0.1".parse().unwrap(),
        session_lifetime: DEFAULT_SESSION_LIFETIME,
    });
    let row = fold_store::projects::find_project(registry.read(), "app")
        .await
        .unwrap()
        .unwrap();
    let away = running.root.with_extension("away");
    std::fs::rename(&running.root, &away).unwrap();

    let error = state
        .projects
        .open_at_start(&row, state.clock.clone())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "project_unavailable");
    assert!(error.hint().unwrap().contains("mount the disk"));
    assert!(state.projects.is_away("app"));
    assert!(running.registered().await);

    std::fs::rename(&away, &running.root).unwrap();
    assert_eq!(project_service::sweep_deleted(&state).await.unwrap(), 1);
    assert!(matches!(state.projects.slot("app"), Some(Slot::Open(_))));
    stop_handles(&state).await;
    registry.close().await.unwrap();
}

#[tokio::test]
async fn a_fold_dir_without_its_database_keeps_the_project_and_says_how_to_get_it_back() {
    let running = Running::start().await;
    let handle = running.handle.clone();
    for suffix in ["", "-wal", "-shm"] {
        let mut name = running.fold_dir().join("fold.db").into_os_string();
        name.push(suffix);
        let _ = std::fs::remove_file(name);
    }

    assert_eq!(
        project_service::sweep_deleted(&running.state)
            .await
            .unwrap(),
        1
    );
    assert!(closed(&handle) && running.registered().await);
    assert!(!running.state.projects.is_away("app"));
    let (status, body) = running.get("").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["details"]["reason"], "database_missing");
    let hint = body["error"]["details"]["hint"].as_str().unwrap();
    assert!(
        hint.starts_with("restore ") && hint.contains("and run `fld init` again"),
        "{hint}"
    );
    assert_eq!(
        project_service::sweep_deleted(&running.state)
            .await
            .unwrap(),
        0
    );
    assert!(running.registered().await);

    let refused = init_project(
        &running.host,
        &running.root,
        None,
        &crate::common::ana(),
        &running.clock,
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code(), Some("project_incomplete"));
    running.close().await;
}
