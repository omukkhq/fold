use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use fold_core::{AgentName, Clock, EventFilter, HumanName, ManualClock, Tag};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::{openapi, router, socket_router};
use fold_daemon::projects::Projects;
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::projects::{
    DEFAULT_SESSION_LIFETIME, create_web_token, list_web_tokens, revoke_web_token,
};
use fold_daemon::{Daemon, DaemonConfig};
use fold_store::events;
use fold_store::projects::{self as registry, ProjectRow};
use fold_store::testing::TestProject;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tower::ServiceExt;

const SESSION: &str = "/api/v1/session";
const INTEGRATOR: &str = "/api/v1/projects/demo/agents/review/tags/integrator";
const THIRTY_DAYS: &str = "Max-Age=2592000";
const ONE_YEAR: &str = "Max-Age=31536000";

#[cfg(test)]
struct Fixture {
    project: TestProject,
    _web: TempDir,
    state: AppState,
    store: fold_store::Store,
}

#[cfg(test)]
impl Fixture {
    /// A daemon state with the project `demo`, its agent `review` and a web folder, listening off loopback when `auth_required`.
    async fn new(auth_required: bool) -> Self {
        let project = TestProject::new().await;
        let web = TempDir::new().unwrap();
        std::fs::write(web.path().join("index.html"), "<html>fold</html>").unwrap();
        std::fs::create_dir_all(web.path().join("assets")).unwrap();
        std::fs::write(web.path().join("assets").join("app.js"), "1").unwrap();
        let clock = Arc::new(project.clock().clone());
        let state: AppState = Arc::new(DaemonState {
            registry: project.registry().clone(),
            projects: Projects::default(),
            clock: clock.clone(),
            assets: AssetSource::Dir(web.path().to_owned()),
            listen: if auth_required {
                "0.0.0.0".parse().unwrap()
            } else {
                "127.0.0.1".parse().unwrap()
            },
            session_lifetime: DEFAULT_SESSION_LIFETIME,
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
            store: handle.store.clone(),
            project,
            _web: web,
            state,
        }
    }

    fn http(&self) -> Router {
        router(self.state.clone())
    }

    async fn token(&self, human: &str) -> String {
        create_web_token(
            self.project.registry(),
            &HumanName::parse(human).unwrap(),
            Some("laptop"),
            self.project.clock().now(),
        )
        .await
        .unwrap()
    }

    /// Signs `human` in with a new token and returns the `Cookie` header value of the session.
    async fn sign_in(&self, human: &str) -> String {
        let token = self.token(human).await;
        sign_in_with(self.http(), &token).await
    }

    /// The actor of every stored `agent.tag_*` event.
    async fn tag_actors(&self) -> Vec<String> {
        events::read_after(self.store.read(), 0, &EventFilter::default(), 1000)
            .await
            .unwrap()
            .into_iter()
            .filter(|e| e.event_type.as_str().starts_with("agent.tag"))
            .map(|e| e.actor.to_string())
            .collect()
    }
}

#[cfg(test)]
struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

#[cfg(test)]
impl Answer {
    fn code(&self) -> &str {
        self.body["error"]["code"].as_str().unwrap_or("")
    }

    fn cookies(&self) -> Vec<String> {
        self.headers
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap().to_owned())
            .collect()
    }
}

#[cfg(test)]
async fn send(
    router: Router,
    method: Method,
    path: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> Answer {
    send_with(router, method, path, cookie, &[], body).await
}

#[cfg(test)]
async fn send_with(
    router: Router,
    method: Method,
    path: &str,
    cookie: Option<&str>,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> Answer {
    let mut request = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    let body = match body {
        Some(body) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = router.oneshot(request.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    // An event stream's body never ends, so only its status and headers are read.
    if headers.get(header::CONTENT_TYPE)
        == Some(&header::HeaderValue::from_static("text/event-stream"))
    {
        return Answer {
            status,
            headers,
            body: Value::Null,
        };
    }
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Answer {
        status,
        headers,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

/// Posts `token` to the session route and returns the `fold_session` pair of the `Set-Cookie` header, ready for a `Cookie` header.
#[cfg(test)]
async fn sign_in_with(router: Router, token: &str) -> String {
    let answer = send(
        router,
        Method::POST,
        SESSION,
        None,
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{:?}", answer.body);
    session_pair(&answer.cookies()[0])
}

#[cfg(test)]
fn session_pair(set_cookie: &str) -> String {
    let pair = set_cookie.split(';').next().unwrap().to_owned();
    assert!(pair.starts_with("fold_session=") && pair.len() > "fold_session=".len());
    pair
}

/// Every operation of the OpenAPI document as `(method, path)`, path parameters filled with the fixture's project and agent.
#[cfg(test)]
fn operations() -> Vec<(Method, String)> {
    let doc = serde_json::to_value(openapi()).unwrap();
    let mut found = Vec::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        let path = path.replace("{slug}", "demo").replace("{name}", "review");
        for method in item.as_object().unwrap().keys() {
            found.push((
                Method::from_bytes(method.to_uppercase().as_bytes()).unwrap(),
                path.clone(),
            ));
        }
    }
    found
}

/// Walks the OpenAPI document, so an operation that is declared but not served on the TCP router answers the fallback's `not_found` and fails here.
#[tokio::test]
async fn off_loopback_every_guarded_route_needs_the_cookie_and_accepts_a_valid_one() {
    let fixture = Fixture::new(true).await;
    let cookie = fixture.sign_in("ana").await;
    let guarded: Vec<_> = operations()
        .into_iter()
        .filter(|(_, path)| path != SESSION)
        .collect();
    assert!(guarded.len() >= 4, "{guarded:?}");
    for (method, path) in guarded {
        let answer = send(fixture.http(), method.clone(), &path, None, None).await;
        assert_eq!(
            (answer.status, answer.code()),
            (StatusCode::UNAUTHORIZED, "auth_required"),
            "{method} {path} is in the OpenAPI document but not served behind the session check"
        );
        let answer = send(fixture.http(), method.clone(), &path, Some(&cookie), None).await;
        assert!(
            !matches!(answer.code(), "auth_required" | "human_name_required"),
            "{method} {path}: {:?}",
            answer.body
        );
    }
    let answer = send(
        fixture.http(),
        Method::GET,
        "/api/v1/projects",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK);
}

#[tokio::test]
async fn off_loopback_the_session_routes_work_without_the_cookie() {
    let fixture = Fixture::new(true).await;
    let token = fixture.token("ana").await;

    let answer = send(fixture.http(), Method::GET, SESSION, None, None).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.body, json!({"human": null, "authenticated": false}));

    let answer = send(
        fixture.http(),
        Method::POST,
        SESSION,
        None,
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    let cookies = answer.cookies();
    assert_eq!(cookies.len(), 1);
    let cookie = session_pair(&cookies[0]);
    assert_eq!(
        cookies[0],
        format!("{cookie}; HttpOnly; SameSite=Strict; Path=/; {THIRTY_DAYS}")
    );
    assert!(!cookies[0].contains("Secure"));
    assert!(!cookies[0].contains(&token), "the cookie carries the token");
    let session = cookie.strip_prefix("fold_session=").unwrap();
    assert_eq!(session.len(), 43);
    assert_ne!(sign_in_with(fixture.http(), &token).await, cookie);

    let answer = send(fixture.http(), Method::GET, SESSION, Some(&cookie), None).await;
    assert_eq!(answer.body, json!({"human": "ana", "authenticated": true}));
    let as_token = format!("fold_session={token}");
    let answer = send(
        fixture.http(),
        Method::GET,
        "/api/v1/projects",
        Some(&as_token),
        None,
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::UNAUTHORIZED, "auth_required"),
        "a token in place of a session id must not sign in"
    );

    let answer = send(fixture.http(), Method::DELETE, SESSION, None, None).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert_eq!(
        answer.cookies(),
        vec![
            "fold_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0".to_owned(),
            "fold_human=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0".to_owned(),
        ]
    );
}

#[tokio::test]
async fn off_loopback_a_wrong_token_is_invalid_token_and_a_wrong_session_auth_required() {
    let fixture = Fixture::new(true).await;
    fixture.sign_in("ana").await;
    let wrong = "fold_session=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let answer = send(
        fixture.http(),
        Method::GET,
        "/api/v1/projects",
        Some(wrong),
        None,
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::UNAUTHORIZED, "auth_required")
    );
    assert!(!answer.body.to_string().contains("AAAAAAAA"));
    let answer = send(
        fixture.http(),
        Method::POST,
        SESSION,
        None,
        Some(json!({"token": "nope"})),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::UNAUTHORIZED, "invalid_token")
    );
    assert!(answer.cookies().is_empty());
    let answer = send(fixture.http(), Method::GET, SESSION, Some(wrong), None).await;
    assert_eq!(answer.body, json!({"human": null, "authenticated": false}));
}

#[tokio::test]
async fn off_loopback_sign_out_ends_the_session_on_the_server() {
    let fixture = Fixture::new(true).await;
    let cookie = fixture.sign_in("ana").await;
    let other = fixture.sign_in("ana").await;
    let projects = "/api/v1/projects";
    let answer = send(fixture.http(), Method::GET, projects, Some(&cookie), None).await;
    assert_eq!(answer.status, StatusCode::OK);

    let answer = send(fixture.http(), Method::DELETE, SESSION, Some(&cookie), None).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    let answer = send(fixture.http(), Method::GET, projects, Some(&cookie), None).await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::UNAUTHORIZED, "auth_required")
    );
    let answer = send(fixture.http(), Method::GET, SESSION, Some(&cookie), None).await;
    assert_eq!(answer.body, json!({"human": null, "authenticated": false}));
    let answer = send(fixture.http(), Method::DELETE, SESSION, Some(&cookie), None).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);

    let answer = send(fixture.http(), Method::GET, projects, Some(&other), None).await;
    assert_eq!(
        answer.status,
        StatusCode::OK,
        "sign-out ended another session"
    );
}

#[tokio::test]
async fn off_loopback_a_revoked_token_cannot_sign_in_and_its_sessions_end() {
    let fixture = Fixture::new(true).await;
    let token = fixture.token("ana").await;
    let cookie = sign_in_with(fixture.http(), &token).await;
    let kept = fixture.sign_in("ana").await;
    let projects = "/api/v1/projects";
    let registry = fixture.project.registry();
    let listed = list_web_tokens(registry).await.unwrap();
    assert_eq!(listed.len(), 2);
    revoke_web_token(registry, listed[0].id, fixture.project.clock().now())
        .await
        .unwrap();

    let answer = send(fixture.http(), Method::GET, projects, Some(&cookie), None).await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::UNAUTHORIZED, "auth_required")
    );
    let answer = send(
        fixture.http(),
        Method::POST,
        SESSION,
        None,
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::UNAUTHORIZED, "invalid_token")
    );
    assert!(answer.cookies().is_empty());
    let answer = send(fixture.http(), Method::GET, projects, Some(&kept), None).await;
    assert_eq!(
        answer.status,
        StatusCode::OK,
        "revoking one token ended another"
    );
}

#[tokio::test]
async fn off_loopback_a_session_expires_after_its_lifetime() {
    let fixture = Fixture::new(true).await;
    let cookie = fixture.sign_in("ana").await;
    let projects = "/api/v1/projects";
    let clock = fixture.project.clock();
    clock
        .advance(DEFAULT_SESSION_LIFETIME - Duration::from_millis(1))
        .unwrap();
    let answer = send(fixture.http(), Method::GET, projects, Some(&cookie), None).await;
    assert_eq!(answer.status, StatusCode::OK);
    clock.advance(Duration::from_millis(1)).unwrap();
    let answer = send(fixture.http(), Method::GET, projects, Some(&cookie), None).await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::UNAUTHORIZED, "auth_required")
    );
    let answer = send(fixture.http(), Method::GET, SESSION, Some(&cookie), None).await;
    assert_eq!(answer.body, json!({"human": null, "authenticated": false}));

    let fresh = fixture.sign_in("ana").await;
    let answer = send(fixture.http(), Method::GET, projects, Some(&fresh), None).await;
    assert_eq!(answer.status, StatusCode::OK);
}

#[tokio::test]
async fn the_cookie_lasts_as_long_as_the_configured_lifetime() {
    let project = TestProject::new().await;
    let state: AppState = Arc::new(DaemonState {
        registry: project.registry().clone(),
        projects: Projects::default(),
        clock: Arc::new(project.clock().clone()),
        assets: AssetSource::Embedded,
        listen: "0.0.0.0".parse().unwrap(),
        session_lifetime: Duration::from_secs(7 * 24 * 60 * 60),
    });
    let token = create_web_token(
        project.registry(),
        &HumanName::parse("ana").unwrap(),
        None,
        project.clock().now(),
    )
    .await
    .unwrap();
    let answer = send(
        router(state.clone()),
        Method::POST,
        SESSION,
        None,
        Some(json!({"token": token})),
    )
    .await;
    assert!(answer.cookies()[0].ends_with("Max-Age=604800"));
    let cookie = session_pair(&answer.cookies()[0]);
    project
        .clock()
        .advance(Duration::from_secs(7 * 24 * 60 * 60))
        .unwrap();
    let answer = send(
        router(state),
        Method::GET,
        "/api/v1/projects",
        Some(&cookie),
        None,
    )
    .await;
    assert_eq!(answer.code(), "auth_required");
}

#[tokio::test]
async fn off_loopback_a_socket_request_passes_without_a_cookie() {
    let fixture = Fixture::new(true).await;
    let socket = socket_router(fixture.state.clone());
    let answer = send(socket.clone(), Method::GET, "/api/v1/projects", None, None).await;
    assert_eq!(answer.status, StatusCode::OK);
    let request = Request::builder()
        .method(Method::GET)
        .uri(SESSION)
        .header("x-fold-human", "tanim")
        .body(Body::empty())
        .unwrap();
    let response = socket.oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body, json!({"human": "tanim", "authenticated": true}));
}

#[tokio::test]
async fn off_loopback_the_web_files_load_without_a_cookie() {
    let fixture = Fixture::new(true).await;
    for path in ["/", "/assets/app.js", "/board"] {
        let answer = send(fixture.http(), Method::GET, path, None, None).await;
        assert_eq!(answer.status, StatusCode::OK, "{path}");
    }
    let answer = send(fixture.http(), Method::GET, "/api/v1/projects", None, None).await;
    assert_eq!(answer.code(), "auth_required");
}

#[tokio::test]
async fn off_loopback_a_write_acts_as_the_session_human_and_ignores_fold_human() {
    let fixture = Fixture::new(true).await;
    let session = fixture.sign_in("ana").await;
    let answer = send(
        fixture.http(),
        Method::PUT,
        INTEGRATOR,
        Some("fold_human=tanim"),
        None,
    )
    .await;
    assert_eq!(answer.code(), "auth_required");
    let cookie = format!("fold_human=tanim; {session}");
    let answer = send(fixture.http(), Method::PUT, INTEGRATOR, Some(&cookie), None).await;
    assert_eq!(answer.status, StatusCode::OK, "{:?}", answer.body);
    assert_eq!(fixture.tag_actors().await, vec!["human:ana".to_owned()]);
}

#[tokio::test]
async fn off_loopback_a_name_is_refused() {
    let fixture = Fixture::new(true).await;
    let answer = send(
        fixture.http(),
        Method::POST,
        SESSION,
        None,
        Some(json!({"name": "tanim"})),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
    assert!(answer.cookies().is_empty());
}

#[tokio::test]
async fn on_loopback_no_route_needs_a_cookie() {
    let fixture = Fixture::new(false).await;
    for (method, path) in operations() {
        let answer = send(fixture.http(), method.clone(), &path, None, None).await;
        assert!(
            !matches!(answer.code(), "auth_required" | "invalid_token"),
            "{method} {path}: {:?}",
            answer.body
        );
    }
    let answer = send(fixture.http(), Method::GET, "/api/v1/projects", None, None).await;
    assert_eq!(answer.status, StatusCode::OK);
}

#[tokio::test]
async fn on_loopback_a_name_sets_fold_human_and_a_write_needs_it() {
    let fixture = Fixture::new(false).await;
    let answer = send(fixture.http(), Method::PUT, INTEGRATOR, None, None).await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::UNAUTHORIZED, "human_name_required")
    );

    let answer = send(
        fixture.http(),
        Method::POST,
        SESSION,
        None,
        Some(json!({"name": "tanim"})),
    )
    .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert_eq!(
        answer.cookies(),
        vec![format!(
            "fold_human=tanim; HttpOnly; SameSite=Strict; Path=/; {ONE_YEAR}"
        )]
    );
    let human = registry::find_human(
        fixture.project.registry().read(),
        &HumanName::parse("tanim").unwrap(),
    )
    .await
    .unwrap();
    assert!(human.is_some());

    let answer = send(
        fixture.http(),
        Method::PUT,
        INTEGRATOR,
        Some("fold_human=tanim"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(fixture.tag_actors().await, vec!["human:tanim".to_owned()]);
}

#[tokio::test]
async fn on_loopback_the_session_reports_the_name_and_is_always_authenticated() {
    let fixture = Fixture::new(false).await;
    let answer = send(fixture.http(), Method::GET, SESSION, None, None).await;
    assert_eq!(answer.body, json!({"human": null, "authenticated": true}));
    let answer = send(
        fixture.http(),
        Method::GET,
        SESSION,
        Some("fold_human=tanim"),
        None,
    )
    .await;
    assert_eq!(
        answer.body,
        json!({"human": "tanim", "authenticated": true})
    );
    let answer = send(
        fixture.http(),
        Method::GET,
        SESSION,
        Some("fold_human=Not%20A%20Name"),
        None,
    )
    .await;
    assert_eq!(answer.body, json!({"human": null, "authenticated": true}));

    let answer = send(
        fixture.http(),
        Method::DELETE,
        SESSION,
        Some("fold_human=tanim"),
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    let cookies = answer.cookies();
    assert_eq!(cookies.len(), 2);
    assert!(cookies.iter().all(|c| c.ends_with("Max-Age=0")));
    assert!(cookies[0].starts_with("fold_session=;"));
    assert!(cookies[1].starts_with("fold_human=;"));
}

#[tokio::test]
async fn on_loopback_a_token_or_a_bad_name_is_refused() {
    let fixture = Fixture::new(false).await;
    let token = fixture.token("ana").await;
    let answer = send(
        fixture.http(),
        Method::POST,
        SESSION,
        None,
        Some(json!({"token": token})),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
    let answer = send(
        fixture.http(),
        Method::POST,
        SESSION,
        None,
        Some(json!({"name": "Not A Name"})),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::BAD_REQUEST, "invalid_name")
    );
    let answer = send(
        fixture.http(),
        Method::POST,
        SESSION,
        None,
        Some(json!({"name": "tanim", "token": token})),
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::BAD_REQUEST, "invalid_request")
    );
}

#[cfg(test)]
async fn raw_get(addr: std::net::SocketAddr, path: &str, cookie: Option<&str>) -> (u16, Value) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let cookie = cookie.map_or_else(String::new, |c| format!("cookie: {c}\r\n"));
    let request =
        format!("GET {path} HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n{cookie}\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let text = String::from_utf8(response).unwrap();
    let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = text.split_once("\r\n\r\n").unwrap().1;
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

/// Signs in over a real TCP connection and returns the `fold_session` pair of the answer.
#[cfg(test)]
async fn raw_sign_in(addr: std::net::SocketAddr, token: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let body = json!({"token": token}).to_string();
    let request = format!(
        "POST {SESSION} HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    let text = String::from_utf8(response).unwrap();
    assert!(text.starts_with("HTTP/1.1 204"), "{text}");
    let set_cookie = text
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
                .map(|(_, value)| value.trim().to_owned())
        })
        .unwrap();
    session_pair(&set_cookie)
}

#[tokio::test]
async fn a_daemon_on_every_address_needs_a_token_even_from_127_0_0_1() {
    let dir = TempDir::new().unwrap();
    let config = DaemonConfig {
        listen: "0.0.0.0:0".parse().unwrap(),
        socket: dir.path().join("run").join("fold.sock"),
        data_dir: dir.path().join("data"),
        runtime_dir: dir.path().join("run"),
        web_dir: None,
        session_lifetime: DEFAULT_SESSION_LIFETIME,
    };
    let clock = ManualClock::new();
    let daemon = Daemon::start(config, Arc::new(clock.clone()))
        .await
        .unwrap();
    let local = std::net::SocketAddr::from(([127, 0, 0, 1], daemon.http_addr().port()));
    let (status, body) = raw_get(local, SESSION, None).await;
    assert_eq!(
        (status, body),
        (200, json!({"human": null, "authenticated": false}))
    );
    let (status, body) = raw_get(local, "/api/v1/projects", Some("fold_human=tanim")).await;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (401, json!("auth_required"))
    );

    let token = create_web_token(
        &daemon.state().registry,
        &HumanName::parse("ana").unwrap(),
        None,
        clock.now(),
    )
    .await
    .unwrap();
    let cookie = raw_sign_in(local, &token).await;
    let (status, body) = raw_get(local, SESSION, Some(&cookie)).await;
    assert_eq!(
        (status, body),
        (200, json!({"human": "ana", "authenticated": true}))
    );
    let (status, _) = raw_get(local, "/api/v1/projects", Some(&cookie)).await;
    assert_eq!(status, 200);
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_foreign_host_or_origin_cannot_reach_the_api() {
    let fixture = Fixture::new(false).await;
    let human = Some("fold_human=tanim");
    let evil_host = [("host", "evil.example:7420")];
    let answer = send_with(
        fixture.http(),
        Method::PUT,
        INTEGRATOR,
        human,
        &evil_host,
        None,
    )
    .await;
    assert_eq!(
        (answer.status, answer.code()),
        (StatusCode::FORBIDDEN, "host_not_allowed")
    );
    let evil_origin = [
        ("host", "127.0.0.1:7420"),
        ("origin", "http://evil.example"),
    ];
    let answer = send_with(
        fixture.http(),
        Method::PUT,
        INTEGRATOR,
        human,
        &evil_origin,
        None,
    )
    .await;
    assert_eq!(answer.code(), "host_not_allowed");
    let answer = send_with(
        fixture.http(),
        Method::POST,
        SESSION,
        None,
        &evil_host,
        Some(json!({"name": "mallory"})),
    )
    .await;
    assert_eq!(answer.code(), "host_not_allowed");
    assert!(answer.cookies().is_empty());
    assert!(fixture.tag_actors().await.is_empty());

    let answer = send_with(fixture.http(), Method::GET, "/", None, &evil_host, None).await;
    assert_eq!(answer.status, StatusCode::OK);
    let socket = socket_router(fixture.state.clone());
    let answer = send_with(
        socket,
        Method::GET,
        "/api/v1/projects",
        None,
        &evil_host,
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK);

    let local = [
        ("host", "localhost:5173"),
        ("origin", "http://localhost:5173"),
    ];
    let answer = send_with(fixture.http(), Method::PUT, INTEGRATOR, human, &local, None).await;
    assert_eq!(answer.status, StatusCode::OK, "{:?}", answer.body);
    assert_eq!(fixture.tag_actors().await, vec!["human:tanim".to_owned()]);
}

#[tokio::test]
async fn off_loopback_an_ip_host_passes_and_a_domain_name_is_refused() {
    let fixture = Fixture::new(true).await;
    let cookie = fixture.sign_in("ana").await;
    let answer = send_with(
        fixture.http(),
        Method::PUT,
        INTEGRATOR,
        Some(&cookie),
        &[
            ("host", "192.168.1.5:7420"),
            ("origin", "http://192.168.1.5:7420"),
        ],
        None,
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK, "{:?}", answer.body);
    let answer = send_with(
        fixture.http(),
        Method::PUT,
        INTEGRATOR,
        Some(&cookie),
        &[("host", "evil.example:7420")],
        None,
    )
    .await;
    assert_eq!(answer.code(), "host_not_allowed");
}

#[tokio::test]
async fn the_humans_route_lists_the_registered_names_in_order() {
    let fixture = Fixture::new(false).await;
    let answer = send(fixture.http(), Method::GET, "/api/v1/humans", None, None).await;
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(answer.body, json!({"humans": []}));

    for name in ["tanim", "ana"] {
        let answer = send(
            fixture.http(),
            Method::POST,
            SESSION,
            None,
            Some(json!({ "name": name })),
        )
        .await;
        assert_eq!(answer.status, StatusCode::NO_CONTENT);
    }
    let answer = send(fixture.http(), Method::GET, "/api/v1/humans", None, None).await;
    assert_eq!(answer.body, json!({"humans": ["ana", "tanim"]}));
}
