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
}

/// The error every transport reports. The message and details name ids, never content.
#[derive(Debug, Clone, PartialEq)]
pub struct FoldError {
    code: &'static str,
    message: String,
    hint: Option<String>,
    details: Option<serde_json::Map<String, serde_json::Value>>,
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
        self.details = Some(
            details
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value))
                .collect(),
        );
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
        self.details.as_ref()
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
