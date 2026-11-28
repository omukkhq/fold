use std::time::Duration;

use fold_core::reviews::checks::{RunReason, RunState, Stage};
use fold_core::{
    Actor, AgentName, CheckRunId, EventActor, FoldError, LabelName, TaskId, codes, policy,
};
use fold_store::reviews::checks::{self, CheckRunRow};
use fold_store::tasks::claims::{self, ClaimTarget};
use fold_store::{StoreError, WriteTx, agents, tasks};
use serde_json::json;

use super::runner::CancelRequest;
use super::{ReviewError, ReviewService, run_not_found};
use crate::services::agents::starter_of;
use crate::services::tasks::{areas_of, task_not_found};

/// How long a cancel waits for its run to end. The run ends at once: its commands are dropped, which kills their process groups, and one write ends its row.
const CANCEL_WAIT: Duration = Duration::from_secs(30);

/// The running check runs a cancel names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelTarget {
    /// One run, which must still run.
    Run(CheckRunId),
    /// Every running proposal-stage run of the task.
    Task(TaskId),
    /// Every running proposal-stage run of the project; only a human names them all.
    All,
    /// Every running proposal-stage run that the worker of the agent asked for or that checks a task the agent claims: the runs a stop or a park of that worker leaves without a reader.
    Worker(AgentName),
    /// Every running proposal-stage run of the project when the project is paused; the pause's own policy decides who pauses, and each run's cancel right is checked as for the other targets.
    Pause,
}

impl ReviewService {
    /// Cancels the running proposal-stage runs that `target` names, and returns the runs this call ended as `cancelled`. Each run drops its commands, which kills the process group of the running command or ends its wait for a permit, starts no later command, and ends as failed with reason `cancelled`, the actor in `checks.finished` and a message to the worker that asked for it. The actor needs the right of `policy::can_cancel_check_run` for every run the target names, or nothing is cancelled. A run that ended first is `check_run_ended`, an integration run is refused with `invalid_request`, and a task or project without running runs gives an empty list.
    pub async fn cancel_check_runs(
        &self,
        actor: &Actor,
        target: CancelTarget,
    ) -> Result<Vec<CheckRunId>, ReviewError> {
        let by = actor.event_actor();
        let runs = self
            .store
            .write(async |tx| cancellable_runs(tx, actor, &target).await)
            .await?;
        let mut cancelled = Vec::with_capacity(runs.len());
        for run in runs {
            if self.cancel_one(&run, &by).await? {
                tracing::info!(run = %run.id, "check run cancelled");
                cancelled.push(run.id);
            } else if let CancelTarget::Run(id) = target {
                return Err(run_ended(id));
            }
        }
        Ok(cancelled)
    }

    /// Whether `run` ended as `cancelled` after the cancel: its own task ends it when a cancel reaches it, and this call ends a running row that no task runs any more.
    async fn cancel_one(&self, run: &CheckRunRow, by: &EventActor) -> Result<bool, ReviewError> {
        let (ended, answer) = tokio::sync::oneshot::channel();
        let request = CancelRequest {
            by: by.clone(),
            ended,
        };
        match self.checks.cancel(run.id, request) {
            Ok(()) => {
                let _ = tokio::time::timeout(CANCEL_WAIT, answer).await;
            }
            Err(_) => self.end_cancelled_run(run, by).await?,
        }
        let row = checks::get_run(self.store.read(), run.id).await?;
        Ok(row.is_some_and(|row| row.reason == Some(RunReason::Cancelled)))
    }
}

/// The running runs of `target` once the actor's right to cancel each is checked on the writer.
async fn cancellable_runs(
    tx: &mut WriteTx,
    actor: &Actor,
    target: &CancelTarget,
) -> Result<Vec<CheckRunRow>, ReviewError> {
    let runs = match *target {
        CancelTarget::Run(id) => {
            let run = checks::get_run(tx.conn(), id)
                .await?
                .ok_or_else(|| run_not_found(id))?;
            if run.state != RunState::Running {
                return Err(run_ended(id));
            }
            if run.stage == Stage::Integration {
                return Err(integration_run(id));
            }
            vec![run]
        }
        CancelTarget::Task(task) => {
            if tasks::fetch(tx.conn(), task).await?.is_none() {
                return Err(task_not_found(task).into());
            }
            running_proposal_runs(tx, Some(task)).await?
        }
        CancelTarget::All => {
            if !actor.is_human() {
                return Err(FoldError::new(
                    codes::HUMAN_REQUIRED,
                    "only a human cancels every check run of the project",
                )
                .with_hint("call cancel_check_run with one run id")
                .into());
            }
            running_proposal_runs(tx, None).await?
        }
        CancelTarget::Worker(ref agent) => worker_runs(tx, agent).await?,
        CancelTarget::Pause => running_proposal_runs(tx, None).await?,
    };
    let starter = starter_of(tx, actor).await?;
    for run in &runs {
        let labels: Vec<LabelName> = tasks::labels_for_conn(tx.conn(), run.task_run()?)
            .await?
            .into_iter()
            .map(|label| label.name)
            .collect();
        let areas = areas_of(tx, &labels).await?;
        let own_run =
            matches!(actor, Actor::Agent(agent) if run.worker.as_ref() == Some(&agent.worker));
        policy::can_cancel_check_run(&starter, own_run, &areas).map_err(|denied| {
            FoldError::from(denied).with_details([
                ("run", json!(run.id.to_string())),
                ("task", json!(run.subject())),
                (
                    "worker",
                    json!(run.worker.as_ref().map(|worker| worker.as_str())),
                ),
            ])
        })?;
    }
    Ok(runs)
}

/// The running proposal-stage runs that a worker of `agent` asked for or whose task `agent` holds the implementation claim of, oldest first.
async fn worker_runs(tx: &mut WriteTx, agent: &AgentName) -> Result<Vec<CheckRunRow>, StoreError> {
    let Some(row) = agents::find_agent(tx.conn(), agent).await? else {
        return Ok(Vec::new());
    };
    let claimed: Vec<TaskId> = claims::unreleased_of_agent(tx.conn(), row.id)
        .await?
        .into_iter()
        .filter_map(|claim| match claim.target {
            ClaimTarget::Task(task) => Some(task),
            ClaimTarget::Review { .. } => None,
        })
        .collect();
    Ok(running_proposal_runs(tx, None)
        .await?
        .into_iter()
        .filter(|run| {
            run.worker
                .as_ref()
                .is_some_and(|worker| worker.belongs_to(agent))
                || run.task.is_some_and(|task| claimed.contains(&task))
        })
        .collect())
}

/// The running proposal-stage runs of `task`, or of the project for `None`, oldest first.
async fn running_proposal_runs(
    tx: &mut WriteTx,
    task: Option<TaskId>,
) -> Result<Vec<CheckRunRow>, StoreError> {
    Ok(checks::running_runs(tx.conn())
        .await?
        .into_iter()
        .filter(|run| {
            run.stage == Stage::Proposal && task.is_none_or(|task| run.task == Some(task))
        })
        .collect())
}

fn run_ended(run: CheckRunId) -> ReviewError {
    FoldError::new(
        codes::CHECK_RUN_ENDED,
        format!("{run} does not run any more"),
    )
    .with_details([("run", json!(run.to_string()))])
    .with_hint("call get_check_run to read its result")
    .into()
}

fn integration_run(run: CheckRunId) -> ReviewError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!("{run} checks an integration, which a cancel does not stop"),
    )
    .with_details([
        ("reason", json!("integration_run")),
        ("run", json!(run.to_string())),
    ])
    .with_hint("wait for the integration to end; its checks finish under the integration lock")
    .into()
}
