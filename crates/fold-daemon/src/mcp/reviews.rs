use std::str::FromStr;

use fold_core::policy::{Gate, GateState, GateStatus};
use fold_core::reviews::{HumanReviewMark, Side, Verdict};
use fold_core::{
    Actor, CommentId, FoldError, HumanName, IdParseError, ProposalId, TaskId, WorkerName, codes,
};
use fold_store::reviews::ProposalState;
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::FoldServer;
use super::error::{from_agent, from_review};
use super::identity::parse;
use super::workspaces::file_line;
use crate::services::reviews::{
    CommentInput, CommentLocation, CommentView, IntegrationOutcome, ProposalDetail, ProposalFilter,
    ProposalSummary, ProposalView, ReviewService, integration_conflicted,
};

pub mod batches;

pub const CREATE_PROPOSAL: &str = "create_proposal";
pub const LIST_PROPOSALS: &str = "list_proposals";
pub const GET_PROPOSAL: &str = "get_proposal";
pub const UPDATE_PROPOSAL: &str = "update_proposal";
pub const CLOSE_PROPOSAL: &str = "close_proposal";
pub const CLAIM_REVIEW: &str = "claim_review";
pub const RELEASE_REVIEW: &str = "release_review";
pub const CREATE_REVIEW_CHECKOUT: &str = "create_review_checkout";
pub const SUBMIT_REVIEW: &str = "submit_review";
pub const GET_REVIEW_COMMENTS: &str = "get_review_comments";
pub const INTEGRATE_PROPOSAL: &str = "integrate_proposal";
pub const REQUIRE_HUMAN_REVIEW: &str = "require_human_review";

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateProposalInput {
    /// The task the work is for, such as T-12. You must hold its claim and own its active workspace.
    pub task: String,
    /// The proposal title: one line of at most 200 characters.
    pub title: String,
    /// What shows the work is done, such as the tests you ran and their result. Leave it out for none. In the text `#` names a fold item (#T-12) and `@` names a file (@web/src/board.ts); the daemon parses nothing.
    #[serde(default)]
    pub evidence: Option<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListProposalsInput {
    /// Only proposals in this state: open, integrated or closed. Leave it out for every state.
    #[serde(default)]
    pub state: Option<String>,
    /// Only proposals that no reviewer has claimed.
    #[serde(default)]
    pub unclaimed: bool,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ProposalVersionInput {
    /// The proposal, such as P-3.
    pub id: String,
    /// A version number, such as 2 for v2. Leave it out for the current version.
    #[serde(default)]
    pub version: Option<u32>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct UpdateProposalInput {
    /// The proposal, such as P-3. Only its author's worker can add a version.
    pub id: String,
    /// New evidence for the new version. Leave it out to keep the current evidence. In the text `#` names a fold item (#T-12) and `@` names a file (@web/src/board.ts); the daemon parses nothing.
    #[serde(default)]
    pub evidence: Option<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CloseProposalInput {
    /// The proposal, such as P-3.
    pub id: String,
    /// Why the proposal closes: 1 to 500 characters.
    pub reason: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ProposalIdInput {
    /// The proposal, such as P-3.
    pub id: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReleaseReviewInput {
    /// The proposal, such as P-3.
    pub id: String,
    /// Why you give the review up, at most 500 characters. Leave it out for none.
    #[serde(default)]
    pub reason: Option<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReviewCommentInput {
    /// The file the comment is about, relative to the project root. Leave it out, with the lines and side, for a comment on the whole version.
    #[serde(default)]
    pub path: Option<String>,
    /// The first line of the range, from 1.
    #[serde(default)]
    pub start_line: Option<u32>,
    /// The last line of the range, inclusive; equal to start_line for one line.
    #[serde(default)]
    pub end_line: Option<u32>,
    /// old for the lines of the version's base, new for the lines of the version.
    #[serde(default)]
    pub side: Option<String>,
    /// The comment text. In the text `#` names a fold item (#T-12) and `@` names a file (@web/src/board.ts); the daemon parses nothing.
    pub body: String,
    /// A comment of the same proposal this one answers, such as rc-5.
    #[serde(default)]
    pub reply_to: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SubmitReviewInput {
    /// The proposal, such as P-3. The review is of its current version.
    pub id: String,
    /// comment, request_changes, approve or reject. Only an agent with the integrator tag can approve or reject; reject closes the proposal.
    pub verdict: String,
    /// The review text. It may be empty only when the review carries a comment; a reject needs it. In the text `#` names a fold item (#T-12) and `@` names a file (@web/src/board.ts); the daemon parses nothing.
    pub body: String,
    /// Line comments that come with the review.
    #[serde(default)]
    pub comments: Vec<ReviewCommentInput>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct IntegrateProposalInput {
    /// The proposal, such as P-3.
    pub id: String,
    /// The version you reviewed, such as 2 for v2. When the current version is another one, nothing integrates and the error is stale_version.
    #[serde(default)]
    pub expected_version: Option<u32>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RequireHumanReviewInput {
    /// The task, such as T-12.
    pub task: String,
    /// The one human whose approval counts, such as tanim. Leave it out to let any human approve.
    #[serde(default)]
    pub reviewer: Option<String>,
    /// Why a human must look: 1 to 500 characters.
    pub reason: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

pub fn tools() -> Vec<Tool> {
    let mut tools = vec![
        Tool::new(
            CREATE_PROPOSAL,
            "Send the work in your task workspace for review as a new proposal (v1). You must hold the task's claim. The task moves to Review.",
            schema_for_type::<CreateProposalInput>(),
        ),
        Tool::new(
            LIST_PROPOSALS,
            "List proposals with their task, author, whose turn it is, the review claim holder and the human review mark.",
            schema_for_type::<ListProposalsInput>(),
        ),
        Tool::new(
            GET_PROPOSAL,
            "Read one proposal: versions, evidence, the task and its prerequisites, the review claim, reviews, comment counts, the gate report for integrating, the human review mark, the conflict report and the changed files.",
            schema_for_type::<ProposalVersionInput>(),
        ),
        Tool::new(
            UPDATE_PROPOSAL,
            "Send the current state of your task workspace as the next version of your open proposal, after a change request or a conflict.",
            schema_for_type::<UpdateProposalInput>(),
        ),
        Tool::new(
            CLOSE_PROPOSAL,
            "Close an open proposal without integrating it. The task workspace stays active.",
            schema_for_type::<CloseProposalInput>(),
        ),
        Tool::new(
            CLAIM_REVIEW,
            "Claim the review of an open proposal before you review it. The author's agent cannot claim it. A repeat call renews the claim.",
            schema_for_type::<ProposalIdInput>(),
        ),
        Tool::new(
            RELEASE_REVIEW,
            "Give up your review claim so another reviewer can claim the proposal.",
            schema_for_type::<ReleaseReviewInput>(),
        ),
        Tool::new(
            CREATE_REVIEW_CHECKOUT,
            "Get a read-only copy of the current version of a proposal you hold the review claim of, to read, build and test it.",
            schema_for_type::<ProposalIdInput>(),
        ),
        Tool::new(
            SUBMIT_REVIEW,
            "Review the current version of a proposal with a verdict, a text and line comments. request_changes gives the turn to the author; reject closes the proposal.",
            schema_for_type::<SubmitReviewInput>(),
        ),
        Tool::new(
            GET_REVIEW_COMMENTS,
            "Read the comments of a proposal with their line range, side, author and reply link. A comment on an earlier version is outdated.",
            schema_for_type::<ProposalVersionInput>(),
        ),
        Tool::new(
            INTEGRATE_PROPOSAL,
            "Integrate the current version of a proposal onto main; needs the integrator tag. Refusals: stale_version, changes_requested, not_approved, human_approval_missing, prerequisites_not_done (the gates are not met), author_cannot_integrate (your agent wrote it), integrator_required, and integration_conflicted (the version conflicts with main; details list the files and kinds).",
            schema_for_type::<IntegrateProposalInput>(),
        ),
        Tool::new(
            REQUIRE_HUMAN_REVIEW,
            "Mark a task so its proposal integrates only after a human approves the current version.",
            schema_for_type::<RequireHumanReviewInput>(),
        ),
    ];
    tools.extend(batches::tools());
    tools
}

/// Runs a proposal tool acting as `as_worker`. `None` when `tool` is not a proposal tool or the server has no review service.
pub async fn call(
    server: &FoldServer,
    tool: &str,
    as_worker: Option<&WorkerName>,
    arguments: &JsonObject,
) -> Result<Option<Value>, FoldError> {
    let Some(reviews) = &server.reviews else {
        return Ok(None);
    };
    if !tools().iter().any(|known| known.name == tool) {
        return Ok(None);
    }
    let session = server.session().await;
    let actor = server
        .agents
        .actor(&session, as_worker)
        .await
        .map_err(from_agent)?;
    let value = match tool {
        CREATE_PROPOSAL => create_proposal(reviews, &actor, arguments).await?,
        LIST_PROPOSALS => list_proposals(reviews, arguments).await?,
        GET_PROPOSAL => get_proposal(reviews, &actor, arguments).await?,
        UPDATE_PROPOSAL => update_proposal(reviews, &actor, arguments).await?,
        CLOSE_PROPOSAL => close_proposal(reviews, &actor, arguments).await?,
        CLAIM_REVIEW => claim_review(reviews, &actor, arguments).await?,
        RELEASE_REVIEW => release_review(reviews, &actor, arguments).await?,
        CREATE_REVIEW_CHECKOUT => create_review_checkout(reviews, &actor, arguments).await?,
        SUBMIT_REVIEW => submit_review(reviews, &actor, arguments).await?,
        GET_REVIEW_COMMENTS => get_review_comments(reviews, arguments).await?,
        INTEGRATE_PROPOSAL => integrate_proposal(reviews, &actor, arguments).await?,
        REQUIRE_HUMAN_REVIEW => require_human_review(reviews, &actor, arguments).await?,
        _ => match batches::run(reviews, tool, &actor, arguments).await? {
            Some(value) => value,
            None => return Ok(None),
        },
    };
    Ok(Some(value))
}

fn id<T>(text: &str, hint: &'static str) -> Result<T, FoldError>
where
    T: FromStr<Err = IdParseError>,
{
    text.parse()
        .map_err(|error: IdParseError| FoldError::from(error).with_hint(hint))
}

fn proposal_id(text: &str) -> Result<ProposalId, FoldError> {
    id(
        text,
        "pass a proposal id such as P-3; call list_proposals to see them",
    )
}

fn task_id(text: &str) -> Result<TaskId, FoldError> {
    id(
        text,
        "pass a task id such as T-12; call get_task_graph to see them",
    )
}

fn version(number: u32) -> String {
    format!("v{number}")
}

fn mark_json(mark: Option<&HumanReviewMark>) -> Value {
    mark.map_or(Value::Null, |mark| {
        json!({
            "reviewer": mark.reviewer.as_ref().map(HumanName::as_str),
            "reason": mark.reason,
            "set_by": mark.set_by.as_str(),
        })
    })
}

fn summary_json(summary: &ProposalSummary) -> Value {
    json!({
        "id": summary.id,
        "title": summary.title,
        "task": summary.task,
        "author": summary.author.as_str(),
        "state": summary.state.as_str(),
        "version": version(summary.current_version),
        "turn": summary.turn.map(|turn| turn.as_str()),
        "claim": summary.review_claim.as_ref().map(WorkerName::as_str),
        "human_review": mark_json(summary.human_review.as_ref()),
    })
}

/// The result of a command that leaves the proposal with a version: its id, current version, state and the task's status.
fn changed_json(detail: &ProposalDetail) -> Value {
    json!({
        "id": detail.summary.id,
        "version": version(detail.summary.current_version),
        "state": detail.summary.state.as_str(),
        "task": detail.summary.task,
        "status": detail.task_status.as_str(),
    })
}

fn gate_json(state: &GateState) -> Value {
    let gate = match state.gate {
        Gate::Open => "open",
        Gate::Version => "version",
        Gate::Caller => "caller",
        Gate::Approval => "approval",
        Gate::HumanReview => "human_review",
        Gate::Prerequisites => "prerequisites",
        Gate::Merge => "merge",
    };
    match &state.status {
        GateStatus::Met => json!({"gate": gate, "status": "met"}),
        GateStatus::Unmet(refusal) => {
            json!({"gate": gate, "status": "unmet", "code": refusal.code()})
        }
        GateStatus::CheckedAtIntegration => {
            json!({"gate": gate, "status": "checked_at_integration"})
        }
    }
}

fn view_json(view: &ProposalView) -> Value {
    let detail = &view.detail;
    let summary = &detail.summary;
    let selected = &detail.selected;
    let versions: Vec<Value> = detail
        .versions
        .iter()
        .map(|entry| {
            json!({
                "version": version(entry.version),
                "base": entry.base,
                "changeset": entry.changeset,
                "created_at": entry.created_at.as_millis(),
            })
        })
        .collect();
    let reviews: Vec<Value> = view
        .reviews
        .iter()
        .map(|review| {
            json!({
                "version": version(review.version),
                "verdict": review.verdict.as_str(),
                "reviewer": review.reviewer.as_str(),
                "body": review.body,
                "comments": review.comments,
            })
        })
        .collect();
    json!({
        "id": summary.id,
        "title": summary.title,
        "author": summary.author.as_str(),
        "state": summary.state.as_str(),
        "turn": summary.turn.map(|turn| turn.as_str()),
        "close_reason": summary.close_reason,
        "version": version(selected.version),
        "current_version": version(summary.current_version),
        "versions": versions,
        "base": selected.base,
        "changeset": selected.changeset,
        "evidence": selected.evidence,
        "task": {
            "id": view.task.id,
            "title": view.task.title.as_str(),
            "status": view.task.status.as_str(),
            "description": view.task.description.as_str(),
        },
        "needs": view.prerequisites.iter().map(|task| json!({
            "id": task.id,
            "title": task.title.as_str(),
            "status": task.status.as_str(),
        })).collect::<Vec<_>>(),
        "claim": view.claim.as_ref().map(|claim| json!({
            "holder": claim.holder.as_str(),
            "stale": claim.stale,
            "renewed_at": claim.renewed_at.as_millis(),
        })),
        "reviews": reviews,
        "comments": {
            "total": view.comments.total,
            "on_version": view.comments.selected,
            "outdated": view.comments.outdated,
        },
        "gates": view.gates.iter().map(gate_json).collect::<Vec<_>>(),
        "human_review": mark_json(summary.human_review.as_ref()),
        "conflict": detail.conflict,
        "files": view.files.iter().map(file_line).collect::<Vec<_>>(),
        "totals": {
            "files": view.totals.files,
            "added": view.totals.added_lines,
            "removed": view.totals.removed_lines,
        },
    })
}

fn comment_json(comment: &CommentView) -> Value {
    let mut line = json!({
        "id": comment.id,
        "version": version(comment.version),
        "author": comment.author.as_str(),
        "verdict": comment.verdict.as_str(),
        "body": comment.body,
        "outdated": comment.outdated,
    });
    if let Some(location) = &comment.location {
        line["path"] = json!(location.path);
        line["side"] = json!(location.side.as_str());
        line["start_line"] = json!(location.start_line);
        line["end_line"] = json!(location.end_line);
    }
    if let Some(reply) = comment.reply_to {
        line["reply_to"] = json!(reply);
    }
    if let Some(link) = &comment.batch_comment {
        line["batch_comment"] = json!({
            "batch": link.batch,
            "number": link.number,
            "members": link.members,
        });
    }
    line
}

fn comment_input(index: usize, input: ReviewCommentInput) -> Result<CommentInput, FoldError> {
    let invalid = |message: &str| {
        FoldError::new(codes::INVALID_REQUEST, format!("comments[{index}]: {message}"))
            .with_details([("field", json!(format!("comments[{index}]")))])
            .with_hint("give path, start_line, end_line and side together, or none of them, then call submit_review again")
    };
    let location = match (input.path, input.start_line, input.end_line, input.side) {
        (None, None, None, None) => None,
        (Some(path), Some(start_line), Some(end_line), Some(side)) => Some(CommentLocation {
            path,
            side: Side::parse(&side).ok_or_else(|| invalid("side is old or new"))?,
            start_line,
            end_line,
        }),
        _ => {
            return Err(invalid(
                "a line comment needs path, start_line, end_line and side",
            ));
        }
    };
    let reply_to = input
        .reply_to
        .as_deref()
        .map(|text| id::<CommentId>(text, "call get_review_comments to see the comment ids"))
        .transpose()?;
    Ok(CommentInput {
        location,
        reply_to,
        body: input.body,
    })
}

async fn create_proposal(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: CreateProposalInput = parse(arguments)?;
    let detail = reviews
        .create_proposal(
            actor,
            task_id(&input.task)?,
            &input.title,
            input.evidence.as_deref(),
        )
        .await
        .map_err(from_review)?;
    Ok(changed_json(&detail))
}

async fn list_proposals(
    reviews: &ReviewService,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: ListProposalsInput = parse(arguments)?;
    let state = input
        .state
        .as_deref()
        .map(|text| {
            ProposalState::parse(text).ok_or_else(|| {
                FoldError::new(
                    codes::INVALID_REQUEST,
                    "state is open, integrated or closed",
                )
                .with_details([("field", json!("state"))])
                .with_hint("call list_proposals with state open, integrated or closed")
            })
        })
        .transpose()?;
    let rows = reviews
        .list_proposals(ProposalFilter {
            state,
            unclaimed: input.unclaimed,
        })
        .await
        .map_err(from_review)?;
    Ok(json!({"proposals": rows.iter().map(summary_json).collect::<Vec<_>>()}))
}

async fn get_proposal(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: ProposalVersionInput = parse(arguments)?;
    let view = reviews
        .view_proposal(actor, proposal_id(&input.id)?, input.version)
        .await
        .map_err(from_review)?;
    Ok(view_json(&view))
}

async fn update_proposal(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: UpdateProposalInput = parse(arguments)?;
    let detail = reviews
        .update_proposal(actor, proposal_id(&input.id)?, input.evidence.as_deref())
        .await
        .map_err(from_review)?;
    Ok(changed_json(&detail))
}

async fn close_proposal(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: CloseProposalInput = parse(arguments)?;
    let detail = reviews
        .close_proposal(actor, proposal_id(&input.id)?, &input.reason)
        .await
        .map_err(from_review)?;
    Ok(changed_json(&detail))
}

async fn claim_review(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: ProposalIdInput = parse(arguments)?;
    let claim = reviews
        .claim_review(actor, proposal_id(&input.id)?)
        .await
        .map_err(from_review)?;
    Ok(json!({
        "id": claim.proposal,
        "holder": claim.holder.as_str(),
        "acquired_at": claim.acquired_at.as_millis(),
        "renewed_at": claim.renewed_at.as_millis(),
        "stale": claim.stale,
    }))
}

async fn release_review(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: ReleaseReviewInput = parse(arguments)?;
    let proposal = proposal_id(&input.id)?;
    reviews
        .release_review(actor, proposal, input.reason.as_deref())
        .await
        .map_err(from_review)?;
    Ok(json!({"id": proposal, "released": true}))
}

async fn create_review_checkout(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: ProposalIdInput = parse(arguments)?;
    let checkout = reviews
        .create_review_checkout(actor, proposal_id(&input.id)?)
        .await
        .map_err(from_review)?;
    Ok(json!({
        "workspace": checkout.workspace.id,
        "path": checkout.workspace.path,
        "version": version(checkout.version),
    }))
}

async fn submit_review(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: SubmitReviewInput = parse(arguments)?;
    let proposal = proposal_id(&input.id)?;
    let verdict = Verdict::parse(&input.verdict).ok_or_else(|| {
        FoldError::new(
            codes::INVALID_REQUEST,
            "verdict is comment, request_changes, approve or reject",
        )
        .with_details([("field", json!("verdict"))])
        .with_hint("call submit_review with verdict comment, request_changes, approve or reject")
    })?;
    let comments = input
        .comments
        .into_iter()
        .enumerate()
        .map(|(index, comment)| comment_input(index, comment))
        .collect::<Result<Vec<_>, _>>()?;
    let submitted = reviews
        .submit_review(actor, proposal, verdict, &input.body, comments)
        .await
        .map_err(from_review)?;
    Ok(json!({
        "id": submitted.proposal,
        "version": version(submitted.version),
        "verdict": submitted.verdict.as_str(),
        "comments": submitted.comments,
        "state": submitted.summary.state.as_str(),
        "turn": submitted.summary.turn.map(|turn| turn.as_str()),
    }))
}

async fn get_review_comments(
    reviews: &ReviewService,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: ProposalVersionInput = parse(arguments)?;
    let comments = reviews
        .list_comments(proposal_id(&input.id)?, input.version)
        .await
        .map_err(from_review)?;
    Ok(json!({"comments": comments.iter().map(comment_json).collect::<Vec<_>>()}))
}

async fn integrate_proposal(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: IntegrateProposalInput = parse(arguments)?;
    let proposal = proposal_id(&input.id)?;
    match reviews
        .integrate_proposal(actor, proposal, input.expected_version)
        .await
        .map_err(from_review)?
    {
        IntegrationOutcome::Succeeded { revision } => Ok(json!({
            "id": proposal,
            "result": "succeeded",
            "revision": revision,
        })),
        IntegrationOutcome::Conflicted(conflicts) => {
            Err(integration_conflicted(proposal, &conflicts))
        }
    }
}

async fn require_human_review(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: RequireHumanReviewInput = parse(arguments)?;
    let task = task_id(&input.task)?;
    let reviewer = input
        .reviewer
        .as_deref()
        .map(HumanName::parse)
        .transpose()
        .map_err(|error| {
            FoldError::from(error).with_hint(
                "pass a human name such as tanim as reviewer, or leave it out, then call require_human_review again",
            )
        })?;
    let mark = reviews
        .require_human_review(actor, task, reviewer, &input.reason)
        .await
        .map_err(from_review)?;
    let mut value = mark_json(Some(&mark));
    value["task"] = json!(task);
    Ok(value)
}
