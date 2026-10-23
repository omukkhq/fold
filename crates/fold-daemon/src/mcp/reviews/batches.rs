use fold_core::reviews::Verdict;
use fold_core::{Actor, FoldError, ReviewBatchId, codes};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::error::from_review;
use super::super::parse;
use super::{ScopeChoiceInput, id, proposal_id, scope_choices, version};
use crate::services::reviews::{
    BatchComment, BatchIntegration, BatchIntegrationView, BatchLines, BatchVerdict,
    IntegrationOutcome, MemberIntegration, ReviewBatchView, ReviewService,
};

pub const CREATE_REVIEW_BATCH: &str = "create_review_batch";
pub const LIST_REVIEW_BATCHES: &str = "list_review_batches";
pub const GET_REVIEW_BATCH: &str = "get_review_batch";
pub const REMOVE_FROM_BATCH: &str = "remove_from_batch";
pub const CREATE_BATCH_CHECKOUT: &str = "create_batch_checkout";
pub const SUBMIT_BATCH_REVIEW: &str = "submit_batch_review";
pub const INTEGRATE_REVIEW_BATCH: &str = "integrate_review_batch";

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateReviewBatchInput {
    /// The open proposals to review together, such as ["P-3", "P-4"]: 2 to 20, none written by your agent. fold takes the review claim of each for you.
    pub proposals: Vec<String>,
    /// For revised versions: the scope of your review of some proposals, each with its reason, when you do not follow fold's suggested_scope from get_proposal_diff. The other proposals follow the suggestion.
    #[serde(default)]
    pub scopes: Vec<ScopeChoiceInput>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListReviewBatchesInput {
    /// Only the batches that are not submitted yet.
    #[serde(default)]
    pub open: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReviewBatchIdInput {
    /// The review batch, such as rb-3.
    pub id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RemoveFromBatchInput {
    /// The review batch, such as rb-3.
    pub id: String,
    /// The member to take out, such as P-4. Its review claim is released.
    pub proposal: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct BatchVerdictInput {
    /// The member, such as P-3.
    pub proposal: String,
    /// comment, request_changes, approve or reject. Only the main agent and the reviewers that the review lead starts can approve or reject; reject closes the proposal. The review lead gives no verdict (review_lead_delegates).
    pub verdict: String,
    /// The review text for this member. An approval may leave it out; request_changes and comment need it or a comment on the member; reject needs it. In the text `#` names a fold item (#T-12) and `@` names a file (@web/src/board.ts); the daemon parses nothing.
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct BatchCommentInput {
    /// A file of the batch checkout, relative to its root. With start_line and end_line the comment is stored on the member that brought those lines; leave all three out for a comment on the batch.
    #[serde(default)]
    pub path: Option<String>,
    /// The first line of the range in the batch checkout, from 1.
    #[serde(default)]
    pub start_line: Option<u32>,
    /// The last line of the range in the batch checkout, inclusive.
    #[serde(default)]
    pub end_line: Option<u32>,
    /// For a comment on the batch: the members the problem concerns, such as ["P-3", "P-4"]. The comment is stored on each of them.
    #[serde(default)]
    pub members: Vec<String>,
    /// The comment text. In the text `#` names a fold item (#T-12) and `@` names a file (@web/src/board.ts); the daemon parses nothing.
    pub body: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SubmitBatchReviewInput {
    /// The review batch, such as rb-3.
    pub id: String,
    /// One verdict for each member. A member that got a new version, closed or integrated is left out and needs no verdict.
    pub verdicts: Vec<BatchVerdictInput>,
    /// Line comments on the batch checkout and comments on the batch.
    #[serde(default)]
    pub comments: Vec<BatchCommentInput>,
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            CREATE_REVIEW_BATCH,
            "For a review of its own by the main agent, when the user tells it to: review related open proposals together. fold takes the review claim of each and orders them by their task dependencies. A proposal whose task has the human review mark is refused with invalid_request: review it on its own. Then call create_batch_checkout and submit_batch_review. A reviewer gets its batch from the review lead's start_worker and gets worker_claim_denied here; the review lead never reviews and gets review_lead_delegates.",
            schema_for_type::<CreateReviewBatchInput>(),
        ),
        Tool::new(
            LIST_REVIEW_BATCHES,
            "List review batches with their holder, state and members.",
            schema_for_type::<ListReviewBatchesInput>(),
        ),
        Tool::new(
            GET_REVIEW_BATCH,
            "Read one review batch: members in merge order with the version each joined at, its current version and, for a member with a newer version than the newest one with a verdict or the one it joined at, since: that earlier version and the size of the change after it (read it with get_proposal_diff and since), the checkout and the verdicts.",
            schema_for_type::<ReviewBatchIdInput>(),
        ),
        Tool::new(
            REMOVE_FROM_BATCH,
            "Take a member out of your open batch, for example after batch_conflict. Its review claim is released and the batch checkout is archived.",
            schema_for_type::<RemoveFromBatchInput>(),
        ),
        Tool::new(
            CREATE_BATCH_CHECKOUT,
            "Get a read-only checkout of main with every member of your batch merged in order, to build and test them once. A conflict is batch_conflict naming both proposals and the paths. The review lead never reviews and gets review_lead_delegates.",
            schema_for_type::<ReviewBatchIdInput>(),
        ),
        Tool::new(
            SUBMIT_BATCH_REVIEW,
            "Give every member of your batch its own verdict in one call, with line comments on the batch checkout and comments on the batch. A member that got a new version is left out. An approve of a member while its author's update or a reviewer's fix runs the checks of its next version refuses the whole call with update_checks_running (details name the member and the run): give that member another verdict, or wait for checks.finished with wait_for_events.",
            schema_for_type::<SubmitBatchReviewInput>(),
        ),
        Tool::new(
            INTEGRATE_REVIEW_BATCH,
            "Integrate the approved members of a submitted batch together; only the main agent, the review lead, a reviewer that wrote none of them, or a human may. fold integrates a ready batch by itself when the project integrates automatically; call this when it does not. A batch is ready when every member has its final state: approved, or out (changes requested, rejected, closed, removed, or waiting for a human or for a member that is out). The ready members merge onto main in dependency order, the integration checks run once on the whole merge, and one transaction publishes one revision per member (result together). A member that conflicts goes back to its author alone; when the one check run fails, fold integrates the members one at a time, each with its own run, and sends back only the member whose run fails (result one_at_a_time). With integration checks the call is queued and fold sends each member's result as a message. Refusals: invalid_request with the reason batch_not_submitted, batch_waiting (details name the pending members), batch_integrated or integration_queued, update_checks_running while the author's update or a reviewer's fix of a member runs its checks, and the caller gates of integrate_proposal. A member can still be integrated alone with integrate_proposal.",
            schema_for_type::<ReviewBatchIdInput>(),
        ),
    ]
}

/// Runs a batch tool. `None` when `tool` is not one.
pub async fn run(
    reviews: &ReviewService,
    tool: &str,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Option<Value>, FoldError> {
    let value = match tool {
        CREATE_REVIEW_BATCH => create_review_batch(reviews, actor, arguments).await?,
        LIST_REVIEW_BATCHES => list_review_batches(reviews, arguments).await?,
        GET_REVIEW_BATCH => get_review_batch(reviews, arguments).await?,
        REMOVE_FROM_BATCH => remove_from_batch(reviews, actor, arguments).await?,
        CREATE_BATCH_CHECKOUT => create_batch_checkout(reviews, actor, arguments).await?,
        SUBMIT_BATCH_REVIEW => submit_batch_review(reviews, actor, arguments).await?,
        INTEGRATE_REVIEW_BATCH => integrate_review_batch(reviews, actor, arguments).await?,
        _ => return Ok(None),
    };
    Ok(Some(value))
}

fn batch_id(text: &str) -> Result<ReviewBatchId, FoldError> {
    id(
        text,
        "pass a review batch id such as rb-3; call list_review_batches to see them",
    )
}

fn batch_json(batch: &ReviewBatchView) -> Value {
    let members: Vec<Value> = batch
        .members
        .iter()
        .map(|member| {
            let mut line = json!({
                "proposal": member.proposal,
                "task": member.task,
                "version": version(member.version),
                "current_version": version(member.current_version),
                "proposal_state": member.proposal_state.as_str(),
                "in_batch": member.released.is_none(),
            });
            if let Some(since) = &member.since {
                line["since"] = json!({
                    "version": version(since.version),
                    "files": since.totals.files,
                    "added": since.totals.added_lines,
                    "removed": since.totals.removed_lines,
                });
            }
            line
        })
        .collect();
    let verdicts: Vec<Value> = batch
        .verdicts
        .iter()
        .map(|verdict| {
            json!({
                "proposal": verdict.proposal,
                "version": version(verdict.version),
                "verdict": verdict.verdict.as_str(),
            })
        })
        .collect();
    json!({
        "id": batch.id,
        "holder": batch.holder.as_str(),
        "state": batch.state.as_str(),
        "members": members,
        "checkout": batch.checkout.as_ref().map(|checkout| json!({
            "workspace": checkout.id,
            "path": checkout.path,
        })),
        "verdicts": verdicts,
        "comments": batch.comments.iter().map(|comment| json!({
            "number": comment.number,
            "body": comment.body,
            "members": comment.members,
        })).collect::<Vec<Value>>(),
        "integration": integration_json(&batch.integration),
    })
}

fn integration_json(view: &BatchIntegrationView) -> Value {
    match view {
        BatchIntegrationView::NotSubmitted => json!({"state": "not_submitted"}),
        BatchIntegrationView::Waiting { pending } => json!({
            "state": "waiting",
            "pending": pending,
        }),
        BatchIntegrationView::Ready {
            members,
            queued,
            updating,
        } => {
            let mut value = json!({
                "state": "ready",
                "members": members,
                "queued": queued,
            });
            // The batch waits for these runs; a member's next version starts without the batch's approval.
            if !updating.is_empty() {
                value["updating"] = updating
                    .iter()
                    .map(|(proposal, update)| {
                        json!({
                            "proposal": proposal,
                            "run": update.run,
                            "purpose": update.purpose.as_str(),
                        })
                    })
                    .collect();
            }
            value
        }
        BatchIntegrationView::Ended {
            outcome,
            run,
            revisions,
        } => json!({
            "state": "ended",
            "outcome": outcome.as_str(),
            "run": run,
            "revisions": revisions.iter().map(|row| json!({
                "proposal": row.proposal,
                "version": version(row.version),
                "revision": row.revision,
            })).collect::<Vec<Value>>(),
        }),
    }
}

fn member_json(member: &MemberIntegration) -> Value {
    match &member.outcome {
        IntegrationOutcome::Succeeded { revision } => json!({
            "proposal": member.proposal,
            "result": "succeeded",
            "revision": revision,
        }),
        IntegrationOutcome::Conflicted(conflicts) => json!({
            "proposal": member.proposal,
            "result": "conflicted",
            "files": conflicts.len(),
        }),
        IntegrationOutcome::ChecksFailed { run } => json!({
            "proposal": member.proposal,
            "result": "checks_failed",
            "run": run,
        }),
        IntegrationOutcome::Queued => json!({
            "proposal": member.proposal,
            "result": "queued",
        }),
    }
}

async fn integrate_review_batch(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: ReviewBatchIdInput = parse(arguments)?;
    let batch = batch_id(&input.id)?;
    let outcome = reviews
        .integrate_review_batch(actor, batch)
        .await
        .map_err(from_review)?;
    Ok(match outcome {
        BatchIntegration::Together { members } => json!({
            "id": batch,
            "result": "together",
            "members": members.iter().map(member_json).collect::<Vec<Value>>(),
        }),
        BatchIntegration::OneAtATime { run, members } => json!({
            "id": batch,
            "result": "one_at_a_time",
            "run": run,
            "members": members.iter().map(member_json).collect::<Vec<Value>>(),
        }),
        BatchIntegration::Nothing => json!({"id": batch, "result": "nothing"}),
        BatchIntegration::Queued => json!({"id": batch, "result": "queued"}),
    })
}

async fn create_review_batch(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: CreateReviewBatchInput = parse(arguments)?;
    let proposals = input
        .proposals
        .iter()
        .map(|text| proposal_id(text))
        .collect::<Result<Vec<_>, _>>()?;
    let scopes = scope_choices(&input.scopes)?;
    let batch = reviews
        .create_review_batch_with_scopes(actor, proposals, scopes)
        .await
        .map_err(from_review)?;
    Ok(batch_json(&batch))
}

async fn list_review_batches(
    reviews: &ReviewService,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: ListReviewBatchesInput = parse(arguments)?;
    let batches = reviews
        .list_review_batches(input.open)
        .await
        .map_err(from_review)?;
    Ok(json!({"batches": batches.iter().map(batch_json).collect::<Vec<_>>()}))
}

async fn get_review_batch(
    reviews: &ReviewService,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: ReviewBatchIdInput = parse(arguments)?;
    let batch = reviews
        .get_review_batch(batch_id(&input.id)?)
        .await
        .map_err(from_review)?;
    Ok(batch_json(&batch))
}

async fn remove_from_batch(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: RemoveFromBatchInput = parse(arguments)?;
    let batch = reviews
        .remove_from_batch(actor, batch_id(&input.id)?, proposal_id(&input.proposal)?)
        .await
        .map_err(from_review)?;
    Ok(batch_json(&batch))
}

async fn create_batch_checkout(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: ReviewBatchIdInput = parse(arguments)?;
    let checkout = reviews
        .create_batch_checkout(actor, batch_id(&input.id)?)
        .await
        .map_err(from_review)?;
    let members: Vec<Value> = checkout
        .members
        .iter()
        .map(|(proposal, number)| json!({"proposal": proposal, "version": version(*number)}))
        .collect();
    Ok(json!({
        "id": checkout.batch,
        "workspace": checkout.workspace.id,
        "path": checkout.workspace.path,
        "members": members,
    }))
}

async fn submit_batch_review(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: SubmitBatchReviewInput = parse(arguments)?;
    let batch = batch_id(&input.id)?;
    let verdicts = input
        .verdicts
        .into_iter()
        .enumerate()
        .map(|(index, verdict)| verdict_input(index, verdict))
        .collect::<Result<Vec<_>, _>>()?;
    let comments = input
        .comments
        .into_iter()
        .enumerate()
        .map(|(index, comment)| comment_input(index, comment))
        .collect::<Result<Vec<_>, _>>()?;
    let submitted = reviews
        .submit_batch_review(actor, batch, verdicts, comments)
        .await
        .map_err(from_review)?;
    let reviewed: Vec<Value> = submitted
        .reviewed
        .iter()
        .map(|review| {
            json!({
                "proposal": review.proposal,
                "version": version(review.version),
                "verdict": review.verdict.as_str(),
                "comments": review.comments,
            })
        })
        .collect();
    let left_out: Vec<Value> = submitted
        .left_out
        .iter()
        .map(|left| {
            json!({
                "proposal": left.proposal,
                "version": version(left.version),
                "reason": left.reason.as_str(),
            })
        })
        .collect();
    Ok(json!({"id": submitted.batch, "reviewed": reviewed, "left_out": left_out}))
}

fn verdict_input(index: usize, input: BatchVerdictInput) -> Result<BatchVerdict, FoldError> {
    let verdict = Verdict::parse(&input.verdict).ok_or_else(|| {
        FoldError::new(
            codes::INVALID_REQUEST,
            "verdict is comment, request_changes, approve or reject",
        )
        .with_details([("field", json!(format!("verdicts[{index}].verdict")))])
        .with_hint(
            "call submit_batch_review with verdict comment, request_changes, approve or reject",
        )
    })?;
    Ok(BatchVerdict {
        proposal: proposal_id(&input.proposal)?,
        verdict,
        body: input.body.unwrap_or_default(),
    })
}

fn comment_input(index: usize, input: BatchCommentInput) -> Result<BatchComment, FoldError> {
    let location = match (input.path, input.start_line, input.end_line) {
        (None, None, None) => None,
        (Some(path), Some(start_line), Some(end_line)) => Some(BatchLines {
            path,
            start_line,
            end_line,
        }),
        _ => {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("comments[{index}]: a line comment needs path, start_line and end_line"),
            )
            .with_details([("field", json!(format!("comments[{index}]")))])
            .with_hint("give path, start_line and end_line together, or none of them with members, then call submit_batch_review again"));
        }
    };
    let members = input
        .members
        .iter()
        .map(|text| proposal_id(text))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(BatchComment {
        location,
        members,
        body: input.body,
    })
}
