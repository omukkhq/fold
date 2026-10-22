use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::{Extension, Json};
use fold_core::{AgentName, FoldError, MessageId};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::SocketOnly;
use super::error::{ApiError, ApiJson, ErrorEnvelope};
use crate::daemon::AppState;
use crate::services::agents::peers;
use crate::services::projects;
use crate::socket::SocketPeer;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(session_start))
        .routes(routes!(stop))
        .routes(routes!(stop_delivered))
        .routes(routes!(edit))
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
    /// The unacknowledged messages of the agent's worker.
    pub unread: u64,
    /// The hook's reason to keep the turn going: the messages that no inbox, notice or wait has shown the agent yet, oldest first, up to 16 KB of text, then how many more there are. `null` when there is none.
    #[schema(required = true)]
    pub reason: Option<String>,
    /// The messages `reason` carries, such as `m-4`. They count as delivered once the hook reports them to `/api/v1/hooks/stop/delivered`.
    pub messages: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StopDelivered {
    /// The session's canonical working directory.
    pub cwd: String,
    /// `FOLD_AGENT` of the session.
    pub agent: String,
    /// The messages of the reason the hook gave to its session.
    pub messages: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct StopMarked {
    /// The recipient rows this call marked delivered.
    pub delivered: u64,
}

/// The Claude Code session-start hook of an agent: stores the session's inbox socket for the session push when `socket` and `token` are both present, and keeps it in the daemon's own file of the agent, outside the project folder, for a restarted daemon; the inbox of an agent the project does not know yet waits in the daemon's memory until a session's adapter registers the agent. Then it returns the session's context. Socket only. A process under a worker names only that worker's own agent.
#[utoipa::path(
    post,
    path = "/api/v1/hooks/session-start",
    operation_id = "hook_session_start",
    request_body = SessionStartHook,
    responses(
        (status = 200, description = "The session's context", body = SessionStartContext),
        (status = 400, description = "The body, the agent name or the push target is not valid", body = ErrorEnvelope),
        (status = 403, description = "`worker_peer_refused`: a process under a worker, or under a check that fold runs, names an agent it may not act as", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, or no registered project holds `cwd`", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn session_start(
    _socket: SocketOnly,
    State(state): State<AppState>,
    peer: Option<Extension<SocketPeer>>,
    ApiJson(request): ApiJson<SessionStartHook>,
) -> Result<Json<SessionStartContext>, ApiError> {
    let agent = AgentName::parse(&request.agent).map_err(FoldError::from)?;
    let handle = projects::open_for_dir(&state, Path::new(&request.cwd)).await?;
    peers::require_peer_agent(&state.projects, pid(peer), &handle.slug, &agent).await?;
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

/// The Claude Code stop hook of an agent: how many messages the agent has not acknowledged, and the text that carries those it has not seen yet. Nothing is marked here, because the hook may give up on the answer; the hook reports what it used to `/api/v1/hooks/stop/delivered`. Socket only. A process under a worker names only that worker's own agent.
#[utoipa::path(
    post,
    path = "/api/v1/hooks/stop",
    operation_id = "hook_stop",
    request_body = StopHook,
    responses(
        (status = 200, description = "The agent's unread count and the messages", body = StopUnread),
        (status = 400, description = "The body or the agent name is not valid", body = ErrorEnvelope),
        (status = 403, description = "`worker_peer_refused`: a process under a worker, or under a check that fold runs, names an agent it may not act as", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, no registered project holds `cwd`, or the agent is not registered (`agent_not_registered`)", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn stop(
    _socket: SocketOnly,
    State(state): State<AppState>,
    peer: Option<Extension<SocketPeer>>,
    ApiJson(request): ApiJson<StopHook>,
) -> Result<Json<StopUnread>, ApiError> {
    let agent = AgentName::parse(&request.agent).map_err(FoldError::from)?;
    let handle = projects::open_for_dir(&state, Path::new(&request.cwd)).await?;
    peers::require_peer_agent(&state.projects, pid(peer), &handle.slug, &agent).await?;
    let unread = handle.agents().unread_of(&agent).await?;
    let carried = if unread == 0 {
        None
    } else {
        handle
            .messages(&state.registry)
            .carry_for_stop(&agent)
            .await?
    };
    Ok(Json(match carried {
        Some(carried) => StopUnread {
            unread,
            reason: Some(carried.text),
            messages: carried.messages.iter().map(ToString::to_string).collect(),
        },
        None => StopUnread {
            unread,
            reason: None,
            messages: Vec::new(),
        },
    }))
}

/// The Claude Code stop hook of an agent after it gave its reason to the session: the messages the reason carried count as delivered, as an inbox read marks them. Socket only. A process under a worker names only that worker's own agent.
#[utoipa::path(
    post,
    path = "/api/v1/hooks/stop/delivered",
    operation_id = "hook_stop_delivered",
    request_body = StopDelivered,
    responses(
        (status = 200, description = "How many rows were marked delivered", body = StopMarked),
        (status = 400, description = "The body, the agent name or a message id is not valid", body = ErrorEnvelope),
        (status = 403, description = "`worker_peer_refused`: a process under a worker, or under a check that fold runs, names an agent it may not act as", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, or no registered project holds `cwd`", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn stop_delivered(
    _socket: SocketOnly,
    State(state): State<AppState>,
    peer: Option<Extension<SocketPeer>>,
    ApiJson(request): ApiJson<StopDelivered>,
) -> Result<Json<StopMarked>, ApiError> {
    let agent = AgentName::parse(&request.agent).map_err(FoldError::from)?;
    let messages = request
        .messages
        .iter()
        .map(|id| id.parse::<MessageId>().map_err(FoldError::from))
        .collect::<Result<Vec<_>, _>>()?;
    let handle = projects::open_for_dir(&state, Path::new(&request.cwd)).await?;
    peers::require_peer_agent(&state.projects, pid(peer), &handle.slug, &agent).await?;
    let delivered = handle
        .messages(&state.registry)
        .mark_carried(&agent, &messages)
        .await?;
    Ok(Json(StopMarked { delivered }))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct EditHook {
    /// The session's canonical working directory.
    pub cwd: String,
    /// `FOLD_AGENT` of the session.
    pub agent: String,
    /// The review checkout directory that holds the file to edit, relative to the project root, such as `.fold/ws/review-p-3`.
    pub workspace: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EditDecision {
    /// The agent may edit files in the directory.
    pub allowed: bool,
    /// Why the edit is refused and what to do instead; `null` when it is allowed.
    #[schema(required = true)]
    pub reason: Option<String>,
}

/// The Claude Code edit guard of an agent, for a file in a review checkout: an agent edits only in an active review checkout of one proposal that its own worker owns, while its agent holds the proposal's review claim. Socket only. A process under a worker names only that worker's own agent.
#[utoipa::path(
    post,
    path = "/api/v1/hooks/edit",
    operation_id = "hook_edit",
    request_body = EditHook,
    responses(
        (status = 200, description = "Whether the agent may edit files in the directory", body = EditDecision),
        (status = 400, description = "The body or the agent name is not valid", body = ErrorEnvelope),
        (status = 403, description = "`worker_peer_refused`: a process under a worker, or under a check that fold runs, names an agent it may not act as", body = ErrorEnvelope),
        (status = 404, description = "Not on the socket, or no registered project holds `cwd`", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn edit(
    _socket: SocketOnly,
    State(state): State<AppState>,
    peer: Option<Extension<SocketPeer>>,
    ApiJson(request): ApiJson<EditHook>,
) -> Result<Json<EditDecision>, ApiError> {
    let agent = AgentName::parse(&request.agent).map_err(FoldError::from)?;
    let handle = projects::open_for_dir(&state, Path::new(&request.cwd)).await?;
    peers::require_peer_agent(&state.projects, pid(peer), &handle.slug, &agent).await?;
    let refusal = handle
        .reviews(&state.registry)
        .checkout_edit_refusal(&agent, &request.workspace)
        .await?;
    Ok(Json(EditDecision {
        allowed: refusal.is_none(),
        reason: refusal.map(str::to_owned),
    }))
}

fn pid(peer: Option<Extension<SocketPeer>>) -> Option<u32> {
    peer.and_then(|Extension(peer)| peer.pid)
}
