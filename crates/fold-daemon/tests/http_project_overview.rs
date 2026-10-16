use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::AgentName;
use fold_core::tree::FlatTree;
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::router;
use fold_daemon::projects::Projects;
use fold_daemon::services::agents::AgentService;
use fold_store::StoreError;
use fold_store::projects::{self as registry, ProjectRow};
use fold_store::testing::TestProject;
use fold_store::workspaces::{create_initial_revision, initial_revision};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

#[cfg(test)]
async fn call(
    state: &AppState,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(method)
        .uri(path)
        .header("cookie", "fold_human=ana");
    let body = match body {
        Some(body) => {
            request = request.header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = router(state.clone())
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[cfg(test)]
fn state_of(project: &TestProject) -> AppState {
    Arc::new(DaemonState {
        registry: project.registry().clone(),
        projects: Projects::default(),
        clock: Arc::new(project.clock().clone()),
        assets: AssetSource::Embedded,
        listen: "127.0.0.1".parse().unwrap(),
        session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
    })
}

#[cfg(test)]
fn row(slug: &str, project: &TestProject) -> ProjectRow {
    ProjectRow {
        slug: slug.to_owned(),
        name: format!("Project {slug}"),
        root: project.paths().root().to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
async fn register(project: &TestProject, slug: &str) -> ProjectRow {
    let row = row(slug, project);
    let inserted = row.clone();
    let now = project.now();
    project
        .registry()
        .write(async move |tx| {
            registry::insert_project(tx, &inserted, now).await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    row
}

#[tokio::test]
#[cfg(test)]
async fn an_open_project_lists_its_counts_agents_head_and_last_change() {
    let project = TestProject::new().await;
    let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
    let tree = write_initial_tree(&objects, FlatTree::default()).unwrap();
    let stored = write_revision(&objects, &initial_revision(tree, project.now()).unwrap()).unwrap();
    create_initial_revision(project.store(), &stored, None)
        .await
        .unwrap();
    let state = state_of(&project);
    let row = register(&project, "demo").await;
    let handle = state
        .projects
        .open(&row, state.clock.clone())
        .await
        .unwrap();
    for title in ["One", "Two"] {
        let (status, body) = call(
            &state,
            Method::POST,
            "/api/v1/projects/demo/tasks",
            Some(json!({"title": title})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let agents = AgentService::new(
        handle.store.clone(),
        handle.bus.clone(),
        handle.clock.clone(),
    );
    let mut ctx = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut ctx, &AgentName::parse("review").unwrap(), &[])
        .await
        .unwrap();

    let (status, body) = call(&state, Method::GET, "/api/v1/projects", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let overview = &body["projects"][0]["overview"];
    assert_eq!(body["projects"][0]["available"], true);
    assert_eq!(
        overview["counts"],
        json!({"backlog": 0, "todo": 2, "in_progress": 0, "review": 0, "done": 0})
    );
    assert_eq!(overview["agents"][0]["name"], "review");
    assert!(overview["agents"][0]["presence"].is_string());
    assert_eq!(overview["head"], "r1");
    assert_eq!(
        overview["last_change_at"].as_i64(),
        Some(project.now().as_millis())
    );

    let (status, shown) = call(&state, Method::GET, "/api/v1/projects/demo", None).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["overview"], *overview);
}

#[tokio::test]
#[cfg(test)]
async fn an_unavailable_project_lists_only_its_slug_name_and_availability() {
    let project = TestProject::new().await;
    register(&project, "late").await;
    let state = state_of(&project);
    let (status, body) = call(&state, Method::GET, "/api/v1/projects", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({"projects": [
            {"slug": "late", "name": "Project late", "available": false, "overview": null}
        ]})
    );
}

#[tokio::test]
#[cfg(test)]
async fn the_git_route_reports_the_repository_the_remotes_and_the_last_push_of_each() {
    use fold_core::RevisionId;
    use fold_store::workspaces::{insert_revision_commits, record_push, record_push_failure};

    let project = TestProject::new().await;
    let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
    let tree = write_initial_tree(&objects, FlatTree::default()).unwrap();
    let stored = write_revision(&objects, &initial_revision(tree, project.now()).unwrap()).unwrap();
    create_initial_revision(project.store(), &stored, None)
        .await
        .unwrap();
    let r1 = RevisionId::try_from_db(1).unwrap();
    let now = project.now();
    project
        .store()
        .write(async |tx| {
            insert_revision_commits(
                tx,
                "main",
                &[(r1, "4f2a9c1e4f2a9c1e4f2a9c1e4f2a9c1e4f2a9c1e".to_owned())],
                now,
            )
            .await
        })
        .await
        .unwrap();
    std::fs::write(
        project.paths().config(),
        "[remotes.backup]\nurl = \"https://ana:secret@example.com/ledger.git\"\nbranch = \"trunk\"\n\n[remotes.origin]\nurl = \"git@example.com:ledger.git\"\nbranch = \"main\"\npush_on_integrate = true\n",
    )
    .unwrap();
    let state = state_of(&project);
    let row = register(&project, "demo").await;
    let handle = state
        .projects
        .open(&row, state.clock.clone())
        .await
        .unwrap();

    let (status, body) = call(&state, Method::GET, "/api/v1/projects/demo/git", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["repository"],
        project.paths().root().to_string_lossy().as_ref()
    );
    assert_eq!(body["branch"], "main");
    assert_eq!(body["remotes"][0]["name"], "backup");
    assert_eq!(
        body["remotes"][0]["location"],
        "https://example.com/ledger.git"
    );
    assert_eq!(body["remotes"][0]["branch"], "trunk");
    assert_eq!(body["remotes"][0]["pushed"], Value::Null);
    assert_eq!(body["remotes"][1]["location"], "git@example.com:ledger.git");
    assert_eq!(body["remotes"][1]["push_on_integrate"], true);

    handle
        .store
        .write(async |tx| {
            record_push(tx, "origin", r1, now).await?;
            record_push_failure(tx, "origin", "remote_ahead", now).await?;
            record_push_failure(tx, "backup", "git_failed", now).await
        })
        .await
        .unwrap();
    let (_, body) = call(&state, Method::GET, "/api/v1/projects/demo/git", None).await;
    assert_eq!(body["remotes"][0]["failed"]["code"], "git_failed");
    assert_eq!(body["remotes"][1]["pushed"]["revision"], "r1");
    assert_eq!(
        body["remotes"][1]["pushed"]["commit"],
        "4f2a9c1e4f2a9c1e4f2a9c1e4f2a9c1e4f2a9c1e"
    );
    assert_eq!(body["remotes"][1]["failed"]["code"], "remote_ahead");

    handle
        .store
        .write(async |tx| record_push(tx, "origin", r1, now).await)
        .await
        .unwrap();
    let (_, body) = call(&state, Method::GET, "/api/v1/projects/demo/git", None).await;
    assert_eq!(body["remotes"][1]["failed"], Value::Null);
    assert_eq!(body["remotes"][1]["pushed"]["revision"], "r1");
}
