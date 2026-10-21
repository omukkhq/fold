#![cfg(test)]

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::{WorkerState, codes};
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::router;
use fold_daemon::projects::Slot;
use fold_daemon::services::projects::{self as project_service, init_project};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use crate::deleted_projects::{Running, closed, entries};
use crate::project_pause::{LEAD, lead_running, until_not_running};
use crate::workers_support::FakeClaude;

/// One request to the daemon's router as the web interface makes it, as the human `human` when one is named.
async fn call(
    state: &fold_daemon::daemon::AppState,
    method: Method,
    path: &str,
    human: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(method)
        .uri(path);
    if let Some(human) = human {
        request = request.header("cookie", format!("fold_human={human}"));
    }
    let response = router(state.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

impl Running {
    async fn remove(&self) -> (StatusCode, Value) {
        call(
            &self.state,
            Method::DELETE,
            "/api/v1/projects/app",
            Some("ana"),
        )
        .await
    }

    async fn restore(&self) -> (StatusCode, Value) {
        call(
            &self.state,
            Method::POST,
            "/api/v1/projects/app/restore",
            Some("ana"),
        )
        .await
    }

    async fn listed(&self, path: &str) -> Vec<Value> {
        let (status, body) = call(&self.state, Method::GET, path, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["projects"].as_array().unwrap().clone()
    }

    async fn removed_at(&self) -> Option<i64> {
        fold_store::projects::find_registration(self.state.registry.read(), "app")
            .await
            .unwrap()
            .unwrap()
            .removed_at
            .map(fold_core::Timestamp::as_millis)
    }

    fn moved_root(&self) -> PathBuf {
        self.root.with_extension("away")
    }
}

#[tokio::test]
async fn a_human_removes_a_project_which_closes_keeps_its_data_and_leaves_the_list() {
    let running = Running::start().await;
    let handle = running.handle.clone();
    let outside_fold = |root: &PathBuf| -> Vec<PathBuf> {
        entries(root)
            .into_iter()
            .filter(|path| !path.starts_with(root.join(".fold")))
            .collect()
    };
    let before = outside_fold(&running.root);

    let (status, body) = call(&running.state, Method::DELETE, "/api/v1/projects/app", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "human_name_required");
    assert_eq!(running.removed_at().await, None);

    let (status, body) = running.remove().await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let removed_at = running.removed_at().await;
    assert_eq!(body["removed_at"].as_i64(), removed_at);
    assert!(removed_at.is_some());
    assert_eq!(body["available"], false);
    assert_eq!(body["folder"], "present");
    assert_eq!(body["root"], running.root.to_str().unwrap());
    assert!(closed(&handle), "the handle closed");
    assert!(running.state.projects.slot("app").is_none());
    assert!(!running.registered().await, "the project is out of fold");
    assert!(
        fold_core::ProjectPaths::new(&running.root).db().is_file(),
        "the data stays"
    );
    assert_eq!(
        outside_fold(&running.root),
        before,
        "the folder is as it was"
    );

    assert!(running.listed("/api/v1/projects").await.is_empty());
    let all = running
        .listed("/api/v1/projects?include_removed=true")
        .await;
    assert_eq!(all.len(), 1);
    assert_eq!(all[0]["slug"], "app");
    assert_eq!(all[0]["removed_at"].as_i64(), removed_at);

    // Every way to the project names the way back, and none of them opens it again.
    let (status, body) = call(&running.state, Method::GET, "/api/v1/projects/app", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "project_not_found");
    assert_eq!(body["error"]["details"]["reason"], "removed");
    let hint = body["error"]["details"]["hint"].as_str().unwrap();
    assert!(
        hint.contains("`fld claude`") && hint.contains("fld init"),
        "{hint}"
    );
    let refused = project_service::open_for_dir(&running.state, &running.root)
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(codes::PROJECT_NOT_FOUND));
    assert_eq!(
        project_service::sweep_deleted(&running.state)
            .await
            .unwrap(),
        0
    );
    assert!(running.state.projects.slot("app").is_none());

    let (status, again) = running.remove().await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(
        again["removed_at"].as_i64(),
        removed_at,
        "a second removal changes nothing"
    );
    running.close().await;
}

#[tokio::test]
async fn the_projects_page_adds_a_removed_project_back_only_while_its_folder_holds_fold() {
    let running = Running::start().await;
    assert_eq!(running.remove().await.0, StatusCode::OK);

    std::fs::rename(&running.root, running.moved_root()).unwrap();
    let (status, body) = running.restore().await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["details"]["reason"], "folder_missing");
    let all = running
        .listed("/api/v1/projects?include_removed=true")
        .await;
    assert_eq!(all[0]["folder"], "folder_missing");
    assert!(running.removed_at().await.is_some(), "the mark stays");
    std::fs::rename(running.moved_root(), &running.root).unwrap();

    let (status, body) = running.restore().await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["removed_at"], Value::Null);
    assert_eq!(body["available"], true);
    assert!(running.removed_at().await.is_none());
    let Some(Slot::Open(again)) = running.state.projects.slot("app") else {
        panic!("the project opened again");
    };
    assert!(!Arc::ptr_eq(&again, &running.handle));
    let (status, _) = call(&running.state, Method::GET, "/api/v1/projects/app", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(running.listed("/api/v1/projects").await.len(), 1);
    let (status, body) = running.restore().await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["available"], true, "a project in fold stays as it is");

    assert_eq!(running.remove().await.0, StatusCode::OK);
    std::fs::remove_dir_all(running.fold_dir()).unwrap();
    let (status, body) = running.restore().await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(body["error"]["details"]["reason"], "fold_missing");
    assert!(
        body["error"]["details"]["hint"]
            .as_str()
            .unwrap()
            .contains("fld init")
    );
    let all = running
        .listed("/api/v1/projects?include_removed=true")
        .await;
    assert_eq!(all[0]["folder"], "fold_missing");
    running.close().await;
}

#[tokio::test]
async fn fld_init_and_the_open_route_bring_a_removed_project_back_under_its_slug() {
    let running = Running::start().await;
    assert_eq!(running.remove().await.0, StatusCode::OK);
    let project = init_project(
        &running.host,
        &running.root,
        Some("other name"),
        &crate::common::ana(),
        &running.clock,
    )
    .await
    .unwrap();
    assert!(project.restored && !project.resumed && !project.replaced);
    assert_eq!(
        (project.slug.as_str(), project.name.as_str()),
        ("app", "app")
    );
    assert!(running.removed_at().await.is_none());
    project_service::open(&running.state, "app").await.unwrap();
    assert_eq!(running.listed("/api/v1/projects").await.len(), 1);

    // `fld claude` and `fld new` open the project through the socket's open route.
    assert_eq!(running.remove().await.0, StatusCode::OK);
    let reopened = project_service::open(&running.state, "app").await.unwrap();
    assert!(!closed(&reopened));
    assert!(running.removed_at().await.is_none());

    // A removed project whose `.fold/` was deleted leaves its slug to the new project `fld init` makes there.
    assert_eq!(running.remove().await.0, StatusCode::OK);
    std::fs::remove_dir_all(running.fold_dir()).unwrap();
    let project = init_project(
        &running.host,
        &running.root,
        None,
        &crate::common::ana(),
        &running.clock,
    )
    .await
    .unwrap();
    assert!(project.replaced && !project.restored);
    assert_eq!(project.slug, "app");
    assert!(running.removed_at().await.is_none());
    running.close().await;
}

#[tokio::test]
async fn a_project_with_a_running_worker_is_not_removed_until_its_agents_are_paused() {
    let fake = FakeClaude::new("init");
    let harness = lead_running(&fake, WorkerState::Working).await;
    let (status, body) = harness
        .http(Method::DELETE, "/api/v1/projects/demo", None, "ana")
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], codes::PROJECT_WORKERS_RUNNING);
    assert_eq!(
        body["error"]["details"]["workers"],
        serde_json::json!([LEAD])
    );
    assert!(
        body["error"]["details"]["hint"]
            .as_str()
            .unwrap()
            .contains("pause")
    );
    assert!(!closed(&harness.handle));
    assert_eq!(
        fold_store::projects::find_project(harness.state.registry.read(), "demo")
            .await
            .unwrap()
            .map(|row| row.slug),
        Some("demo".to_owned())
    );

    let (status, body) = harness
        .http(Method::POST, "/api/v1/projects/demo/pause", None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    until_not_running(&harness, LEAD).await;
    let (status, body) = harness
        .http(Method::DELETE, "/api/v1/projects/demo", None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["removed_at"].is_i64());
    assert!(closed(&harness.handle));
}
