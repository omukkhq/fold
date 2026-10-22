use fold_core::{Actor, CheckRunId, FoldError};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::error::from_review;
use super::super::parse;
use super::{id, task_id, version};
use crate::services::reviews::ReviewService;
use crate::services::reviews::checks::{CheckRunView, Started, log_tail};

pub const RUN_CHECKS: &str = "run_checks";
pub const GET_CHECK_RUN: &str = "get_check_run";

/// The most lines of a failed result's log that `get_check_run` returns.
const RESULT_TAIL_LINES: usize = 200;

/// The most characters of a failed result's log that `get_check_run` returns.
const RESULT_TAIL_CHARS: usize = 20_000;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RunChecksInput {
    /// The task, such as T-12, whose active task workspace you own. fold checks the workspace's recorded files there.
    pub task: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetCheckRunInput {
    /// The check run, such as cr-4, from run_checks, a result message or get_proposal.
    pub run: String,
    /// The worker that makes this call, such as frontend/ui. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            RUN_CHECKS,
            "Ask fold to run the project's checks from fold-checks.toml (its setup, then the proposal checks whose files match your change) in your task workspace. It returns at once; fold sends the result as a message, a failure with the check names and the end of the log. Wait for checks.finished with wait_for_events. A check that changes recorded files fails the run. checks none means the tree has no fold-checks.toml or no check runs for the change.",
            schema_for_type::<RunChecksInput>(),
        ),
        Tool::new(
            GET_CHECK_RUN,
            "Read one check run: its stage, state and reason, and each command's outcome, exit code and duration, with the end of the log of each command that failed.",
            schema_for_type::<GetCheckRunInput>(),
        ),
    ]
}

/// Runs a check tool. `None` when `tool` is not one.
pub async fn run(
    reviews: &ReviewService,
    tool: &str,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Option<Value>, FoldError> {
    let value = match tool {
        RUN_CHECKS => run_checks(reviews, actor, arguments).await?,
        GET_CHECK_RUN => get_check_run(reviews, arguments).await?,
        _ => return Ok(None),
    };
    Ok(Some(value))
}

async fn run_checks(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: RunChecksInput = parse(arguments)?;
    let task = task_id(&input.task)?;
    let started = reviews.run_checks(actor, task).await.map_err(from_review)?;
    Ok(started_json(task, started))
}

/// The answer of a command that started checks, or found none to run.
pub fn started_json(task: fold_core::TaskId, started: Started) -> Value {
    match started {
        Started::Running { run, workspace, .. } => json!({
            "task": task,
            "workspace": workspace,
            "checks": "running",
            "run": run,
        }),
        Started::NoChecks => json!({"task": task, "checks": "none"}),
    }
}

async fn get_check_run(
    reviews: &ReviewService,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: GetCheckRunInput = parse(arguments)?;
    let run: CheckRunId = id(
        &input.run,
        "pass a check run id such as cr-4; get_proposal lists the runs of a proposal",
    )?;
    let view = reviews
        .get_check_run(run, true)
        .await
        .map_err(from_review)?;
    let mut value = run_json(&view);
    value["task"] = json!(view.run.task);
    value["workspace"] = json!(view.run.workspace);
    value["results"] = json!(
        view.results
            .iter()
            .map(|result| {
                let mut line = json!({
                    "name": result.check.as_deref().unwrap_or("setup"),
                    "outcome": result.outcome.as_str(),
                    "required": result.required,
                    "exit_code": result.exit_code,
                    "duration_ms": result.duration_ms,
                });
                if result.outcome.is_failure() {
                    line["log_tail"] =
                        json!(log_tail(&result.log, RESULT_TAIL_LINES, RESULT_TAIL_CHARS));
                    line["log_bytes"] = json!(result.log_bytes);
                }
                line
            })
            .collect::<Vec<_>>()
    );
    Ok(value)
}

/// One run as `get_proposal` lists it: compact, with the names of the commands that failed.
pub fn run_json(view: &CheckRunView) -> Value {
    let run = &view.run;
    let failed: Vec<&str> = view
        .results
        .iter()
        .filter(|result| result.outcome.is_failure())
        .map(|result| result.check.as_deref().unwrap_or("setup"))
        .collect();
    let mut line = json!({
        "run": run.id,
        "stage": run.stage.as_str(),
        "state": run.state.as_str(),
        "reason": run.reason.map(|reason| reason.as_str()),
        "failed": failed,
    });
    if let Some(number) = run.version {
        line["version"] = json!(version(number));
    }
    if let Some(batch) = run.batch {
        line["batch"] = json!(batch);
    }
    line
}
