pub mod actor;
pub mod agents;
pub mod clock;
pub mod config;
pub mod error;
pub mod events;
pub mod ids;
pub mod layout;
pub mod messages;
pub mod policy;
pub mod projects;
pub mod reviews;
pub mod tasks;
pub mod text;
pub mod time;
pub mod workspaces;

// Every domain submodule is also reachable at the crate root, so callers outside the crate name it without its domain.
pub use agents::{carry, cost, presence, push, roles, transcript, turn_errors};
pub use tasks::{claims, container, dag, readiness, requests, status};
pub use workspaces::{checkpoint_policy, merge, object, revision, stall, tree};

pub use actor::{Actor, AgentActor};
pub use agents::presence::{ACTIVE_WINDOW_MS, Presence, SessionFact, presence_of};
pub use agents::roles::{
    CacheTtl, Role, WorkerState, main_prompt, shared_prompt, with_shared_rules,
};
pub use agents::{AgentName, AreaName, HumanName, MAIN_AREA, MAIN_TAG, NameError, Tag, WorkerName};
pub use clock::{Clock, ManualClock};
pub use config::{
    CONFIG_TEMPLATE, GitConfig, IntegrationConfig, MainConfig, ProjectConfig, RemoteConfig,
};
pub use error::{FoldError, codes};
pub use events::{
    AgentEventFacts, Event, EventActor, EventError, EventFilter, EventType, LiveEvent, NewEvent,
    concerns,
};
pub use ids::{
    ChangesetId, CheckRunId, ChecklistItemId, CheckpointId, CommentId, IdKind, IdParseError,
    IntegrationId, MessageId, PermissionQuestionId, ProposalId, RequestId, ReviewBatchId,
    ReviewPlanId, RevisionId, TaskId, WebTokenId, WorkspaceId,
};
pub use layout::{
    HostPaths, ProjectPaths, main_workspace_path, review_batch_checkout_path, review_checkout_path,
    task_workspace_path,
};
pub use messages::{
    Address, AddressError, AddressedAs, Choice, Directory, DirectoryAgent, MessageBody,
    MessageChoices, Recipient, ResolveError, Sender, Target,
};
pub use policy::PolicyDenied;
pub use projects::{choose_slug, slugify};
pub use tasks::status::TaskStatus;
pub use tasks::{
    ChecklistName, DependencyOrigin, HoldMark, HoldReason, LabelColor, LabelName, StepText,
    TaskDescription, TaskKind, TaskTitle, TaskValueError, unnamed_item_checklist,
};
pub use time::{TimeError, Timestamp};
