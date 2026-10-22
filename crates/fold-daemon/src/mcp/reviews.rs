use std::str::FromStr;

use fold_core::policy::{Gate, GateState, GateStatus};
use fold_core::reviews::{
    HumanReviewMark, ReviewScope, ReviewScopeRecord, ScopeChoice, Side, Verdict,
};
use fold_core::{
    Actor, ChangesetId, CommentId, FoldError, HumanName, IdParseError, ProposalId, TaskId,
    WorkerName, codes,
};
use fold_store::reviews::ProposalState;
use fold_vcs::compare::Totals;
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::FoldServer;
use super::error::from_review;
use super::parse;
use super::workspaces::{file_diff_value, file_line};
use crate::services::reviews::{
    ChangeRange, CommentInput, CommentLocation, CommentView, DEFAULT_PROPOSAL_LIST, DirectChange,
    DirectOutcome, FixOutcome, IntegrationOutcome, ProposalDetail, ProposalFilter, ProposalSummary,
    ProposalView, Proposed, ReviewFix, ReviewService, direct_conflicted, integration_conflicted,
    scope_suggestion,
};
use crate::services::workspaces::StateComparison;

pub mod batches;
pub mod checks;
pub mod plans;

pub const CREATE_PROPOSAL: &str = "create_proposal";
pub const LIST_PROPOSALS: &str = "list_proposals";
pub const GET_PROPOSAL: &str = "get_proposal";
pub const GET_PROPOSAL_DIFF: &str = "get_proposal_diff";
pub const UPDATE_PROPOSAL: &str = "update_proposal";
pub const CLOSE_PROPOSAL: &str = "close_proposal";
pub const CLAIM_REVIEW: &str = "claim_review";
pub const RELEASE_REVIEW: &str = "release_review";
pub const CREATE_REVIEW_CHECKOUT: &str = "create_review_checkout";
pub const SUBMIT_REVIEW: &str = "submit_review";
pub const GET_REVIEW_COMMENTS: &str = "get_review_comments";
pub const INTEGRATE_PROPOSAL: &str = "integrate_proposal";
pub const REQUIRE_HUMAN_REVIEW: &str = "require_human_review";
pub const SUBMIT_MAIN_CHANGE: &str = "submit_main_change";
pub const SUBMIT_REVIEW_FIX: &str = "submit_review_fix";
pub const SET_REVIEW_SCOPE: &str = "set_review_scope";

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
    /// The most proposals to return, the newest ones, in number order: 1 to 200, 50 when left out.
    #[serde(default)]
    pub limit: Option<u32>,
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
pub struct GetProposalDiffInput {
    /// The proposal, such as P-3.
    pub id: String,
    /// The version to read, such as 2 for v2. Leave it out for the current version.
    #[serde(default)]
    pub version: Option<u32>,
    /// An earlier version, such as 1 for v1, to read only what changed since it; pass the last_reviewed version of get_proposal, the newest earlier version that has a verdict. Leave it out for the full change against the version's base.
    #[serde(default)]
    pub since: Option<u32>,
    /// One changed file, such as src/lib.rs. With it the result holds that file's hunks; without it, the list of changed files.
    #[serde(default)]
    pub path: Option<String>,
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
pub struct ClaimReviewInput {
    /// The proposal, such as P-3.
    pub id: String,
    /// For a revised version: the scope of your review, change or full, when you do not follow fold's suggested_scope from get_proposal_diff. Pass reason with it. Leave it out to follow the suggestion.
    #[serde(default)]
    pub scope: Option<String>,
    /// Why the chosen scope fits, in one line, such as "the change renames a public API" or "only strings changed". Needed with scope.
    #[serde(default)]
    pub reason: Option<String>,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

/// One review scope that the review lead chooses when it assigns a review.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ScopeChoiceInput {
    /// The proposal, such as P-3; one of the proposals of the call.
    pub proposal: String,
    /// change or full: the scope of the review of the proposal's current version, instead of fold's suggested_scope.
    pub scope: String,
    /// Why this scope fits, in one line, such as "the change renames a public API" or "only strings changed".
    pub reason: String,
}

/// The review scope `scope` of `proposal` with its reason, as the tools take it.
pub(crate) fn scope_choice(
    proposal: &str,
    scope: &str,
    reason: &str,
) -> Result<ScopeChoice, FoldError> {
    let proposal = proposal_id(proposal)?;
    let scope = ReviewScope::parse(scope).ok_or_else(|| {
        FoldError::new(codes::INVALID_REQUEST, "scope is change or full")
            .with_details([("field", json!("scope"))])
            .with_hint("pass change or full")
    })?;
    ScopeChoice::new(proposal, scope, reason)
}

/// The scope choices of a call's `scopes` list.
pub(crate) fn scope_choices(inputs: &[ScopeChoiceInput]) -> Result<Vec<ScopeChoice>, FoldError> {
    inputs
        .iter()
        .map(|input| scope_choice(&input.proposal, &input.scope, &input.reason))
        .collect()
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SetReviewScopeInput {
    /// The proposal, such as P-3.
    pub id: String,
    /// The version whose review scope you choose, such as 3: one that came after you assigned the review, such as a reviewer's own fix version, or one whose scope you choose again.
    pub version: u32,
    /// change or full: the scope of the review of that version, instead of fold's suggested_scope.
    pub scope: String,
    /// Why this scope fits, in one line, such as "the fix only adds a test".
    pub reason: String,
    /// The worker that makes this call, such as review/scope. Leave it out to act as the main worker.
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
    /// The proposal, such as P-3.
    pub id: String,
    /// The version you reviewed, such as 2 for v2: the version of your review checkout. When the current version is another one, nothing is written and the error is stale_version.
    pub version: u32,
    /// comment, request_changes, approve or reject. Only the main agent and the reviewers that the review lead starts can approve or reject; reject closes the proposal. The review lead gives no verdict (review_lead_delegates).
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

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SubmitMainChangeInput {
    /// The newest changeset of your main workspace, such as C-40: the change whose diff the user saw and approved.
    pub changeset: String,
    /// The Git commit message of the change, 1 to 5000 characters: a subject line, then a blank line and a body when one helps. Show it to the user with the diff.
    pub message: String,
    /// The worker that makes this call. Only the main agent's main worker submits a direct change, so leave it out.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReviewFixInput {
    /// A file you changed in your review checkout, relative to the checkout root, such as src/lib.rs.
    pub path: String,
    /// Why the file changed, in one line of at most 500 characters: what was wrong, and the regression test or check that shows the fix.
    pub reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SubmitReviewFixInput {
    /// The proposal, such as P-3. You must hold its review claim.
    pub id: String,
    /// The version your review checkout holds and you fixed, such as 2 for v2. When the current version is another one, nothing is written and the error is stale_version.
    pub version: u32,
    /// Every file that differs from that version in your checkout, each once with its reason, and no other file: 1 to 50 entries.
    pub fixes: Vec<ReviewFixInput>,
    /// The commands you ran on the fixed files, such as the narrow tests of each fix, the build and the required checks, with their results.
    pub checks: String,
    /// The worker that makes this call, such as review/r1. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

pub fn tools() -> Vec<Tool> {
    let mut tools = vec![
        Tool::new(
            CREATE_PROPOSAL,
            "Send the work in your task workspace for review as a new proposal (v1). You must hold the task's claim. The task moves to Review. When fold-checks.toml has proposal checks for your change and no passed run_checks of the same files stands, it returns at once with checks running and a run id: fold runs the checks and opens the proposal only when they pass; the result comes as a message from fold. Wait for checks.finished with wait_for_events. A change of fold-checks.toml puts the human review mark on the task.",
            schema_for_type::<CreateProposalInput>(),
        ),
        Tool::new(
            LIST_PROPOSALS,
            "List proposals with their task, author, whose turn it is, the review claim holder and the human review mark.",
            schema_for_type::<ListProposalsInput>(),
        ),
        Tool::new(
            GET_PROPOSAL,
            "Read one proposal: versions, evidence, the task and its prerequisites, the review claim, reviews with the scope of each review of a revised version (the scope followed, fold's suggestion, its rules and the reason of a choice), comment counts, last_reviewed (the newest earlier version that has a verdict, by any reviewer), review_scope (that since version and the review lead's newest chosen scope for the version, which the review follows instead of fold's suggestion), the gate report for integrating, the human review mark, the conflict report, the check runs of the version (read their results instead of running the same checks again) and the changed files.",
            schema_for_type::<ProposalVersionInput>(),
        ),
        Tool::new(
            GET_PROPOSAL_DIFF,
            "Read the change of a proposal version: against its base, or with since only what changed after an earlier version, such as last_reviewed, the newest earlier version that has a verdict. Lists the changed files with line counts, or one file's hunks when you pass path. A since change leaves out what main gained when the worker synced, and its file list adds earlier (the earlier version's own change size), new_files (files the earlier version did not change), base_moved and base_conflicts (files where the worker merged main by hand), with fold's suggested_scope, change or full, and scope_rules, the rules behind a suggested full review: new_files, large_delta, base_conflicts or v3_or_later. On a since change, comment with side new and the new line numbers: they are the version's own lines.",
            schema_for_type::<GetProposalDiffInput>(),
        ),
        Tool::new(
            UPDATE_PROPOSAL,
            "Send the current state of your task workspace as the next version of your open proposal, after a change request or a conflict. The proposal checks gate the version as for create_proposal: checks running means fold adds the version only when they pass.",
            schema_for_type::<UpdateProposalInput>(),
        ),
        Tool::new(
            CLOSE_PROPOSAL,
            "Close an open proposal without integrating it. The task workspace stays active.",
            schema_for_type::<CloseProposalInput>(),
        ),
        Tool::new(
            CLAIM_REVIEW,
            "For a review of its own, when the user tells it to: the main agent claims the review of an open proposal before it reviews it. The review lead never reviews and gets review_lead_delegates: it gives the review to a reviewer with start_worker. A reviewer gets its claims from the review lead's start_worker and gets worker_claim_denied here; any other agent gets integrator_required and messages the review lead. The author's agent cannot claim it. Nobody claims the review of a task of the area main (main_area): a human approves it. A repeat call renews the claim.",
            schema_for_type::<ClaimReviewInput>(),
        ),
        Tool::new(
            RELEASE_REVIEW,
            "Give up your review claim so another reviewer can claim the proposal.",
            schema_for_type::<ReleaseReviewInput>(),
        ),
        Tool::new(
            CREATE_REVIEW_CHECKOUT,
            "Get a copy of the current version of a proposal you hold the review claim of, to read, build and test it, and to fix what you find in the task's scope; submit_review_fix makes the fix the next version. The review lead never reviews and gets review_lead_delegates.",
            schema_for_type::<ProposalIdInput>(),
        ),
        Tool::new(
            SUBMIT_REVIEW,
            "Review the version of a proposal you read, the version of your review checkout, with a verdict, a text and line comments. request_changes gives the turn to the author; reject closes the proposal. A version that is no longer current is refused with stale_version; an approve of a version whose integration checks failed is refused with invalid_request and the reason checks_failed until the author sends a new version. On a task of the area main only a human approves or rejects, or the main agent while the task has no human review mark, its own proposal included; any other agent gets main_area. The review lead gives no verdict and gets review_lead_delegates.",
            schema_for_type::<SubmitReviewInput>(),
        ),
        Tool::new(
            GET_REVIEW_COMMENTS,
            "Read the comments of a proposal with their line range, side, author and reply link. A comment on an earlier version is outdated.",
            schema_for_type::<ProposalVersionInput>(),
        ),
        Tool::new(
            INTEGRATE_PROPOSAL,
            "Integrate the current version of a proposal onto main; only the main agent, the review lead and its reviewers may. Refusals: stale_version, changes_requested, not_approved, human_approval_missing, prerequisites_not_done (the gates are not met), author_cannot_integrate (your agent wrote it), integrator_required, and integration_conflicted (the version conflicts with main; details list the files and kinds). When fold-checks.toml has integration checks, the result is queued: fold integrates the queued proposals one at a time in the order asked, after the integration checks pass on the merge with main; failed checks (result checks_failed when it ran at once) send the proposal back to its author, and until a new version the call is refused with invalid_request and the reason checks_failed. A second call while the version waits in the queue is refused with invalid_request and the reason integration_queued; get_proposal shows integration_queued. When fold itself cannot run the checks, it answers workspace_io with the reason fold_failed: nothing is sent back, and you integrate again once the fault is fixed. When fold-checks.toml on main itself is not valid, the integration stays queued until a revision of main fixes it. fold sends each result as a message.",
            schema_for_type::<IntegrateProposalInput>(),
        ),
        Tool::new(
            REQUIRE_HUMAN_REVIEW,
            "Mark a task so its proposal integrates only after a human approves the current version.",
            schema_for_type::<RequireHumanReviewInput>(),
        ),
    ];
    tools.extend(batches::tools());
    tools.extend(plans::tools());
    tools.push(Tool::new(
        SUBMIT_MAIN_CHANGE,
        "Main agent only, and only after the user's explicit OK to the diff and summary you showed. Integrate the newest changeset of your main workspace onto main as one revision and one Git commit with your message, with no task, proposal or review; the main workspace then closes. integration_conflicted means main moved: details list the files; call sync_workspace, resolve them, call record_changes, show the user the new diff and submit again.",
        schema_for_type::<SubmitMainChangeInput>(),
    ));
    tools.push(Tool::new(
        SUBMIT_REVIEW_FIX,
        "Reviewer only: send the fixes you made in your review checkout as the next version of the proposal, under your name, with each changed file and its reason and the checks you ran. Fix only what the task's agreed scope settles. Send work that the task still misses back with request_changes, describe new work outside the task with create_request (the proposal can still be approved), and ask the humans with asks_human for a product decision or a conflict with a settled decision. When fold-checks.toml has proposal checks for the fixed files, it returns at once with checks running and a run id: fold runs them in your checkout and makes the version only when they pass; the result comes as a message from fold. The version starts without approvals and your checkout then holds it: build it, run the required checks, then approve it with submit_review. The author's agent gets a push to sync its workspace. A batch checkout never becomes a version: fix a member in its own create_review_checkout. Refusals: review_claim_required, author_cannot_review, review_lead_delegates (the review lead never reviews), stale_version, version_unchanged, worker_busy, workspace_not_found (no checkout of that version) and invalid_request with reason fixes_mismatch (details list the unlisted and unchanged files).",
        schema_for_type::<SubmitReviewFixInput>(),
    ));
    tools.extend(checks::tools());
    tools.push(Tool::new(
        SET_REVIEW_SCOPE,
        "Review lead or main agent only: choose the review scope, change or full, of a version of an open proposal, with a reason, when fold's suggested_scope does not fit and you did not choose it when you assigned the review, for example a reviewer's own fix version. The newest choice for a version applies, and the reviewer follows it. A version with no earlier version that has a verdict is reviewed whole and is refused with invalid_request; another agent gets integrator_required.",
        schema_for_type::<SetReviewScopeInput>(),
    ));
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
    let actor = server.actor(as_worker).await?;
    let value = match tool {
        CREATE_PROPOSAL => create_proposal(reviews, &actor, arguments).await?,
        LIST_PROPOSALS => list_proposals(reviews, arguments).await?,
        GET_PROPOSAL => get_proposal(reviews, &actor, arguments).await?,
        GET_PROPOSAL_DIFF => get_proposal_diff(reviews, arguments).await?,
        UPDATE_PROPOSAL => update_proposal(reviews, &actor, arguments).await?,
        CLOSE_PROPOSAL => close_proposal(reviews, &actor, arguments).await?,
        SET_REVIEW_SCOPE => set_review_scope(reviews, &actor, arguments).await?,
        CLAIM_REVIEW => claim_review(reviews, &actor, arguments).await?,
        RELEASE_REVIEW => release_review(reviews, &actor, arguments).await?,
        CREATE_REVIEW_CHECKOUT => create_review_checkout(reviews, &actor, arguments).await?,
        SUBMIT_REVIEW => submit_review(reviews, &actor, arguments).await?,
        GET_REVIEW_COMMENTS => get_review_comments(reviews, arguments).await?,
        INTEGRATE_PROPOSAL => integrate_proposal(reviews, &actor, arguments).await?,
        REQUIRE_HUMAN_REVIEW => require_human_review(reviews, &actor, arguments).await?,
        SUBMIT_MAIN_CHANGE => submit_main_change(reviews, &actor, arguments).await?,
        SUBMIT_REVIEW_FIX => submit_review_fix(reviews, &actor, arguments).await?,
        _ => match batches::run(reviews, tool, &actor, arguments).await? {
            Some(value) => value,
            None => match plans::run(reviews, tool, &actor, arguments).await? {
                Some(value) => value,
                None => match checks::run(reviews, tool, &actor, arguments).await? {
                    Some(value) => value,
                    None => return Ok(None),
                },
            },
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

/// The result of `create_proposal` or `update_proposal`: the proposal when its version was made, or the check run fold makes it after.
fn proposed_json(proposed: Proposed) -> Value {
    match proposed {
        Proposed::Made(detail) => changed_json(&detail),
        Proposed::Checking {
            run,
            task,
            workspace,
        } => json!({
            "task": task,
            "workspace": workspace,
            "checks": "running",
            "run": run,
        }),
    }
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

/// The scope a review followed, with fold's suggestion and the rules behind it.
fn scope_record_json(scope: &ReviewScopeRecord) -> Value {
    json!({
        "scope": scope.scope.as_str(),
        "since": version(scope.since),
        "suggested": scope.suggested.as_str(),
        "rules": scope.rules.iter().map(|rule| rule.as_str()).collect::<Vec<_>>(),
        "reason": scope.reason,
    })
}

fn view_json(view: &ProposalView) -> Value {
    let detail = &view.detail;
    let summary = &detail.summary;
    let selected = &detail.selected;
    let versions: Vec<Value> = detail
        .versions
        .iter()
        .map(|entry| {
            let mut line = json!({
                "version": version(entry.version),
                "base": entry.base,
                "changeset": entry.changeset,
                "created_by": entry.created_by,
                "created_at": entry.created_at.as_millis(),
            });
            // The reviewer that holds the review claim made this version in its review checkout, so `created_by` is not the author.
            if entry.fixed_in_checkout.is_some() {
                line["reviewer_fix"] = json!(true);
            }
            line
        })
        .collect();
    let reviews: Vec<Value> = view
        .reviews
        .iter()
        .map(|review| {
            let mut line = json!({
                "version": version(review.version),
                "verdict": review.verdict.as_str(),
                "reviewer": review.reviewer.as_str(),
                "body": review.body,
                "comments": review.comments,
            });
            // A human edit keeps the agent approvals of the version it was made from.
            if let Some(kept) = review.kept_from {
                line["kept_from"] = json!(version(kept));
            }
            if let Some(scope) = &review.scope {
                line["scope"] = scope_record_json(scope);
            }
            line
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
        "last_reviewed": view.last_reviewed.map(version),
        "review_scope": view.last_reviewed.map(|since| json!({
            "since": version(since),
            "chosen": view.scope_choice.as_ref().map(|choice| json!({
                "scope": choice.scope.as_str(),
                "reason": choice.reason,
                "by": choice.chosen_by,
            })),
        })),
        "gates": view.gates.iter().map(gate_json).collect::<Vec<_>>(),
        "human_review": mark_json(summary.human_review.as_ref()),
        "conflict": detail.conflict,
        "checks": view.checks.iter().map(checks::run_json).collect::<Vec<_>>(),
        "checks_failed": summary.checks_failed,
        "integration_queued": summary.integration_queued,
        "review_plan": view.review_plan.as_ref().map(plans::proposal_plan_json),
        "integrated": view.integrated.map(|(revision, batch)| json!({
            "revision": revision,
            "batch": batch,
        })),
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
    let field = format!("comments[{index}]");
    let side = input
        .side
        .as_deref()
        .map(|side| {
            Side::parse(side).ok_or_else(|| {
                FoldError::new(
                    codes::INVALID_REQUEST,
                    format!("{field}: side is old or new"),
                )
                .with_details([("field", json!(field))])
                .with_hint("give side as old or new, then call submit_review again")
            })
        })
        .transpose()?;
    let location =
        CommentLocation::from_parts(&field, input.path, side, input.start_line, input.end_line)?;
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
    Ok(proposed_json(detail))
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
            limit: Some(input.limit.unwrap_or(DEFAULT_PROPOSAL_LIST)),
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

async fn get_proposal_diff(
    reviews: &ReviewService,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: GetProposalDiffInput = parse(arguments)?;
    let proposal = proposal_id(&input.id)?;
    let range = ChangeRange {
        version: input.version,
        since: input.since,
    };
    let (shown, since, mut value) = match input.path {
        Some(path) => {
            let diff = reviews
                .proposal_file_diff(proposal, range, &path)
                .await
                .map_err(from_review)?;
            (diff.version, diff.since, file_diff_value(&diff.change))
        }
        None => match input.since {
            Some(since) => {
                let changes = reviews
                    .proposal_since_change(proposal, input.version, since)
                    .await
                    .map_err(from_review)?;
                let facts = &changes.change;
                let mut value = comparison_value(proposal, &facts.delta);
                value["earlier"] = totals_value(&facts.earlier);
                value["new_files"] = json!(facts.new_files);
                value["base_moved"] = json!(facts.base_moved);
                value["base_conflicts"] = json!(facts.base_conflicts);
                let suggestion = scope_suggestion(changes.version, facts);
                value["suggested_scope"] = json!(suggestion.scope.as_str());
                value["scope_rules"] = json!(
                    suggestion
                        .rules
                        .iter()
                        .map(|rule| rule.as_str())
                        .collect::<Vec<_>>()
                );
                (changes.version, changes.since, value)
            }
            None => {
                let changes = reviews
                    .proposal_changes(proposal, range)
                    .await
                    .map_err(from_review)?;
                let value = comparison_value(proposal, &changes.change);
                (changes.version, changes.since, value)
            }
        },
    };
    let last = reviews
        .last_reviewed(proposal, shown)
        .await
        .map_err(from_review)?;
    value["version"] = json!(version(shown));
    value["since"] = json!(since.map(version));
    value["last_reviewed"] = json!(last.map(version));
    Ok(value)
}

fn comparison_value(proposal: ProposalId, comparison: &StateComparison) -> Value {
    json!({
        "id": proposal,
        "from": comparison.from.label,
        "to": comparison.to.label,
        "files": comparison.files.iter().map(file_line).collect::<Vec<_>>(),
        "totals": totals_value(&comparison.totals),
    })
}

fn totals_value(totals: &Totals) -> Value {
    json!({
        "files": totals.files,
        "added": totals.added_lines,
        "removed": totals.removed_lines,
    })
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
    Ok(proposed_json(detail))
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
    let input: ClaimReviewInput = parse(arguments)?;
    let choice = match (&input.scope, &input.reason) {
        (Some(scope), reason) => Some(scope_choice(
            &input.id,
            scope,
            reason.as_deref().unwrap_or_default(),
        )?),
        (None, Some(_)) => {
            return Err(
                FoldError::new(codes::INVALID_REQUEST, "a reason goes with a scope")
                    .with_details([("field", json!("reason"))])
                    .with_hint("pass scope with the reason, or leave both out"),
            );
        }
        (None, None) => None,
    };
    let claim = reviews
        .claim_review_with_scope(actor, proposal_id(&input.id)?, choice)
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

async fn set_review_scope(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: SetReviewScopeInput = parse(arguments)?;
    let choice = scope_choice(&input.id, &input.scope, &input.reason)?;
    let proposal = choice.proposal;
    let scope = choice.scope;
    reviews
        .set_review_scope(actor, input.version, choice)
        .await
        .map_err(from_review)?;
    Ok(json!({
        "id": proposal,
        "version": version(input.version),
        "scope": scope.as_str(),
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

async fn submit_review_fix(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: SubmitReviewFixInput = parse(arguments)?;
    let fixes: Vec<ReviewFix> = input
        .fixes
        .into_iter()
        .map(|fix| ReviewFix {
            path: fix.path,
            reason: fix.reason,
        })
        .collect();
    let fixed = reviews
        .submit_review_fix(
            actor,
            proposal_id(&input.id)?,
            input.version,
            &fixes,
            &input.checks,
        )
        .await
        .map_err(from_review)?;
    let fixed = match fixed {
        FixOutcome::Fixed(fixed) => fixed,
        FixOutcome::Checking {
            run,
            task,
            checkout,
        } => {
            return Ok(json!({
                "task": task,
                "checkout": checkout,
                "checks": "running",
                "run": run,
            }));
        }
    };
    let mut value = changed_json(&fixed.detail);
    value["from_version"] = json!(version(fixed.from_version));
    value["changeset"] = json!(fixed.changeset);
    value["checkout"] = json!(fixed.checkout);
    Ok(value)
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
        .submit_review(
            actor,
            proposal,
            input.version,
            verdict,
            &input.body,
            comments,
        )
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
        IntegrationOutcome::ChecksFailed { run } => Ok(json!({
            "id": proposal,
            "result": "checks_failed",
            "run": run,
        })),
        IntegrationOutcome::Queued => Ok(json!({
            "id": proposal,
            "result": "queued",
        })),
    }
}

async fn submit_main_change(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: SubmitMainChangeInput = parse(arguments)?;
    let changeset: ChangesetId = id(
        &input.changeset,
        "pass the changeset that record_changes returned, such as C-40",
    )?;
    let change = DirectChange {
        changeset,
        message: input.message,
    };
    match reviews
        .integrate_direct(actor, change)
        .await
        .map_err(from_review)?
    {
        DirectOutcome::Succeeded { revision } => Ok(json!({
            "changeset": changeset,
            "result": "succeeded",
            "revision": revision,
        })),
        DirectOutcome::Conflicted(conflicts) => Err(direct_conflicted(changeset, &conflicts)),
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
