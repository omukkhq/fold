use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use fold_core::{Clock, HumanName, Timestamp, WebTokenId};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::router;
use fold_daemon::projects::Projects;
use fold_daemon::services::projects::{
    DEFAULT_SESSION_LIFETIME, check_web_session, create_web_session, create_web_token,
    end_web_session, list_web_tokens, revoke_web_token,
};
use fold_store::Store;
use fold_store::testing::TestProject;
use http_body_util::BodyExt;
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

#[cfg(test)]
async fn session_times(registry: &Store) -> Vec<(i64, i64, i64)> {
    sqlx::query("SELECT created_at, last_used_at, expires_at FROM web_sessions ORDER BY id")
        .fetch_all(registry.read())
        .await
        .unwrap()
        .iter()
        .map(|row| (row.get(0), row.get(1), row.get(2)))
        .collect()
}

#[tokio::test]
async fn a_token_opens_a_session_and_the_registry_holds_only_the_session_hash() {
    let project = TestProject::new().await;
    let registry = project.registry();
    let clock = project.clock();
    let token = create_web_token(registry, &name("ana"), None, project.now())
        .await
        .unwrap();
    let signed = create_web_session(registry, clock, DEFAULT_SESSION_LIFETIME, &token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(signed.human, name("ana"));
    assert_ne!(signed.session, token);
    assert_eq!(signed.session.len(), 43);
    for wrong in ["nope", &token[1..], &signed.session] {
        assert_eq!(
            create_web_session(registry, clock, DEFAULT_SESSION_LIFETIME, wrong)
                .await
                .unwrap(),
            None
        );
    }

    let row = sqlx::query(
        "SELECT s.session_hash, h.name, t.token_hash FROM web_sessions s JOIN humans h ON h.id = s.human_id JOIN web_tokens t ON t.id = s.token_id",
    )
    .fetch_one(registry.read())
    .await
    .unwrap();
    assert_eq!(
        row.get::<Vec<u8>, _>(0),
        blake3::hash(signed.session.as_bytes()).as_bytes().to_vec()
    );
    assert_eq!(row.get::<String, _>(1), "ana");
    assert_eq!(
        row.get::<Vec<u8>, _>(2),
        blake3::hash(token.as_bytes()).as_bytes().to_vec()
    );
    let now = clock.now().as_millis();
    let lifetime = i64::try_from(DEFAULT_SESSION_LIFETIME.as_millis()).unwrap();
    assert_eq!(
        session_times(registry).await,
        vec![(now, now, now + lifetime)]
    );
    assert_eq!(last_used(registry).await, Some(now));

    assert_eq!(
        check_web_session(registry, clock, &signed.session)
            .await
            .unwrap(),
        Some(name("ana"))
    );
    assert_eq!(
        check_web_session(registry, clock, &token).await.unwrap(),
        None
    );
    assert_eq!(
        check_web_session(registry, clock, &signed.session[1..])
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn each_use_moves_last_used_at_at_most_once_a_minute() {
    let project = TestProject::new().await;
    let registry = project.registry();
    let clock = project.clock();
    let token = create_web_token(registry, &name("ana"), None, clock.now())
        .await
        .unwrap();
    assert_eq!(last_used(registry).await, None);
    let signed = create_web_session(registry, clock, DEFAULT_SESSION_LIFETIME, &token)
        .await
        .unwrap()
        .unwrap();
    let first = clock.now().as_millis();
    let session_used = async || session_times(registry).await[0].1;
    assert_eq!(session_used().await, first);

    clock.advance(Duration::from_secs(59)).unwrap();
    check_web_session(registry, clock, &signed.session)
        .await
        .unwrap();
    assert_eq!(session_used().await, first);
    assert_eq!(last_used(registry).await, Some(first));

    let minute = clock.advance(Duration::from_secs(1)).unwrap();
    check_web_session(registry, clock, &signed.session)
        .await
        .unwrap();
    assert_eq!(session_used().await, minute.as_millis());
    assert_eq!(last_used(registry).await, Some(minute.as_millis()));

    clock.advance(Duration::from_secs(30)).unwrap();
    check_web_session(registry, clock, &signed.session)
        .await
        .unwrap();
    assert_eq!(session_used().await, minute.as_millis());
    assert!(Timestamp::from_millis(first).unwrap() < minute);
}

#[tokio::test]
async fn an_expired_session_signs_in_nobody_and_the_next_sign_in_deletes_it() {
    let project = TestProject::new().await;
    let registry = project.registry();
    let clock = project.clock();
    let token = create_web_token(registry, &name("ana"), None, clock.now())
        .await
        .unwrap();
    let lifetime = Duration::from_secs(3600);
    let old = create_web_session(registry, clock, lifetime, &token)
        .await
        .unwrap()
        .unwrap();
    clock.advance(lifetime).unwrap();
    assert_eq!(
        check_web_session(registry, clock, &old.session)
            .await
            .unwrap(),
        None
    );
    assert_eq!(session_times(registry).await.len(), 1);
    let fresh = create_web_session(registry, clock, lifetime, &token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(session_times(registry).await.len(), 1);
    assert_eq!(
        check_web_session(registry, clock, &fresh.session)
            .await
            .unwrap(),
        Some(name("ana"))
    );
}

#[tokio::test]
async fn ending_a_session_deletes_only_that_session() {
    let project = TestProject::new().await;
    let registry = project.registry();
    let clock = project.clock();
    let token = create_web_token(registry, &name("ana"), None, clock.now())
        .await
        .unwrap();
    let first = create_web_session(registry, clock, DEFAULT_SESSION_LIFETIME, &token)
        .await
        .unwrap()
        .unwrap();
    let second = create_web_session(registry, clock, DEFAULT_SESSION_LIFETIME, &token)
        .await
        .unwrap()
        .unwrap();
    assert!(end_web_session(registry, &first.session).await.unwrap());
    assert!(!end_web_session(registry, &first.session).await.unwrap());
    assert!(!end_web_session(registry, &token).await.unwrap());
    assert_eq!(
        check_web_session(registry, clock, &first.session)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        check_web_session(registry, clock, &second.session)
            .await
            .unwrap(),
        Some(name("ana"))
    );
}

#[tokio::test]
async fn revoking_a_token_ends_its_sessions_and_keeps_its_id_taken() {
    let project = TestProject::new().await;
    let registry = project.registry();
    let clock = project.clock();
    let ana = create_web_token(registry, &name("ana"), Some("laptop"), clock.now())
        .await
        .unwrap();
    let bo = create_web_token(registry, &name("bo"), None, clock.now())
        .await
        .unwrap();
    let ana_session = create_web_session(registry, clock, DEFAULT_SESSION_LIFETIME, &ana)
        .await
        .unwrap()
        .unwrap();
    let bo_session = create_web_session(registry, clock, DEFAULT_SESSION_LIFETIME, &bo)
        .await
        .unwrap()
        .unwrap();

    let listed = list_web_tokens(registry).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id.to_string(), "wt-1");
    assert_eq!(listed[0].human, name("ana"));
    assert_eq!(listed[0].label.as_deref(), Some("laptop"));
    assert_eq!(listed[0].created_at, project.now());
    assert_eq!(listed[0].last_used_at, Some(clock.now()));
    assert_eq!(listed[1].id.to_string(), "wt-2");
    assert_eq!(listed[1].label, None);

    let ana_id: WebTokenId = "wt-1".parse().unwrap();
    revoke_web_token(registry, ana_id, clock.now())
        .await
        .unwrap();
    let again = revoke_web_token(registry, ana_id, clock.now())
        .await
        .unwrap_err();
    assert_eq!(again.code(), Some("token_not_found"));
    let unknown = revoke_web_token(registry, "wt-9".parse().unwrap(), clock.now())
        .await
        .unwrap_err();
    assert_eq!(unknown.code(), Some("token_not_found"));

    assert_eq!(
        check_web_session(registry, clock, &ana_session.session)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        create_web_session(registry, clock, DEFAULT_SESSION_LIFETIME, &ana)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        check_web_session(registry, clock, &bo_session.session)
            .await
            .unwrap(),
        Some(name("bo"))
    );
    let sessions: i64 = sqlx::query("SELECT COUNT(*) FROM web_sessions")
        .fetch_one(registry.read())
        .await
        .unwrap()
        .get(0);
    assert_eq!(sessions, 1);
    let listed = list_web_tokens(registry).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].human, name("bo"));

    create_web_token(registry, &name("ana"), None, clock.now())
        .await
        .unwrap();
    let ids: Vec<String> = list_web_tokens(registry)
        .await
        .unwrap()
        .iter()
        .map(|token| token.id.to_string())
        .collect();
    assert_eq!(ids, ["wt-2", "wt-3"]);
}

#[tokio::test]
async fn a_token_id_is_its_number_and_never_its_row_id() {
    let project = TestProject::new().await;
    let registry = project.registry();
    let clock = project.clock();
    let ana = create_web_token(registry, &name("ana"), None, clock.now())
        .await
        .unwrap();
    registry
        .write(async |tx| {
            sqlx::query("UPDATE web_tokens SET number = 10")
                .execute(tx.conn())
                .await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    let bo = create_web_token(registry, &name("bo"), None, clock.now())
        .await
        .unwrap();
    let ana_session = create_web_session(registry, clock, DEFAULT_SESSION_LIFETIME, &ana)
        .await
        .unwrap()
        .unwrap();
    let bo_session = create_web_session(registry, clock, DEFAULT_SESSION_LIFETIME, &bo)
        .await
        .unwrap()
        .unwrap();
    let ids: Vec<String> = list_web_tokens(registry)
        .await
        .unwrap()
        .iter()
        .map(|token| token.id.to_string())
        .collect();
    assert_eq!(ids, ["wt-10", "wt-11"]);

    for row_id in ["wt-1", "wt-2"] {
        let refused = revoke_web_token(registry, row_id.parse().unwrap(), clock.now())
            .await
            .unwrap_err();
        assert_eq!(refused.code(), Some("token_not_found"), "{row_id}");
    }
    revoke_web_token(registry, "wt-11".parse().unwrap(), clock.now())
        .await
        .unwrap();
    assert_eq!(
        check_web_session(registry, clock, &bo_session.session)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        check_web_session(registry, clock, &ana_session.session)
            .await
            .unwrap(),
        Some(name("ana"))
    );
    let ids: Vec<String> = list_web_tokens(registry)
        .await
        .unwrap()
        .iter()
        .map(|token| token.id.to_string())
        .collect();
    assert_eq!(ids, ["wt-10"]);
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

#[cfg(test)]
struct Sent {
    status: StatusCode,
    set_cookie: Vec<String>,
    body: String,
}

#[cfg(test)]
async fn call(
    state: &AppState,
    method: Method,
    path: &str,
    cookie: Option<&str>,
    body: Option<String>,
) -> Sent {
    let mut request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(method)
        .uri(path);
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
    let status = response.status();
    let set_cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Sent {
        status,
        set_cookie,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
    }
}

#[tokio::test]
async fn no_log_line_or_error_carries_a_token_or_a_session_id() {
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
        session_lifetime: Duration::from_secs(3600),
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
    let mut answers = Vec::new();
    let sign_in = format!("{{\"token\":\"{token}\"}}");

    let signed = call(
        &state,
        Method::POST,
        "/api/v1/session",
        None,
        Some(sign_in.clone()),
    )
    .await;
    assert_eq!(signed.status, StatusCode::NO_CONTENT);
    let pair = signed.set_cookie[0].split(';').next().unwrap().to_owned();
    let session = pair.strip_prefix("fold_session=").unwrap().to_owned();
    let expiring = call(
        &state,
        Method::POST,
        "/api/v1/session",
        None,
        Some(sign_in.clone()),
    )
    .await;
    let expiring = expiring.set_cookie[0].split(';').next().unwrap().to_owned();
    let wrong_cookie = format!("fold_session={wrong}");
    for (method, path, cookie, body) in [
        (
            Method::POST,
            "/api/v1/session",
            None,
            Some(format!("{{\"token\":\"{wrong}\"}}")),
        ),
        (Method::GET, "/api/v1/projects", Some(pair.as_str()), None),
        (Method::GET, "/api/v1/session", Some(pair.as_str()), None),
        (
            Method::GET,
            "/api/v1/projects",
            Some(wrong_cookie.as_str()),
            None,
        ),
        (
            Method::GET,
            "/api/v1/session",
            Some(wrong_cookie.as_str()),
            None,
        ),
        (
            Method::DELETE,
            "/api/v1/session",
            Some(wrong_cookie.as_str()),
            None,
        ),
        (Method::DELETE, "/api/v1/session", Some(pair.as_str()), None),
        (Method::GET, "/api/v1/projects", Some(pair.as_str()), None),
    ] {
        answers.push(call(&state, method, path, cookie, body).await);
    }
    project.clock().advance(Duration::from_secs(3600)).unwrap();
    answers.push(
        call(
            &state,
            Method::GET,
            "/api/v1/projects",
            Some(&expiring),
            None,
        )
        .await,
    );
    let id = list_web_tokens(project.registry()).await.unwrap()[0].id;
    revoke_web_token(project.registry(), id, project.now())
        .await
        .unwrap();
    answers.push(call(&state, Method::POST, "/api/v1/session", None, Some(sign_in)).await);

    let statuses: Vec<_> = answers.iter().map(|answer| answer.status).collect();
    assert_eq!(
        statuses,
        [
            StatusCode::UNAUTHORIZED,
            StatusCode::OK,
            StatusCode::OK,
            StatusCode::UNAUTHORIZED,
            StatusCode::OK,
            StatusCode::NO_CONTENT,
            StatusCode::NO_CONTENT,
            StatusCode::UNAUTHORIZED,
            StatusCode::UNAUTHORIZED,
            StatusCode::UNAUTHORIZED,
        ]
    );
    let expiring = expiring.strip_prefix("fold_session=").unwrap();
    let plain = [token.as_str(), session.as_str(), expiring, wrong];
    let hashes: Vec<String> = plain
        .iter()
        .map(|secret| blake3::hash(secret.as_bytes()).to_hex().to_string())
        .collect();
    let secrets: Vec<&str> = plain
        .into_iter()
        .chain(hashes.iter().map(String::as_str))
        .collect();
    for answer in &answers {
        for secret in &secrets {
            assert!(!answer.body.contains(secret), "{}", answer.body);
        }
    }

    let log = captured.text();
    assert!(log.contains("sign-in refused"), "{log}");
    assert!(log.contains("/api/v1/session"), "{log}");
    for secret in &secrets {
        assert!(!log.contains(secret), "{log}");
    }
}
