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
}

/// The error every transport reports. The message and details name ids, never content.
#[derive(Debug, Clone, PartialEq)]
pub struct FoldError {
    code: &'static str,
    message: String,
    hint: Option<String>,
    details: Option<serde_json::Value>,
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

    #[must_use]
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
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

    pub fn details(&self) -> Option<&serde_json::Value> {
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
            .with_details(serde_json::json!({"task": "T-1"}));
        assert_eq!(error.code(), "already_claimed");
        assert_eq!(error.message(), "T-1 is claimed");
        assert_eq!(error.to_string(), "T-1 is claimed");
        assert_eq!(error.hint(), Some("call task_list"));
        assert_eq!(error.details(), Some(&serde_json::json!({"task": "T-1"})));
        let bare = FoldError::new(codes::DEPENDENCY_CYCLE, "cycle");
        assert_eq!(bare.hint(), None);
        assert_eq!(bare.details(), None);
    }
}
