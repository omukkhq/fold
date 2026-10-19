use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::{Actor, AgentActor, AgentName, EventFilter, FoldError, TaskId, WorkerName, codes};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::{openapi, router};
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::tasks::{self, TaskError};
use fold_store::events;
use fold_store::projects::ProjectRow;
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::testing::{TestProject, execute};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const BASE: &str = "/api/v1/projects/demo";
const ANA: (&str, &str) = ("cookie", "fold_human=ana");

#[cfg(test)]
struct Fixture {
    project: TestProject,
    state: AppState,
    handle: Arc<ProjectHandle>,
}

#[cfg(test)]
impl Fixture {
    async fn new() -> Self {
        Self::with_config(None).await.unwrap()
    }

    /// The fixture with `.fold/config.toml` holding `config`, or the error the open returns.
    async fn with_config(config: Option<&str>) -> Result<Self, FoldError> {
        let project = TestProject::new().await;
        if let Some(text) = config {
            std::fs::write(project.paths().config(), text).unwrap();
        }
        let clock = Arc::new(project.clock().clone());
        let state: AppState = Arc::new(DaemonState {
            registry: project.registry().clone(),
            projects: Projects::default(),
            clock: clock.clone(),
            assets: AssetSource::Embedded,
            listen: "127.0.0.1".parse().unwrap(),
            session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
        });
        let row = ProjectRow {
            slug: "demo".to_owned(),
            name: "Demo".to_owned(),
            root: project.paths().root().to_string_lossy().into_owned(),
        };
        let handle = state.projects.open(&row, clock).await?;
        Ok(Self {
            project,
            state,
            handle,
        })
    }

    fn http(&self) -> Router {
        router(self.state.clone())
    }

    async fn send(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.send_as(method, path, body, &[ANA]).await
    }

    async fn send_as(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .header(CLIENT_HEADER, "test")
            .method(method)
            .uri(format!("{BASE}{path}"));
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let body = match body {
            Some(value) => {
                request = request.header("content-type", "application/json");
                Body::from(value.to_string())
            }
            None => Body::empty(),
        };
        let response = self
            .http()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, value)
    }

    async fn ok(&self, method: Method, path: &str, body: Option<Value>) -> Value {
        let (status, value) = self.send(method.clone(), path, body).await;
        assert!(status.is_success(), "{method} {path}: {status} {value}");
        value
    }

    async fn head(&self) -> u64 {
        events::head_seq(self.handle.store.read()).await.unwrap()
    }

    /// `(type, actor)` of every event after `after` that a human or an agent made, leaving out the status steps `fold` records.
    async fn actors_after(&self, after: u64) -> Vec<(String, String)> {
        events::read_after(
            self.handle.store.read(),
            after,
            &EventFilter::default(),
            1000,
        )
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.actor.as_str() != "fold")
        .map(|event| (event.event_type.to_string(), event.actor.to_string()))
        .collect()
    }

    async fn complete(&self, number: u32) {
        let now = self.project.now();
        let task = TaskId::from_number(number).unwrap();
        self.handle
            .store
            .write(async |tx| {
                completions::insert(tx, task, CompletionKind::Children, None, now).await?;
                tasks::recompute(tx, &[task], now).await?;
                Ok::<_, TaskError>(())
            })
            .await
            .unwrap();
    }
}

/// Fails unless `status` is a declared response of the operation and an error body is the declared envelope.
#[cfg(test)]
fn assert_declared(method: &str, template: &str, status: StatusCode, body: &Value) {
    let doc = serde_json::to_value(openapi()).unwrap();
    let operation = &doc["paths"][format!("{BASE_TEMPLATE}{template}")][method];
    assert!(operation.is_object(), "{method} {template} is not declared");
    let response = &operation["responses"][status.as_str()];
    assert!(
        response.is_object(),
        "{method} {template} does not declare {status}"
    );
    if status.is_client_error() || status.is_server_error() {
        assert_eq!(
            response["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/ErrorEnvelope"
        );
        let error = body["error"].as_object().unwrap();
        let mut keys: Vec<&str> = error.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["code", "details", "message"], "{body}");
        assert!(error["details"].is_object());
    }
}

const BASE_TEMPLATE: &str = "/api/v1/projects/{slug}";

/// Method, path, route template, body, expected status and the event types the write records.
#[cfg(test)]
type WriteCase = (
    Method,
    &'static str,
    &'static str,
    Option<Value>,
    StatusCode,
    Vec<&'static str>,
);

/// Method, path, route template, body, expected status and the error code.
#[cfg(test)]
type ErrorCase = (
    Method,
    &'static str,
    &'static str,
    Option<Value>,
    StatusCode,
    &'static str,
);

#[tokio::test]
async fn the_board_returns_five_columns_in_order_with_blocked_blocks_and_attention() {
    let fx = Fixture::new().await;
    fx.ok(Method::POST, "/tasks", Some(json!({"title": "Schema"})))
        .await;
    fx.ok(
        Method::POST,
        "/tasks",
        Some(json!({"title": "Endpoint", "needs": ["T-1"]})),
    )
    .await;
    fx.ok(Method::POST, "/tasks", Some(json!({"title": "Shipped"})))
        .await;
    fx.complete(3).await;
    fx.ok(Method::POST, "/tasks", Some(json!({"title": "Started"})))
        .await;
    execute(
        fx.project.store(),
        "INSERT INTO revisions (number, hash, tree_hash, created_at) VALUES (1, 'h1', 't1', 1);
         INSERT INTO workspaces (number, task_id, kind, path, base_revision_id, state, created_at)
         VALUES (5, (SELECT id FROM tasks WHERE number = 4), 'task', 'ws', 1, 'active', 1);",
    )
    .await;
    fx.ok(
        Method::POST,
        "/tasks/T-4/dependencies",
        Some(json!({"needs": "T-1"})),
    )
    .await;

    let (status, board) = fx.send(Method::GET, "/board", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_declared("get", "/board", status, &board);
    let statuses: Vec<&str> = board["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|column| column["status"].as_str().unwrap())
        .collect();
    assert_eq!(
        statuses,
        ["backlog", "todo", "in_progress", "review", "done"]
    );
    let backlog = &board["columns"][0];
    assert_eq!(backlog["total"], 1);
    assert_eq!(
        backlog["cards"][0],
        json!({
            "task": "T-2",
            "title": "Endpoint",
            "status": "backlog",
            "blocked": false,
            "kind": "proposal",
            "report": null,
            "labels": [],
            "areas_conflict": false,
            "checklist": {"steps_done": 0, "steps_total": 0, "linked_done": 0, "linked_total": 0},
            "needs": ["T-1"],
            "blocked_by": ["T-1"],
            "blocks": 0,
            "worker": null,
            "workspace": null,
            "human_review": null,
            "blocker": null,
            "messages": 0,
            "completed_at": null,
            "attention": [],
            "proposal": null,
            "activity": null,
        })
    );
    let todo = &board["columns"][1]["cards"][0];
    assert_eq!(todo["task"], "T-1");
    assert_eq!(todo["blocks"], 2);
    let started = &board["columns"][2]["cards"][0];
    assert_eq!(started["task"], "T-4");
    assert_eq!(started["status"], "in_progress");
    assert_eq!(started["blocked"], true);
    assert_eq!(started["blocked_by"], json!(["T-1"]));
    assert_eq!(started["workspace"], "ws-5");
    assert_eq!(started["attention"], json!([]));
    assert_eq!(todo["attention"], json!([]));
    let done = &board["columns"][4];
    assert_eq!(done["total"], 1);
    assert_eq!(done["cards"][0]["task"], "T-3");
    assert_eq!(
        done["cards"][0]["completed_at"],
        json!(fx.project.now().as_millis())
    );

    fx.ok(Method::POST, "/tasks/T-1/archive", None).await;
    let board = fx.ok(Method::GET, "/board", None).await;
    assert_eq!(board["columns"][0]["cards"], json!([]));
    assert_eq!(board["columns"][1]["cards"][0]["task"], "T-2");
    assert_eq!(board["columns"][2]["cards"][0]["blocked"], false);
    assert_eq!(board["columns"][1]["cards"][0]["needs"], json!([]));
    assert_eq!(board["columns"][1]["total"], 1);
}

#[tokio::test]
async fn the_task_list_leaves_archived_tasks_out() {
    let fx = Fixture::new().await;
    fx.ok(
        Method::POST,
        "/labels",
        Some(json!({"name": "api", "color": "teal"})),
    )
    .await;
    fx.ok(
        Method::POST,
        "/tasks",
        Some(json!({"title": "Keep", "labels": ["api"]})),
    )
    .await;
    fx.ok(Method::POST, "/tasks", Some(json!({"title": "Hide"})))
        .await;
    fx.ok(Method::POST, "/tasks/T-2/archive", None).await;
    let (status, list) = fx.send(Method::GET, "/tasks", None).await;
    assert_declared("get", "/tasks", status, &list);
    assert_eq!(
        list,
        json!({"tasks": [{
            "task": "T-1",
            "title": "Keep",
            "status": "todo",
            "blocked": false,
            "labels": [{"name": "api", "color": "teal"}],
            "areas_conflict": false,
        }]})
    );
    let archived = fx.ok(Method::GET, "/tasks/T-2", None).await;
    assert!(archived["archived_at"].is_i64());
}

#[tokio::test]
async fn every_write_route_records_the_human() {
    let fx = Fixture::new().await;
    let human = "human:ana".to_owned();
    let cases: Vec<WriteCase> = vec![
        (
            Method::POST,
            "/labels",
            "/labels",
            Some(json!({"name": "api", "color": "sage"})),
            StatusCode::CREATED,
            vec!["label.created"],
        ),
        (
            Method::POST,
            "/tasks",
            "/tasks",
            Some(json!({"title": "Parent", "labels": ["api"]})),
            StatusCode::CREATED,
            vec!["task.created"],
        ),
        (
            Method::POST,
            "/tasks",
            "/tasks",
            Some(json!({"title": "Child", "parent": "T-1"})),
            StatusCode::CREATED,
            vec![
                "task.created",
                "dependency.added",
                "checklist.created",
                "checklist.item_added",
            ],
        ),
        (
            Method::POST,
            "/tasks",
            "/tasks",
            Some(json!({"title": "Other"})),
            StatusCode::CREATED,
            vec!["task.created"],
        ),
        (
            Method::PATCH,
            "/tasks/T-1",
            "/tasks/{id}",
            Some(json!({"title": "Parent v2"})),
            StatusCode::OK,
            vec!["task.updated"],
        ),
        (
            Method::DELETE,
            "/tasks/T-1/labels/api",
            "/tasks/{id}/labels/{name}",
            None,
            StatusCode::OK,
            vec!["task.updated"],
        ),
        (
            Method::PUT,
            "/tasks/T-1/labels/api",
            "/tasks/{id}/labels/{name}",
            None,
            StatusCode::OK,
            vec!["task.updated"],
        ),
        (
            Method::POST,
            "/tasks/T-3/dependencies",
            "/tasks/{id}/dependencies",
            Some(json!({"needs": "T-2"})),
            StatusCode::NO_CONTENT,
            vec!["dependency.added"],
        ),
        (
            Method::DELETE,
            "/tasks/T-3/dependencies/T-2",
            "/tasks/{id}/dependencies/{needs}",
            None,
            StatusCode::NO_CONTENT,
            vec!["dependency.removed"],
        ),
        (
            Method::POST,
            "/tasks/T-1/checklist-items",
            "/tasks/{id}/checklist-items",
            Some(json!({"text": "Wire it"})),
            StatusCode::CREATED,
            vec!["checklist.item_added"],
        ),
        (
            Method::PATCH,
            "/checklist-items/ci-2",
            "/checklist-items/{id}",
            Some(json!({"done": true})),
            StatusCode::OK,
            vec!["checklist.item_done"],
        ),
        (
            Method::PATCH,
            "/checklist-items/ci-2",
            "/checklist-items/{id}",
            Some(json!({"position": 0})),
            StatusCode::OK,
            vec!["checklist.item_updated"],
        ),
        (
            Method::DELETE,
            "/checklist-items/ci-2",
            "/checklist-items/{id}",
            None,
            StatusCode::NO_CONTENT,
            vec!["checklist.item_removed"],
        ),
        (
            Method::POST,
            "/tasks/T-3/archive",
            "/tasks/{id}/archive",
            None,
            StatusCode::NO_CONTENT,
            vec!["task.archived"],
        ),
        (
            Method::POST,
            "/tasks",
            "/tasks",
            Some(json!({"title": "Spare"})),
            StatusCode::CREATED,
            vec!["task.created"],
        ),
        (
            Method::DELETE,
            "/tasks/T-4",
            "/tasks/{id}",
            None,
            StatusCode::NO_CONTENT,
            vec!["task.deleted"],
        ),
    ];
    for (method, path, template, body, expected, kinds) in cases {
        let head = fx.head().await;
        let (status, value) = fx.send(method.clone(), path, body).await;
        assert_eq!(status, expected, "{method} {path}: {value}");
        assert_declared(&method.as_str().to_lowercase(), template, status, &value);
        let recorded = fx.actors_after(head).await;
        let expected: Vec<(String, String)> = kinds
            .iter()
            .map(|kind| ((*kind).to_owned(), human.clone()))
            .collect();
        assert_eq!(recorded, expected, "{method} {path}");
    }
}

#[tokio::test]
async fn reads_and_writes_return_the_declared_shapes() {
    let fx = Fixture::new().await;
    let (status, created) = fx
        .send(
            Method::POST,
            "/tasks",
            Some(json!({"title": "Schema", "description": "Tables"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["task"], "T-1");
    assert_eq!(created["description"], "Tables");
    assert_eq!(created["created_by"], "human:ana");
    fx.ok(
        Method::POST,
        "/tasks",
        Some(json!({"title": "Endpoint", "parent": "T-1"})),
    )
    .await;
    fx.ok(
        Method::POST,
        "/tasks/T-1/checklist-items",
        Some(json!({"text": "Docs"})),
    )
    .await;
    let (_, item) = fx
        .send(
            Method::PATCH,
            "/checklist-items/ci-2",
            Some(json!({"position": 0, "checklist": "Later"})),
        )
        .await;
    assert_eq!(
        item,
        json!({"id": "ci-2", "task": "T-1", "checklist": "Later", "text": "Docs", "linked_task": null, "done": false})
    );
    let view = fx.ok(Method::GET, "/tasks/T-1", None).await;
    assert_eq!(
        view["checklists"],
        json!([
            {"name": "Subtasks", "items": [
                {"kind": "linked", "id": "ci-1", "task": "T-2", "title": "Endpoint", "status": "todo", "done": false},
            ]},
            {"name": "Later", "items": [
                {"kind": "step", "id": "ci-2", "text": "Docs", "done": false},
            ]},
        ])
    );
    assert_eq!(
        view["dependencies"],
        json!([{"task": "T-2", "title": "Endpoint", "status": "todo", "direction": "needs", "origin": "checklist"}])
    );
    assert_eq!(view["claims"], json!([]));
    assert_eq!(view["attention"], json!([]));
    assert_eq!(view["status_reason"], "Calculated: waits for T-2");
    for null in [
        "workspace_info",
        "activity",
        "proposal",
        "integration",
        "human_review_set_at",
    ] {
        assert_eq!(view[null], Value::Null, "{null}");
    }
    assert_eq!(view["changesets"], json!([]));
    assert_eq!(view["can_change_human_review"], true);
    let history = view["history"].as_array().unwrap();
    assert_eq!(history[0]["type"], "checklist.item_updated");
    assert_eq!(history[0]["actor"], "human:ana");
    assert!(
        history
            .iter()
            .any(|e| e["type"] == "task.created" && e["actor"] == "human:ana")
    );
    assert!(history[0]["seq"].as_u64() > history[1]["seq"].as_u64());
    let (_, anonymous) = fx.send_as(Method::GET, "/tasks/T-1", None, &[]).await;
    assert_eq!(anonymous["can_change_human_review"], false);
    let board = fx.ok(Method::GET, "/board", None).await;
    let card = &board["columns"][1]["cards"][0];
    assert_eq!(
        (card["proposal"].clone(), card["activity"].clone()),
        (Value::Null, Value::Null)
    );
    assert_eq!(
        view["progress"],
        json!({"steps_done": 0, "steps_total": 1, "linked_done": 0, "linked_total": 1})
    );
    let child = fx.ok(Method::GET, "/tasks/T-2", None).await;
    assert_eq!(child["dependencies"][0]["direction"], "needed_by");
    let patched = fx
        .ok(
            Method::PATCH,
            "/tasks/T-1",
            Some(json!({"description": "More tables"})),
        )
        .await;
    assert_eq!(patched["description"], "More tables");
    let labels = fx.ok(Method::GET, "/labels", None).await;
    assert_eq!(labels, json!({"labels": []}));
}

#[tokio::test]
async fn errors_use_the_envelope_and_a_declared_status() {
    let fx = Fixture::new().await;
    fx.ok(Method::POST, "/tasks", Some(json!({"title": "A"})))
        .await;
    fx.ok(
        Method::POST,
        "/tasks",
        Some(json!({"title": "B", "needs": ["T-1"]})),
    )
    .await;
    fx.ok(
        Method::POST,
        "/labels",
        Some(json!({"name": "api", "color": "sage"})),
    )
    .await;
    execute(
        fx.project.store(),
        "INSERT INTO messages (number, thread_id, sender_kind, sender_human, task_id, body, created_at)
         VALUES (1, 1, 'human', 'ana', (SELECT id FROM tasks WHERE number = 1), 'hello', 1)",
    )
    .await;
    let cases: Vec<ErrorCase> = vec![
        (
            Method::GET,
            "/tasks/T-9",
            "/tasks/{id}",
            None,
            StatusCode::NOT_FOUND,
            "task_not_found",
        ),
        (
            Method::GET,
            "/tasks/9",
            "/tasks/{id}",
            None,
            StatusCode::BAD_REQUEST,
            "invalid_id",
        ),
        (
            Method::POST,
            "/tasks/T-1/dependencies",
            "/tasks/{id}/dependencies",
            Some(json!({"needs": "T-2"})),
            StatusCode::CONFLICT,
            "dependency_cycle",
        ),
        (
            Method::POST,
            "/tasks/T-2/dependencies",
            "/tasks/{id}/dependencies",
            Some(json!({"needs": "T-1"})),
            StatusCode::CONFLICT,
            "dependency_exists",
        ),
        (
            Method::POST,
            "/tasks",
            "/tasks",
            Some(json!({"title": "C", "needs": ["T-2"], "parent": "T-2"})),
            StatusCode::CONFLICT,
            "dependency_cycle",
        ),
        (
            Method::POST,
            "/tasks",
            "/tasks",
            Some(json!({"title": ""})),
            StatusCode::BAD_REQUEST,
            "invalid_task_title",
        ),
        (
            Method::POST,
            "/tasks",
            "/tasks",
            Some(json!({"title": "x", "labels": ["nope"]})),
            StatusCode::NOT_FOUND,
            "label_not_found",
        ),
        (
            Method::POST,
            "/tasks",
            "/tasks",
            Some(json!({"nope": 1})),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            Method::DELETE,
            "/tasks/T-1",
            "/tasks/{id}",
            None,
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            Method::DELETE,
            "/tasks/T-1/dependencies/T-2",
            "/tasks/{id}/dependencies/{needs}",
            None,
            StatusCode::NOT_FOUND,
            "dependency_not_found",
        ),
        (
            Method::POST,
            "/tasks/T-1/checklist-items",
            "/tasks/{id}/checklist-items",
            Some(json!({"text": "a", "linked_task": "T-2"})),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            Method::PATCH,
            "/checklist-items/ci-9",
            "/checklist-items/{id}",
            Some(json!({"done": true})),
            StatusCode::NOT_FOUND,
            "checklist_item_not_found",
        ),
        (
            Method::PATCH,
            "/checklist-items/ci-9",
            "/checklist-items/{id}",
            Some(json!({"done": true, "position": 1})),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        ),
        (
            Method::DELETE,
            "/checklist-items/ci-9",
            "/checklist-items/{id}",
            None,
            StatusCode::NOT_FOUND,
            "checklist_item_not_found",
        ),
        (
            Method::POST,
            "/labels",
            "/labels",
            Some(json!({"name": "api", "color": "sage"})),
            StatusCode::CONFLICT,
            "label_exists",
        ),
        (
            Method::POST,
            "/labels",
            "/labels",
            Some(json!({"name": "ui", "color": "red"})),
            StatusCode::BAD_REQUEST,
            "invalid_label_color",
        ),
        (
            Method::POST,
            "/labels",
            "/labels",
            Some(json!({"name": " ", "color": "sage"})),
            StatusCode::BAD_REQUEST,
            "invalid_label_name",
        ),
        (
            Method::POST,
            "/tasks/T-1/checklist-items",
            "/tasks/{id}/checklist-items",
            Some(json!({"text": " "})),
            StatusCode::BAD_REQUEST,
            "invalid_step_text",
        ),
        (
            Method::POST,
            "/tasks/T-1/checklist-items",
            "/tasks/{id}/checklist-items",
            Some(json!({"text": "a", "checklist": " "})),
            StatusCode::BAD_REQUEST,
            "invalid_checklist_name",
        ),
        (
            Method::PATCH,
            "/checklist-items/ci-9",
            "/checklist-items/{id}",
            Some(json!({"checklist": " "})),
            StatusCode::BAD_REQUEST,
            "invalid_checklist_name",
        ),
        (
            Method::PATCH,
            "/tasks/T-9",
            "/tasks/{id}",
            Some(json!({"title": "x"})),
            StatusCode::NOT_FOUND,
            "task_not_found",
        ),
        (
            Method::POST,
            "/tasks/T-9/archive",
            "/tasks/{id}/archive",
            None,
            StatusCode::NOT_FOUND,
            "task_not_found",
        ),
    ];
    for (method, path, template, body, expected, code) in cases {
        let (status, value) = fx.send(method.clone(), path, body).await;
        assert_eq!(status, expected, "{method} {path}: {value}");
        assert_eq!(value["error"]["code"], code, "{method} {path}");
        assert_declared(&method.as_str().to_lowercase(), template, status, &value);
    }
    let (_, cycle) = fx
        .send(
            Method::POST,
            "/tasks/T-1/dependencies",
            Some(json!({"needs": "T-2"})),
        )
        .await;
    assert!(cycle["error"]["details"]["hint"].is_string());
    let (_, history) = fx.send(Method::DELETE, "/tasks/T-1", None).await;
    assert_eq!(history["error"]["details"]["reason"], "task_has_history");

    let (status, value) = fx
        .send_as(Method::POST, "/tasks", Some(json!({"title": "x"})), &[])
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_declared("post", "/tasks", status, &value);
    let (status, value) = fx.send_as(Method::GET, "/board", None, &[]).await;
    assert_eq!(status, StatusCode::OK, "reads need no human: {value}");
    let (status, missing) = fx.send_as(Method::GET, "/nope/board", None, &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
}

#[test]
fn every_task_route_is_declared_with_a_unique_operation() {
    let doc = serde_json::to_value(openapi()).unwrap();
    let routes = [
        ("/board", "get", "get_board"),
        ("/tasks", "get", "list_tasks"),
        ("/tasks", "post", "create_task"),
        ("/tasks/{id}", "get", "get_task"),
        ("/tasks/{id}", "patch", "update_task"),
        ("/tasks/{id}", "delete", "delete_task"),
        ("/tasks/{id}/labels/{name}", "put", "attach_label"),
        ("/tasks/{id}/labels/{name}", "delete", "detach_label"),
        ("/tasks/{id}/archive", "post", "archive_task"),
        ("/tasks/{id}/dependencies", "post", "add_dependency"),
        (
            "/tasks/{id}/dependencies/{needs}",
            "delete",
            "remove_dependency",
        ),
        ("/tasks/{id}/checklist-items", "post", "add_checklist_item"),
        ("/checklist-items/{id}", "patch", "update_checklist_item"),
        ("/checklist-items/{id}", "delete", "remove_checklist_item"),
        ("/labels", "get", "list_labels"),
        ("/labels", "post", "create_label"),
    ];
    for (path, method, operation) in routes {
        assert_eq!(
            doc["paths"][format!("{BASE_TEMPLATE}{path}")][method]["operationId"],
            operation
        );
    }
    let kinds = &doc["components"]["schemas"]["AttentionKind"]["enum"];
    assert_eq!(
        kinds,
        &json!(["question", "human_review", "stalled", "stale_claim"])
    );
}

/// Registers the agent `backend` and returns its main worker as an actor.
#[cfg(test)]
async fn backend(fx: &Fixture) -> Actor {
    let agents = AgentService::new(
        fx.handle.store.clone(),
        fx.handle.bus.clone(),
        fx.handle.clock.clone(),
    );
    let mut session = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut session, &AgentName::parse("backend").unwrap(), &[])
        .await
        .unwrap();
    let worker = WorkerName::parse("backend").unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

#[tokio::test]
async fn the_project_handle_applies_the_configured_lease() {
    let fx = Fixture::with_config(Some("lease_minutes = 7\n"))
        .await
        .unwrap();
    let actor = backend(&fx).await;
    crate::common::roles::lead_work_areas(&fx.handle.store).await;
    // `backend` claims the task as the lead of its one area.
    let labels = [crate::common::roles::WORK_AREAS[1].1];
    fx.ok(
        Method::POST,
        "/tasks",
        Some(json!({"title": "Leased", "labels": labels})),
    )
    .await;
    let task = TaskId::from_number(1).unwrap();
    fx.handle.tasks.claim_task(&actor, task).await.unwrap();
    fx.project
        .clock()
        .advance(Duration::from_secs(7 * 60 - 1))
        .unwrap();
    let view = fx.ok(Method::GET, "/tasks/T-1", None).await;
    assert_eq!(view["claims"][0]["stale"], false);
    fx.project.clock().advance(Duration::from_secs(1)).unwrap();
    let view = fx.ok(Method::GET, "/tasks/T-1", None).await;
    assert_eq!(view["claims"][0]["holder"], "backend");
    assert_eq!(view["claims"][0]["kind"], "implementation");
    assert_eq!(view["claims"][0]["stale"], true);
}

#[tokio::test]
async fn an_invalid_configuration_fails_only_what_needs_the_lease() {
    let fx = Fixture::with_config(Some("lease_minutes = 0\n"))
        .await
        .unwrap();
    let actor = backend(&fx).await;
    crate::common::roles::lead_work_areas(&fx.handle.store).await;
    // `backend` claims the task as the lead of its one area.
    let labels = [crate::common::roles::WORK_AREAS[1].1];
    fx.ok(
        Method::POST,
        "/tasks",
        Some(json!({"title": "Unclaimed", "labels": labels})),
    )
    .await;
    fx.ok(Method::GET, "/board", None).await;
    let view = fx.ok(Method::GET, "/tasks/T-1", None).await;
    assert_eq!(view["claims"], json!([]));
    let task = TaskId::from_number(1).unwrap();
    let TaskError::Fold(refused) = fx.handle.tasks.claim_task(&actor, task).await.unwrap_err()
    else {
        panic!("an invalid configuration is a refusal");
    };
    assert_eq!(refused.code(), codes::INVALID_CONFIG);
    assert!(refused.hint().unwrap().contains("config.toml"));

    std::fs::write(fx.project.paths().config(), "lease_minutes = 7\n").unwrap();
    fx.handle.tasks.claim_task(&actor, task).await.unwrap();
    std::fs::write(fx.project.paths().config(), "lease_minutes = 0\n").unwrap();
    let (status, body) = fx.send(Method::GET, "/tasks/T-1", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], codes::INVALID_CONFIG);
    assert_declared("get", "/tasks/{id}", status, &body);
    fx.ok(Method::GET, "/board", None).await;
}

#[tokio::test]
async fn labels_attach_and_detach_one_at_a_time_without_undoing_each_other() {
    let fx = Fixture::new().await;
    for (name, color) in [("api", "teal"), ("ui", "sage")] {
        fx.ok(
            Method::POST,
            "/labels",
            Some(json!({"name": name, "color": color})),
        )
        .await;
    }
    fx.ok(Method::POST, "/tasks", Some(json!({"title": "Work"})))
        .await;

    let (status, view) = fx.send(Method::PUT, "/tasks/T-1/labels/api", None).await;
    assert_declared("put", "/tasks/{id}/labels/{name}", status, &view);
    assert_eq!(status, StatusCode::OK);
    let (status, view) = fx.send(Method::PUT, "/tasks/T-1/labels/ui", None).await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = view["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|label| label["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"api") && names.contains(&"ui"));

    let after = fx.head().await;
    let (status, view) = fx.send(Method::PUT, "/tasks/T-1/labels/ui", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["labels"].as_array().unwrap().len(), 2);
    assert_eq!(
        fx.head().await,
        after,
        "a label already there writes no event"
    );

    let (status, view) = fx.send(Method::DELETE, "/tasks/T-1/labels/api", None).await;
    assert_declared("delete", "/tasks/{id}/labels/{name}", status, &view);
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["labels"], json!([{"name": "ui", "color": "sage"}]));
    let (status, view) = fx.send(Method::DELETE, "/tasks/T-1/labels/api", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["labels"], json!([{"name": "ui", "color": "sage"}]));
}

#[tokio::test]
async fn attaching_a_label_refuses_an_unknown_label_task_and_archived_task() {
    let fx = Fixture::new().await;
    fx.ok(
        Method::POST,
        "/labels",
        Some(json!({"name": "api", "color": "teal"})),
    )
    .await;
    fx.ok(Method::POST, "/tasks", Some(json!({"title": "Work"})))
        .await;
    let (status, error) = fx.send(Method::PUT, "/tasks/T-1/labels/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "label_not_found");
    let (status, error) = fx.send(Method::PUT, "/tasks/T-9/labels/api", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], "task_not_found");
    fx.ok(Method::POST, "/tasks/T-1/archive", None).await;
    let (status, error) = fx.send(Method::PUT, "/tasks/T-1/labels/api", None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "task_closed");
}

#[tokio::test]
async fn a_label_name_with_a_slash_and_a_space_travels_percent_encoded() {
    let fx = Fixture::new().await;
    fx.ok(
        Method::POST,
        "/labels",
        Some(json!({"name": "front/end ui", "color": "teal"})),
    )
    .await;
    fx.ok(Method::POST, "/tasks", Some(json!({"title": "Work"})))
        .await;
    let (status, view) = fx
        .send(Method::PUT, "/tasks/T-1/labels/front%2Fend%20ui", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        view["labels"],
        json!([{"name": "front/end ui", "color": "teal"}])
    );
    let (status, view) = fx
        .send(Method::DELETE, "/tasks/T-1/labels/front%2Fend%20ui", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(view["labels"], json!([]));
}
