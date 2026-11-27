use fold_core::{Actor, CheckRunId, FoldError, codes};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::error::from_review;
use super::super::parse;
use super::{id, task_id, version};
use crate::services::reviews::ReviewService;
use crate::services::reviews::checks::{
    BUILD_SLOT, CancelTarget, CheckRunView, RUN_SLOT, Started, log_tail,
};

pub const RUN_CHECKS: &str = "run_checks";
pub const GET_CHECK_RUN: &str = "get_check_run";
pub const CANCEL_CHECK_RUN: &str = "cancel_check_run";

/// The most lines of a failed result's log that `get_check_run` returns with `detail: full`.
const RESULT_TAIL_LINES: usize = 200;

/// The most characters of a failed result's log that `get_check_run` returns with `detail: full`.
const RESULT_TAIL_CHARS: usize = 20_000;

/// The most lines of a failed result's log in the short form: the end of a compiler or test report, where the cause is.
const SHORT_TAIL_LINES: usize = 60;

/// The most characters of a failed result's log in the short form, so a red run with several failed checks stays a few thousand characters.
const SHORT_TAIL_CHARS: usize = 3_000;

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct RunChecksInput {
    /// The task, such as T-12, whose active task workspace you own. fold checks the workspace's recorded files there.
    pub task: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetCheckRunInput {
    /// The check run, such as cr-4, from run_checks, a result message or get_proposal.
    pub run: String,
    /// full lists every command of the run, and gives each failed one the last 200 lines (at most 20000 characters) of its log. Leave it out for the short form: only the failed commands, each with the last 60 lines (at most 3000 characters) of its log, and not_shown, the number of the other commands.
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CancelCheckRunInput {
    /// The running check run to cancel, such as cr-4, from run_checks, a result message or get_proposal.
    pub run: String,
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            RUN_CHECKS,
            "Ask fold to run the project's checks from fold-checks.toml (its setup, then the proposal checks whose files match your change) in your task workspace. It returns at once; fold sends the result as a message, a failure with the check names and the end of the log. Wait for checks.finished with wait_for_events, and do not edit the files of the workspace until it comes: the first edit of a tracked file, by you or by a check, stops the run at once with files_changed and the path. checks none means the tree has no fold-checks.toml or no check runs for the change.",
            schema_for_type::<RunChecksInput>(),
        ),
        Tool::new(
            GET_CHECK_RUN,
            "Read one check run: its stage, state and reason, and each command that failed with its outcome, exit code, duration and the end of its log (the last 60 lines, at most 3000 characters), with not_shown counting the other commands; pass detail full for every command and the last 200 lines of each failed log. A running run with waiting_for memory holds its permit and waits until the machine has enough free memory to start its commands; with waiting_for check, it waits before the command of waiting_check until fewer runs than that check's max_parallel run it; with waiting_for run_slot, it waits at position (1 is next) until fewer runs of the project than [runs] max_parallel_runs of fold-checks.toml run; with waiting_for build_slot, a command waits for a slot of the machine's build gate, and its timeout starts once it has one.",
            schema_for_type::<GetCheckRunInput>(),
        ),
        Tool::new(
            CANCEL_CHECK_RUN,
            "Cancel a running check run: fold ends the process group of its running command, or its wait for memory or a run slot, starts no later check, and ends the run as failed with reason cancelled. The worker that asked for the run gets fold's message as for any end. The worker that asked for the run, the lead of its task's area and the main agent may cancel it; others get check_cancel_denied. A run that ended first gives check_run_ended, and an integration run is not cancelled.",
            schema_for_type::<CancelCheckRunInput>(),
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
        CANCEL_CHECK_RUN => cancel_check_run(reviews, actor, arguments).await?,
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

/// What a worker does while its run goes: the first edit of a tracked file stops the run.
pub const EDIT_HINT: &str = "do not edit the files of the workspace until checks.finished: the first edit of a tracked file stops the run with files_changed";

/// The answer of a command that started checks, or found none to run.
pub fn started_json(task: fold_core::TaskId, started: Started) -> Value {
    match started {
        Started::Running { run, workspace, .. } => json!({
            "task": task,
            "workspace": workspace,
            "checks": "running",
            "run": run,
            "hint": EDIT_HINT,
        }),
        Started::NoChecks => json!({"task": task, "checks": "none"}),
    }
}

async fn cancel_check_run(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: CancelCheckRunInput = parse(arguments)?;
    let run: CheckRunId = id(
        &input.run,
        "pass a check run id such as cr-4; get_proposal lists the runs of a proposal",
    )?;
    reviews
        .cancel_check_runs(actor, CancelTarget::Run(run))
        .await
        .map_err(from_review)?;
    Ok(json!({"run": run, "state": "failed", "reason": "cancelled"}))
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
    let full = match input.detail.as_deref() {
        None => false,
        Some("full") => true,
        Some(_) => {
            return Err(
                FoldError::new(codes::INVALID_REQUEST, "detail is full or left out")
                    .with_details([("field", json!("detail"))])
                    .with_hint(
                        "call get_check_run with detail full, or leave detail out for the failed commands only",
                    ),
            );
        }
    };
    let (tail_lines, tail_chars) = if full {
        (RESULT_TAIL_LINES, RESULT_TAIL_CHARS)
    } else {
        (SHORT_TAIL_LINES, SHORT_TAIL_CHARS)
    };
    let view = reviews
        .get_check_run(run, true)
        .await
        .map_err(from_review)?;
    let mut value = run_json(&view);
    value["task"] = json!(view.run.task);
    value["workspace"] = json!(view.run.workspace);
    let shown: Vec<_> = view
        .results
        .iter()
        .filter(|result| full || result.outcome.is_failure())
        .collect();
    if !full {
        value["not_shown"] = json!(view.results.len() - shown.len());
    }
    value["results"] = json!(
        shown
            .into_iter()
            .map(|result| {
                let mut line = json!({
                    "name": result.check.as_deref().unwrap_or("setup"),
                    "outcome": result.outcome.as_str(),
                    "required": result.required,
                    "exit_code": result.exit_code,
                    "duration_ms": result.duration_ms,
                });
                if result.outcome.is_failure() {
                    line["log_tail"] = json!(log_tail(&result.log, tail_lines, tail_chars));
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
    if let Some(since) = view.waiting_for_memory {
        line["waiting_for"] = json!("memory");
        line["waiting_since"] = json!(since.as_millis());
    }
    if let Some((check, since)) = &view.waiting_for_check {
        line["waiting_for"] = json!("check");
        line["waiting_check"] = json!(check);
        line["waiting_since"] = json!(since.as_millis());
    }
    if let Some(since) = view.waiting_for_build_slot {
        line["waiting_for"] = json!(BUILD_SLOT);
        line["waiting_since"] = json!(since.as_millis());
    }
    if let Some(position) = view.waiting_for_run_slot {
        line["waiting_for"] = json!(RUN_SLOT);
        line["position"] = json!(position);
    }
    line
}
