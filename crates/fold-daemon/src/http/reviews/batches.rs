use axum::Json;
use axum::extract::{Path, State};
use fold_core::{ProposalId, ReviewBatchId, TaskId, Timestamp, WorkspaceId};
use fold_store::reviews::batches::BatchState;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::super::Project;
use super::super::error::{ApiError, ApiQuery, ErrorEnvelope};
use super::super::tasks::id;
use super::{ProposalStateView, ReviewVerdict, service};
use crate::daemon::AppState;
use crate::services::reviews::ReviewBatchView as Batch;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_review_batches))
        .routes(routes!(get_review_batch))
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
    /// Relative to the project root, such as `ws/review-rb-3`.
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
