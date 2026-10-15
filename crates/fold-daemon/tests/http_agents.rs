use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::{AgentName, EventFilter, HumanName, Tag, Timestamp};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::{router, socket_router};
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::agents::AgentService;
use fold_store::events;
use fold_store::projects::{self as registry, ProjectRow};
use fold_store::testing::TestProject;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const PATH: &str = "/api/v1/projects/demo/agents/review/tags/integrator";

#[cfg(test)]
struct Fixture {
    project: TestProject,
    state: AppState,
    handle: Arc<ProjectHandle>,
}

#[cfg(test)]
impl Fixture {
    async fn new() -> Self {
        let project = TestProject::new().await;
        let clock = Arc::new(project.clock().clone());
        let state: AppState = Arc::new(DaemonState {
            registry: project.registry().clone(),
            projects: Projects::default(),
            clock: clock.clone(),
            assets: AssetSource::Embedded,
        });
        let row = ProjectRow {
            slug: "demo".to_owned(),
            name: "Demo".to_owned(),
            root: project.paths().root().to_string_lossy().into_owned(),
        };
        let handle = state.projects.open(&row, clock).await.unwrap();
        let agents = AgentService::new(
            handle.store.clone(),
            handle.bus.clone(),
            handle.clock.clone(),
        );
        let mut ctx = agents.start_session(1).await.unwrap();
        agents
            .register_agent(
                &mut ctx,
                &AgentName::parse("review").unwrap(),
                &[Tag::parse("review").unwrap()],
            )
            .await
            .unwrap();
        Self {
            project,
            state,
            handle,
        }
    }

    fn http(&self) -> Router {
        router(self.state.clone())
    }

    fn socket(&self) -> Router {
        socket_router(self.state.clone())
    }

    /// `(type, actor, data)` of every stored `agent.tag_*` event.
    async fn tag_events(&self) -> Vec<(String, String, Value)> {
        events::read_after(self.handle.store.read(), 0, &EventFilter::default(), 1000)
            .await
            .unwrap()
            .into_iter()
            .filter(|e| e.event_type.as_str().starts_with("agent.tag"))
            .map(|e| (e.event_type.to_string(), e.actor.to_string(), e.data))
            .collect()
    }
}

#[cfg(test)]
async fn call(
    router: Router,
    method: Method,
    path: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[cfg(test)]
fn code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap()
}

#[tokio::test]
async fn put_with_the_cookie_grants_integrator_as_that_human() {
    let fixture = Fixture::new().await;
    let (status, body) = call(
        fixture.http(),
        Method::PUT,
        PATH,
        &[("cookie", "theme=dark; fold_human=tanim")],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({"agent": "review", "tags": ["integrator", "review"]})
    );
    assert_eq!(
        fixture.tag_events().await,
        vec![(
            "agent.tag_added".to_owned(),
            "human:tanim".to_owned(),
            json!({"agent": "review", "tag": "integrator"})
        )]
    );
    let human = registry::find_human(
        fixture.project.registry().read(),
        &HumanName::parse("tanim").unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(human.created_at, fixture.project.now());
}

#[tokio::test]
async fn a_repeated_put_appends_no_second_event_and_keeps_the_human() {
    let fixture = Fixture::new().await;
    let cookie = [("cookie", "fold_human=tanim")];
    call(fixture.http(), Method::PUT, PATH, &cookie).await;
    let first = registry::find_human(
        fixture.project.registry().read(),
        &HumanName::parse("tanim").unwrap(),
    )
    .await
    .unwrap();
    fixture
        .project
        .clock()
        .set(Timestamp::from_millis(fixture.project.now().as_millis() + 5000).unwrap());
    let (status, body) = call(fixture.http(), Method::PUT, PATH, &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tags"], json!(["integrator", "review"]));
    assert_eq!(fixture.tag_events().await.len(), 1);
    let second = registry::find_human(
        fixture.project.registry().read(),
        &HumanName::parse("tanim").unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(second, first);
}

#[tokio::test]
async fn the_socket_takes_the_name_from_the_header() {
    let fixture = Fixture::new().await;
    let (status, body) = call(
        fixture.socket(),
        Method::PUT,
        PATH,
        &[("x-fold-human", "tanim")],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(fixture.tag_events().await[0].1, "human:tanim");
}

#[tokio::test]
async fn a_request_without_a_name_gets_human_name_required() {
    let fixture = Fixture::new().await;
    // Each transport reads only its own source: the header is ignored over HTTP and the cookie over the socket.
    for (router, headers) in [
        (fixture.http(), vec![]),
        (fixture.socket(), vec![]),
        (fixture.http(), vec![("x-fold-human", "tanim")]),
        (fixture.socket(), vec![("cookie", "fold_human=tanim")]),
        (fixture.http(), vec![("cookie", "fold_human=; other=1")]),
        (fixture.socket(), vec![("x-fold-human", "")]),
    ] {
        let (status, body) = call(router, Method::PUT, PATH, &headers).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{headers:?}");
        assert_eq!(code(&body), "human_name_required");
    }
    assert!(fixture.tag_events().await.is_empty());
}

#[tokio::test]
async fn an_invalid_name_gets_invalid_name() {
    let fixture = Fixture::new().await;
    for (router, headers, path) in [
        (fixture.http(), vec![("cookie", "fold_human=Tanim")], PATH),
        (fixture.socket(), vec![("x-fold-human", "a b")], PATH),
        (
            fixture.socket(),
            vec![("x-fold-human", "tanim")],
            "/api/v1/projects/demo/agents/Review/tags/integrator",
        ),
    ] {
        let (status, body) = call(router, Method::PUT, path, &headers).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path} {headers:?}");
        assert_eq!(code(&body), "invalid_name");
    }
    let request = Request::builder()
        .method(Method::PUT)
        .uri(PATH)
        .header("x-fold-human", &b"t\xffm"[..])
        .body(Body::empty())
        .unwrap();
    let response = fixture.socket().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(fixture.tag_events().await.is_empty());
}

#[tokio::test]
async fn an_unknown_agent_or_project_is_not_found() {
    let fixture = Fixture::new().await;
    let cookie = [("cookie", "fold_human=tanim")];
    let (status, body) = call(
        fixture.http(),
        Method::PUT,
        "/api/v1/projects/demo/agents/ghost/tags/integrator",
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "agent_not_found");
    assert_eq!(body["error"]["details"]["agent"], "ghost");
    let (status, body) = call(
        fixture.http(),
        Method::DELETE,
        "/api/v1/projects/other/agents/review/tags/integrator",
        &cookie,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "project_not_found");
}

#[tokio::test]
async fn delete_removes_the_tag_once() {
    let fixture = Fixture::new().await;
    let header = [("x-fold-human", "tanim")];
    call(fixture.socket(), Method::PUT, PATH, &header).await;
    let (status, body) = call(fixture.socket(), Method::DELETE, PATH, &header).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"agent": "review", "tags": ["review"]}));
    let (status, _) = call(fixture.socket(), Method::DELETE, PATH, &header).await;
    assert_eq!(status, StatusCode::OK);
    let types: Vec<String> = fixture
        .tag_events()
        .await
        .into_iter()
        .map(|(event_type, actor, _)| {
            assert_eq!(actor, "human:tanim");
            event_type
        })
        .collect();
    assert_eq!(types, ["agent.tag_added", "agent.tag_removed"]);
}

#[tokio::test]
async fn both_routes_declare_their_errors() {
    let doc = serde_json::to_value(fold_daemon::http::openapi()).unwrap();
    let item = &doc["paths"]["/api/v1/projects/{slug}/agents/{name}/tags/integrator"];
    for (method, operation_id) in [("put", "grant_integrator"), ("delete", "revoke_integrator")] {
        let operation = &item[method];
        assert_eq!(operation["operationId"], operation_id);
        for status in ["200", "400", "401", "403", "404"] {
            assert!(
                operation["responses"][status].is_object(),
                "{method} {status}"
            );
        }
    }
    assert!(fold_daemon::http::openapi::check_operations(&fold_daemon::http::openapi()).is_ok());
}
