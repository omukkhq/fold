use axum::Json;
use axum::extract::{Path, State};
use fold_core::reviews::batches::BatchOutcome;
use fold_core::{
    CheckRunId, ProposalId, ReviewBatchId, RevisionId, TaskId, Timestamp, WorkspaceId,
};
use fold_store::reviews::batches::BatchState;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::super::Project;
use super::super::actor::Human;
use super::super::error::{ApiError, ApiQuery, ErrorEnvelope};
use super::super::tasks::id;
use super::super::workspaces::ComparisonTotals;
use super::{ProposalStateView, ReviewVerdict, service};
use crate::daemon::AppState;
use crate::services::reviews::{
    BatchIntegration, BatchIntegrationView, IntegrationOutcome, MemberIntegration,
    ReviewBatchView as Batch,
};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_review_batches))
        .routes(routes!(get_review_batch))
        .routes(routes!(integrate_review_batch))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewBatchState {
    Open,
    Submitted,
    /// Every member left or ended without a batch review.
    Closed,
}

impl From<BatchState> for ReviewBatchState {
    fn from(state: BatchState) -> Self {
        match state {
            BatchState::Open => Self::Open,
            BatchState::Submitted => Self::Submitted,
            BatchState::Closed => Self::Closed,
        }
    }
}

/// One member of a batch.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewBatchMember {
    #[schema(value_type = String, example = "P-3")]
    pub proposal: ProposalId,
    #[schema(value_type = String, example = "T-12")]
    pub task: TaskId,
    /// The member's place in merge order, from 1.
    pub position: u32,
    /// The version the batch reviews: the one the proposal had when it joined.
    pub version: u32,
    /// The proposal's current version. When it is higher than `version`, the batch's verdict leaves this member out.
    pub current_version: u32,
    pub proposal_state: ProposalStateView,
    /// False once the member was removed or the batch was submitted.
    pub in_batch: bool,
    /// For an open proposal with a version after the newest one that has a verdict, by any reviewer, or after the one it joined at: that earlier version and the size of the change after it.
    #[schema(required = true)]
    pub since: Option<ReviewBatchSince>,
}

/// The change of a member's current version after an earlier version.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewBatchSince {
    pub version: u32,
    pub totals: ComparisonTotals,
}

/// A verdict the batch gave.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewBatchVerdict {
    #[schema(value_type = String, example = "P-3")]
    pub proposal: ProposalId,
    /// The member version the verdict is about.
    pub version: u32,
    pub verdict: ReviewVerdict,
}

/// The active checkout of a batch: main with every member merged in order.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewBatchCheckout {
    #[schema(value_type = String, example = "ws-12")]
    pub workspace: WorkspaceId,
    /// Relative to the project root, such as `.fold/ws/review-rb-3`.
    pub path: String,
    /// The revision of main the members were merged onto.
    #[schema(value_type = String, example = "r17")]
    pub base: fold_core::RevisionId,
}

/// Related proposals one reviewer reviews together, with one verdict per member.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewBatch {
    #[schema(value_type = String, example = "rb-3")]
    pub id: ReviewBatchId,
    /// The worker that holds the batch and the review claims of its members, such as `review/batch`.
    pub holder: String,
    pub state: ReviewBatchState,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub created_at: Timestamp,
    /// UTC milliseconds; null while the batch is open.
    #[schema(value_type = Option<i64>, required = true)]
    pub submitted_at: Option<Timestamp>,
    /// UTC milliseconds; null unless the batch closed without a batch review.
    #[schema(value_type = Option<i64>, required = true)]
    pub closed_at: Option<Timestamp>,
    /// Every member the batch has had, in merge order.
    pub members: Vec<ReviewBatchMember>,
    #[schema(required = true)]
    pub checkout: Option<ReviewBatchCheckout>,
    pub verdicts: Vec<ReviewBatchVerdict>,
    /// The comments about members together, in number order.
    pub comments: Vec<ReviewBatchComment>,
    pub integration: ReviewBatchIntegration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewBatchIntegrationState {
    /// The batch is open or closed, so its members do not integrate together.
    NotSubmitted,
    /// Members without a final state hold the approved ones back; `pending` names them.
    Waiting,
    /// Every member has its final state; `members` integrate together, none when it is empty.
    Ready,
    /// The batch's integration ended; `outcome` says how.
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewBatchOutcome {
    /// The ready members were published together after one check run.
    Together,
    /// The one check run failed, so each member integrated on its own.
    OneAtATime,
    /// No member was ready to integrate with the batch.
    None,
}

impl From<BatchOutcome> for ReviewBatchOutcome {
    fn from(outcome: BatchOutcome) -> Self {
        match outcome {
            BatchOutcome::Together => Self::Together,
            BatchOutcome::OneAtATime => Self::OneAtATime,
            BatchOutcome::Nothing => Self::None,
        }
    }
}

/// A member the batch published together with the others.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewBatchRevision {
    #[schema(value_type = String, example = "P-3")]
    pub proposal: ProposalId,
    /// The version that was integrated.
    pub version: u32,
    #[schema(value_type = String, example = "r18")]
    pub revision: RevisionId,
}

/// Whether the approved members of a submitted batch integrate together now, wait for the others, or did.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewBatchIntegration {
    pub state: ReviewBatchIntegrationState,
    /// While `waiting`: the members without a final state.
    #[schema(value_type = Vec<String>, example = json!(["P-4"]))]
    pub pending: Vec<ProposalId>,
    /// While `ready`: the members that integrate together, in merge order.
    #[schema(value_type = Vec<String>, example = json!(["P-3", "P-5"]))]
    pub members: Vec<ProposalId>,
    /// While `ready`: a request to integrate the batch waits in the integration queue.
    pub queued: bool,
    /// Once `ended`.
    #[schema(required = true)]
    pub outcome: Option<ReviewBatchOutcome>,
    /// Once `ended`: the one check run of the members together, when integration checks ran.
    #[schema(value_type = Option<String>, required = true, example = "cr-9")]
    pub run: Option<CheckRunId>,
    /// Once `ended`: the members published together, in revision order.
    pub revisions: Vec<ReviewBatchRevision>,
}

impl From<BatchIntegrationView> for ReviewBatchIntegration {
    fn from(view: BatchIntegrationView) -> Self {
        let mut out = Self {
            state: ReviewBatchIntegrationState::NotSubmitted,
            pending: Vec::new(),
            members: Vec::new(),
            queued: false,
            outcome: None,
            run: None,
            revisions: Vec::new(),
        };
        match view {
            BatchIntegrationView::NotSubmitted => {}
            BatchIntegrationView::Waiting { pending } => {
                out.state = ReviewBatchIntegrationState::Waiting;
                out.pending = pending;
            }
            BatchIntegrationView::Ready { members, queued } => {
                out.state = ReviewBatchIntegrationState::Ready;
                out.members = members;
                out.queued = queued;
            }
            BatchIntegrationView::Ended {
                outcome,
                run,
                revisions,
            } => {
                out.state = ReviewBatchIntegrationState::Ended;
                out.outcome = Some(outcome.into());
                out.run = run;
                out.revisions = revisions
                    .into_iter()
                    .map(|row| ReviewBatchRevision {
                        proposal: row.proposal,
                        version: row.version,
                        revision: row.revision,
                    })
                    .collect();
            }
        }
        out
    }
}

/// A comment of a batch review about members together. Each member it is on has a whole-version comment that links to it.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewBatchComment {
    /// The comment's number in the batch, from 1.
    pub number: u32,
    pub body: String,
    /// The proposals the comment is on, in number order.
    #[schema(value_type = Vec<String>, example = json!(["P-3", "P-4"]))]
    pub members: Vec<ProposalId>,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub created_at: Timestamp,
}

impl From<Batch> for ReviewBatch {
    fn from(batch: Batch) -> Self {
        Self {
            id: batch.id,
            holder: batch.holder.as_str().to_owned(),
            state: batch.state.into(),
            created_at: batch.created_at,
            submitted_at: batch.submitted_at,
            closed_at: batch.closed_at,
            members: batch
                .members
                .into_iter()
                .map(|member| ReviewBatchMember {
                    proposal: member.proposal,
                    task: member.task,
                    position: member.position,
                    version: member.version,
                    current_version: member.current_version,
                    proposal_state: member.proposal_state.into(),
                    in_batch: member.released.is_none(),
                    since: member.since.map(|since| ReviewBatchSince {
                        version: since.version,
                        totals: since.totals.into(),
                    }),
                })
                .collect(),
            checkout: batch.checkout.map(|checkout| ReviewBatchCheckout {
                workspace: checkout.id,
                path: checkout.path,
                base: checkout.base,
            }),
            verdicts: batch
                .verdicts
                .into_iter()
                .map(|verdict| ReviewBatchVerdict {
                    proposal: verdict.proposal,
                    version: verdict.version,
                    verdict: verdict.verdict.into(),
                })
                .collect(),
            comments: batch
                .comments
                .into_iter()
                .map(|comment| ReviewBatchComment {
                    number: comment.number,
                    body: comment.body,
                    members: comment.members,
                    created_at: comment.created_at,
                })
                .collect(),
            integration: batch.integration.into(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewBatchList {
    /// In number order.
    pub batches: Vec<ReviewBatch>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReviewBatchQuery {
    /// Only the batches that are not submitted yet.
    #[serde(default)]
    pub open: Option<bool>,
}

/// The project's review batches with their members, checkout and verdicts.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/reviews/batches",
    operation_id = "list_review_batches",
    params(("slug" = String, Path, description = "The project slug"), ReviewBatchQuery),
    responses(
        (status = 200, description = "The review batches", body = ReviewBatchList),
        (status = 400, description = "`invalid_request`: the query string is not valid", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`: the slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_review_batches(
    State(state): State<AppState>,
    project: Project,
    ApiQuery(query): ApiQuery<ReviewBatchQuery>,
) -> Result<Json<ReviewBatchList>, ApiError> {
    let batches = service(&state, &project)
        .list_review_batches(query.open.unwrap_or(false))
        .await?;
    Ok(Json(ReviewBatchList {
        batches: batches.into_iter().map(ReviewBatch::from).collect(),
    }))
}

/// One review batch.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/reviews/batches/{id}",
    operation_id = "get_review_batch",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The review batch id, such as rb-3"),
    ),
    responses(
        (status = 200, description = "The batch", body = ReviewBatch),
        (status = 400, description = "`invalid_id`", body = ErrorEnvelope),
        (status = 404, description = "`review_batch_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn get_review_batch(
    State(state): State<AppState>,
    project: Project,
    Path((_slug, batch)): Path<(String, String)>,
) -> Result<Json<ReviewBatch>, ApiError> {
    let batch: ReviewBatchId = id(&batch)?;
    let view = service(&state, &project).get_review_batch(batch).await?;
    Ok(Json(ReviewBatch::from(view)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BatchIntegrationResultKind {
    /// The ready members were published together after one check run; a member that conflicted went back alone after them.
    Together,
    /// The one check run failed, so fold integrated the members one at a time, each with its own run.
    OneAtATime,
    /// No member was ready to integrate with the batch.
    Nothing,
    /// The project has integration checks, so the batch waits its turn and runs in the background; its results come as events.
    Queued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BatchMemberResultKind {
    Succeeded,
    /// The member conflicted with main and went back to its author with the conflict report.
    Conflicted,
    /// The member's own integration checks failed and it went back to its author.
    ChecksFailed,
}

/// What one member's integration did.
#[derive(Debug, Serialize, ToSchema)]
pub struct BatchMemberResult {
    #[schema(value_type = String, example = "P-3")]
    pub proposal: ProposalId,
    pub result: BatchMemberResultKind,
    #[schema(value_type = Option<String>, required = true, example = "r18")]
    pub revision: Option<RevisionId>,
    /// The member's own failed check run.
    #[schema(value_type = Option<String>, required = true, example = "cr-9")]
    pub run: Option<CheckRunId>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BatchIntegrationResult {
    #[schema(value_type = String, example = "rb-3")]
    pub batch: ReviewBatchId,
    pub result: BatchIntegrationResultKind,
    /// The failed check run of the members together, for `one_at_a_time`.
    #[schema(value_type = Option<String>, required = true, example = "cr-9")]
    pub run: Option<CheckRunId>,
    /// Each member's integration in the order it ran.
    pub members: Vec<BatchMemberResult>,
}

fn member_result(member: MemberIntegration) -> Option<BatchMemberResult> {
    let (result, revision, run) = match member.outcome {
        IntegrationOutcome::Succeeded { revision } => {
            (BatchMemberResultKind::Succeeded, Some(revision), None)
        }
        IntegrationOutcome::Conflicted(_) => (BatchMemberResultKind::Conflicted, None, None),
        IntegrationOutcome::ChecksFailed { run } => {
            (BatchMemberResultKind::ChecksFailed, None, Some(run))
        }
        IntegrationOutcome::Queued => return None,
    };
    Some(BatchMemberResult {
        proposal: member.proposal,
        result,
        revision,
        run,
    })
}

/// A named human integrates the approved members of a submitted batch together, once every member has its final state: one run of the integration checks on their merge with main, then one revision per member in one transaction. A member that conflicts goes back alone; a failed run integrates the members one at a time.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/reviews/batches/{id}/integrate",
    operation_id = "integrate_review_batch",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The review batch id, such as rb-3"),
    ),
    responses(
        (status = 200, description = "Integrated together, one at a time, nothing to integrate, or queued behind other integrations when the project has integration checks", body = BatchIntegrationResult),
        (status = 400, description = "`invalid_request` (`details.reason` is `batch_not_submitted`, `batch_waiting` with `details.pending`, `batch_integrated`, or `integration_queued`), `invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`review_batch_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "A gate refusal for a member, with `details.batch` and `details.proposal`: `changes_requested`, `not_approved`, `human_approval_missing` (`details.reviewer`), `agent_approval_required` (a marked task whose version no reviewer agent approved), `prerequisites_not_done` (`details.prerequisites`) or `proposal_not_open`; or `head_moved`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`, or `integration_failed` (`details.cause`)", body = ErrorEnvelope),
        (status = 500, description = "Internal error, or `workspace_io` with `details.reason` `fold_failed` when fold itself could not run the integration checks", body = ErrorEnvelope),
    )
)]
pub async fn integrate_review_batch(
    State(state): State<AppState>,
    human: Human,
    project: Project,
    Path((_slug, batch)): Path<(String, String)>,
) -> Result<Json<BatchIntegrationResult>, ApiError> {
    let batch: ReviewBatchId = id(&batch)?;
    let outcome = service(&state, &project)
        .integrate_review_batch(&human.actor(), batch)
        .await?;
    let (result, run, members) = match outcome {
        BatchIntegration::Together { members } => {
            (BatchIntegrationResultKind::Together, None, members)
        }
        BatchIntegration::OneAtATime { run, members } => {
            (BatchIntegrationResultKind::OneAtATime, Some(run), members)
        }
        BatchIntegration::Nothing => (BatchIntegrationResultKind::Nothing, None, Vec::new()),
        BatchIntegration::Queued => (BatchIntegrationResultKind::Queued, None, Vec::new()),
    };
    Ok(Json(BatchIntegrationResult {
        batch,
        result,
        run,
        members: members.into_iter().filter_map(member_result).collect(),
    }))
}
