use fold_core::{FoldError, codes};
use fold_store::StoreError;
use rmcp::model::{CallToolResult, ContentBlock};
use serde::{Deserialize, Serialize};

use crate::services::agents::AgentError;
use crate::services::events::EventsError;
use crate::services::projects::ProjectError;

/// The next step for the codes an agent can fix itself. A code in this table always gets this hint, whatever the error carried, so every transport names the same next step.
const HINTS: &[(&str, &str)] = &[
    (codes::AGENT_NOT_REGISTERED, "call register_agent"),
    (codes::INVALID_EVENT_FILTER, "call get_events"),
    (codes::WORKER_NOT_FOUND, "call register_worker"),
    (
        codes::PROJECT_NOT_FOUND,
        "run fold init in the project root",
    ),
];

pub fn hint_for(code: &str) -> Option<&'static str> {
    HINTS
        .iter()
        .find_map(|(known, hint)| (*known == code).then_some(*hint))
}

/// The error of a tool result and of a refused handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    pub hint: Option<String>,
}

impl From<&FoldError> for ErrorBody {
    fn from(error: &FoldError) -> Self {
        let hint = hint_for(error.code()).or(error.hint()).map(str::to_owned);
        Self {
            code: error.code().to_owned(),
            message: error.message().to_owned(),
            hint,
        }
    }
}

/// A store failure without a code is internal: the agent gets `internal_error` and the log gets the cause, which names tables and constraints, never values.
pub fn from_store(error: StoreError) -> FoldError {
    match error.code() {
        Some(code) => FoldError::new(code, error.to_string()),
        None => {
            tracing::error!(%error, "tool call failed in the store");
            FoldError::new(codes::INTERNAL_ERROR, "internal error")
        }
    }
}

pub fn from_agent(error: AgentError) -> FoldError {
    match error {
        AgentError::Fold(error) => error,
        AgentError::Store(error) => from_store(error),
        AgentError::Event(error) => {
            tracing::error!(%error, "tool call failed to build an event");
            FoldError::new(codes::INTERNAL_ERROR, "internal error")
        }
    }
}

pub fn from_events(error: EventsError) -> FoldError {
    match error {
        EventsError::Fold(error) => error,
        EventsError::Store(error) => from_store(error),
    }
}

pub fn from_project(error: ProjectError) -> FoldError {
    match error {
        ProjectError::Fold(error) => error,
        ProjectError::Store(error) => from_store(error),
    }
}

/// A tool-level error result: MCP clients show its content to the agent, while a protocol error would reach the agent as an opaque failure.
pub fn tool_error(error: &FoldError) -> CallToolResult {
    let body = serde_json::json!({ "error": ErrorBody::from(error) });
    CallToolResult::error(vec![ContentBlock::text(body.to_string())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_beats_the_hint_the_error_carried() {
        let error = FoldError::new(codes::WORKER_NOT_FOUND, "gone").with_hint("look elsewhere");
        assert_eq!(
            ErrorBody::from(&error).hint.as_deref(),
            Some("call register_worker")
        );
        let own = FoldError::new(codes::HUMAN_REQUIRED, "no").with_hint("ask a human");
        assert_eq!(ErrorBody::from(&own).hint.as_deref(), Some("ask a human"));
        let bare = FoldError::new(codes::INVALID_NAME, "bad");
        assert_eq!(ErrorBody::from(&bare).hint, None);
        assert_eq!(
            hint_for(codes::PROJECT_NOT_FOUND),
            Some("run fold init in the project root")
        );
    }

    #[test]
    fn store_errors_keep_only_their_stable_codes() {
        let coded = from_agent(AgentError::Store(StoreError::SchemaTooNew { version: 9 }));
        assert_eq!(coded.code(), codes::SCHEMA_TOO_NEW);
        let internal = from_agent(AgentError::Store(StoreError::Corrupt("agents.name")));
        assert_eq!(internal.code(), codes::INTERNAL_ERROR);
        assert_eq!(internal.message(), "internal error");
        let fold = from_agent(AgentError::Fold(FoldError::new(codes::INVALID_NAME, "bad")));
        let event = from_agent(AgentError::Event(fold_core::EventError::InvalidActor));
        assert_eq!(event.code(), codes::INTERNAL_ERROR);
        let missing = from_project(ProjectError::Fold(FoldError::new(
            codes::PROJECT_NOT_FOUND,
            "x",
        )));
        assert_eq!(missing.code(), codes::PROJECT_NOT_FOUND);
        let store = from_project(ProjectError::Store(StoreError::Corrupt("projects.root")));
        assert_eq!(store.code(), codes::INTERNAL_ERROR);
        assert_eq!(fold.code(), codes::INVALID_NAME);
    }

    #[test]
    fn a_tool_error_is_an_error_result_with_the_body() {
        let result = tool_error(&FoldError::new(codes::AGENT_NOT_REGISTERED, "unbound"));
        assert_eq!(result.is_error, Some(true));
        let text = &result.content[0].as_text().unwrap().text;
        let body: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"error": {
                "code": "agent_not_registered",
                "message": "unbound",
                "hint": "call register_agent",
            }})
        );
    }
}
