pub mod actor;
pub mod agents;
pub mod assets;
pub mod auth;
pub mod error;
pub mod events;
pub mod host;
pub mod messages;
pub mod openapi;
pub mod projects;
pub mod tasks;
pub mod workspaces;

use axum::extract::{FromRequestParts, MatchedPath, RawPathParams, Request};
use axum::http::request::Parts;
use axum::{Extension, Router, middleware};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::Level;
use utoipa_axum::router::OpenApiRouter;

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

/// Every `/api` route that [`auth::require_session`] guards.
fn guarded_routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(agents::routes())
        .merge(events::routes())
        .merge(messages::routes())
        .merge(projects::routes())
        .merge(workspaces::routes())
        .merge(tasks::routes())
}

/// Every `/api` route with its declaration, so a route and its schema cannot drift apart.
pub fn api_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(openapi::base())
        .merge(guarded_routes())
        .merge(auth::routes())
}

/// The OpenAPI document. It needs no state, socket or project.
pub fn openapi() -> utoipa::openapi::OpenApi {
    let mut doc = api_router().split_for_parts().1;
    openapi::declare_layer_errors(&mut doc, &guarded_routes().split_for_parts().1);
    doc
}

/// The router the TCP listener serves. The session check is a route layer of the guarded routes only, so the session routes and the web files of the fallback load without a cookie. The host check is a route layer of every `/api` route and runs first.
pub fn router(state: AppState) -> Router {
    let (guarded, _) = guarded_routes().split_for_parts();
    let (session, _) = auth::routes().split_for_parts();
    guarded
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_session,
        ))
        .merge(session)
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            host::require_listen_host,
        ))
        .fallback(assets::serve)
        .method_not_allowed_fallback(unmatched)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request| {
                    // The route template and never the URI, which can carry a query string.
                    let route = request.extensions().get::<MatchedPath>().map_or_else(
                        || assets::path_class(request.uri().path()),
                        MatchedPath::as_str,
                    );
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
