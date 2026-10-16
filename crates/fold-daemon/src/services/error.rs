use fold_core::{EventError, FoldError, codes};
use fold_store::StoreError;

use super::agents::AgentError;
use super::events::EventsError;
use super::messages::MessageError;
use super::projects::ProjectError;
use super::reviews::ReviewError;
use super::tasks::TaskError;
use super::workspaces::WorkspaceError;

/// The `FoldError` every transport answers with for a store failure. One without a stable code is internal: the caller gets `internal_error` and the log gets the cause, which names tables and constraints, never values.
pub fn from_store(error: StoreError) -> FoldError {
    match error.code() {
        Some(code) => FoldError::new(code, error.to_string()),
        None => {
            tracing::error!(%error, "a command failed in the store");
            internal()
        }
    }
}

/// An event the declared types refuse is a defect in the command, never in the input.
fn from_event(error: &EventError) -> FoldError {
    tracing::error!(%error, "a command failed to build an event");
    internal()
}

fn internal() -> FoldError {
    FoldError::new(codes::INTERNAL_ERROR, "internal error")
}

impl From<AgentError> for FoldError {
    fn from(error: AgentError) -> Self {
        match error {
            AgentError::Fold(error) => error,
            AgentError::Store(error) => from_store(error),
            AgentError::Event(error) => from_event(&error),
        }
    }
}

impl From<EventsError> for FoldError {
    fn from(error: EventsError) -> Self {
        match error {
            EventsError::Fold(error) => error,
            EventsError::Store(error) => from_store(error),
        }
    }
}

impl From<TaskError> for FoldError {
    fn from(error: TaskError) -> Self {
        match error {
            TaskError::Fold(error) => error,
            TaskError::Store(error) => from_store(error),
            TaskError::Event(error) => from_event(&error),
        }
    }
}

impl From<WorkspaceError> for FoldError {
    fn from(error: WorkspaceError) -> Self {
        match error {
            WorkspaceError::Fold(error) => error,
            WorkspaceError::Store(error) => from_store(error),
            WorkspaceError::Task(error) => error.into(),
            WorkspaceError::Event(error) => from_event(&error),
        }
    }
}

impl From<MessageError> for FoldError {
    fn from(error: MessageError) -> Self {
        match error {
            MessageError::Fold(error) => error,
            MessageError::Store(error) => from_store(error),
            MessageError::Event(error) => from_event(&error),
        }
    }
}

impl From<ProjectError> for FoldError {
    fn from(error: ProjectError) -> Self {
        match error {
            ProjectError::Fold(error) => error,
            ProjectError::Store(error) => from_store(error),
        }
    }
}

impl From<ReviewError> for FoldError {
    fn from(error: ReviewError) -> Self {
        match error {
            ReviewError::Fold(error) => error,
            ReviewError::Store(error) => from_store(error),
            ReviewError::Task(error) => error.into(),
            ReviewError::Event(error) => from_event(&error),
        }
    }
}
