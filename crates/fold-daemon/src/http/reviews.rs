use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use fold_core::merge::{
    ConflictReport, HunkSide, ReportEntry, ReportEntryKind, ReportFile, ReportHunk, ReportKind,
};
use fold_core::policy::{Gate, GateState, GateStatus};
use fold_core::reviews::{ReviewScope, ReviewScopeRecord, ScopeRule, Side, Verdict};
use fold_core::{
    FoldError, HumanName, ProposalId, ReviewBatchId, RevisionId, TaskId, Timestamp, WorkspaceId,
};
use fold_store::reviews::ProposalState;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::Project;
use super::actor::Human;
use super::diff::{DiffView, comparison_view, file_view};
use super::error::{ApiError, ApiJson, ApiQuery, ErrorEnvelope};
use super::messages::MessageView;
use super::tasks::{HumanReviewMark, TaskStatus, TaskView, id, task_view};
use super::workspaces::{
    ChangeSource, ComparedFile, ComparisonTotals, FileContentView, file_view as content_view,
};
use crate::daemon::AppState;
use crate::services::reviews::{
    self as service, ChangeRange, CheckoutTarget, CommentInput, CommentLocation,
    IntegrationOutcome, ListedCheckout, ProposalFilter, ReviewService, Turn,
    integration_conflicted,
};

pub mod batches;
pub mod checks;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .merge(batches::routes())
        .merge(checks::routes())
        .routes(routes!(set_human_review, clear_human_review))
        .routes(routes!(list_proposals))
        .routes(routes!(get_proposal))
        .routes(routes!(get_proposal_diff))
        .routes(routes!(list_review_comments, add_review_comment))
        .routes(routes!(ask_proposal_author))
        .routes(routes!(submit_review))
        .routes(routes!(integrate_proposal))
        .routes(routes!(read_proposal_file))
        .routes(routes!(edit_proposal_file))
        .routes(routes!(send_for_review_again))
        .routes(routes!(list_review_checkouts))
}

fn service(state: &AppState, project: &Project) -> ReviewService {
    project.0.reviews(&state.registry)
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct HumanReviewBody {
    /// The one human whose approval counts; left out or null, any human may approve. It must be a human who has used the CLI or the web interface.
    #[serde(default)]
    pub reviewer: Option<String>,
    /// Why a human must look; not empty, at most 500 characters.
    pub reason: String,
}

/// Turns on the human review mark, or changes the reviewer and the reason. The mark never changes the task's status; a proposal of a marked task waits for a human approval of its current version.
#[utoipa::path(
    put,
    path = "/api/v1/projects/{slug}/tasks/{id}/human-review",
    operation_id = "set_human_review",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    request_body = HumanReviewBody,
    responses(
        (status = 200, description = "The mark as it now stands", body = HumanReviewMark),
        (status = 400, description = "`invalid_request` (`details.field` names the field), `invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found`, `unknown_human`: the reviewer is not a known human, or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`task_closed`: the task is Done or archived", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn set_human_review(
    State(state): State<AppState>,
    human: Human,
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
    ApiJson(body): ApiJson<HumanReviewBody>,
) -> Result<Json<HumanReviewMark>, ApiError> {
    let task: TaskId = id(&task)?;
    let reviewer = body
        .reviewer
        .as_deref()
        .map(HumanName::parse)
        .transpose()
        .map_err(FoldError::from)?;
    let mark = service(&state, &project)
        .require_human_review(&human.actor(), task, reviewer, &body.reason)
        .await?;
    Ok(Json(mark.into()))
}

/// Turns the human review mark off. Clearing a task without a mark changes nothing.
#[utoipa::path(
    delete,
    path = "/api/v1/projects/{slug}/tasks/{id}/human-review",
    operation_id = "clear_human_review",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    responses(
        (status = 200, description = "The task without a mark", body = TaskView),
        (status = 400, description = "`invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`task_closed`: the task is Done or archived, or `invalid_config`: the task has a claim and the project configuration that sets its lease is invalid", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn clear_human_review(
    State(state): State<AppState>,
    human: Human,
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
) -> Result<Json<TaskView>, ApiError> {
    let task: TaskId = id(&task)?;
    service(&state, &project)
        .clear_human_review(&human.actor(), task)
        .await?;
    Ok(Json(task_view(
        project.0.tasks.get_view(task).await?,
        Some(&human),
    )))
}

/// Where a proposal stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStateView {
    Open,
    Integrated,
    Closed,
}

impl From<ProposalState> for ProposalStateView {
    fn from(state: ProposalState) -> Self {
        match state {
            ProposalState::Open => Self::Open,
            ProposalState::Integrated => Self::Integrated,
            ProposalState::Closed => Self::Closed,
        }
    }
}

impl From<ProposalStateView> for ProposalState {
    fn from(state: ProposalStateView) -> Self {
        match state {
            ProposalStateView::Open => Self::Open,
            ProposalStateView::Integrated => Self::Integrated,
            ProposalStateView::Closed => Self::Closed,
        }
    }
}

/// Whose move it is on an open proposal: with the reviewer the task is in Review, with the author it is In Progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalTurn {
    WithReviewer,
    WithAuthor,
}

impl From<Turn> for ProposalTurn {
    fn from(turn: Turn) -> Self {
        match turn {
            Turn::WithReviewer => Self::WithReviewer,
            Turn::WithAuthor => Self::WithAuthor,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Comment,
    RequestChanges,
    Approve,
    Reject,
}

impl From<Verdict> for ReviewVerdict {
    fn from(verdict: Verdict) -> Self {
        match verdict {
            Verdict::Comment => Self::Comment,
            Verdict::RequestChanges => Self::RequestChanges,
            Verdict::Approve => Self::Approve,
            Verdict::Reject => Self::Reject,
        }
    }
}

impl From<ReviewVerdict> for Verdict {
    fn from(verdict: ReviewVerdict) -> Self {
        match verdict {
            ReviewVerdict::Comment => Self::Comment,
            ReviewVerdict::RequestChanges => Self::RequestChanges,
            ReviewVerdict::Approve => Self::Approve,
            ReviewVerdict::Reject => Self::Reject,
        }
    }
}

/// Which tree a line comment reads: `old` is the version's base, `new` the version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommentSide {
    Old,
    New,
}

impl From<Side> for CommentSide {
    fn from(side: Side) -> Self {
        match side {
            Side::Old => Self::Old,
            Side::New => Self::New,
        }
    }
}

impl From<CommentSide> for Side {
    fn from(side: CommentSide) -> Self {
        match side {
            CommentSide::Old => Self::Old,
            CommentSide::New => Self::New,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ProposalSummary {
    #[schema(example = "P-3")]
    pub id: String,
    #[schema(example = "T-12")]
    pub task: String,
    pub title: String,
    /// The worker that opened the proposal, named as its agent.
    pub author: String,
    pub state: ProposalStateView,
    /// The latest integration of the current version conflicted with main.
    pub conflicted: bool,
    /// The integration checks of the current version failed on its merge with main, so the proposal is with its author until a new version.
    pub checks_failed: bool,
    /// The highest version number.
    pub current_version: u32,
    /// Null once the proposal is no longer open.
    #[schema(required = true)]
    pub turn: Option<ProposalTurn>,
    /// The latest verdict on the current version; comments are not verdicts.
    #[schema(required = true)]
    pub latest_verdict: Option<ReviewVerdict>,
    /// The task's human review mark is met on the current version: a human it accepts approved it and no later `request_changes` withdrew that. True without a mark, so an approved proposal with this false is an agent's approval that waits for a human.
    pub human_approved: bool,
    /// A request to integrate the current version waits in the integration queue; it ends when fold integrates, refuses or sends the proposal back.
    pub integration_queued: bool,
    /// The worker that holds the review claim.
    #[schema(required = true)]
    pub review_claim: Option<String>,
    /// The task's human review mark.
    #[schema(required = true)]
    pub human_review: Option<HumanReviewMark>,
    /// Why a closed proposal closed.
    #[schema(required = true)]
    pub close_reason: Option<String>,
    /// UTC milliseconds.
    pub created_at: i64,
    /// UTC milliseconds.
    pub updated_at: i64,
    /// UTC milliseconds.
    #[schema(required = true)]
    pub closed_at: Option<i64>,
}

impl From<service::ProposalSummary> for ProposalSummary {
    fn from(summary: service::ProposalSummary) -> Self {
        Self {
            id: summary.id.to_string(),
            task: summary.task.to_string(),
            title: summary.title,
            author: summary.author.to_string(),
            state: summary.state.into(),
            conflicted: summary.conflicted,
            checks_failed: summary.checks_failed,
            current_version: summary.current_version,
            turn: summary.turn.map(ProposalTurn::from),
            latest_verdict: summary.latest_verdict.map(ReviewVerdict::from),
            human_approved: summary.human_approved,
            integration_queued: summary.integration_queued,
            review_claim: summary.review_claim.map(|worker| worker.to_string()),
            human_review: summary.human_review.map(HumanReviewMark::from),
            close_reason: summary.close_reason,
            created_at: summary.created_at.as_millis(),
            updated_at: summary.updated_at.as_millis(),
            closed_at: summary.closed_at.map(|at| at.as_millis()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ProposalList {
    /// In number order.
    pub proposals: Vec<ProposalSummary>,
    /// The open proposals of the whole project, whatever the filter, whose current version waits for the signed-in human's review, in number order: the `human_review` items of Needs you that are theirs, which the Proposals screen marks. Empty when the request names no human.
    #[schema(example = json!(["P-3"]))]
    pub waiting_for_you: Vec<String>,
}

/// One immutable version of a proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ProposalVersion {
    pub version: u32,
    /// The revision the version is based on.
    #[schema(example = "r17")]
    pub base: String,
    /// The changeset that holds the proposed tree.
    #[schema(example = "C-40")]
    pub changeset: String,
    /// The proposed tree's hash.
    pub tree: String,
    pub files_added: u32,
    pub files_modified: u32,
    pub files_deleted: u32,
    /// The author's evidence that the work is done.
    #[schema(required = true)]
    pub evidence: Option<String>,
    /// The worker, the reviewer's worker for a reviewer's fix, or `human:<name>` for a human edit.
    pub created_by: String,
    /// UTC milliseconds.
    pub created_at: i64,
    /// A human made this version by editing the one before it in a human review. It keeps the agent approvals of that version until it is sent for review again; a human approves it again.
    pub human_edit: bool,
    /// A human sent this version for review again, so the approvals it kept no longer count.
    pub review_again: bool,
    /// The reviewer that holds the review claim made this version by fixing the one before it in its review checkout. It starts without approvals like every version.
    pub reviewer_fix: bool,
}

impl From<service::VersionView> for ProposalVersion {
    fn from(version: service::VersionView) -> Self {
        Self {
            version: version.version,
            base: version.base.to_string(),
            changeset: version.changeset.to_string(),
            tree: version.tree.to_string(),
            files_added: version.stats.files_added,
            files_modified: version.stats.files_modified,
            files_deleted: version.stats.files_deleted,
            human_edit: version.human_edit,
            evidence: version.evidence,
            created_by: version.created_by,
            created_at: version.created_at.as_millis(),
            review_again: version.review_again,
            reviewer_fix: version.fixed_in_checkout.is_some(),
        }
    }
}

/// How much of a revised version a review reads: the change since the earlier version with a verdict and the code it touches, or the whole version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewScopeKind {
    Change,
    Full,
}

impl From<ReviewScope> for ReviewScopeKind {
    fn from(scope: ReviewScope) -> Self {
        match scope {
            ReviewScope::Change => Self::Change,
            ReviewScope::Full => Self::Full,
        }
    }
}

/// A fact fold checks that makes it suggest a full review of a revised version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewScopeRule {
    /// The change touches a file that the earlier version did not change.
    NewFiles,
    /// The change has more added and removed lines than half of the earlier version's own change.
    LargeDelta,
    /// The author merged a newer main into files of the earlier version's change by hand.
    BaseConflicts,
    /// The version is the third or later.
    #[serde(rename = "v3_or_later")]
    V3OrLater,
}

impl From<ScopeRule> for ReviewScopeRule {
    fn from(rule: ScopeRule) -> Self {
        match rule {
            ScopeRule::NewFiles => Self::NewFiles,
            ScopeRule::LargeDelta => Self::LargeDelta,
            ScopeRule::BaseConflicts => Self::BaseConflicts,
            ScopeRule::ThirdVersion => Self::V3OrLater,
        }
    }
}

/// The scope a review of a revised version followed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ProposalReviewScope {
    /// The scope the review followed: the review lead's choice when it made one, else fold's suggestion.
    pub scope: ReviewScopeKind,
    /// The earlier version with a verdict that the change is read since.
    pub since: u32,
    /// fold's suggestion from the change since that version.
    pub suggested: ReviewScopeKind,
    /// The rules that made fold suggest a full review, in their declared order; empty for a change review.
    pub rules: Vec<ReviewScopeRule>,
    /// Why the review lead chose this scope; null when the review followed the suggestion.
    #[schema(required = true)]
    pub reason: Option<String>,
}

impl From<ReviewScopeRecord> for ProposalReviewScope {
    fn from(record: ReviewScopeRecord) -> Self {
        Self {
            scope: record.scope.into(),
            since: record.since,
            suggested: record.suggested.into(),
            rules: record
                .rules
                .into_iter()
                .map(ReviewScopeRule::from)
                .collect(),
            reason: record.reason,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ProposalReview {
    pub version: u32,
    pub verdict: ReviewVerdict,
    /// The worker, or `human:<name>`.
    pub reviewer: String,
    pub body: String,
    /// How many comments came with the review.
    pub comments: u32,
    /// UTC milliseconds.
    pub created_at: i64,
    /// For an approval that a human edit kept, the version whose approval it copies.
    #[schema(required = true)]
    pub kept_from: Option<u32>,
    /// The scope of an agent's review of a revised version; null for a version reviewed whole and for a human's review.
    #[schema(required = true)]
    pub scope: Option<ProposalReviewScope>,
}

impl From<service::ReviewView> for ProposalReview {
    fn from(review: service::ReviewView) -> Self {
        Self {
            version: review.version,
            verdict: review.verdict.into(),
            reviewer: review.reviewer.to_string(),
            body: review.body,
            comments: review.comments,
            created_at: review.created_at.as_millis(),
            kept_from: review.kept_from,
            scope: review.scope.map(ProposalReviewScope::from),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
pub struct CommentCounts {
    pub total: u32,
    /// On the selected version.
    pub on_version: u32,
    /// On a version before the current one.
    pub outdated: u32,
}

/// The gates in the order the integration procedure checks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateName {
    Open,
    Version,
    Caller,
    Approval,
    HumanReview,
    Prerequisites,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateStatusKind {
    Met,
    Unmet,
    /// Only the integration procedure can tell, such as whether the version merges cleanly.
    CheckedAtIntegration,
}

/// One gate between the requesting human and integrating the selected version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ProposalGate {
    pub gate: GateName,
    pub status: GateStatusKind,
    /// The refusal an integration would get from an unmet gate, such as `not_approved`.
    #[schema(required = true)]
    pub code: Option<String>,
}

impl From<GateState> for ProposalGate {
    fn from(state: GateState) -> Self {
        let gate = match state.gate {
            Gate::Open => GateName::Open,
            Gate::Version => GateName::Version,
            Gate::Caller => GateName::Caller,
            Gate::Approval => GateName::Approval,
            Gate::HumanReview => GateName::HumanReview,
            Gate::Prerequisites => GateName::Prerequisites,
            Gate::Merge => GateName::Merge,
        };
        let (status, code) = match state.status {
            GateStatus::Met => (GateStatusKind::Met, None),
            GateStatus::Unmet(refusal) => (GateStatusKind::Unmet, Some(refusal.code().to_owned())),
            GateStatus::CheckedAtIntegration => (GateStatusKind::CheckedAtIntegration, None),
        };
        Self { gate, status, code }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ReviewClaim {
    /// The worker, named as its agent.
    pub holder: String,
    /// UTC milliseconds.
    pub claimed_at: i64,
    /// UTC milliseconds of the last lease renewal.
    pub renewed_at: i64,
    /// True once the lease ran out without a renewal.
    pub stale: bool,
}

/// A task as the proposal shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ProposalTask {
    #[schema(example = "T-12")]
    pub task: String,
    pub title: String,
    /// Markdown; empty for a prerequisite.
    pub description: String,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    Text,
    BothModifiedBinary,
    ModifyDelete,
    AddAdd,
    TypeChange,
}

impl From<ReportKind> for ConflictKind {
    fn from(kind: ReportKind) -> Self {
        match kind {
            ReportKind::Text => Self::Text,
            ReportKind::BothModifiedBinary => Self::BothModifiedBinary,
            ReportKind::ModifyDelete => Self::ModifyDelete,
            ReportKind::AddAdd => Self::AddAdd,
            ReportKind::TypeChange => Self::TypeChange,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConflictEntryKind {
    File,
    Executable,
    Symlink,
    Directory,
}

/// What one side of a merge has at a conflicting path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ConflictEntry {
    pub kind: ConflictEntryKind,
    pub hash: String,
}

impl From<ReportEntry> for ConflictEntry {
    fn from(entry: ReportEntry) -> Self {
        let kind = match entry.kind {
            ReportEntryKind::File => ConflictEntryKind::File,
            ReportEntryKind::Executable => ConflictEntryKind::Executable,
            ReportEntryKind::Symlink => ConflictEntryKind::Symlink,
            ReportEntryKind::Directory => ConflictEntryKind::Directory,
        };
        Self {
            kind,
            hash: entry.hash.to_string(),
        }
    }
}

/// The lines of one side of a hunk, each with its terminator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ConflictHunkSide {
    /// The number, from 1, of the first line; an empty side starts at the line after the span's position.
    pub start: usize,
    pub count: usize,
    pub lines: Vec<String>,
}

impl From<HunkSide> for ConflictHunkSide {
    fn from(side: HunkSide) -> Self {
        Self {
            start: side.start,
            count: side.count,
            lines: side.lines,
        }
    }
}

/// One span of a text file that main and the version changed differently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ConflictHunk {
    pub base: ConflictHunkSide,
    pub main: ConflictHunkSide,
    pub version: ConflictHunkSide,
}

impl From<ReportHunk> for ConflictHunk {
    fn from(hunk: ReportHunk) -> Self {
        Self {
            base: hunk.base.into(),
            main: hunk.main.into(),
            version: hunk.version.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ConflictFile {
    pub path: String,
    pub kind: ConflictKind,
    /// Null where that side has nothing at the path.
    #[schema(required = true)]
    pub base: Option<ConflictEntry>,
    #[schema(required = true)]
    pub main: Option<ConflictEntry>,
    #[schema(required = true)]
    pub version: Option<ConflictEntry>,
    /// Empty unless `kind` is `text`.
    pub hunks: Vec<ConflictHunk>,
}

impl From<ReportFile> for ConflictFile {
    fn from(file: ReportFile) -> Self {
        Self {
            path: file.path,
            kind: file.kind.into(),
            base: file.base.map(ConflictEntry::from),
            main: file.main.map(ConflictEntry::from),
            version: file.version.map(ConflictEntry::from),
            hunks: file.hunks.into_iter().map(ConflictHunk::from).collect(),
        }
    }
}

/// The root trees of the three sides of the merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ConflictTrees {
    pub base: String,
    pub main: String,
    pub version: String,
}

/// Why a version could not be integrated, as the conflicted integration stored it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ConflictReportView {
    #[schema(example = "P-3")]
    pub proposal: String,
    pub version: u32,
    #[schema(example = "r17")]
    pub base_revision: String,
    /// The head of main the merge ran onto.
    #[schema(example = "r19")]
    pub main_revision: String,
    pub trees: ConflictTrees,
    /// In path order.
    pub files: Vec<ConflictFile>,
}

impl From<ConflictReport> for ConflictReportView {
    fn from(report: ConflictReport) -> Self {
        Self {
            proposal: report.proposal.to_string(),
            version: report.version,
            base_revision: report.base_revision.to_string(),
            main_revision: report.main_revision.to_string(),
            trees: ConflictTrees {
                base: report.trees.base.to_string(),
                main: report.trees.main.to_string(),
                version: report.trees.version.to_string(),
            },
            files: report.files.into_iter().map(ConflictFile::from).collect(),
        }
    }
}

/// The review plan an open proposal without a review waits in: the review lead gives it to one reviewer together with the plan's other proposals once the awaited tasks open theirs or the wait ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ReviewPlanWait {
    #[schema(example = "rp-2")]
    pub plan: String,
    /// When the wait ends at the latest, in UTC milliseconds.
    pub deadline_at: i64,
    /// The wait ended, so the plan waits only for the review lead to start its reviewer.
    pub wait_ended: bool,
    /// The tasks whose proposals the plan still waits for, by task number.
    pub awaited: Vec<String>,
    /// The plan's open proposals, this one included.
    pub proposals: Vec<String>,
}

impl From<crate::services::reviews::ProposalPlan> for ReviewPlanWait {
    fn from(plan: crate::services::reviews::ProposalPlan) -> Self {
        Self {
            plan: plan.plan.to_string(),
            deadline_at: plan.deadline_at.as_millis(),
            wait_ended: plan.wait_ended,
            awaited: plan.awaited.iter().map(ToString::to_string).collect(),
            proposals: plan.proposals.iter().map(ToString::to_string).collect(),
        }
    }
}

/// A proposal as the requesting human sees it.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProposalDetail {
    pub summary: ProposalSummary,
    /// The task's status on the board.
    pub task_status: TaskStatus,
    pub task: ProposalTask,
    /// The direct prerequisites of the task.
    pub prerequisites: Vec<ProposalTask>,
    /// Oldest first.
    pub versions: Vec<ProposalVersion>,
    /// The version that was asked for, else the current one.
    pub selected: ProposalVersion,
    #[schema(required = true)]
    pub review_claim: Option<ReviewClaim>,
    /// Every version's reviews, oldest first.
    pub reviews: Vec<ProposalReview>,
    pub comments: CommentCounts,
    /// The newest version before the selected one that has a verdict, by any reviewer, agent or human: what changed since it is what is left to review.
    #[schema(required = true)]
    pub last_reviewed_version: Option<u32>,
    /// What stands between the requesting human and integrating the selected version.
    pub gates: Vec<ProposalGate>,
    /// A reviewer agent (the main agent, the review lead or a reviewer) approved the current version and no request for changes came after it. On a task with the human review mark a human approves only then; otherwise the approval is `agent_approval_required`.
    pub agent_approved: bool,
    /// The task is in the built-in area Main, the main agent's own work: no review agent reviews it, and a human approves it without a reviewer agent first, even with the human review mark.
    pub main_task: bool,
    /// The latest conflict report of the selected version.
    #[schema(required = true)]
    pub conflict: Option<ConflictReportView>,
    /// The files the selected version changes against its base, in path order.
    pub files: Vec<ComparedFile>,
    pub totals: ComparisonTotals,
    /// The check runs of the selected version, newest first, without their logs: the proposal-stage runs of exactly its files, then its integration runs.
    pub checks: Vec<checks::CheckRun>,
    /// The check run in progress of the author's update or a reviewer's fix, which makes the next version when its checks pass; null when none runs. It has no version yet, so it is not among `checks`, and while it goes the current version is neither approved nor integrated (`update_checks_running`).
    #[schema(required = true)]
    pub update_run: Option<checks::NextVersionRun>,
    /// The review plan the open proposal waits in; null when it waits in none.
    #[schema(required = true)]
    pub review_plan: Option<ReviewPlanWait>,
}

impl From<service::ProposalView> for ProposalDetail {
    fn from(view: service::ProposalView) -> Self {
        let detail = view.detail;
        Self {
            summary: detail.summary.into(),
            task_status: detail.task_status.into(),
            task: ProposalTask {
                task: view.task.id.to_string(),
                title: view.task.title.to_string(),
                description: view.task.description.as_str().to_owned(),
                status: view.task.status.into(),
            },
            prerequisites: view
                .prerequisites
                .into_iter()
                .map(|task| ProposalTask {
                    task: task.id.to_string(),
                    title: task.title.to_string(),
                    description: String::new(),
                    status: task.status.into(),
                })
                .collect(),
            versions: detail
                .versions
                .into_iter()
                .map(ProposalVersion::from)
                .collect(),
            selected: detail.selected.into(),
            review_claim: view.claim.map(|claim| ReviewClaim {
                holder: claim.holder.to_string(),
                claimed_at: claim.acquired_at.as_millis(),
                renewed_at: claim.renewed_at.as_millis(),
                stale: claim.stale,
            }),
            reviews: view.reviews.into_iter().map(ProposalReview::from).collect(),
            comments: CommentCounts {
                total: view.comments.total,
                on_version: view.comments.selected,
                outdated: view.comments.outdated,
            },
            last_reviewed_version: view.last_reviewed,
            gates: view.gates.into_iter().map(ProposalGate::from).collect(),
            agent_approved: view.agent_approved,
            main_task: view.main_task,
            conflict: detail.conflict.map(ConflictReportView::from),
            files: view.files.into_iter().map(ComparedFile::from).collect(),
            totals: view.totals.into(),
            checks: view
                .checks
                .into_iter()
                .map(checks::CheckRun::from)
                .collect(),
            update_run: view.update_run.map(checks::NextVersionRun::from),
            review_plan: view.review_plan.map(ReviewPlanWait::from),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ReviewComment {
    #[schema(example = "rc-5")]
    pub id: String,
    pub version: u32,
    /// The verdict of the review the comment came with.
    pub verdict: ReviewVerdict,
    /// The worker, or `human:<name>`.
    pub author: String,
    /// Null for a comment on the whole version.
    #[schema(required = true)]
    pub path: Option<String>,
    #[schema(required = true)]
    pub side: Option<CommentSide>,
    #[schema(required = true)]
    pub start_line: Option<u32>,
    #[schema(required = true)]
    pub end_line: Option<u32>,
    /// The comment this one answers.
    #[schema(required = true)]
    pub reply_to: Option<String>,
    pub body: String,
    /// UTC milliseconds.
    pub created_at: i64,
    /// Made on a version before the current one.
    pub outdated: bool,
    /// Set when the comment is a comment of a review batch about members together.
    #[schema(required = true)]
    pub batch_comment: Option<CommentBatchLink>,
}

/// The comment of a review batch that a comment puts on its proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CommentBatchLink {
    #[schema(example = "rb-3")]
    pub batch: String,
    /// The comment's number in the batch, from 1.
    pub number: u32,
    /// Every proposal the batch comment is on, this one included, in number order.
    pub members: Vec<String>,
}

impl From<service::CommentView> for ReviewComment {
    fn from(comment: service::CommentView) -> Self {
        let location = comment.location;
        Self {
            id: comment.id.to_string(),
            version: comment.version,
            verdict: comment.verdict.into(),
            author: comment.author.to_string(),
            path: location.as_ref().map(|at| at.path.clone()),
            side: location.as_ref().map(|at| at.side.into()),
            start_line: location.as_ref().map(|at| at.start_line),
            end_line: location.as_ref().map(|at| at.end_line),
            reply_to: comment.reply_to.map(|reply| reply.to_string()),
            body: comment.body,
            created_at: comment.created_at.as_millis(),
            outdated: comment.outdated,
            batch_comment: comment.batch_comment.map(|link| CommentBatchLink {
                batch: link.batch.to_string(),
                number: link.number,
                members: link.members.iter().map(ToString::to_string).collect(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CommentList {
    /// In number order.
    pub comments: Vec<ReviewComment>,
}

/// One comment: on a run of lines when `path`, `side`, `start_line` and `end_line` are all given, on the whole version when none is.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CommentBody {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub side: Option<CommentSide>,
    /// From 1.
    #[serde(default)]
    pub start_line: Option<u32>,
    /// Inclusive.
    #[serde(default)]
    pub end_line: Option<u32>,
    /// A comment of the same proposal this one answers, such as rc-5.
    #[serde(default)]
    pub reply_to: Option<String>,
    pub body: String,
}

impl CommentBody {
    fn into_input(self, field: &str) -> Result<CommentInput, ApiError> {
        let location = CommentLocation::from_parts(
            field,
            self.path,
            self.side.map(Side::from),
            self.start_line,
            self.end_line,
        )?;
        Ok(CommentInput {
            location,
            reply_to: self.reply_to.as_deref().map(id).transpose()?,
            body: self.body,
        })
    }
}

/// One comment on the version the human read, as its own `comment` review.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AddCommentBody {
    /// The version the human read and commented on. When the current version is another one, nothing is written and the answer is `stale_version`.
    pub version: u32,
    #[serde(flatten)]
    pub comment: CommentBody,
}

/// A human verdict on the version the human read. The web screens offer approve, request changes and comment; the route also takes reject, which closes the proposal.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ReviewBody {
    /// The version the human read. When the current version is another one, nothing is written and the answer is `stale_version`.
    pub version: u32,
    pub verdict: ReviewVerdict,
    /// May be empty only when the review carries a comment; a reject needs it.
    pub body: String,
    #[serde(default)]
    pub comments: Vec<CommentBody>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct SubmittedReview {
    #[schema(example = "P-3")]
    pub proposal: String,
    /// The version the review is of.
    pub version: u32,
    pub verdict: ReviewVerdict,
    /// The new comments in order.
    pub comments: Vec<String>,
    /// The proposal after the review.
    pub summary: ProposalSummary,
}

impl From<service::SubmittedReview> for SubmittedReview {
    fn from(review: service::SubmittedReview) -> Self {
        Self {
            proposal: review.proposal.to_string(),
            version: review.version,
            verdict: review.verdict.into(),
            comments: review.comments.iter().map(ToString::to_string).collect(),
            summary: review.summary.into(),
        }
    }
}

/// A human's change of one text file of the version the human read, in a human review.
#[derive(Debug, Deserialize, ToSchema)]
pub struct EditFileBody {
    /// The version the human read and edited. When the current version is another one, nothing is written and the answer is `stale_version`.
    pub version: u32,
    /// The file, such as `src/lib.rs`: a text file of that version.
    pub path: String,
    /// The whole new content of the file, as UTF-8 text.
    pub content: String,
}

/// The human edit the human sends for review again.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ReviewAgainBody {
    /// The version the human read. When the current version is another one, nothing is written and the answer is `stale_version`.
    pub version: u32,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct VersionFileQuery {
    /// A version number; left out, the current version.
    #[serde(default)]
    pub version: Option<u32>,
    /// A file path relative to the version's root, such as `src/lib.rs`.
    pub path: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct IntegrateBody {
    /// The version the human reviewed. When the current version is another one, nothing integrates and the answer is `stale_version`.
    pub expected_version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationResultKind {
    Succeeded,
    /// The integration checks of the merged tree failed; the proposal went back to its author.
    ChecksFailed,
    /// The project has integration checks, so the integration waits its turn and runs in the background; its result comes as events.
    Queued,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct IntegrationResult {
    #[schema(example = "P-3")]
    pub proposal: String,
    pub result: IntegrationResultKind,
    /// The new head of main, once the proposal integrated.
    #[schema(example = "r18", required = true)]
    pub revision: Option<String>,
    /// The check run of failed integration checks.
    #[schema(example = "cr-9", required = true)]
    pub run: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ProposalListQuery {
    /// Only proposals in this state.
    #[serde(default)]
    pub state: Option<ProposalStateView>,
    /// Only proposals that no reviewer holds; a review claim whose lease has run out counts as unclaimed.
    #[serde(default)]
    pub unclaimed: Option<bool>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct VersionQuery {
    /// A version number; left out, the current version.
    #[serde(default)]
    pub version: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ProposalDiffQuery {
    /// A version number; left out, the current version.
    #[serde(default)]
    pub version: Option<u32>,
    /// An earlier version: the answer holds only what changed after it. Left out, the version against its base revision.
    #[serde(default)]
    pub since: Option<u32>,
    /// One changed file, such as `src/lib.rs`: its new path for a rename, its old path for a deletion. With it the answer holds that file's hunks; without it, the list of changed files.
    #[serde(default)]
    pub path: Option<String>,
}

fn proposal(text: &str) -> Result<ProposalId, ApiError> {
    id(text)
}

/// Lists the proposals in number order, with the ones that wait for the signed-in human's review.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/proposals",
    operation_id = "list_proposals",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ProposalListQuery,
    ),
    responses(
        (status = 200, description = "The proposals", body = ProposalList),
        (status = 400, description = "`invalid_request`: the query string is not valid", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_proposals(
    human: Option<Human>,
    State(state): State<AppState>,
    project: Project,
    ApiQuery(query): ApiQuery<ProposalListQuery>,
) -> Result<Json<ProposalList>, ApiError> {
    let filter = ProposalFilter {
        state: query.state.map(ProposalState::from),
        unclaimed: query.unclaimed.unwrap_or(false),
        limit: None,
    };
    let listing = service(&state, &project)
        .list_proposals_for(filter, human.as_ref().map(|Human(name)| name))
        .await?;
    Ok(Json(ProposalList {
        proposals: listing
            .proposals
            .into_iter()
            .map(ProposalSummary::from)
            .collect(),
        waiting_for_you: listing
            .waiting_for_human
            .iter()
            .map(ToString::to_string)
            .collect(),
    }))
}

/// One proposal as the requesting human sees it, with the gate report for that human integrating the selected version.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/proposals/{id}",
    operation_id = "get_proposal",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The proposal id, such as P-3"),
        VersionQuery,
    ),
    responses(
        (status = 200, description = "The proposal", body = ProposalDetail),
        (status = 400, description = "`invalid_id`, `invalid_name` or `invalid_request`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`proposal_not_found`: no such proposal or version, or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`invalid_config`: the proposal has a review claim and the project configuration that sets its lease is invalid", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn get_proposal(
    State(state): State<AppState>,
    human: Human,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiQuery(query): ApiQuery<VersionQuery>,
) -> Result<Json<ProposalDetail>, ApiError> {
    let view = service(&state, &project)
        .view_proposal(&human.actor(), proposal(&id)?, query.version)
        .await?;
    Ok(Json(view.into()))
}

/// The change of a proposal version, calculated on request from stored trees: against its base revision, or with `since` only what changed after an earlier version. When the version has a newer base than the earlier one, the earlier version is carried onto that base first, so the change leaves out what main gained. A version side is named by its changeset. On a change since an earlier version the new side's line numbers are the version's own, so a `new` line comment taken from them lands on the version.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/proposals/{id}/diff",
    operation_id = "get_proposal_diff",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The proposal id, such as P-3"),
        ProposalDiffQuery,
    ),
    responses(
        (status = 200, description = "The file list, or one file's hunks", body = DiffView),
        (status = 400, description = "`invalid_id`, `invalid_path`, or `invalid_request`: the query string is not valid or `since` is not a version before the one read", body = ErrorEnvelope),
        (status = 404, description = "`proposal_not_found`: no such proposal or version, `path_not_in_comparison`, or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn get_proposal_diff(
    State(state): State<AppState>,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiQuery(query): ApiQuery<ProposalDiffQuery>,
) -> Result<Json<DiffView>, ApiError> {
    let proposal = proposal(&id)?;
    let range = ChangeRange {
        version: query.version,
        since: query.since,
    };
    let reviews = service(&state, &project);
    let view = match query.path {
        Some(path) => DiffView::File(file_view(
            ChangeSource::Recorded,
            reviews
                .proposal_file_diff(proposal, range, &path)
                .await?
                .change,
        )),
        None => DiffView::Files(comparison_view(
            ChangeSource::Recorded,
            reviews.proposal_changes(proposal, range).await?.change,
        )),
    };
    Ok(Json(view))
}

/// The comments of a proposal in number order, each marked outdated when it was made on an earlier version.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/proposals/{id}/comments",
    operation_id = "list_review_comments",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The proposal id, such as P-3"),
        VersionQuery,
    ),
    responses(
        (status = 200, description = "The comments", body = CommentList),
        (status = 400, description = "`invalid_id` or `invalid_request`", body = ErrorEnvelope),
        (status = 404, description = "`proposal_not_found`: no such proposal or version, or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_review_comments(
    State(state): State<AppState>,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiQuery(query): ApiQuery<VersionQuery>,
) -> Result<Json<CommentList>, ApiError> {
    let comments = service(&state, &project)
        .list_comments(proposal(&id)?, query.version)
        .await?;
    Ok(Json(CommentList {
        comments: comments.into_iter().map(ReviewComment::from).collect(),
    }))
}

/// Adds one human comment to the version the human read, as a `comment` review; it never changes whose turn it is.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/proposals/{id}/comments",
    operation_id = "add_review_comment",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The proposal id, such as P-3"),
    ),
    request_body = AddCommentBody,
    responses(
        (status = 201, description = "The comment", body = ReviewComment),
        (status = 400, description = "`invalid_request` (`details.field` names the field), `invalid_path`, `invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`proposal_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`stale_version` (`details.expected_version`, `details.current_version`), `proposal_not_open` or `task_closed`", body = ErrorEnvelope),
        (status = 422, description = "`unknown_comment_path` or `invalid_comment_range`: the lines are not in the file on that side", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn add_review_comment(
    State(state): State<AppState>,
    human: Human,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiJson(body): ApiJson<AddCommentBody>,
) -> Result<(StatusCode, Json<ReviewComment>), ApiError> {
    let proposal = proposal(&id)?;
    let input = body.comment.into_input("comment")?;
    let comment = service(&state, &project)
        .add_comment(&human.actor(), proposal, body.version, input)
        .await?;
    Ok((StatusCode::CREATED, Json(comment.into())))
}

/// A human's question to a proposal's author: about a run of lines of one file of a version when `path`, `side`, `start_line` and `end_line` are all given, about the whole proposal when none is.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AskAuthorBody {
    /// The version the question is about; any version of the proposal.
    pub version: u32,
    /// The file the lines belong to, relative to the project root.
    #[serde(default)]
    #[schema(example = "web/src/board.ts")]
    pub path: Option<String>,
    #[serde(default)]
    pub side: Option<CommentSide>,
    /// From 1.
    #[serde(default)]
    pub start_line: Option<u32>,
    /// Inclusive.
    #[serde(default)]
    pub end_line: Option<u32>,
    /// The question, plain text with some visible character. In message text `#` names a fold item and `@` names a file; the daemon parses nothing.
    pub body: String,
}

/// Asks the proposal's author about a run of lines of a version, or about the whole proposal: one message from the named human to the worker holding the task's implementation claim, else the proposal's author, linked to the task and the proposal and carrying the lines and their text when it names lines. It waits for the author's answer in its thread. It writes no review, verdict or comment and never changes whose turn it is, and it changes no task state.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/proposals/{id}/questions",
    operation_id = "ask_proposal_author",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The proposal id, such as P-3"),
    ),
    request_body = AskAuthorBody,
    responses(
        (status = 201, description = "The stored question", body = MessageView),
        (status = 400, description = "`invalid_request`: an empty body, some but not all of the four line fields, or a malformed request; `invalid_path` or `invalid_id`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`proposal_not_found`: no such proposal or version, or `project_not_found`", body = ErrorEnvelope),
        (status = 422, description = "`unknown_comment_path` or `invalid_comment_range`: the lines are not in the file on that side", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn ask_proposal_author(
    State(state): State<AppState>,
    human: Human,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiJson(body): ApiJson<AskAuthorBody>,
) -> Result<(StatusCode, Json<MessageView>), ApiError> {
    let proposal = proposal(&id)?;
    let location = CommentLocation::question_parts(
        body.path,
        body.side.map(Side::from),
        body.start_line,
        body.end_line,
    )?;
    let question = service(&state, &project)
        .ask_author(
            &human.actor(),
            proposal,
            body.version,
            location,
            fold_core::MessageBody::parse(body.body)?,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(question.into())))
}

/// A human verdict on the version the human read, with line comments. `request_changes` gives the turn to the author, `approve` counts for integration and `reject` closes the proposal.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/proposals/{id}/reviews",
    operation_id = "submit_review",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The proposal id, such as P-3"),
    ),
    request_body = ReviewBody,
    responses(
        (status = 201, description = "The review", body = SubmittedReview),
        (status = 400, description = "`invalid_request` (`details.field` names the field, or `details.reason` is `checks_failed` for an approve of a version whose integration checks failed), `invalid_path`, `invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 404, description = "`proposal_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`stale_version` (`details.expected_version`, `details.current_version`), `agent_approval_required` (a human's approve on a task with the human review mark before a reviewer agent approved the version; a task of the area main needs no reviewer agent first), `update_checks_running` (an approve while the author's update or a reviewer's fix runs the checks of the next version; `details.run`, `details.purpose`), `proposal_not_open` or `task_closed`", body = ErrorEnvelope),
        (status = 422, description = "`empty_review`, `unknown_comment_path` or `invalid_comment_range`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn submit_review(
    State(state): State<AppState>,
    human: Human,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiJson(body): ApiJson<ReviewBody>,
) -> Result<(StatusCode, Json<SubmittedReview>), ApiError> {
    let proposal = proposal(&id)?;
    let comments = body
        .comments
        .into_iter()
        .enumerate()
        .map(|(index, comment)| comment.into_input(&format!("comments[{index}]")))
        .collect::<Result<Vec<_>, _>>()?;
    let submitted = service(&state, &project)
        .submit_review(
            &human.actor(),
            proposal,
            body.version,
            body.verdict.into(),
            &body.body,
            comments,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(submitted.into())))
}

/// A named human integrates the current version onto main and approves it in the same action. A conflicting merge publishes nothing: the answer is `integration_conflicted` with each conflicting path and its kind in `details.files`, and the full report is on the proposal.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/proposals/{id}/integrate",
    operation_id = "integrate_proposal",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The proposal id, such as P-3"),
    ),
    request_body = IntegrateBody,
    responses(
        (status = 200, description = "Integrated, failed its integration checks, or queued behind other integrations when the project has integration checks", body = IntegrationResult),
        (status = 400, description = "`invalid_request` (`details.reason` is `checks_failed` while the integration checks of the version failed, or `integration_queued` while an integration of the version waits in the queue), `invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 403, description = "`author_cannot_integrate`: the caller's agent wrote the proposal", body = ErrorEnvelope),
        (status = 404, description = "`proposal_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "A gate refusal: `stale_version` (`details.expected_version`, `details.current_version`), `changes_requested`, `not_approved`, `human_approval_missing` (`details.reviewer`), `agent_approval_required` (a marked task whose version no reviewer agent approved), `prerequisites_not_done` (`details.prerequisites`); or `proposal_not_open`, `integration_conflicted` (`details.files`: path and kind), `head_moved`, or `update_checks_running` while the author's update or a reviewer's fix runs the checks of the next version (`details.run`, `details.purpose`)", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`, or `integration_failed` (`details.cause`)", body = ErrorEnvelope),
        (status = 500, description = "Internal error, or `workspace_io` with `details.reason` `fold_failed` when fold itself could not run the integration checks: the version is not sent back, so integrate again once the fault is fixed", body = ErrorEnvelope),
    )
)]
pub async fn integrate_proposal(
    State(state): State<AppState>,
    human: Human,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiJson(body): ApiJson<IntegrateBody>,
) -> Result<Json<IntegrationResult>, ApiError> {
    let proposal = proposal(&id)?;
    match service(&state, &project)
        .integrate_proposal(&human.actor(), proposal, Some(body.expected_version))
        .await?
    {
        IntegrationOutcome::Succeeded { revision } => Ok(Json(IntegrationResult {
            proposal: proposal.to_string(),
            result: IntegrationResultKind::Succeeded,
            revision: Some(revision.to_string()),
            run: None,
        })),
        IntegrationOutcome::Conflicted(conflicts) => {
            Err(integration_conflicted(proposal, &conflicts).into())
        }
        IntegrationOutcome::ChecksFailed { run } => Ok(Json(IntegrationResult {
            proposal: proposal.to_string(),
            result: IntegrationResultKind::ChecksFailed,
            revision: None,
            run: Some(run.to_string()),
        })),
        IntegrationOutcome::Queued => Ok(Json(IntegrationResult {
            proposal: proposal.to_string(),
            result: IntegrationResultKind::Queued,
            revision: None,
            run: None,
        })),
    }
}

/// One file of a proposal version, the content a human edit starts from. A symlink gives its target text and is never followed.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/proposals/{id}/file",
    operation_id = "read_proposal_file",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The proposal id, such as P-3"),
        VersionFileQuery,
    ),
    responses(
        (status = 200, description = "The file", body = FileContentView),
        (status = 400, description = "`invalid_id`, `invalid_path` or `invalid_request`: the query string is not valid", body = ErrorEnvelope),
        (status = 404, description = "`proposal_not_found`: no such proposal or version, `not_found`: the version has no file at the path, or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn read_proposal_file(
    State(state): State<AppState>,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiQuery(query): ApiQuery<VersionFileQuery>,
) -> Result<Json<FileContentView>, ApiError> {
    let content = service(&state, &project)
        .version_file(proposal(&id)?, query.version, &query.path)
        .await?;
    Ok(Json(content_view(query.path, content)))
}

/// A human edits one text file of the version the human read, in a human review: only on a task with the human review mark, by the human the mark names or by any human when it names none. The edit becomes a new version under the human's name that keeps the agent approvals of the version it was made from, but no human approval; send it for review again to withdraw them. The task worker's next sync writes the edit into its workspace.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/proposals/{id}/edits",
    operation_id = "edit_proposal_file",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The proposal id, such as P-3"),
    ),
    request_body = EditFileBody,
    responses(
        (status = 201, description = "The new version", body = ProposalVersion),
        (status = 400, description = "`invalid_request`, `invalid_path`, `invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 403, description = "`human_edit_denied`: the task has no human review mark, or the mark names another human (`details.reviewer`)", body = ErrorEnvelope),
        (status = 404, description = "`proposal_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`stale_version` (`details.expected_version`, `details.current_version`), `worker_busy` (`details.reason`: `working`, `unfinished_write` or `newer_work`), `version_unchanged`, `proposal_not_open`, `workspace_not_active`, `not_workspace_owner` or `task_closed`", body = ErrorEnvelope),
        (status = 422, description = "`file_not_editable` (`details.reason`: `missing`, `not_a_file`, `binary` or `too_large`)", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn edit_proposal_file(
    State(state): State<AppState>,
    human: Human,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiJson(body): ApiJson<EditFileBody>,
) -> Result<(StatusCode, Json<ProposalVersion>), ApiError> {
    let edit = service(&state, &project)
        .edit_as_human(
            &human.actor(),
            proposal(&id)?,
            body.version,
            &body.path,
            &body.content,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(edit.detail.selected.into())))
}

/// A human sends the current version, a human edit, for review again: the approvals it kept stop counting and the reviewers are told. Only in a human review, for the human the mark names or any human when it names none.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{slug}/proposals/{id}/review-again",
    operation_id = "send_for_review_again",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The proposal id, such as P-3"),
    ),
    request_body = ReviewAgainBody,
    responses(
        (status = 200, description = "The proposal after the request", body = ProposalSummary),
        (status = 400, description = "`invalid_request` (`details.reason`: `not_human_edit` or `already_sent`), `invalid_id` or `invalid_name`", body = ErrorEnvelope),
        (status = 401, description = "`human_name_required`", body = ErrorEnvelope),
        (status = 403, description = "`human_edit_denied`", body = ErrorEnvelope),
        (status = 404, description = "`proposal_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 409, description = "`stale_version` (`details.expected_version`, `details.current_version`), `proposal_not_open` or `task_closed`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn send_for_review_again(
    State(state): State<AppState>,
    human: Human,
    project: Project,
    Path((_slug, id)): Path<(String, String)>,
    ApiJson(body): ApiJson<ReviewAgainBody>,
) -> Result<Json<ProposalSummary>, ApiError> {
    let detail = service(&state, &project)
        .send_for_review_again(&human.actor(), proposal(&id)?, body.version)
        .await?;
    Ok(Json(detail.summary.into()))
}

/// A read-only checkout of a proposal version, or of a review batch, for its reviewer. The daemon records no command run in it.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewCheckoutView {
    #[schema(value_type = String, example = "ws-12")]
    pub workspace: WorkspaceId,
    /// Relative to the project root, such as `.fold/ws/review-p-4` or `.fold/ws/review-rb-3`.
    pub path: String,
    /// What the checkout's files hold.
    pub target: ReviewCheckoutTarget,
    /// The review worker that owns the checkout, such as `review/ci`.
    #[schema(required = true)]
    pub worker: Option<String>,
    /// The base revision: the version's base for a proposal checkout, the revision of main the members were merged onto for a batch checkout.
    #[schema(value_type = String, example = "r17")]
    pub base: RevisionId,
    /// UTC milliseconds.
    #[schema(value_type = i64)]
    pub created_at: Timestamp,
}

/// One proposal version, or the members of a review batch merged onto main.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewCheckoutTarget {
    Proposal {
        #[schema(value_type = String, example = "P-4")]
        proposal: ProposalId,
        /// The proposal version the checkout holds.
        version: u32,
    },
    Batch {
        #[schema(value_type = String, example = "rb-3")]
        batch: ReviewBatchId,
        /// The members the checkout holds, in merge order.
        members: Vec<ReviewCheckoutMember>,
    },
}

/// A member of a batch checkout at the version it joined the batch with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ReviewCheckoutMember {
    #[schema(value_type = String, example = "P-4")]
    pub proposal: ProposalId,
    pub version: u32,
}

impl From<CheckoutTarget> for ReviewCheckoutTarget {
    fn from(target: CheckoutTarget) -> Self {
        match target {
            CheckoutTarget::Proposal { proposal, version } => {
                ReviewCheckoutTarget::Proposal { proposal, version }
            }
            CheckoutTarget::Batch { batch, members } => ReviewCheckoutTarget::Batch {
                batch,
                members: members
                    .into_iter()
                    .map(|(proposal, version)| ReviewCheckoutMember { proposal, version })
                    .collect(),
            },
        }
    }
}

impl From<ListedCheckout> for ReviewCheckoutView {
    fn from(checkout: ListedCheckout) -> Self {
        let workspace = checkout.workspace;
        Self {
            workspace: workspace.id,
            path: workspace.path,
            target: checkout.target.into(),
            worker: workspace.worker.map(|worker| worker.as_str().to_owned()),
            base: workspace.base,
            created_at: workspace.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewCheckoutList {
    /// The active review checkouts, oldest first.
    pub checkouts: Vec<ReviewCheckoutView>,
}

/// The project's active review checkouts, of one proposal or of a review batch, with their worker, directory and the proposal versions they hold.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/reviews/checkouts",
    operation_id = "list_review_checkouts",
    params(("slug" = String, Path, description = "The project slug")),
    responses(
        (status = 200, description = "The active review checkouts", body = ReviewCheckoutList),
        (status = 404, description = "`project_not_found`: the slug is not registered", body = ErrorEnvelope),
        (status = 503, description = "The project failed to open", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn list_review_checkouts(
    State(state): State<AppState>,
    project: Project,
) -> Result<Json<ReviewCheckoutList>, ApiError> {
    let checkouts = service(&state, &project).list_review_checkouts().await?;
    Ok(Json(ReviewCheckoutList {
        checkouts: checkouts
            .into_iter()
            .map(ReviewCheckoutView::from)
            .collect(),
    }))
}
