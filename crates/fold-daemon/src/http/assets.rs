use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use fold_core::{FoldError, codes};
use percent_encoding::percent_decode_str;
use rust_embed::RustEmbed;

use super::error::ApiError;
use crate::daemon::AppState;

/// The web build, compiled into the binary. A debug build reads the folder from disk instead.
#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct WebBuild;

const INDEX: &str = "index.html";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const REVALIDATE: &str = "no-cache";

/// Where the web files come from.
#[derive(Debug, Clone)]
pub enum AssetSource {
    Embedded,
    /// Read from disk on every request, so a rebuilt folder is served at once.
    Dir(PathBuf),
}

/// An embedded file's bytes are borrowed from the binary, so serving one copies nothing.
struct Asset {
    bytes: Cow<'static, [u8]>,
    etag: String,
}

enum Lookup {
    Found(Asset),
    Absent,
    /// The path resolves outside the source, through a symlink for example.
    Refused,
}

impl AssetSource {
    /// `path` is a normalised relative path. Anything that is not a regular file inside the source is absent. A file on disk whose entity tag `if_none_match` already names is not read.
    async fn read(&self, path: &str, if_none_match: Option<&str>) -> Lookup {
        match self {
            Self::Embedded => {
                let path = path.to_owned();
                // A debug build reads the file from disk here.
                let file = tokio::task::spawn_blocking(move || {
                    WebBuild::get(&path).map(|file| Asset {
                        etag: format!("\"{}\"", hex(&file.metadata.sha256_hash())),
                        bytes: file.data,
                    })
                })
                .await
                .ok()
                .flatten();
                file.map_or(Lookup::Absent, Lookup::Found)
            }
            Self::Dir(root) => read_dir_file(root, path, if_none_match).await,
        }
    }
}

async fn read_dir_file(root: &Path, path: &str, if_none_match: Option<&str>) -> Lookup {
    let Ok(root) = tokio::fs::canonicalize(root).await else {
        return Lookup::Absent;
    };
    // Canonicalising follows symlinks, so a link that leaves the folder fails the prefix check.
    let Ok(file) = tokio::fs::canonicalize(root.join(path)).await else {
        return Lookup::Absent;
    };
    if !file.starts_with(&root) {
        return Lookup::Refused;
    }
    let Ok(meta) = tokio::fs::metadata(&file).await else {
        return Lookup::Absent;
    };
    if !meta.is_file() {
        return Lookup::Absent;
    }
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |age| age.as_nanos());
    let etag = format!("\"{:x}-{modified:x}\"", meta.len());
    if if_none_match.is_some_and(|header| etag_matches(header, &etag)) {
        return Lookup::Found(Asset {
            bytes: Cow::Borrowed(&[]),
            etag,
        });
    }
    match tokio::fs::read(&file).await {
        Ok(bytes) => Lookup::Found(Asset {
            etag,
            bytes: Cow::Owned(bytes),
        }),
        Err(_) => Lookup::Absent,
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Decodes a request path into a relative path of plain components. `None` for an absolute path, `..`, NUL or an invalid encoding.
fn normalise(request_path: &str) -> Option<String> {
    let decoded = percent_decode_str(request_path).decode_utf8().ok()?;
    if decoded.contains('\0') {
        return None;
    }
    let mut parts = Vec::new();
    for segment in decoded.split('/').filter(|segment| !segment.is_empty()) {
        let mut components = Path::new(segment).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(name)), None) if name == segment => parts.push(segment),
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

fn is_api(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

/// The class a request that matched no route is logged under; the path itself never reaches the log.
pub fn path_class(path: &str) -> &'static str {
    if is_api(path) {
        "unmatched"
    } else if path.starts_with("/assets/") {
        "web.asset"
    } else {
        "web"
    }
}

/// The fallback of the one listener: the web files beside the `/api` routes.
pub async fn serve(State(state): State<AppState>, request: Request) -> Response {
    let path = request.uri().path();
    if is_api(path) || !matches!(*request.method(), Method::GET | Method::HEAD) {
        return ApiError::not_found().into_response();
    }
    let Some(relative) = normalise(path) else {
        return ApiError::not_found().into_response();
    };
    let if_none_match = request
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok());
    let (found, name, fallback) = match state.assets.read(&relative, if_none_match).await {
        Lookup::Found(asset) => (Lookup::Found(asset), relative, false),
        Lookup::Refused => return ApiError::not_found().into_response(),
        _ => (
            state.assets.read(INDEX, if_none_match).await,
            INDEX.to_owned(),
            true,
        ),
    };
    let Lookup::Found(asset) = found else {
        return ApiError(
            FoldError::new(codes::WEB_NOT_BUILT, "the web application is not built")
                .with_hint("run `make web-build`, or pass `--web-dir <path>` to `fld serve`"),
        )
        .into_response();
    };
    let cache = if !fallback && name.starts_with("assets/") {
        IMMUTABLE
    } else {
        REVALIDATE
    };
    respond(asset, &name, cache, request.headers())
}

fn respond(asset: Asset, name: &str, cache: &'static str, request: &HeaderMap) -> Response {
    let matched = request
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| etag_matches(value, &asset.etag));
    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let content_type = if mime.type_() == mime_guess::mime::TEXT {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_string()
    };
    let mut response = Response::new(if matched {
        Body::empty()
    } else {
        Body::from(asset.bytes)
    });
    if matched {
        *response.status_mut() = StatusCode::NOT_MODIFIED;
    }
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Ok(value) = HeaderValue::from_str(&asset.etag) {
        headers.insert(header::ETAG, value);
    }
    if !matched && let Ok(value) = HeaderValue::from_str(&content_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    response
}

/// Weak comparison of an `If-None-Match` list against one entity tag.
fn etag_matches(header: &str, etag: &str) -> bool {
    header
        .split(',')
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate.trim_start_matches("W/") == etag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_keeps_plain_paths_and_refuses_the_rest() {
        assert_eq!(normalise("/"), Some(String::new()));
        assert_eq!(normalise("/assets/a.js"), Some("assets/a.js".into()));
        assert_eq!(normalise("/a//b/"), Some("a/b".into()));
        assert_eq!(normalise("/a%20b"), Some("a b".into()));
        for bad in ["/..", "/a/../b", "/%2e%2e/x", "/a/./b", "/a%00b", "/%ff"] {
            assert_eq!(normalise(bad), None, "{bad}");
        }
    }

    #[test]
    fn unmatched_paths_are_logged_by_class() {
        assert_eq!(path_class("/api/v1/nothing"), "unmatched");
        assert_eq!(path_class("/assets/app-1.js"), "web.asset");
        assert_eq!(path_class("/tasks/T-1"), "web");
    }

    #[test]
    fn entity_tags_match_by_list_weakness_and_star() {
        assert!(etag_matches("\"a\"", "\"a\""));
        assert!(etag_matches("\"b\", W/\"a\"", "\"a\""));
        assert!(etag_matches("*", "\"a\""));
        assert!(!etag_matches("\"b\"", "\"a\""));
    }
}
