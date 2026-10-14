pub mod error;
pub mod projects;

use axum::extract::{FromRequestParts, MatchedPath, RawPathParams, Request};
use axum::http::request::Parts;
use axum::{Extension, Router};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::daemon::AppState;
use crate::projects::{ProjectHandle, Slot};
use error::ApiError;
use fold_core::{FoldError, codes};
use std::sync::Arc;

/// How a request reached the daemon. The socket listener adds `Socket` to every request it serves; the HTTP listener adds nothing, so a route that needs [`SocketOnly`] does not exist on the network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Socket,
}

/// The router the TCP listener serves.
pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(projects::routes())
        .fallback(unmatched)
        .method_not_allowed_fallback(unmatched)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request| {
                    // The route template and never the URI, which can carry a query string.
                    let route = request
                        .extensions()
                        .get::<MatchedPath>()
                        .map_or("unmatched", MatchedPath::as_str);
                    tracing::info_span!("request", method = %request.method(), route)
                })
                .on_request(())
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(state)
}

/// The router the Unix socket serves: the same routes, marked as arriving over the socket.
pub fn socket_router(state: AppState) -> Router {
    router(state).layer(Extension(Transport::Socket))
}

async fn unmatched() -> ApiError {
    ApiError::not_found()
}

/// Succeeds only for a request that came over the Unix socket, whose peers are the host's user. Anything else gets `not_found`.
#[derive(Debug, Clone, Copy)]
pub struct SocketOnly;

impl<S: Send + Sync> FromRequestParts<S> for SocketOnly {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, ApiError> {
        match parts.extensions.get::<Transport>() {
            Some(Transport::Socket) => Ok(Self),
            None => Err(ApiError::not_found()),
        }
    }
}

/// Resolves `{slug}` of a `/api/v1/projects/{slug}` route to its open handle.
#[derive(Debug, Clone)]
pub struct Project(pub Arc<ProjectHandle>);

impl FromRequestParts<AppState> for Project {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let params = RawPathParams::from_request_parts(parts, state)
            .await
            .map_err(|_| ApiError::not_found())?;
        let slug = params
            .iter()
            .find_map(|(name, value)| (name == "slug").then_some(value))
            .ok_or_else(ApiError::not_found)?;
        match state.projects.slot(slug) {
            Some(Slot::Open(handle)) => Ok(Self(handle)),
            Some(Slot::Unavailable) => Err(FoldError::new(
                codes::PROJECT_UNAVAILABLE,
                format!("project {slug} is unavailable"),
            )
            .into()),
            None => Err(FoldError::new(
                codes::PROJECT_NOT_FOUND,
                format!("project {slug} is not registered"),
            )
            .into()),
        }
    }
}
