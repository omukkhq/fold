use std::fmt;

/// Every error code exists once, here. A shipped code never changes; a new code adds a constant and a line to `tests/error_codes.golden`.
macro_rules! define_codes {
    ($($name:ident = $value:literal),+ $(,)?) => {
        pub mod codes {
            $(pub const $name: &str = $value;)+
            pub const ALL: &[&str] = &[$($name),+];
        }
    };
}

define_codes! {
    INVALID_ID = "invalid_id",
    INVALID_TIMESTAMP = "invalid_timestamp",
    AGENT_NOT_REGISTERED = "agent_not_registered",
    DEPENDENCY_CYCLE = "dependency_cycle",
    ALREADY_CLAIMED = "already_claimed",
    INVALID_HASH = "invalid_hash",
    TREE_INVALID = "tree_invalid",
    REVISION_INVALID = "revision_invalid",
    OBJECT_MISSING = "object_missing",
    OBJECT_CORRUPT = "object_corrupt",
    OBJECT_IO = "object_io",
    SCHEMA_TOO_NEW = "schema_too_new",
    INVALID_PATH = "invalid_path",
    TARGET_NOT_EMPTY = "target_not_empty",
    WORKSPACE_IO = "workspace_io",
    NOT_FOUND = "not_found",
    PROJECT_NOT_FOUND = "project_not_found",
    INVALID_REQUEST = "invalid_request",
    PROJECT_UNAVAILABLE = "project_unavailable",
    INTERNAL_ERROR = "internal_error",
    RUNTIME_DIR_MISSING = "runtime_dir_missing",
    DAEMON_ALREADY_RUNNING = "daemon_already_running",
    AUTH_REQUIRED = "auth_required",
    // Nothing raises it: the socket server takes its handshake handler at bind. It stays so that no other error reuses a shipped code.
    HANDSHAKE_UNAVAILABLE = "handshake_unavailable",
    INVALID_NAME = "invalid_name",
    HUMAN_REQUIRED = "human_required",
    AGENT_ALREADY_BOUND = "agent_already_bound",
    WORKER_NOT_FOUND = "worker_not_found",
    REVISION_NOT_FOUND = "revision_not_found",
    HEAD_MOVED = "head_moved",
    INVALID_TASK_TITLE = "invalid_task_title",
    INVALID_LABEL_NAME = "invalid_label_name",
    INVALID_LABEL_COLOR = "invalid_label_color",
    INVALID_TASK_STATUS = "invalid_task_status",
    TASK_NOT_FOUND = "task_not_found",
    TASK_CLOSED = "task_closed",
    LABEL_NOT_FOUND = "label_not_found",
    LABEL_EXISTS = "label_exists",
    DEPENDENCY_EXISTS = "dependency_exists",
    DEPENDENCY_NOT_FOUND = "dependency_not_found",
    UNRECORDED_CHANGES = "unrecorded_changes",
    PROJECT_EXISTS = "project_exists",
    INVALID_HOST_PATHS = "invalid_host_paths",
    INVALID_CONFIG = "invalid_config",
    ALREADY_CURRENT = "already_current",
    WORKSPACE_CHANGED_DURING_SYNC = "workspace_changed_during_sync",
    PROTOCOL_VERSION_UNSUPPORTED = "protocol_version_unsupported",
    INVALID_HANDSHAKE = "invalid_handshake",
    NOT_CLAIM_HOLDER = "not_claim_holder",
    CLAIM_NOT_FOUND = "claim_not_found",
    WEB_NOT_BUILT = "web_not_built",
    DATABASE_LOCKED = "database_locked",
    GIT_NOT_FOUND = "git_not_found",
    GIT_FAILED = "git_failed",
    EXPORT_DIVERGED = "export_diverged",
    GIT_SYNC_MAP_INVALID = "git_sync_map_invalid",
    DOCTOR_CHECKS_FAILED = "doctor_checks_failed",
    SERVICE_INSTALL_FAILED = "service_install_failed",
    INVALID_CHECKLIST_NAME = "invalid_checklist_name",
    INVALID_STEP_TEXT = "invalid_step_text",
    CHECKLIST_ITEM_NOT_FOUND = "checklist_item_not_found",
    INVALID_EVENT_FILTER = "invalid_event_filter",
    GC_IN_PROGRESS = "gc_in_progress",
    HUMAN_NAME_REQUIRED = "human_name_required",
    AGENT_NOT_FOUND = "agent_not_found",
    ACTOR_CANNOT_CLAIM = "actor_cannot_claim",
    WORKSPACE_EXISTS = "workspace_exists",
    WORKSPACE_NOT_FOUND = "workspace_not_found",
    WORKSPACE_NOT_ACTIVE = "workspace_not_active",
    NOT_WORKSPACE_OWNER = "not_workspace_owner",
    INVALID_STATE_REF = "invalid_state_ref",
    PATH_NOT_IN_COMPARISON = "path_not_in_comparison",
    // Only a fired failpoint of the `failpoints` test feature returns it; a release build never does.
    FAILPOINT = "failpoint",
    MESSAGE_NOT_FOUND = "message_not_found",
    RECIPIENT_NOT_FOUND = "recipient_not_found",
    UNKNOWN_HUMAN = "unknown_human",
    PROPOSAL_NOT_FOUND = "proposal_not_found",
    INVALID_TOKEN = "invalid_token",
    HOST_NOT_ALLOWED = "host_not_allowed",
    REMOTE_NOT_SET = "remote_not_set",
    REMOTE_AHEAD = "remote_ahead",
    UNRESOLVED_CONFLICTS = "unresolved_conflicts",
    CHECKPOINT_NOT_FOUND = "checkpoint_not_found",
    WORKSPACE_WRITE_UNFINISHED = "workspace_write_unfinished",
    TOKEN_NOT_FOUND = "token_not_found",
    PROPOSAL_EXISTS = "proposal_exists",
    PROPOSAL_NOT_OPEN = "proposal_not_open",
    NOT_PROPOSAL_AUTHOR = "not_proposal_author",
    VERSION_UNCHANGED = "version_unchanged",
    CLAIM_NOT_STALE = "claim_not_stale",
    NOT_CLAIMED = "not_claimed",
    ALREADY_HOLDER = "already_holder",
    AUTHOR_CANNOT_REVIEW = "author_cannot_review",
    INTEGRATOR_REQUIRED = "integrator_required",
    UNKNOWN_COMMENT_PATH = "unknown_comment_path",
    INVALID_COMMENT_RANGE = "invalid_comment_range",
    EMPTY_REVIEW = "empty_review",
    REVIEW_CLAIM_REQUIRED = "review_claim_required",
    CHECKOUT_FAILED = "checkout_failed",
    INVALID_LAST_EVENT_ID = "invalid_last_event_id",
    INVALID_PUSH_TARGET = "invalid_push_target",
    HUMAN_REVIEW_ALREADY_SET = "human_review_already_set",
    STALE_VERSION = "stale_version",
    AUTHOR_CANNOT_INTEGRATE = "author_cannot_integrate",
    CHANGES_REQUESTED = "changes_requested",
    NOT_APPROVED = "not_approved",
    HUMAN_APPROVAL_MISSING = "human_approval_missing",
    PREREQUISITES_NOT_DONE = "prerequisites_not_done",
    INTEGRATION_FAILED = "integration_failed",
    DAEMON_UNREACHABLE = "daemon_unreachable",
    INTEGRATION_CONFLICTED = "integration_conflicted",
    SEED_EXISTS = "seed_exists",
    PROJECT_INCOMPLETE = "project_incomplete",
    REQUEST_NOT_FOUND = "request_not_found",
    REQUEST_NOT_OPEN = "request_not_open",
    REQUESTER_REQUIRED = "requester_required",
    DRAFTER_REQUIRED = "drafter_required",
    ORCHESTRATOR_REQUIRED = "orchestrator_required",
    INVALID_DRAFT = "invalid_draft",
    REVIEW_BATCH_NOT_FOUND = "review_batch_not_found",
    REVIEW_BATCH_NOT_OPEN = "review_batch_not_open",
    ALREADY_IN_BATCH = "already_in_batch",
    NOT_BATCH_MEMBER = "not_batch_member",
    BATCH_CONFLICT = "batch_conflict",
    UNATTRIBUTED_LINES = "unattributed_lines",
    AREA_NOT_FOUND = "area_not_found",
    AREA_EXISTS = "area_exists",
    AREA_HAS_LEAD = "area_has_lead",
    MAIN_AGENT_REQUIRED = "main_agent_required",
    WORKER_START_DENIED = "worker_start_denied",
    WORKER_STOP_DENIED = "worker_stop_denied",
    TASK_OUTSIDE_AREA = "task_outside_area",
    WORKER_LIMIT_REACHED = "worker_limit_reached",
    WORKER_EXISTS = "worker_exists",
    WORKER_PROCESS_NOT_FOUND = "worker_process_not_found",
}

/// The error every transport reports. The message and details name ids, never content.
#[derive(Debug, Clone, PartialEq)]
pub struct FoldError {
    code: &'static str,
    message: String,
    hint: Option<String>,
    /// Boxed because the order-preserving map would make every `Result<_, FoldError>` large.
    details: Option<Box<serde_json::Map<String, serde_json::Value>>>,
}

impl FoldError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: None,
            details: None,
        }
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Details are named fields, so every transport sends them as one JSON object.
    #[must_use]
    pub fn with_details(
        mut self,
        details: impl IntoIterator<Item = (&'static str, serde_json::Value)>,
    ) -> Self {
        self.details = Some(Box::new(
            details
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value))
                .collect(),
        ));
        self
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    pub fn details(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.details.as_deref()
    }
}

impl fmt::Display for FoldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FoldError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builders_set_every_field() {
        let error = FoldError::new(codes::ALREADY_CLAIMED, "T-1 is claimed")
            .with_hint("call task_list")
            .with_details([("task", serde_json::json!("T-1"))]);
        assert_eq!(error.code(), "already_claimed");
        assert_eq!(error.message(), "T-1 is claimed");
        assert_eq!(error.to_string(), "T-1 is claimed");
        assert_eq!(error.hint(), Some("call task_list"));
        assert_eq!(
            error.details().cloned().map(serde_json::Value::Object),
            Some(serde_json::json!({"task": "T-1"}))
        );
        let bare = FoldError::new(codes::DEPENDENCY_CYCLE, "cycle");
        assert_eq!(bare.hint(), None);
        assert_eq!(bare.details(), None);
    }
}
