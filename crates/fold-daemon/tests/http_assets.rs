use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use fold_core::ManualClock;
use fold_daemon::http::router;
use fold_daemon::{Daemon, DaemonConfig, RunningDaemon};
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

const INDEX: &str = "<html>fold</html>";

#[cfg(test)]
struct Site {
    dir: TempDir,
    daemon: RunningDaemon,
}

#[cfg(test)]
impl Site {
    /// A daemon that serves `<tmp>/web` through `--web-dir`.
    async fn new(files: &[(&str, &str)]) -> Self {
        let dir = TempDir::new().unwrap();
        let web = dir.path().join("web");
        std::fs::create_dir_all(&web).unwrap();
        for (name, content) in files {
            put(&web, name, content);
        }
        let config = DaemonConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            socket: dir.path().join("run").join("fold.sock"),
            data_dir: dir.path().join("data"),
            runtime_dir: dir.path().join("run"),
            web_dir: Some(web),
        };
        let daemon = Daemon::start(config, Arc::new(ManualClock::new()))
            .await
            .unwrap();
        Self { dir, daemon }
    }

    fn web(&self) -> std::path::PathBuf {
        self.dir.path().join("web")
    }

    async fn get(&self, uri: &str) -> Response {
        self.send(Method::GET, uri, None).await
    }

    async fn send(&self, method: Method, uri: &str, etag: Option<&str>) -> Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(etag) = etag {
            request = request.header(header::IF_NONE_MATCH, etag);
        }
        router(self.daemon.state().clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
}

#[cfg(test)]
fn put(web: &Path, name: &str, content: &str) {
    let path = web.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[cfg(test)]
async fn text(response: Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[cfg(test)]
fn header_of(response: &Response, name: header::HeaderName) -> &str {
    response.headers()[name].to_str().unwrap()
}

#[tokio::test]
async fn a_file_is_served_with_its_type_and_a_hashed_asset_is_immutable() {
    let site = Site::new(&[
        ("index.html", INDEX),
        ("assets/app-1a2b.js", "console.log(1)"),
        ("favicon.svg", "<svg/>"),
    ])
    .await;

    let response = site.get("/assets/app-1a2b.js").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        header_of(&response, header::CONTENT_TYPE),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        header_of(&response, header::CACHE_CONTROL),
        "public, max-age=31536000, immutable"
    );
    assert_eq!(text(response).await, "console.log(1)");

    let response = site.get("/favicon.svg").await;
    assert_eq!(header_of(&response, header::CONTENT_TYPE), "image/svg+xml");
    assert_eq!(header_of(&response, header::CACHE_CONTROL), "no-cache");

    let response = site.get("/index.html").await;
    assert_eq!(
        header_of(&response, header::CONTENT_TYPE),
        "text/html; charset=utf-8"
    );
    assert_eq!(header_of(&response, header::CACHE_CONTROL), "no-cache");
    assert_eq!(text(response).await, INDEX);
}

#[tokio::test]
async fn the_root_and_client_routes_get_index_html() {
    let site = Site::new(&[("index.html", INDEX)]).await;
    for uri in ["/", "/board", "/tasks/T-12?tab=review", "/assets/gone.js"] {
        let response = site.get(uri).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri}");
        assert_eq!(header_of(&response, header::CACHE_CONTROL), "no-cache");
        assert_eq!(text(response).await, INDEX, "{uri}");
    }
    let head = site.send(Method::HEAD, "/board", None).await;
    assert_eq!(head.status(), StatusCode::OK);
    assert!(text(head).await.is_empty());
}

#[tokio::test]
async fn an_unknown_api_path_is_a_json_not_found_never_index_html() {
    let site = Site::new(&[("index.html", INDEX)]).await;
    for uri in ["/api/v1/nothing", "/api/", "/api"] {
        let response = site.get(uri).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        let body: Value = serde_json::from_str(&text(response).await).unwrap();
        assert_eq!(body["error"]["code"], "not_found");
    }
    let response = site.send(Method::POST, "/board", None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_folder_without_index_html_answers_web_not_built_with_a_hint() {
    let site = Site::new(&[]).await;
    for uri in ["/", "/board"] {
        let response = site.get(uri).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body: Value = serde_json::from_str(&text(response).await).unwrap();
        assert_eq!(body["error"]["code"], "web_not_built");
        let hint = body["error"]["details"]["hint"].as_str().unwrap();
        assert!(hint.contains("make web-build") && hint.contains("--web-dir"));
    }
}

#[tokio::test]
async fn if_none_match_answers_304_until_the_file_changes() {
    let site = Site::new(&[("index.html", INDEX), ("assets/app-1.js", "one")]).await;
    let first = site.get("/assets/app-1.js").await;
    let etag = header_of(&first, header::ETAG).to_owned();

    let response = site
        .send(Method::GET, "/assets/app-1.js", Some(&etag))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(header_of(&response, header::ETAG), etag);
    assert!(text(response).await.is_empty());

    let response = site
        .send(Method::GET, "/assets/app-1.js", Some("\"other\""))
        .await;
    assert_eq!(response.status(), StatusCode::OK);

    put(&site.web(), "assets/app-1.js", "changed content");
    let response = site
        .send(Method::GET, "/assets/app-1.js", Some(&etag))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(text(response).await, "changed content");

    let index = site.get("/board").await;
    let etag = header_of(&index, header::ETAG).to_owned();
    let response = site.send(Method::GET, "/board", Some(&etag)).await;
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(header_of(&response, header::CACHE_CONTROL), "no-cache");
}

#[tokio::test]
async fn a_path_that_leaves_the_folder_is_refused() {
    let site = Site::new(&[("index.html", INDEX)]).await;
    std::fs::write(site.dir.path().join("secret.txt"), "secret").unwrap();
    for uri in [
        "/..",
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/assets/../../secret.txt",
        "/a%00b",
        "/%ff",
    ] {
        let response = site.get(uri).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        let body: Value = serde_json::from_str(&text(response).await).unwrap();
        assert_eq!(body["error"]["code"], "not_found", "{uri}");
    }

    std::os::unix::fs::symlink(
        site.dir.path().join("secret.txt"),
        site.web().join("link.txt"),
    )
    .unwrap();
    std::os::unix::fs::symlink(site.dir.path(), site.web().join("outside")).unwrap();
    for uri in ["/link.txt", "/outside/secret.txt"] {
        let response = site.get(uri).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        let body: Value = serde_json::from_str(&text(response).await).unwrap();
        assert_eq!(body["error"]["code"], "not_found", "{uri}");
    }

    std::os::unix::fs::symlink(
        site.web().join("index.html"),
        site.web().join("inside.html"),
    )
    .unwrap();
    assert_eq!(text(site.get("/inside.html").await).await, INDEX);
}

#[tokio::test]
async fn a_rebuilt_folder_is_served_without_a_restart() {
    let site = Site::new(&[("index.html", "first build")]).await;
    assert_eq!(text(site.get("/").await).await, "first build");
    put(&site.web(), "index.html", "second build");
    put(&site.web(), "assets/new-9.js", "new");
    assert_eq!(text(site.get("/").await).await, "second build");
    assert_eq!(text(site.get("/assets/new-9.js").await).await, "new");
    std::fs::remove_file(site.web().join("index.html")).unwrap();
    let response = site.get("/").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_directory_is_not_a_file() {
    let site = Site::new(&[("index.html", INDEX), ("assets/app.js", "x")]).await;
    assert_eq!(text(site.get("/assets").await).await, INDEX);
}

#[tokio::test]
async fn the_embedded_source_serves_the_folder_it_was_built_from() {
    let dir = TempDir::new().unwrap();
    let config = DaemonConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        socket: dir.path().join("run").join("fold.sock"),
        data_dir: dir.path().join("data"),
        runtime_dir: dir.path().join("run"),
        web_dir: None,
    };
    let daemon = Daemon::start(config, Arc::new(ManualClock::new()))
        .await
        .unwrap();
    let built = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist/index.html");
    let response = router(daemon.state().clone())
        .oneshot(Request::get("/board").body(Body::empty()).unwrap())
        .await
        .unwrap();
    match std::fs::read_to_string(built) {
        Ok(index) => {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(text(response).await, index);
        }
        Err(_) => assert_eq!(response.status(), StatusCode::NOT_FOUND),
    }
}
