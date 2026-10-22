use std::sync::Arc;

use fold_core::{Clock, EventError, FoldError};
use fold_store::{Store, StoreError};
use thiserror::Error;

use super::events::EventBus;
use super::messages::MessageError;
use super::projects::ProjectError;
use super::tasks::TaskError;
use super::workspaces::{WorkspaceError, WorkspaceService};
use checks::CheckRunner;

mod batches;
mod changes;
mod checkouts;
pub mod checks;
mod claims;
mod comments;
mod conflicts;
mod human_edits;
mod human_review;
mod integrate;
mod notify;
mod plans;
mod proposals;
mod questions;
mod review_fixes;
mod scopes;
mod verdicts;
mod views;

pub use batches::{
    BatchCheckout, BatchComment, BatchCommentView, BatchLines, BatchMemberView, BatchReviewed,
    BatchVerdict, BatchVerdictView, LeftOut, LeftOutReason, MemberSince, ReviewBatchView,
    SubmittedBatch,
};
pub(crate) use batches::{close_batches_of, free_reviews_for_start};
pub use changes::{ChangeRange, ProposalChange, SinceChange};
pub use checkouts::{CheckoutTarget, ListedCheckout, ReviewCheckout};
pub use claims::ReviewClaimView;
pub(crate) use claims::{Assignment, assign_reviews_in_tx};
pub use comments::{CommentInput, CommentLocation, CommentView};
pub use human_edits::HumanEdit;
pub use human_review::{AwaitingHumanReview, awaiting_human_review, awaiting_human_review_in};
pub use integrate::{
    AutomaticIntegration, DirectChange, DirectOutcome, HumanRecord, IntegrationOutcome,
    PullIntegration, PullOutcome, RecordIntegration,
};
pub use plans::{AwaitedTask, ProposalPlan, ReviewPlanView, proposal_plan, proposal_plans};
pub(crate) use plans::{mark_plan_started, refuse_planned, settle_plans, take_plan_for_start};
pub(crate) use proposals::proposal_missing;
pub use proposals::{
    DEFAULT_PROPOSAL_LIST, MAX_PROPOSAL_LIST, ProposalDetail, ProposalFilter, ProposalListing,
    ProposalSummary, Proposed, Retirement, Turn, VersionView, retire_proposal,
};
pub use review_fixes::{FixOutcome, FixedVersion, MAX_CHECKS_CHARS, MAX_FIXES, ReviewFix};
pub use scopes::{SuggestedScope, scope_suggestion};
pub use verdicts::SubmittedReview;
pub use views::{
    CommentCounts, ProposalView, ReviewView, TaskRequirements, direct_conflicted,
    integration_conflicted,
};

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

impl From<MessageError> for ReviewError {
    fn from(error: MessageError) -> Self {
        match error {
            MessageError::Fold(error) => Self::Fold(error),
            MessageError::Store(error) => Self::Store(error),
            MessageError::Event(error) => Self::Event(error),
        }
    }
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

    /// The code a log line names: the stable code, or `internal_error` for an internal failure.
    pub fn log_code(&self) -> &'static str {
        self.code().unwrap_or(fold_core::codes::INTERNAL_ERROR)
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

impl From<ProjectError> for ReviewError {
    fn from(error: ProjectError) -> Self {
        match error {
            ProjectError::Fold(error) => Self::Fold(error),
            ProjectError::Store(error) => Self::Store(error),
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
    registry: Option<Store>,
    integration: Arc<tokio::sync::Mutex<()>>,
    /// The project's background check runs and the machine-wide permits they take.
    checks: CheckRunner,
}

impl std::fmt::Debug for ReviewService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewService").finish_non_exhaustive()
    }
}

impl ReviewService {
    pub(crate) fn registry(&self) -> Option<&Store> {
        self.registry.as_ref()
    }

    /// `integration` is the project's integration mutex (`ProjectHandle::integration`). Every review service of a project must share it, because it is what keeps two integrations from running at the same time.
    pub fn new(
        store: Store,
        bus: EventBus,
        clock: Arc<dyn Clock>,
        workspaces: WorkspaceService,
        integration: Arc<tokio::sync::Mutex<()>>,
    ) -> Self {
        Self {
            store,
            bus,
            clock,
            workspaces,
            registry: None,
            integration,
            checks: CheckRunner::default(),
        }
    }

    /// Runs the project's checks through `checks` (`ProjectHandle::checks`), so every review service of the project shares its runs and the machine-wide permits.
    #[must_use]
    pub fn with_checks(mut self, checks: CheckRunner) -> Self {
        self.checks = checks;
        self
    }

    /// The review claim follows the same lease as the implementation claims of `workspaces`, which in the daemon reads the project's `config.toml` on each use.
    async fn lease_minutes(&self) -> Result<u64, ReviewError> {
        Ok(self
            .workspaces
            .lease()
            .staleness_minutes(&self.store)
            .await?)
    }

    /// The host registry, which says which humans exist. Without it no human is known, so a mark that names a reviewer is refused with `unknown_human`.
    #[must_use]
    pub fn with_registry(mut self, registry: Store) -> Self {
        self.registry = Some(registry);
        self
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
