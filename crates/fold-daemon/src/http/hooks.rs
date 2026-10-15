use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::State;
use fold_core::{AgentName, FoldError};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::SocketOnly;
use super::error::{ApiError, ApiJson, ErrorEnvelope};
use crate::daemon::AppState;
use crate::services::projects;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(session_start))
        .routes(routes!(stop))
}

/// Not `Debug`: `token` must never reach a log line.
#[derive(Deserialize, ToSchema)]
pub struct SessionStartHook {
    /// The session's canonical working directory; the project is the registered one whose root holds it.
    pub cwd: String,
    /// `FOLD_AGENT` of the session.
    pub agent: String,
    /// `CLAUDE_CODE_MESSAGING_SOCKET`, the absolute path of the session's inbox socket. Used only together with `token`.
    pub socket: Option<String>,
    /// `CLAUDE_CODE_MESSAGING_TOKEN`. Used only together with `socket`.
    pub token: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SessionStartContext {
    /// The text for the session's context, at most 1500 characters.
    pub context: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StopHook {
    /// The session's canonical working directory.
    pub cwd: String,
    /// `FOLD_AGENT` of the session.
    pub agent: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StopUnread {
    /// The unacknowledged messages of every worker of the agent.
    pub unread: u64,
}

/// The Claude Code session-start hook of an agent: stores the session's inbox socket for the session push when `socket` and `token` are both present, then returns the session's context. Socket only.
#[utoipa::path(
    post,
    path = "/api/v1/hooks/session-start",
    operation_id = "hook_session_start",
    request_body = SessionStartHook,
    responses(
        (status = 200, description = "The session's context", body = SessionStartContext),
        (status = 400, description = "The body, the agent name or the push target is not valid", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, or no registered project holds `cwd`", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn session_start(
    _socket: SocketOnly,
    State(state): State<AppState>,
    ApiJson(request): ApiJson<SessionStartHook>,
) -> Result<Json<SessionStartContext>, ApiError> {
    let agent = AgentName::parse(&request.agent).map_err(FoldError::from)?;
    let handle = projects::open_for_dir(&state, Path::new(&request.cwd)).await?;
    let push = match (request.socket, request.token.as_deref()) {
        (Some(socket), Some(token)) => Some((PathBuf::from(socket), token)),
        _ => None,
    };
    let context = handle
        .agents()
        .session_start(&handle.slug, &agent, &handle.tasks, push)
        .await?;
    Ok(Json(SessionStartContext { context }))
}

/// The Claude Code stop hook of an agent: how many messages the agent has not acknowledged. Socket only.
#[utoipa::path(
    post,
    path = "/api/v1/hooks/stop",
    operation_id = "hook_stop",
    request_body = StopHook,
    responses(
        (status = 200, description = "The agent's unread count", body = StopUnread),
        (status = 400, description = "The body or the agent name is not valid", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, no registered project holds `cwd`, or the agent is not registered (`agent_not_registered`)", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn stop(
    _socket: SocketOnly,
    State(state): State<AppState>,
    ApiJson(request): ApiJson<StopHook>,
) -> Result<Json<StopUnread>, ApiError> {
    let agent = AgentName::parse(&request.agent).map_err(FoldError::from)?;
    let handle = projects::open_for_dir(&state, Path::new(&request.cwd)).await?;
    let unread = handle.agents().unread_of(&agent).await?;
    Ok(Json(StopUnread { unread }))
}
