use std::sync::Arc;

use fold_core::{Clock, EventError, FoldError};
use fold_store::{Store, StoreError};
use thiserror::Error;

use super::events::EventBus;
use super::tasks::TaskError;
use super::workspaces::{WorkspaceError, WorkspaceService};

mod comments;
mod proposals;
mod verdicts;

pub use comments::{CommentInput, CommentLocation, CommentView};
pub use proposals::{
    ProposalDetail, ProposalFilter, ProposalSummary, Retirement, Turn, VersionView, retire_proposal,
};
pub use verdicts::SubmittedReview;

#[derive(Debug, Error)]
pub enum ReviewError {
    #[error(transparent)]
    Fold(#[from] FoldError),
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A refusal of the task or status step a review command runs, such as `task_closed`.
    #[error(transparent)]
    Task(#[from] TaskError),
    /// A command built an event the declared types refuse: a defect in the command, not in the input.
    #[error(transparent)]
    Event(#[from] EventError),
}

impl ReviewError {
    /// The stable code of the errors that have one.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Fold(error) => Some(error.code()),
            Self::Store(error) => error.code(),
            Self::Task(error) => error.code(),
            Self::Event(_) => None,
        }
    }
}

/// A refusal of the workspace step a review command runs, such as `unresolved_conflicts`, is reported as the review command's own.
impl From<WorkspaceError> for ReviewError {
    fn from(error: WorkspaceError) -> Self {
        match error {
            WorkspaceError::Fold(error) => Self::Fold(error),
            WorkspaceError::Store(error) => Self::Store(error),
            WorkspaceError::Task(error) => Self::Task(error),
            WorkspaceError::Event(error) => Self::Event(error),
        }
    }
}

/// Commands and reads for the proposals and reviews of one project. Each command is one write transaction; its events reach the bus only after the commit.
///
/// `workspaces` snapshots the task workspace a proposal is made from. In the daemon it is `WorkspaceService::for_project`, so a snapshot takes the same per-workspace lock as a record, restore or sync, then the project's object lock shared, and holds both until the transaction that names its objects commits.
#[derive(Clone)]
pub struct ReviewService {
    store: Store,
    bus: EventBus,
    clock: Arc<dyn Clock>,
    workspaces: WorkspaceService,
}

impl std::fmt::Debug for ReviewService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewService").finish_non_exhaustive()
    }
}

impl ReviewService {
    pub fn new(
        store: Store,
        bus: EventBus,
        clock: Arc<dyn Clock>,
        workspaces: WorkspaceService,
    ) -> Self {
        Self {
            store,
            bus,
            clock,
            workspaces,
        }
    }
}

#[cfg(test)]
mod tests {
    use fold_core::codes;

    use super::*;

    /// Every refusal of a workspace step keeps its own variant and code when a review command reports it.
    #[test]
    fn workspace_errors_map_to_the_same_review_error() {
        let fold = ReviewError::from(WorkspaceError::Fold(FoldError::new(
            codes::UNRESOLVED_CONFLICTS,
            "conflicts",
        )));
        assert!(matches!(fold, ReviewError::Fold(_)));
        assert_eq!(fold.code(), Some(codes::UNRESOLVED_CONFLICTS));

        let store = ReviewError::from(WorkspaceError::Store(StoreError::Corrupt("t.c")));
        assert!(matches!(
            store,
            ReviewError::Store(StoreError::Corrupt("t.c"))
        ));
        assert_eq!(store.code(), None);

        let task = ReviewError::from(WorkspaceError::Task(TaskError::Fold(FoldError::new(
            codes::ALREADY_CLAIMED,
            "claimed",
        ))));
        assert!(matches!(task, ReviewError::Task(_)));
        assert_eq!(task.code(), Some(codes::ALREADY_CLAIMED));

        let event = ReviewError::from(WorkspaceError::Event(EventError::UnknownType));
        assert!(matches!(event, ReviewError::Event(EventError::UnknownType)));
        assert_eq!(event.code(), None);
        assert_eq!(event.to_string(), "event type is not declared");
    }
}
