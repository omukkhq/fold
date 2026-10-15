use std::borrow::Cow;

use fold_core::events::{Event, EventType};
use fold_core::{CheckpointId, Presence, ProposalId, RevisionId, TaskId, Timestamp, WorkspaceId};
use serde::{Deserialize, Serialize};
use utoipa::openapi::schema::{ObjectBuilder, SchemaType, Type};
use utoipa::openapi::{RefOr, Schema};
use utoipa::{PartialSchema, ToSchema};

use super::agents::PresenceState;

/// One frame of a project's event stream, told apart by `kind`: a stored event, a live workspace change, a live presence change, or the snapshot sent after a replay.
// The `event` variant serialises as the stored `Event` plus `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventEnvelope {
    #[schema(value_type = DurableEvent)]
    Event(Event),
    WorkspaceChanged(LiveWorkspace),
    AgentPresenceChanged(PresenceChange),
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

/// Every stored event type. The live types arrive only as their own variants: `workspace.changed` as `workspace_changed` and `agent.presence_changed` as `agent_presence_changed`.
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
    /// A short change summary, such as `3 files, +40 -12`.
    pub summary: String,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub last_activity_at: Timestamp,
    pub checkpoints: Vec<CheckpointSummary>,
    #[schema(value_type = Option<String>, required = true, example = "P-3")]
    pub proposal: Option<ProposalId>,
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
