use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::agents::{AgentName, HumanName, WorkerName};
use crate::ids::{MessageId, ProposalId, RevisionId, TaskId, WorkspaceId};
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
    REVIEW_BATCH_CREATED = "review.batch_created",
    REVIEW_BATCH_SUBMITTED = "review.batch_submitted",
    REVIEW_BATCH_CLOSED = "review.batch_closed",
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
    REQUEST_CREATED = "request.created",
    REQUEST_CLAIMED = "request.claimed",
    REQUEST_RELEASED = "request.released",
    REQUEST_DRAFT_SUBMITTED = "request.draft_submitted",
    REQUEST_CHANGES_REQUESTED = "request.changes_requested",
    REQUEST_APPROVED = "request.approved",
    REQUEST_CLOSED = "request.closed",
    GIT_COMMIT_FAILED = "git.commit_failed",
    AGENT_PRESENCE_CHANGED = "agent.presence_changed",
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

    /// `workspace.changed` and `agent.presence_changed` are the live types: they are sent on the bus and never stored.
    pub fn is_live(self) -> bool {
        self == types::WORKSPACE_CHANGED || self == types::AGENT_PRESENCE_CHANGED
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

/// Who caused an event: a worker (`frontend`, `frontend/ui`, any depth a `WorkerName` allows), a human (`human:ana`) or `fold`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventActor(String);

impl EventActor {
    pub const FOLD: &'static str = "fold";

    pub fn parse(label: &str) -> Result<EventActor, EventError> {
        let valid = if label == Self::FOLD {
            true
        } else if let Some(human) = label.strip_prefix("human:") {
            Self::valid_human(human)
        } else if WorkerName::parse(label).is_ok() {
            true
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

    pub fn worker(worker: &WorkerName) -> EventActor {
        EventActor(worker.as_str().to_owned())
    }

    pub fn human(name: &HumanName) -> EventActor {
        EventActor(format!("human:{name}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The agent of a worker actor, which is its first segment; `None` for a human and for `fold`.
    pub fn agent(&self) -> Option<&str> {
        if self.0 == Self::FOLD || self.0.starts_with("human:") {
            return None;
        }
        self.0.split('/').next()
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
    /// Stored in its own column for the recipient rule of message events; it is not part of a stored `Event`, whose `data` names the message.
    pub message: Option<MessageId>,
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
            message: None,
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

    #[must_use]
    pub fn message(mut self, message: MessageId) -> Self {
        self.message = Some(message);
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

/// A working-change or presence notice. It has no `seq` because it is never stored.
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

/// What decides whether an event concerns one agent. It is loaded once per call from the claims, dependencies and proposals of every worker of the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEventFacts {
    pub agent: AgentName,
    /// Tasks a worker of the agent holds an unreleased implementation claim on.
    pub claimed: BTreeSet<TaskId>,
    /// Tasks that a claimed task needs directly.
    pub prerequisites: BTreeSet<TaskId>,
    /// Proposals a worker of the agent authored.
    pub authored: BTreeSet<ProposalId>,
    /// Proposals a worker of the agent holds the unreleased review claim on.
    pub reviewing: BTreeSet<ProposalId>,
}

impl AgentEventFacts {
    pub fn new(agent: AgentName) -> Self {
        Self {
            agent,
            claimed: BTreeSet::new(),
            prerequisites: BTreeSet::new(),
            authored: BTreeSet::new(),
            reviewing: BTreeSet::new(),
        }
    }
}

/// An event concerns an agent when it touches a task the agent works on or needs, a proposal it wrote or reviews, or moves main. What the agent's own workers did is never news to it.
pub fn concerns(event: &Event, facts: &AgentEventFacts) -> bool {
    if event.actor.agent() == Some(facts.agent.as_str()) {
        return false;
    }
    event.event_type == types::MAIN_ADVANCED
        || event.task.is_some_and(|task| {
            facts.claimed.contains(&task) || facts.prerequisites.contains(&task)
        })
        || event.proposal.is_some_and(|proposal| {
            facts.authored.contains(&proposal) || facts.reviewing.contains(&proposal)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(event_type: EventType, actor: &str) -> Event {
        Event {
            seq: 1,
            event_type,
            actor: EventActor::parse(actor).unwrap(),
            task: None,
            proposal: None,
            workspace: None,
            revision: None,
            data: serde_json::json!({}),
            created_at: Timestamp::from_millis(0).unwrap(),
        }
    }

    fn task(number: u32) -> TaskId {
        TaskId::from_number(number).unwrap()
    }

    fn proposal(number: u32) -> ProposalId {
        ProposalId::from_number(number).unwrap()
    }

    fn facts() -> AgentEventFacts {
        let mut facts = AgentEventFacts::new(AgentName::parse("frontend").unwrap());
        facts.claimed.insert(task(1));
        facts.prerequisites.insert(task(2));
        facts.authored.insert(proposal(3));
        facts.reviewing.insert(proposal(4));
        facts
    }

    fn on_task(number: u32, actor: &str) -> Event {
        Event {
            task: Some(task(number)),
            ..event(types::TASK_UPDATED, actor)
        }
    }

    fn on_proposal(number: u32, actor: &str) -> Event {
        Event {
            proposal: Some(proposal(number)),
            ..event(types::REVIEW_SUBMITTED, actor)
        }
    }

    #[test]
    fn an_event_on_a_claimed_task_concerns_the_holder() {
        assert!(concerns(&on_task(1, "backend"), &facts()));
    }

    #[test]
    fn an_event_on_a_prerequisite_concerns_the_holder() {
        assert!(concerns(&on_task(2, "human:ana"), &facts()));
    }

    #[test]
    fn an_event_on_an_authored_proposal_concerns_the_author() {
        assert!(concerns(&on_proposal(3, "review"), &facts()));
    }

    #[test]
    fn an_event_on_a_reviewed_proposal_concerns_the_reviewer() {
        assert!(concerns(&on_proposal(4, "fold"), &facts()));
    }

    #[test]
    fn main_advanced_concerns_every_agent() {
        assert!(concerns(&event(types::MAIN_ADVANCED, "fold"), &facts()));
        let unrelated = AgentEventFacts::new(AgentName::parse("docs").unwrap());
        assert!(concerns(&event(types::MAIN_ADVANCED, "review"), &unrelated));
    }

    #[test]
    fn the_agents_own_workers_never_count() {
        assert!(!concerns(&on_task(1, "frontend"), &facts()));
        assert!(!concerns(&on_task(2, "frontend/ui"), &facts()));
        assert!(!concerns(
            &event(types::MAIN_ADVANCED, "frontend"),
            &facts()
        ));
        assert!(concerns(&on_task(1, "frontend-two"), &facts()));
    }

    #[test]
    fn an_unrelated_event_does_not_concern_the_agent() {
        assert!(!concerns(&on_task(9, "backend"), &facts()));
        assert!(!concerns(&on_proposal(9, "backend"), &facts()));
        assert!(!concerns(&event(types::TASK_CREATED, "backend"), &facts()));
    }

    #[test]
    fn only_a_worker_actor_has_an_agent() {
        assert_eq!(
            EventActor::parse("frontend/ui").unwrap().agent(),
            Some("frontend")
        );
        assert_eq!(
            EventActor::parse("frontend").unwrap().agent(),
            Some("frontend")
        );
        assert_eq!(EventActor::fold().agent(), None);
        assert_eq!(EventActor::parse("human:ana").unwrap().agent(), None);
    }
}
