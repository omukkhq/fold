use fold_core::reviews::{IntentPart, ProposalIntent};
use fold_core::{Actor, FoldError, ProposalId, TaskId, codes};
use fold_daemon::services::reviews::{FixOutcome, Proposed, ReviewError, ReviewFix, ReviewService};
use rmcp::model::{CallToolRequestParams, CallToolResult, JsonObject};
use rmcp::{Peer, RoleClient};
use serde_json::{Value, json};

/// The intent a test writes when the intent is not what it tests. The MCP call helpers that go through [`call_tool`] send it as one part that covers every file the version changes; [`Covering`] does the same for the service calls.
pub const WHOLE_CHANGE: &str = "Make the change the task asks for.";

/// One part named "The change" with `files`, and [`WHOLE_CHANGE`] as the summary and as what the part does.
pub fn whole_change(files: &[&str]) -> ProposalIntent {
    ProposalIntent {
        summary: WHOLE_CHANGE.to_owned(),
        parts: vec![IntentPart {
            name: "The change".to_owned(),
            does: WHOLE_CHANGE.to_owned(),
            files: files.iter().map(|path| (*path).to_owned()).collect(),
        }],
        ..ProposalIntent::default()
    }
}

/// An intent of `summary` and `parts`, each a name, what it does and its files.
pub fn intent_of(summary: &str, parts: &[(&str, &str, &[&str])]) -> ProposalIntent {
    ProposalIntent {
        summary: summary.to_owned(),
        parts: parts
            .iter()
            .map(|(name, does, files)| IntentPart {
                name: (*name).to_owned(),
                does: (*does).to_owned(),
                files: files.iter().map(|path| (*path).to_owned()).collect(),
            })
            .collect(),
        ..ProposalIntent::default()
    }
}

/// [`whole_change`] as the JSON of an MCP call's `intent`.
pub fn whole_change_json(files: &[&str]) -> Value {
    serde_json::to_value(whole_change(files)).unwrap()
}

/// The changed files an `invalid_intent` refusal names as belonging to no part.
fn uncovered(details: Option<&serde_json::Map<String, Value>>) -> Option<Vec<String>> {
    let details = details?;
    if details.get("reason")? != "part_files_uncovered" {
        return None;
    }
    Some(
        details
            .get("uncovered")?
            .as_array()?
            .iter()
            .filter_map(|path| path.as_str().map(str::to_owned))
            .collect(),
    )
}

/// Calls `tool` with `arguments`. A `create_proposal`, `update_proposal` or `submit_review_fix` whose `intent` is [`WHOLE_CHANGE`] goes first with one part of no files and, when fold names changed files that no part covers, again with them, as an author adds the files fold names. A refused call writes nothing, so the second call sees the same project.
pub async fn call_tool(
    client: &Peer<RoleClient>,
    tool: &str,
    mut arguments: JsonObject,
) -> CallToolResult {
    let proposes = matches!(
        tool,
        "create_proposal" | "update_proposal" | "submit_review_fix"
    );
    if !proposes || arguments.get("intent") != Some(&json!(WHOLE_CHANGE)) {
        return send(client, tool, arguments).await;
    }
    arguments.insert("intent".to_owned(), whole_change_json(&[]));
    let first = send(client, tool, arguments.clone()).await;
    let error = (first.is_error == Some(true))
        .then(|| {
            let text = &first.content[0].as_text()?.text;
            serde_json::from_str::<Value>(text).ok()
        })
        .flatten()
        .map(|body| body["error"].clone());
    let files = error
        .filter(|error| error["code"] == codes::INVALID_INTENT)
        .and_then(|error| uncovered(error["details"].as_object()));
    match files {
        Some(files) => {
            let files: Vec<&str> = files.iter().map(String::as_str).collect();
            arguments.insert("intent".to_owned(), whole_change_json(&files));
            send(client, tool, arguments).await
        }
        None => first,
    }
}

async fn send(client: &Peer<RoleClient>, tool: &str, arguments: JsonObject) -> CallToolResult {
    client
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
        .await
        .unwrap()
}

/// Runs `call` with [`whole_change`] of no files and, when fold names changed files that no part covers, again with them.
async fn covering<T>(
    call: impl AsyncFn(ProposalIntent) -> Result<T, ReviewError>,
) -> Result<T, ReviewError> {
    covering_with(whole_change(&[]), call).await
}

/// Runs `call` with `intent` and, when fold names changed files that no part covers, again with them added to its first part.
pub async fn covering_with<T>(
    intent: ProposalIntent,
    call: impl AsyncFn(ProposalIntent) -> Result<T, ReviewError>,
) -> Result<T, ReviewError> {
    match call(intent.clone()).await {
        Err(ReviewError::Fold(error)) => match uncovered_of(&error) {
            Some(files) => {
                let mut intent = intent;
                if let Some(first) = intent.parts.first_mut() {
                    first.files.extend(files);
                }
                call(intent).await
            }
            None => Err(ReviewError::Fold(error)),
        },
        other => other,
    }
}

fn uncovered_of(error: &FoldError) -> Option<Vec<String>> {
    (error.code() == codes::INVALID_INTENT)
        .then(|| uncovered(error.details()))
        .flatten()
}

/// The proposal commands of [`ReviewService`] with an intent of one part that covers every file the version changes, for the tests whose subject is not the intent.
#[expect(
    async_fn_in_trait,
    reason = "a test helper that only the test program calls"
)]
pub trait Covering {
    async fn create_proposal_covering(
        &self,
        actor: &Actor,
        task: TaskId,
        title: &str,
        evidence: Option<&str>,
    ) -> Result<Proposed, ReviewError>;

    async fn update_proposal_covering(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        evidence: Option<&str>,
    ) -> Result<Proposed, ReviewError>;

    async fn submit_review_fix_covering(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        version: u32,
        fixes: &[ReviewFix],
        checks: &str,
    ) -> Result<FixOutcome, ReviewError>;
}

impl Covering for ReviewService {
    async fn create_proposal_covering(
        &self,
        actor: &Actor,
        task: TaskId,
        title: &str,
        evidence: Option<&str>,
    ) -> Result<Proposed, ReviewError> {
        covering(async |intent| {
            self.create_proposal(actor, task, title, intent, evidence)
                .await
        })
        .await
    }

    async fn update_proposal_covering(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        evidence: Option<&str>,
    ) -> Result<Proposed, ReviewError> {
        covering(async |intent| {
            self.update_proposal(actor, proposal, intent, evidence)
                .await
        })
        .await
    }

    async fn submit_review_fix_covering(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        version: u32,
        fixes: &[ReviewFix],
        checks: &str,
    ) -> Result<FixOutcome, ReviewError> {
        covering(async |intent| {
            self.submit_review_fix(actor, proposal, version, fixes, checks, intent)
                .await
        })
        .await
    }
}
