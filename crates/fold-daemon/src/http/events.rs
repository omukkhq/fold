use std::borrow::Cow;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::request::Parts;
use axum::response::sse::{self, KeepAlive, Sse};
use fold_core::events::{Event, EventType, types};
use fold_core::{
    CheckpointId, LiveEvent, Presence, ProposalId, RevisionId, TaskId, Timestamp, WorkspaceId,
};
use futures_util::{Stream, StreamExt as _};
use serde::{Deserialize, Serialize};
use utoipa::openapi::schema::{ObjectBuilder, SchemaType, Type};
use utoipa::openapi::{RefOr, Schema};
use utoipa::{IntoParams, PartialSchema, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::actor::cookie;
use super::agents::PresenceState;
use super::auth::SESSION_COOKIE;
use super::error::{ApiError, ApiQuery, ErrorEnvelope};
use super::{Project, Transport};
use crate::daemon::AppState;
use crate::services::events::{self, Feed, FeedItem, HistoryPage, parse_last_event_id};
use crate::services::projects;

/// A comment line this often keeps proxies from closing a quiet stream; the web client counts the stream dead after three missed periods.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(stream_events))
        .routes(routes!(list_event_history))
}

/// The project's events as Server-Sent Events: every stored event after `Last-Event-ID`, then one `snapshot` of the live workspaces, then each stored and live event as it happens. A stored event's frame carries its `seq` as the SSE `id`; live and snapshot frames carry none, so the browser keeps the last stored `seq`. Live frames a client has not read yet collapse to the newest per workspace and per agent. A client that falls more than the bus capacity behind has its stream ended and reconnects with its last id. Off loopback the stream ends at the first keep-alive period after its session is signed out, revoked or expired, and the reconnect gets `auth_required`.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/events",
    operation_id = "stream_events",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("Last-Event-ID" = Option<String>, Header, description = "The `seq` of the last stored event the client applied. Without it the stream starts at the snapshot and sends no older event."),
    ),
    responses(
        (status = 200, description = "The event stream; the `data` of every frame is one `EventEnvelope`", content_type = "text/event-stream", body = EventEnvelope),
        (status = 400, description = "`invalid_last_event_id`: `Last-Event-ID` is not a number or is above the newest event of the project", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`: the slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn stream_events(
    State(state): State<AppState>,
    Project(handle): Project,
    parts: Parts,
) -> Result<Sse<impl Stream<Item = Result<sse::Event, axum::Error>>>, ApiError> {
    let headers = &parts.headers;
    let session = (state.auth_required()
        && parts.extensions.get::<Transport>() != Some(&Transport::Socket))
    .then(|| cookie(headers, SESSION_COOKIE))
    .flatten();
    let last_event_id = headers
        .get("last-event-id")
        .map(|value| parse_last_event_id(value.as_bytes()))
        .transpose()?;
    let feed = Feed::open(&handle.store, &handle.bus, &handle.live, last_event_id).await?;
    let frames = futures_util::stream::unfold(feed, async |mut feed| {
        loop {
            match feed.next().await? {
                Ok(item) => {
                    if let Some(frame) = frame(item) {
                        return Some((frame, feed));
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        code = error.code().unwrap_or("store_error"),
                        "the event stream could not read the events and ends"
                    );
                    return None;
                }
            }
        }
    });
    let frames = frames.take_until(session_ended(state, session));
    Ok(Sse::new(frames).keep_alive(KeepAlive::new().interval(KEEP_ALIVE)))
}

/// Resolves once the web session `session` no longer signs anyone in, checked once per keep-alive period; never for a stream without a session. A failed check ends the stream too, so a stream never outlives a session it cannot confirm.
async fn session_ended(state: AppState, session: Option<String>) {
    let Some(session) = session else {
        return std::future::pending().await;
    };
    loop {
        tokio::time::sleep(KEEP_ALIVE).await;
        match projects::check_web_session(&state.registry, state.clock.as_ref(), &session).await {
            Ok(Some(_)) => {}
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(
                    code = error.code().unwrap_or("store_error"),
                    "the event stream could not check its session and ends"
                );
                return;
            }
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HistoryQuery {
    /// Only events with a lower `seq`; the newest events when absent.
    pub before: Option<u64>,
    /// At most this many events; 100 when absent, at least 1, and above 500 gives 500.
    pub limit: Option<u32>,
}

/// One page of stored events, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct EventHistory {
    #[schema(value_type = Vec<DurableEvent>)]
    pub events: Vec<Event>,
    /// Pass it as `before` for the next older page; null once the page reaches the first event.
    #[schema(required = true)]
    pub before: Option<u64>,
}

impl From<HistoryPage> for EventHistory {
    fn from(page: HistoryPage) -> Self {
        Self {
            events: page.events,
            before: page.before,
        }
    }
}

/// The project's stored events, newest first, one page at a time. The event stream carries every newer event as it happens.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/events/history",
    operation_id = "list_event_history",
    params(("slug" = String, Path, description = "The project slug"), HistoryQuery),
    responses(
        (status = 200, description = "One page of stored events", body = EventHistory),
        (status = 400, description = "`invalid_request`: the query string is not valid; `invalid_event_filter`: `limit` is 0", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`: the slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_event_history(
    Project(handle): Project,
    ApiQuery(query): ApiQuery<HistoryQuery>,
) -> Result<Json<EventHistory>, ApiError> {
    let page = events::history(&handle.store, query.before, query.limit).await?;
    Ok(Json(page.into()))
}

/// `None` for a live event that has no envelope variant, which this daemon never sends.
fn frame(item: FeedItem) -> Option<Result<sse::Event, axum::Error>> {
    match item {
        FeedItem::Durable(event) => Some(
            sse::Event::default()
                .event(event.event_type.as_str())
                .id(event.seq.to_string())
                .json_data(EventEnvelope::Event(Event::clone(&event))),
        ),
        FeedItem::Snapshot { seq, workspaces } => {
            let workspaces = workspaces.iter().map(|state| state.view.clone()).collect();
            Some(
                sse::Event::default()
                    .event("snapshot")
                    .json_data(EventEnvelope::Snapshot(Snapshot { seq, workspaces })),
            )
        }
        FeedItem::Live(event) => live_envelope(&event).map(|envelope| {
            sse::Event::default()
                .event(event.event_type.as_str())
                .json_data(envelope)
        }),
    }
}

fn live_envelope(event: &LiveEvent) -> Option<EventEnvelope> {
    let envelope = if event.event_type == types::WORKSPACE_CHANGED {
        serde_json::from_value(event.data.clone()).map(EventEnvelope::WorkspaceChanged)
    } else if event.event_type == types::AGENT_PRESENCE_CHANGED {
        serde_json::from_value(event.data.clone()).map(EventEnvelope::AgentPresenceChanged)
    } else if event.event_type == types::WORKER_OUTPUT {
        serde_json::from_value(event.data.clone()).map(EventEnvelope::WorkerOutput)
    } else {
        return None;
    };
    envelope
        .inspect_err(|_| {
            tracing::error!(
                event_type = event.event_type.as_str(),
                "a live event does not match its envelope"
            );
        })
        .ok()
}

/// One frame of a project's event stream, told apart by `kind`: a stored event, a live workspace change, a live presence change, a live worker step, or the snapshot sent after a replay.
// The `event` variant serialises as the stored `Event` plus `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventEnvelope {
    #[schema(value_type = DurableEvent)]
    Event(Event),
    WorkspaceChanged(LiveWorkspace),
    AgentPresenceChanged(PresenceChange),
    WorkerOutput(WorkerOutput),
    Snapshot(Snapshot),
}

/// A stored event. The related ids are public ids.
// Never built: it only describes the serialised `Event`, and a test holds the two to the same fields.
#[derive(ToSchema)]
pub struct DurableEvent {
    /// The per-project sequence number; it is never reused.
    pub seq: u64,
    #[schema(rename = "type")]
    pub event_type: DurableEventType,
    /// A worker (`frontend`, `frontend/ui`), a human (`human:ana`) or `fold`.
    pub actor: String,
    #[schema(value_type = Option<String>, required = true, example = "T-12")]
    pub task: Option<TaskId>,
    #[schema(value_type = Option<String>, required = true, example = "P-3")]
    pub proposal: Option<ProposalId>,
    #[schema(value_type = Option<String>, required = true, example = "ws-9")]
    pub workspace: Option<WorkspaceId>,
    #[schema(value_type = Option<String>, required = true, example = "r17")]
    pub revision: Option<RevisionId>,
    /// Ids, counts and names; never content.
    #[schema(value_type = std::collections::BTreeMap<String, serde_json::Value>)]
    pub data: serde_json::Value,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub created_at: Timestamp,
}

/// Every stored event type. The live types arrive only as their own variants: `workspace.changed` as `workspace_changed`, `agent.presence_changed` as `agent_presence_changed` and `worker.output` as `worker_output`.
pub struct DurableEventType;

impl DurableEventType {
    pub fn names() -> impl Iterator<Item = &'static str> {
        EventType::ALL
            .iter()
            .filter(|event_type| !event_type.is_live())
            .map(|event_type| event_type.as_str())
    }
}

impl PartialSchema for DurableEventType {
    fn schema() -> RefOr<Schema> {
        ObjectBuilder::new()
            .schema_type(SchemaType::new(Type::String))
            .enum_values(Some(Self::names()))
            .into()
    }
}

impl ToSchema for DurableEventType {
    fn name() -> Cow<'static, str> {
        Cow::Borrowed("DurableEventType")
    }
}

/// An agent's presence after it crossed a boundary: `active` after a call of an agent that was not active, and the new presence once an active agent's window passes or its last session ends. It is the `data` of the live `agent.presence_changed` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PresenceChange {
    #[schema(example = "frontend")]
    pub agent: String,
    #[schema(value_type = PresenceState)]
    pub presence: Presence,
}

/// One step a worker's Claude Code process reported on stdout: its kind, and the tool's name or the text's length, never the text or the tool's input. It is the `data` of the live `worker.output` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkerOutput {
    /// The worker's agent name.
    #[schema(example = "web-coder")]
    pub agent: String,
    /// Such as `init`, `thinking`, `text`, `tool_use`, `tool_result`, `permission_request`, `result` or `result_error`.
    #[schema(example = "tool_use")]
    pub step: String,
    /// The tool of a `tool_use`, `permission_request` or `permission_denied` step.
    #[serde(default)]
    #[schema(required = true, example = "Bash")]
    pub tool: Option<String>,
    /// The length in bytes of a `text` step's text or of a `result` step's final text.
    #[serde(default)]
    #[schema(required = true)]
    pub text_length: Option<u64>,
}

/// The newest durable sequence number and every live workspace, sent after a replay so a client starts from current state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Snapshot {
    /// 0 while the project has no stored event.
    pub seq: u64,
    pub workspaces: Vec<LiveWorkspace>,
}

/// What an active workspace holds right now. These are working changes, never history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LiveWorkspace {
    #[schema(value_type = String, example = "ws-9")]
    pub workspace: WorkspaceId,
    #[schema(value_type = String, example = "T-12")]
    pub task: TaskId,
    /// The worker label, `agent` or `agent/worker`.
    #[schema(example = "frontend/ui")]
    pub worker: String,
    /// The revision the workspace is based on.
    #[schema(value_type = String, example = "r17")]
    pub base: RevisionId,
    pub files: Vec<ChangedFile>,
    /// A one-line change summary, such as `3 files: 2 modified, 1 added (+40 -5)`, or `no changes`.
    pub summary: String,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub last_activity_at: Timestamp,
    pub checkpoints: Vec<CheckpointSummary>,
    #[schema(value_type = Option<String>, required = true, example = "P-3")]
    pub proposal: Option<ProposalId>,
    /// The worker holds the task's implementation claim and nothing in the workspace changed for `stall_minutes`. Calls do not count, so a stalled worker can hold a fresh lease.
    pub stalled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChangedFile {
    /// Relative to the workspace root.
    pub path: String,
    /// The path before a rename; null for every other change.
    #[schema(required = true)]
    pub previous_path: Option<String>,
    pub change: ChangeKind,
    pub added_lines: u32,
    pub removed_lines: u32,
    /// A binary file is compared by hash only, so its line counts are 0.
    pub binary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CheckpointSummary {
    #[schema(value_type = String, example = "ck-88")]
    pub id: CheckpointId,
    #[schema(required = true)]
    pub label: Option<String>,
    /// False for a checkpoint an agent recorded explicitly.
    pub automatic: bool,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub created_at: Timestamp,
}
