use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use fold_core::{Clock, HumanName, Timestamp};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::router;
use fold_daemon::projects::Projects;
use fold_daemon::services::projects::{check_web_token, create_web_token};
use fold_store::Store;
use fold_store::testing::TestProject;
use sqlx::Row;
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;

#[cfg(test)]
fn name(text: &str) -> HumanName {
    HumanName::parse(text).unwrap()
}

#[cfg(test)]
async fn last_used(registry: &Store) -> Option<i64> {
    sqlx::query("SELECT last_used_at FROM web_tokens")
        .fetch_one(registry.read())
        .await
        .unwrap()
        .get(0)
}

#[tokio::test]
async fn the_plaintext_is_returned_once_and_the_registry_holds_only_its_hash() {
    let project = TestProject::new().await;
    let registry = project.registry();
    let first = create_web_token(registry, &name("ana"), Some("laptop"), project.now())
        .await
        .unwrap();
    let second = create_web_token(registry, &name("ana"), None, project.now())
        .await
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(first.len(), 43);
    assert!(
        first
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );

    let rows = sqlx::query(
        "SELECT t.token_hash, t.label, t.last_used_at, h.name FROM web_tokens t JOIN humans h ON h.id = t.human_id ORDER BY t.id",
    )
    .fetch_all(registry.read())
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    let hash: Vec<u8> = rows[0].get("token_hash");
    assert_eq!(hash, blake3::hash(first.as_bytes()).as_bytes().to_vec());
    assert_eq!(
        rows[0].get::<Option<String>, _>("label").as_deref(),
        Some("laptop")
    );
    assert_eq!(rows[0].get::<Option<i64>, _>("last_used_at"), None);
    assert_eq!(rows[1].get::<String, _>("name"), "ana");

    let dump: Vec<String> =
        sqlx::query("SELECT quote(token_hash) || ' ' || coalesce(label, '') FROM web_tokens")
            .fetch_all(registry.read())
            .await
            .unwrap()
            .iter()
            .map(|row| row.get(0))
            .collect();
    for line in dump {
        assert!(!line.contains(&first) && !line.contains(&second), "{line}");
    }
}

#[tokio::test]
async fn a_token_signs_in_its_human_and_an_unknown_one_signs_in_nobody() {
    let project = TestProject::new().await;
    let registry = project.registry();
    let token = create_web_token(registry, &name("ana"), None, project.now())
        .await
        .unwrap();
    let clock = project.clock();
    assert_eq!(
        check_web_token(registry, clock, &token).await.unwrap(),
        Some(name("ana"))
    );
    assert_eq!(
        check_web_token(registry, clock, "nope").await.unwrap(),
        None
    );
    assert_eq!(
        check_web_token(registry, clock, &token[1..]).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn last_used_at_moves_at_most_once_a_minute() {
    let project = TestProject::new().await;
    let registry = project.registry();
    let clock = project.clock();
    let token = create_web_token(registry, &name("ana"), None, clock.now())
        .await
        .unwrap();
    assert_eq!(last_used(registry).await, None);

    check_web_token(registry, clock, &token).await.unwrap();
    let first = clock.now().as_millis();
    assert_eq!(last_used(registry).await, Some(first));

    clock.advance(Duration::from_secs(59)).unwrap();
    check_web_token(registry, clock, &token).await.unwrap();
    assert_eq!(last_used(registry).await, Some(first));

    let minute = clock.advance(Duration::from_secs(1)).unwrap();
    check_web_token(registry, clock, &token).await.unwrap();
    assert_eq!(last_used(registry).await, Some(minute.as_millis()));

    clock.advance(Duration::from_secs(30)).unwrap();
    check_web_token(registry, clock, &token).await.unwrap();
    assert_eq!(last_used(registry).await, Some(minute.as_millis()));
    assert!(Timestamp::from_millis(first).unwrap() < minute);
}

#[cfg(test)]
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

#[cfg(test)]
impl Captured {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

#[cfg(test)]
impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
impl<'a> MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn no_log_line_carries_a_token() {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(captured.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let project = TestProject::new().await;
    let state: AppState = Arc::new(DaemonState {
        registry: project.registry().clone(),
        projects: Projects::default(),
        clock: Arc::new(project.clock().clone()),
        assets: AssetSource::Embedded,
        listen: "0.0.0.0".parse().unwrap(),
    });
    let token = create_web_token(
        project.registry(),
        &name("ana"),
        Some("laptop"),
        project.now(),
    )
    .await
    .unwrap();
    let wrong = "WrongTokenWrongTokenWrongTokenWrongToken123";
    for (method, path, cookie, body) in [
        (
            Method::POST,
            "/api/v1/session",
            None,
            Some(format!("{{\"token\":\"{token}\"}}")),
        ),
        (
            Method::POST,
            "/api/v1/session",
            None,
            Some(format!("{{\"token\":\"{wrong}\"}}")),
        ),
        (
            Method::GET,
            "/api/v1/projects",
            Some(format!("fold_session={token}")),
            None,
        ),
        (
            Method::GET,
            "/api/v1/projects",
            Some(format!("fold_session={wrong}")),
            None,
        ),
        (
            Method::GET,
            "/api/v1/session",
            Some(format!("fold_session={wrong}")),
            None,
        ),
    ] {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        let body = match body {
            Some(body) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(body)
            }
            None => Body::empty(),
        };
        let response = router(state.clone())
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        assert_ne!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    let log = captured.text();
    assert!(log.contains("sign-in refused"), "{log}");
    assert!(log.contains("/api/v1/session"), "{log}");
    assert!(!log.contains(&token), "{log}");
    assert!(!log.contains(wrong), "{log}");
}
