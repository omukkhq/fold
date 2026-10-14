use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::ids::{ProposalId, RevisionId, TaskId, WorkspaceId};
use crate::time::Timestamp;

/// The messages never repeat the rejected text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum EventError {
    #[error("event type is not declared")]
    UnknownType,
    #[error("event actor is not a worker name, human:<name> or fold")]
    InvalidActor,
    #[error("a live event type is never stored")]
    LiveNotStored,
    #[error("only a live event type can be sent as a live event")]
    NotLive,
    #[error("event data is not a JSON object")]
    DataNotObject,
}

macro_rules! define_event_types {
    ($($name:ident = $value:literal),+ $(,)?) => {
        /// One constant per declared event type.
        pub mod types {
            use super::EventType;
            $(pub const $name: EventType = EventType($value);)+
        }

        impl EventType {
            /// Every declared type, in declaration order.
            pub const ALL: &'static [EventType] = &[$(types::$name),+];
        }
    };
}

/// A declared event name of the form `<domain>.<verb>` in lowercase snake case. The only way to build one is a declared constant or `parse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventType(&'static str);

define_event_types! {
    AGENT_REGISTERED = "agent.registered",
    WORKER_REGISTERED = "worker.registered",
    SESSION_STARTED = "session.started",
    SESSION_ENDED = "session.ended",
    AGENT_TAG_ADDED = "agent.tag_added",
    AGENT_TAG_REMOVED = "agent.tag_removed",
    TASK_CREATED = "task.created",
    TASK_UPDATED = "task.updated",
    TASK_DELETED = "task.deleted",
    TASK_ARCHIVED = "task.archived",
    TASK_COMPLETED = "task.completed",
    TASK_STATUS_CHANGED = "task.status_changed",
    TASK_BLOCKED_CHANGED = "task.blocked_changed",
    TASK_HUMAN_REVIEW_SET = "task.human_review_set",
    TASK_HUMAN_REVIEW_CLEARED = "task.human_review_cleared",
    LABEL_CREATED = "label.created",
    DEPENDENCY_ADDED = "dependency.added",
    DEPENDENCY_REMOVED = "dependency.removed",
    CHECKLIST_CREATED = "checklist.created",
    CHECKLIST_ITEM_ADDED = "checklist.item_added",
    CHECKLIST_ITEM_UPDATED = "checklist.item_updated",
    CHECKLIST_ITEM_DONE = "checklist.item_done",
    CHECKLIST_ITEM_REMOVED = "checklist.item_removed",
    CLAIM_ACQUIRED = "claim.acquired",
    CLAIM_RELEASED = "claim.released",
    CLAIM_TAKEN_OVER = "claim.taken_over",
    CLAIM_STALE = "claim.stale",
    WORKSPACE_CREATED = "workspace.created",
    WORKSPACE_ADOPTED = "workspace.adopted",
    WORKSPACE_ABANDONED = "workspace.abandoned",
    WORKSPACE_ARCHIVED = "workspace.archived",
    WORKSPACE_SYNCED = "workspace.synced",
    WORKSPACE_MISSING = "workspace.missing",
    WORKSPACE_RECOVERED = "workspace.recovered",
    WORKSPACE_WATCH_DEGRADED = "workspace.watch_degraded",
    WORKSPACE_CHANGED = "workspace.changed",
    CHECKPOINT_RECORDED = "checkpoint.recorded",
    CHECKPOINT_RESTORED = "checkpoint.restored",
    CHECKPOINT_PRUNED = "checkpoint.pruned",
    CHANGESET_RECORDED = "changeset.recorded",
    PROPOSAL_OPENED = "proposal.opened",
    PROPOSAL_VERSION_ADDED = "proposal.version_added",
    PROPOSAL_CLOSED = "proposal.closed",
    PROPOSAL_INTEGRATED = "proposal.integrated",
    REVIEW_SUBMITTED = "review.submitted",
    REVIEW_COMMENT_ADDED = "review.comment_added",
    INTEGRATION_SUCCEEDED = "integration.succeeded",
    INTEGRATION_CONFLICTED = "integration.conflicted",
    INTEGRATION_REFUSED = "integration.refused",
    MAIN_ADVANCED = "main.advanced",
    REMOTE_PUSHED = "remote.pushed",
    REMOTE_PUSH_FAILED = "remote.push_failed",
    REMOTE_PULLED = "remote.pulled",
    REMOTE_PULL_CONFLICTED = "remote.pull_conflicted",
    MESSAGE_SENT = "message.sent",
    MESSAGE_ACKNOWLEDGED = "message.acknowledged",
}

impl EventType {
    pub fn parse(name: &str) -> Result<EventType, EventError> {
        Self::ALL
            .iter()
            .find(|declared| declared.0 == name)
            .copied()
            .ok_or(EventError::UnknownType)
    }

    pub fn as_str(self) -> &'static str {
        self.0
    }

    /// `workspace.changed` is the only live type: it is sent on the bus and never stored.
    pub fn is_live(self) -> bool {
        self == types::WORKSPACE_CHANGED
    }
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Serialize for EventType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0)
    }
}

impl<'de> Deserialize<'de> for EventType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        EventType::parse(&name).map_err(serde::de::Error::custom)
    }
}

const MAX_LABEL_LEN: usize = 64;

/// Who caused an event: a worker (`frontend`, `frontend/ui`), a human (`human:ana`) or `fold`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventActor(String);

impl EventActor {
    pub const FOLD: &'static str = "fold";

    pub fn parse(label: &str) -> Result<EventActor, EventError> {
        let valid = if label == Self::FOLD {
            true
        } else if let Some(human) = label.strip_prefix("human:") {
            Self::valid_human(human)
        } else {
            let mut parts = label.split('/');
            let agent = parts.next().unwrap_or_default();
            let worker = parts.next();
            parts.next().is_none() && Self::valid_name(agent) && worker.is_none_or(Self::valid_name)
        };
        if valid {
            Ok(EventActor(label.to_owned()))
        } else {
            Err(EventError::InvalidActor)
        }
    }

    pub fn fold() -> EventActor {
        EventActor(Self::FOLD.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn valid_name(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= MAX_LABEL_LEN
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    }

    fn valid_human(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= MAX_LABEL_LEN
            && name.trim() == name
            && !name.chars().any(char::is_control)
    }
}

impl fmt::Display for EventActor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for EventActor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for EventActor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let label = String::deserialize(deserializer)?;
        EventActor::parse(&label).map_err(serde::de::Error::custom)
    }
}

/// An event before it is stored. `data` holds ids, counts and names, never content.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent {
    pub event_type: EventType,
    pub actor: EventActor,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
    pub workspace: Option<WorkspaceId>,
    pub revision: Option<RevisionId>,
    pub data: serde_json::Value,
}

impl NewEvent {
    /// A live type is refused here because it is never stored.
    pub fn new(event_type: EventType, actor: EventActor) -> Result<NewEvent, EventError> {
        if event_type.is_live() {
            return Err(EventError::LiveNotStored);
        }
        Ok(NewEvent {
            event_type,
            actor,
            task: None,
            proposal: None,
            workspace: None,
            revision: None,
            data: serde_json::Value::Object(serde_json::Map::new()),
        })
    }

    #[must_use]
    pub fn task(mut self, task: TaskId) -> Self {
        self.task = Some(task);
        self
    }

    #[must_use]
    pub fn proposal(mut self, proposal: ProposalId) -> Self {
        self.proposal = Some(proposal);
        self
    }

    #[must_use]
    pub fn workspace(mut self, workspace: WorkspaceId) -> Self {
        self.workspace = Some(workspace);
        self
    }

    #[must_use]
    pub fn revision(mut self, revision: RevisionId) -> Self {
        self.revision = Some(revision);
        self
    }

    pub fn data(mut self, data: serde_json::Value) -> Result<Self, EventError> {
        if !data.is_object() {
            return Err(EventError::DataNotObject);
        }
        self.data = data;
        Ok(self)
    }
}

/// A stored event. The reference fields are public ids; `seq` is the per-project sequence number that is never reused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub actor: EventActor,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
    pub workspace: Option<WorkspaceId>,
    pub revision: Option<RevisionId>,
    pub data: serde_json::Value,
    pub created_at: Timestamp,
}

/// A working-change notice. It has no `seq` because it is never stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveEvent {
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub workspace: Option<WorkspaceId>,
    pub data: serde_json::Value,
}

impl LiveEvent {
    pub fn new(
        event_type: EventType,
        workspace: Option<WorkspaceId>,
        data: serde_json::Value,
    ) -> Result<LiveEvent, EventError> {
        if !event_type.is_live() {
            return Err(EventError::NotLive);
        }
        if !data.is_object() {
            return Err(EventError::DataNotObject);
        }
        Ok(LiveEvent {
            event_type,
            workspace,
            data,
        })
    }
}

/// Selects stored events. An empty `types` matches every type; `task` and `proposal` match only events that carry that id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventFilter {
    pub types: Vec<EventType>,
    pub task: Option<TaskId>,
    pub proposal: Option<ProposalId>,
}

impl EventFilter {
    pub fn matches(&self, event: &Event) -> bool {
        (self.types.is_empty() || self.types.contains(&event.event_type))
            && self.task.is_none_or(|task| event.task == Some(task))
            && self
                .proposal
                .is_none_or(|proposal| event.proposal == Some(proposal))
    }
}
