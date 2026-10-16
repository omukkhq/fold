use fold_daemon::http::host::CLIENT_HEADER;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::{EventFilter, HumanName, TaskId, codes};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::{openapi, router};
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::projects::{DEFAULT_SESSION_LIFETIME, ensure_human};
use fold_daemon::services::tasks::TaskError;
use fold_store::events;
use fold_store::projects::ProjectRow;
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::testing::TestProject;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const BASE: &str = "/api/v1/projects/demo";
const ANA: (&str, &str) = ("cookie", "fold_human=ana");
const TANIM: (&str, &str) = ("cookie", "fold_human=tanim");
const MARK: &str = "/tasks/T-1/human-review";

#[cfg(test)]
struct Fixture {
    project: TestProject,
    state: AppState,
    handle: Arc<ProjectHandle>,
}

#[cfg(test)]
impl Fixture {
    /// Task `T-1` by `ana`; the registry knows `ana` and `tanim`.
    async fn new() -> Self {
        let project = TestProject::new().await;
        let clock = Arc::new(project.clock().clone());
        let state: AppState = Arc::new(DaemonState {
            registry: project.registry().clone(),
            projects: Projects::default(),
            clock: clock.clone(),
            assets: AssetSource::Embedded,
            listen: "127.0.0.1".parse().unwrap(),
            session_lifetime: DEFAULT_SESSION_LIFETIME,
        });
        let row = ProjectRow {
            slug: "demo".to_owned(),
            name: "Demo".to_owned(),
            root: project.paths().root().to_string_lossy().into_owned(),
        };
        let handle = state.projects.open(&row, clock).await.unwrap();
        ensure_human(
            project.registry(),
            project.clock(),
            &HumanName::parse("tanim").unwrap(),
        )
        .await
        .unwrap();
        let fx = Self {
            project,
            state,
            handle,
        };
        let (status, _) = fx
            .send(
                Method::POST,
                "/tasks",
                Some(json!({"title": "auth"})),
                &[ANA],
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
        fx
    }

    async fn send(
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
        let response = router(self.state.clone())
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

    async fn task(&self) -> Value {
        let (status, task) = self.send(Method::GET, "/tasks/T-1", None, &[]).await;
        assert_eq!(status, StatusCode::OK);
        task
    }

    /// `(type, actor, data)` of every event after `after`.
    async fn events_after(&self, after: u64) -> Vec<(String, String, Value)> {
        events::read_after(
            self.handle.store.read(),
            after,
            &EventFilter::default(),
            1000,
        )
        .await
        .unwrap()
        .into_iter()
        .map(|event| {
            (
                event.event_type.to_string(),
                event.actor.to_string(),
                event.data,
            )
        })
        .collect()
    }

    async fn head(&self) -> u64 {
        events::head_seq(self.handle.store.read()).await.unwrap()
    }
}

#[tokio::test]
async fn put_sets_the_mark_as_the_named_human() {
    let fx = Fixture::new().await;
    let before = fx.head().await;
    let (status, mark) = fx
        .send(
            Method::PUT,
            MARK,
            Some(json!({"reviewer": "tanim", "reason": "touches auth"})),
            &[ANA],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{mark}");
    let expected = json!({"reviewer": "tanim", "reason": "touches auth", "set_by": "human:ana"});
    assert_eq!(mark, expected);
    let task = fx.task().await;
    assert_eq!(task["human_review"], expected);
    assert_eq!(task["human_review_set_at"], fx.project.now().as_millis());
    let (_, board) = fx.send(Method::GET, "/board", None, &[]).await;
    let card = board["columns"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|column| column["cards"].as_array().unwrap())
        .find(|card| card["task"] == "T-1")
        .unwrap()
        .clone();
    assert_eq!(card["human_review"], expected);
    assert_eq!(
        fx.events_after(before).await,
        [(
            "task.human_review_set".to_owned(),
            "human:ana".to_owned(),
            json!({"reviewer": "tanim", "reason": "touches auth", "set_by": "human:ana"}),
        )]
    );

    let (status, any) = fx
        .send(
            Method::PUT,
            "/tasks/T-1/human-review",
            Some(json!({"reviewer": null, "reason": "any human"})),
            &[TANIM],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(any["reviewer"], Value::Null);
}

#[tokio::test]
async fn put_by_a_human_changes_an_existing_mark() {
    let fx = Fixture::new().await;
    fx.send(
        Method::PUT,
        MARK,
        Some(json!({"reviewer": "tanim", "reason": "touches auth"})),
        &[ANA],
    )
    .await;
    let before = fx.head().await;
    let (status, same) = fx
        .send(
            Method::PUT,
            MARK,
            Some(json!({"reviewer": "tanim", "reason": "touches auth"})),
            &[TANIM],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        same["set_by"], "human:ana",
        "the same mark is left as it is"
    );
    assert_eq!(fx.head().await, before);

    let (status, changed) = fx
        .send(
            Method::PUT,
            MARK,
            Some(json!({"reviewer": "ana", "reason": "ana wrote the auth code"})),
            &[TANIM],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(
        changed,
        json!({"reviewer": "ana", "reason": "ana wrote the auth code", "set_by": "human:tanim"})
    );
    assert_eq!(fx.task().await["human_review"], changed);
    assert_eq!(fx.events_after(before).await.len(), 1);
}

#[tokio::test]
async fn delete_clears_the_mark() {
    let fx = Fixture::new().await;
    fx.send(
        Method::PUT,
        MARK,
        Some(json!({"reviewer": "tanim", "reason": "touches auth"})),
        &[ANA],
    )
    .await;
    let before = fx.head().await;
    let (status, task) = fx.send(Method::DELETE, MARK, None, &[TANIM]).await;
    assert_eq!(status, StatusCode::OK, "{task}");
    assert_eq!(task["task"], "T-1");
    assert_eq!(task["human_review"], Value::Null);
    assert_eq!(task["human_review_set_at"], Value::Null);
    assert_eq!(task["can_change_human_review"], true);
    assert_eq!(fx.task().await["human_review"], Value::Null);
    assert_eq!(
        fx.events_after(before).await,
        [(
            "task.human_review_cleared".to_owned(),
            "human:tanim".to_owned(),
            json!({"reviewer": "tanim", "reason": "touches auth"}),
        )]
    );
    let (status, _) = fx.send(Method::DELETE, MARK, None, &[TANIM]).await;
    assert_eq!(status, StatusCode::OK, "clearing again changes nothing");
    assert_eq!(fx.events_after(before).await.len(), 1);
}

#[tokio::test]
async fn request_without_a_human_name_is_refused() {
    let fx = Fixture::new().await;
    let before = fx.head().await;
    let body = json!({"reason": "touches auth"});
    for (method, body) in [(Method::PUT, Some(body)), (Method::DELETE, None)] {
        let (status, error) = fx.send(method.clone(), MARK, body, &[]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method}");
        assert_eq!(error["error"]["code"], codes::HUMAN_NAME_REQUIRED);
        assert_declared(method.as_str(), status, &error);
    }
    assert_eq!(fx.head().await, before);
}

#[tokio::test]
async fn refusals_carry_their_declared_status() {
    let fx = Fixture::new().await;
    let cases = [
        (
            "/tasks/T-1/human-review",
            json!({"reviewer": "nobody", "reason": "x"}),
            StatusCode::NOT_FOUND,
            codes::UNKNOWN_HUMAN,
        ),
        (
            "/tasks/T-9/human-review",
            json!({"reason": "x"}),
            StatusCode::NOT_FOUND,
            codes::TASK_NOT_FOUND,
        ),
        (
            "/tasks/T-1/human-review",
            json!({"reason": " "}),
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
        ),
        (
            "/tasks/T-1/human-review",
            json!({"reviewer": "", "reason": "x"}),
            StatusCode::BAD_REQUEST,
            codes::INVALID_NAME,
        ),
        (
            "/tasks/X-1/human-review",
            json!({"reason": "x"}),
            StatusCode::BAD_REQUEST,
            codes::INVALID_ID,
        ),
    ];
    for (path, body, status, code) in cases {
        let (got, error) = fx.send(Method::PUT, path, Some(body), &[ANA]).await;
        assert_eq!(got, status, "{path} {error}");
        assert_eq!(error["error"]["code"], code);
        assert_declared("PUT", got, &error);
    }
    let (status, error) = fx
        .send(Method::DELETE, "/tasks/T-9/human-review", None, &[ANA])
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["error"]["code"], codes::TASK_NOT_FOUND);
    assert_declared("DELETE", status, &error);

    let now = fx.project.now();
    let task = TaskId::from_number(1).unwrap();
    fx.handle
        .store
        .write(async |tx| {
            completions::insert(tx, task, CompletionKind::Children, None, now).await?;
            fold_daemon::services::tasks::recompute(tx, &[task], now).await?;
            Ok::<_, TaskError>(())
        })
        .await
        .unwrap();
    for (method, body) in [
        (Method::PUT, Some(json!({"reason": "late"}))),
        (Method::DELETE, None),
    ] {
        let (status, error) = fx.send(method.clone(), MARK, body, &[ANA]).await;
        assert_eq!(status, StatusCode::CONFLICT, "{method}");
        assert_eq!(error["error"]["code"], codes::TASK_CLOSED);
        assert_declared(method.as_str(), status, &error);
    }
}

#[test]
fn openapi_declares_both_routes_and_their_errors() {
    let doc = serde_json::to_value(openapi()).unwrap();
    let path = &doc["paths"]["/api/v1/projects/{slug}/tasks/{id}/human-review"];
    let put = &path["put"];
    assert_eq!(put["operationId"], "set_human_review");
    assert_eq!(
        put["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/HumanReviewMark"
    );
    assert_eq!(
        put["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/HumanReviewBody"
    );
    let delete = &path["delete"];
    assert_eq!(delete["operationId"], "clear_human_review");
    assert_eq!(
        delete["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/TaskView"
    );
    for (operation, statuses) in [
        (put, ["400", "401", "403", "404", "409", "500", "503"]),
        (delete, ["400", "401", "403", "404", "409", "500", "503"]),
    ] {
        for status in statuses {
            assert_eq!(
                operation["responses"][status]["content"]["application/json"]["schema"]["$ref"],
                "#/components/schemas/ErrorEnvelope",
                "{status}"
            );
        }
    }
    let mark = &doc["components"]["schemas"]["HumanReviewMark"];
    let mut required: Vec<&str> = mark["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect();
    required.sort_unstable();
    assert_eq!(required, ["reason", "reviewer", "set_by"]);
}

/// Fails unless `status` is a declared response of the human-review operation and an error body is the declared envelope.
#[cfg(test)]
fn assert_declared(method: &str, status: StatusCode, body: &Value) {
    let doc = serde_json::to_value(openapi()).unwrap();
    let operation =
        &doc["paths"]["/api/v1/projects/{slug}/tasks/{id}/human-review"][method.to_lowercase()];
    let response = &operation["responses"][status.as_str()];
    assert!(response.is_object(), "{method} does not declare {status}");
    assert_eq!(
        response["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/ErrorEnvelope"
    );
    assert!(body["error"]["details"].is_object(), "{body}");
}
