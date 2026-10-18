use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::reviews::checks::{
    ChecksFile, Outcome, Purpose, RunReason, RunState, Stage, runs_any,
};
use fold_core::{
    Actor, AddressedAs, CheckRunId, Clock, EventActor, FoldError, IdKind, MessageBody, NewEvent,
    ProposalId, Recipient, Target, TaskId, Timestamp, WorkerName, WorkspaceId, codes, policy,
};
use fold_store::reviews::checks::{self, CheckResultRow, CheckRunRow, NewCheckResult, NewCheckRun};
use fold_store::workspaces::{self, WorkspaceRow, WorkspaceState};
use fold_store::{SqliteConnection, Store, StoreError, WriteTx, counters, tasks};
use serde_json::json;

use super::proposals::{no_task_workspace, usable_workspace};
use super::{ReviewError, ReviewService};
use crate::services::events::{EventBuffer, EventBus};
use crate::services::messages::send_from_fold_in_tx;
use crate::services::tasks::{closed_refusal, proposal_kind_refusal, task_not_found};
use crate::services::workspaces::ProposalSnapshot;

mod runner;

pub use runner::{
    CheckPermits, CheckRunner, CommandResult, CommandSpec, LOG_LIMIT, RunLimits, commands,
    run_commands, run_commands_for,
};

/// The most lines of a failed command's log a result message carries.
const TAIL_LINES: usize = 30;

/// The most characters of a failed command's log a result message carries.
const TAIL_CHARS: usize = 3000;

/// One stored run with its results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRunView {
    pub run: CheckRunRow,
    /// In run order; each log is empty unless the read asked for logs.
    pub results: Vec<CheckResultRow>,
}

/// What a request for checks started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Started {
    /// fold runs the checks in the background and sends the result as a message.
    Running {
        run: CheckRunId,
        task: TaskId,
        workspace: WorkspaceId,
    },
    /// The tree has no `fold-checks.toml`, or no check of the stage runs for its change.
    NoChecks,
}

/// How a run ended, before its row says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Verdict {
    pub state: RunState,
    pub reason: Option<RunReason>,
}

impl Verdict {
    fn passed() -> Self {
        Self {
            state: RunState::Passed,
            reason: None,
        }
    }

    fn failed(reason: RunReason) -> Self {
        Self {
            state: RunState::Failed,
            reason: Some(reason),
        }
    }

    /// From the commands' results alone: a blocking setup failure, then a blocking check failure, else a pass.
    pub(super) fn of(results: &[CommandResult]) -> Self {
        if results
            .iter()
            .any(|result| result.check.is_none() && result.blocks())
        {
            return Self::failed(RunReason::SetupFailed);
        }
        if results.iter().any(CommandResult::blocks) {
            return Self::failed(RunReason::CheckFailed);
        }
        Self::passed()
    }
}

/// The commands a proposal-stage run of `tree` would run, read from the tree's checks file against the change from `base`.
pub(super) struct ProposalChecks {
    pub specs: Vec<CommandSpec>,
}

impl ReviewService {
    /// Starts a run of the proposal checks in the actor's own task workspace of `task`, by the worker that owns it: the workspace's recorded files are snapshotted, and that tree on the workspace's base is what is checked. The call returns at once; fold runs the setup and the checks in the workspace and sends the result to the worker as a message from fold. A tree without `fold-checks.toml`, or whose change no proposal check runs for, starts nothing. An invalid file is refused with `checks_file_invalid`, and a second run while one goes in the workspace with `checks_running`.
    pub async fn run_checks(&self, actor: &Actor, task: TaskId) -> Result<Started, ReviewError> {
        let workspace = self.checks_workspace(actor, task).await?;
        let snapshot = self.workspaces.snapshot_for_proposal(&workspace).await?;
        let tree = snapshot.tree;
        drop(snapshot);
        let Some(checks) = self.proposal_checks(workspace.base, tree).await? else {
            return Ok(Started::NoChecks);
        };
        let run = self
            .start_proposal_run(actor, &workspace, tree, Purpose::Check, Waiting::default())
            .await?;
        let started = Started::Running {
            run: run.id,
            task,
            workspace: workspace.id,
        };
        self.spawn_proposal_run(run, checks.specs);
        Ok(started)
    }

    /// One stored run with its results, and their logs when `logs` is true.
    pub async fn get_check_run(
        &self,
        run: CheckRunId,
        logs: bool,
    ) -> Result<CheckRunView, ReviewError> {
        let mut read = self.store.read().begin().await.map_err(StoreError::from)?;
        let row = checks::get_run(&mut *read, run)
            .await?
            .ok_or_else(|| run_not_found(run))?;
        let results = checks::list_results(&mut *read, run, logs).await?;
        read.commit().await.map_err(StoreError::from)?;
        Ok(CheckRunView { run: row, results })
    }

    /// The task workspace a worker runs checks in: the task is open and of a kind with a proposal, and the actor's worker owns its active workspace.
    async fn checks_workspace(
        &self,
        actor: &Actor,
        task: TaskId,
    ) -> Result<WorkspaceRow, ReviewError> {
        let read = self.store.read();
        let row = tasks::get_by_number(read, task)
            .await?
            .ok_or_else(|| task_not_found(task))?;
        if let Some(refusal) = closed_refusal(&row).or_else(|| proposal_kind_refusal(&row)) {
            return Err(refusal.into());
        }
        let live = workspaces::live_task_workspace(read, task).await?;
        let Some(found) = &live else {
            return Err(no_task_workspace(task));
        };
        let owner = found.worker.clone();
        policy::can_run_checks(actor, owner.as_ref()).map_err(|denied| {
            FoldError::from(denied).with_details([
                ("task", json!(task.to_string())),
                ("owner", json!(owner.as_ref().map(WorkerName::as_str))),
            ])
        })?;
        let owner = owner.ok_or(StoreError::Corrupt("workspaces.worker_id"))?;
        usable_workspace(live, task, &owner)
    }

    /// The commands of a proposal-stage run of `tree` on `base`, or `None` when the tree has no checks file or no proposal check runs for the change.
    pub(super) async fn proposal_checks(
        &self,
        base: fold_core::RevisionId,
        tree: ObjectHash,
    ) -> Result<Option<ProposalChecks>, ReviewError> {
        let base_tree = workspaces::get_by_number(self.store.read(), base)
            .await?
            .tree_hash;
        let input = self.workspaces.checks_input(base_tree, tree).await?;
        let Some(text) = input.file else {
            return Ok(None);
        };
        let file = ChecksFile::parse(&text)?;
        let plan = file.plan(Stage::Proposal, &input.changed);
        if !runs_any(&plan) {
            return Ok(None);
        }
        Ok(Some(ProposalChecks {
            specs: runner::commands(&file, &plan),
        }))
    }

    /// Writes the running run of `tree` in `snapped`, with `checks.started`, after the same refusals again on the writer: the task's active workspace is still `snapped` on the same base and owned by the actor's worker.
    pub(super) async fn start_proposal_run(
        &self,
        actor: &Actor,
        snapped: &WorkspaceRow,
        tree: ObjectHash,
        purpose: Purpose,
        waiting: Waiting<'_>,
    ) -> Result<CheckRunRow, ReviewError> {
        let task = snapped
            .task
            .ok_or(StoreError::Corrupt("workspaces.task_id"))?;
        let now = self.clock.now();
        let label = actor.event_actor();
        let mut buffer = EventBuffer::new();
        let run = self
            .store
            .write(async |tx| {
                let live = workspaces::live_task_workspace(tx.conn(), task).await?;
                let owner = live.as_ref().and_then(|row| row.worker.clone());
                policy::can_run_checks(actor, owner.as_ref()).map_err(FoldError::from)?;
                let live = live.ok_or(StoreError::Corrupt("workspaces.task_id"))?;
                if live.id != snapped.id || live.base != snapped.base {
                    return Err(changed_meanwhile(snapped.id));
                }
                if let Some(going) = checks::running_in_workspace(tx.conn(), live.id).await? {
                    return Err(checks_running(going.id, live.id));
                }
                let number = counters::allocate(tx, IdKind::CheckRun).await?;
                let id =
                    CheckRunId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
                let new = NewCheckRun {
                    id,
                    stage: Stage::Proposal,
                    purpose,
                    task,
                    workspace: Some(live.id),
                    worker_id: live.worker_id,
                    proposal: waiting.proposal,
                    version: None,
                    base: live.base,
                    tree,
                    actor: label.as_str(),
                    title: waiting.title,
                    evidence: waiting.evidence,
                };
                checks::insert_run(tx, &new, now).await?;
                let mut event = NewEvent::new(types::CHECKS_STARTED, label.clone())?
                    .task(task)
                    .workspace(live.id)
                    .revision(live.base);
                if let Some(proposal) = waiting.proposal {
                    event = event.proposal(proposal);
                }
                let event = event.data(json!({
                    "run": id.to_string(),
                    "stage": Stage::Proposal.as_str(),
                    "purpose": purpose.as_str(),
                }))?;
                buffer.append(tx, &event, now).await?;
                checks::get_run(tx.conn(), id)
                    .await?
                    .ok_or(StoreError::Corrupt("check_runs.number"))
                    .map_err(ReviewError::from)
            })
            .await?;
        self.bus.publish(buffer);
        tracing::info!(run = %run.id, task = %task, workspace = %snapped.id, purpose = purpose.as_str(), "check run started");
        Ok(run)
    }

    /// Runs the commands of `run` in the background and ends it.
    pub(super) fn spawn_proposal_run(&self, run: CheckRunRow, specs: Vec<CommandSpec>) {
        let service = self.clone();
        self.checks.spawn(async move {
            let id = run.id;
            if let Err(error) = service.complete_proposal_run(run, specs).await {
                tracing::warn!(run = %id, code = error.log_code(), "a check run could not end; the next start of the daemon ends it as stopped");
            }
        });
    }

    /// Takes a machine-wide permit, runs the commands in the workspace and snapshots it again. The run fails when the snapshot differs from the checked tree, or when the workspace was replaced, synced or restored meanwhile.
    async fn complete_proposal_run(
        &self,
        run: CheckRunRow,
        specs: Vec<CommandSpec>,
    ) -> Result<(), ReviewError> {
        let workspace = run
            .workspace
            .ok_or(StoreError::Corrupt("check_runs.workspace_id"))?;
        let row = workspaces::get_workspace(self.store.read(), workspace)
            .await?
            .ok_or(StoreError::Corrupt("check_runs.workspace_id"))?;
        let permit = self.checks.permits().acquire().await;
        let dir = self.workspaces.workspace_dir(&row);
        let results = run_commands(&dir, specs, self.checks.limits(), &*self.clock).await;
        drop(permit);
        let (verdict, snapshot) = self.recheck_workspace(&run, &results).await?;
        self.end_proposal_run(&run, &results, verdict, snapshot)
            .await
    }

    /// The verdict of a finished run once the workspace is snapshotted again, with that snapshot when it equals the checked tree.
    async fn recheck_workspace(
        &self,
        run: &CheckRunRow,
        results: &[CommandResult],
    ) -> Result<(Verdict, Option<ProposalSnapshot>), ReviewError> {
        let live = workspaces::live_task_workspace(self.store.read(), run.task).await?;
        let Some(live) = live.filter(|live| {
            Some(live.id) == run.workspace
                && live.base == run.base
                && live.state == WorkspaceState::Active
        }) else {
            return Ok((Verdict::failed(RunReason::WorkspaceChanged), None));
        };
        let snapshot = match self.workspaces.snapshot_for_proposal(&live).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::debug!(run = %run.id, code = error.log_code(), "the workspace could not be snapshotted after its checks");
                return Ok((Verdict::failed(RunReason::WorkspaceChanged), None));
            }
        };
        if snapshot.tree != run.tree {
            return Ok((Verdict::failed(RunReason::FilesChanged), None));
        }
        Ok((Verdict::of(results), Some(snapshot)))
    }

    /// One write transaction: the results, the run's end, `checks.finished` and the result message to the requesting worker. `snapshot` is held until the commit.
    async fn end_proposal_run(
        &self,
        run: &CheckRunRow,
        results: &[CommandResult],
        verdict: Verdict,
        snapshot: Option<ProposalSnapshot>,
    ) -> Result<(), ReviewError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let ended = self
            .store
            .write(async |tx| {
                if !end_run_in_tx(tx, &mut buffer, run, results, verdict, now).await? {
                    return Ok(false);
                }
                let body = result_message(run, results, verdict);
                tell_requester(tx, &mut buffer, run, &body, now).await?;
                Ok::<_, ReviewError>(true)
            })
            .await?;
        drop(snapshot);
        if ended {
            self.bus.publish(buffer);
            tracing::info!(run = %run.id, state = verdict.state.as_str(), reason = verdict.reason.map(RunReason::as_str), "check run ended");
        }
        Ok(())
    }

    /// The runs of one version of a proposal, newest first, without logs, on `conn`.
    pub(super) async fn version_checks(
        conn: &mut SqliteConnection,
        task: TaskId,
        proposal: ProposalId,
        version: u32,
    ) -> Result<Vec<CheckRunView>, ReviewError> {
        let runs = checks::runs_of_version(&mut *conn, task, proposal, version).await?;
        let mut views = Vec::with_capacity(runs.len());
        for run in runs {
            let results = checks::list_results(&mut *conn, run.id, false).await?;
            views.push(CheckRunView { run, results });
        }
        Ok(views)
    }
}

/// What a proposal-stage run carries for the proposal or version it waits to make.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Waiting<'a> {
    pub proposal: Option<ProposalId>,
    pub title: Option<&'a str>,
    pub evidence: Option<&'a str>,
}

/// Writes the results and the end of a running run with `checks.finished`, in the caller's write. False, with nothing written, when the run had already ended, as after a daemon start ended it as stopped.
pub(super) async fn end_run_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    run: &CheckRunRow,
    results: &[CommandResult],
    verdict: Verdict,
    now: Timestamp,
) -> Result<bool, ReviewError> {
    let current = checks::get_run(tx.conn(), run.id).await?;
    if current.is_none_or(|row| row.state != RunState::Running) {
        return Ok(false);
    }
    for (position, result) in results.iter().enumerate() {
        let new = NewCheckResult {
            position: u32::try_from(position).unwrap_or(u32::MAX),
            check: result.check.as_deref(),
            required: result.required,
            outcome: result.outcome,
            exit_code: result.exit_code,
            duration_ms: result.duration_ms,
            log: &result.log,
            log_bytes: result.log_bytes,
        };
        checks::insert_result(tx, run.id, &new).await?;
    }
    if !checks::finish_run(tx, run.id, verdict.state, verdict.reason, now).await? {
        return Ok(false);
    }
    let mut event = NewEvent::new(types::CHECKS_FINISHED, EventActor::fold())?
        .task(run.task)
        .revision(run.base);
    if let Some(workspace) = run.workspace {
        event = event.workspace(workspace);
    }
    if let Some(proposal) = run.proposal {
        event = event.proposal(proposal);
    }
    let event = event.data(json!({
        "run": run.id.to_string(),
        "stage": run.stage.as_str(),
        "purpose": run.purpose.as_str(),
        "state": verdict.state.as_str(),
        "reason": verdict.reason.map(RunReason::as_str),
        "version": run.version,
    }))?;
    buffer.append(tx, &event, now).await?;
    Ok(true)
}

/// Sends `body` from fold to the worker that asked for the run, linked to its task and proposal.
pub(super) async fn tell_requester(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    run: &CheckRunRow,
    body: &str,
    now: Timestamp,
) -> Result<(), ReviewError> {
    let Some(worker) = &run.worker else {
        return Ok(());
    };
    let body = MessageBody::parse(body)?;
    let to = Recipient {
        target: Target::Worker(worker.clone()),
        addressed_as: AddressedAs::Worker,
    };
    send_from_fold_in_tx(tx, buffer, vec![to], (run.task, run.proposal), &body, now).await?;
    Ok(())
}

/// The result message of a run: which commands failed, and the end of the first failed log, so the worker can fix it without another call.
pub(super) fn result_message(
    run: &CheckRunRow,
    results: &[CommandResult],
    verdict: Verdict,
) -> String {
    let subject = format!("The checks of {} ({})", run.task, run.id);
    let again = again_tool(run.purpose);
    match verdict.reason {
        None => {
            let names: Vec<&str> = results
                .iter()
                .filter(|result| result.outcome == Outcome::Passed)
                .filter_map(|result| result.check.as_deref())
                .collect();
            let optional = failed_names(results, false);
            let mut text = format!("{subject} passed: {}.", list_or_none(&names));
            if !optional.is_empty() {
                text.push_str(&format!(
                    " Failed but not required: {}.",
                    optional.join(", ")
                ));
            }
            text
        }
        Some(RunReason::FilesChanged) => format!(
            "{subject} failed: the recorded files of the workspace changed during the run. A check must leave tracked files as they are: put what it writes in .gitignore or .foldignore, and do not edit files until the run ends, then call {again} again."
        ),
        Some(RunReason::WorkspaceChanged) => format!(
            "{subject} failed: the workspace was synced, restored or replaced during the run, or it holds unresolved conflicts. Call {again} again."
        ),
        Some(reason) => {
            let failed = failed_names(results, true);
            let what = if reason == RunReason::SetupFailed {
                "the setup failed, so no check ran".to_owned()
            } else {
                failed.join(", ")
            };
            let mut text = format!("{subject} failed: {what}.");
            if let Some(first) = results.iter().find(|result| result.blocks()) {
                let name = first.check.as_deref().unwrap_or("setup");
                text.push_str(&format!(
                    "\n\nThe end of the log of {name}:\n{}",
                    log_tail(&first.log, TAIL_LINES, TAIL_CHARS)
                ));
            }
            text.push_str(&format!(
                "\n\nCall get_check_run with {} for every result.",
                run.id
            ));
            text
        }
    }
}

/// The tool that asks for the same run again.
fn again_tool(purpose: Purpose) -> &'static str {
    match purpose {
        Purpose::CreateProposal => "create_proposal",
        Purpose::UpdateProposal => "update_proposal",
        Purpose::Check | Purpose::Integrate => "run_checks",
    }
}

fn failed_names(results: &[CommandResult], required: bool) -> Vec<String> {
    results
        .iter()
        .filter(|result| result.required == required && result.outcome.is_failure())
        .map(|result| {
            let name = result.check.as_deref().unwrap_or("setup");
            match (result.outcome, result.exit_code) {
                (Outcome::TimedOut, _) => format!("{name} timed out"),
                (_, Some(code)) => format!("{name} failed (exit {code})"),
                (_, None) => format!("{name} failed"),
            }
        })
        .collect()
}

fn list_or_none(names: &[&str]) -> String {
    if names.is_empty() {
        "no check ran".to_owned()
    } else {
        names.join(", ")
    }
}

/// The last `lines` lines of a log, at most `chars` characters, as text.
pub fn log_tail(log: &[u8], lines: usize, chars: usize) -> String {
    let text = String::from_utf8_lossy(log);
    let text = text.trim_end();
    let start = text
        .rmatch_indices('\n')
        .nth(lines.saturating_sub(1))
        .map_or(0, |(at, _)| at + 1);
    let tail = &text[start..];
    let count = tail.chars().count();
    if count <= chars {
        return tail.to_owned();
    }
    tail.chars().skip(count - chars).collect()
}

/// Ends every run a stopped daemon left running as `stopped` with `checks.finished`, and tells each requesting worker to ask again. Nothing waits on these runs any more, and no version or revision exists for them: a run's results and what they make are written in one transaction.
pub async fn recover_check_runs(
    store: &Store,
    bus: &EventBus,
    clock: &dyn Clock,
) -> Result<usize, ReviewError> {
    let running = checks::running_runs(store.read()).await?;
    let mut ended = 0;
    for run in running {
        let now = clock.now();
        let verdict = Verdict {
            state: RunState::Stopped,
            reason: Some(RunReason::DaemonStopped),
        };
        let mut buffer = EventBuffer::new();
        let done = store
            .write(async |tx| {
                if !end_run_in_tx(tx, &mut buffer, &run, &[], verdict, now).await? {
                    return Ok(false);
                }
                let again = again_tool(run.purpose);
                let body = format!(
                    "fold stopped while the checks of {} ran ({}), so they have no result. Call {again} again.",
                    run.task, run.id
                );
                tell_requester(tx, &mut buffer, &run, &body, now).await?;
                Ok::<_, ReviewError>(true)
            })
            .await?;
        if done {
            bus.publish(buffer);
            ended += 1;
            tracing::info!(run = %run.id, "a check run that a daemon stop cut short ended as stopped");
        }
    }
    Ok(ended)
}

fn run_not_found(run: CheckRunId) -> ReviewError {
    FoldError::new(codes::CHECK_RUN_NOT_FOUND, format!("{run} does not exist"))
        .with_details([("run", json!(run.to_string()))])
        .with_hint("call get_proposal to see the check runs of a proposal")
        .into()
}

pub(super) fn checks_running(run: CheckRunId, workspace: WorkspaceId) -> ReviewError {
    FoldError::new(
        codes::CHECKS_RUNNING,
        format!("{run} still runs the checks in {workspace}"),
    )
    .with_details([
        ("run", json!(run.to_string())),
        ("workspace", json!(workspace.to_string())),
    ])
    .with_hint("wait for fold's message with the result, then try again")
    .into()
}

fn changed_meanwhile(workspace: WorkspaceId) -> ReviewError {
    FoldError::new(
        codes::INVALID_REQUEST,
        format!("{workspace} changed while it was recorded"),
    )
    .with_details([
        ("reason", json!("workspace_changed")),
        ("workspace", json!(workspace.to_string())),
    ])
    .with_hint("call the command again")
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(check: Option<&str>, outcome: Outcome, required: bool) -> CommandResult {
        CommandResult {
            check: check.map(str::to_owned),
            required,
            outcome,
            exit_code: match outcome {
                Outcome::Passed => Some(0),
                Outcome::Failed => Some(2),
                _ => None,
            },
            duration_ms: 5,
            log: b"line 1\nline 2\nboom\n".to_vec(),
            log_bytes: 19,
        }
    }

    fn run() -> CheckRunRow {
        CheckRunRow {
            id: CheckRunId::from_number(4).unwrap(),
            stage: Stage::Proposal,
            purpose: Purpose::Check,
            task: TaskId::from_number(12).unwrap(),
            workspace: WorkspaceId::from_number(3),
            worker: None,
            proposal: None,
            version: None,
            base: fold_core::RevisionId::from_number(1).unwrap(),
            tree: ObjectHash::from_bytes([1; 32]),
            actor: "backend".to_owned(),
            title: None,
            evidence: None,
            state: RunState::Running,
            reason: None,
            created_at: Timestamp::from_millis(0).unwrap(),
            finished_at: None,
        }
    }

    #[test]
    fn the_verdict_follows_the_blocking_results() {
        let ok = result(Some("test"), Outcome::Passed, true);
        let lint = result(Some("lint"), Outcome::Failed, false);
        assert_eq!(Verdict::of(&[ok.clone(), lint.clone()]), Verdict::passed());
        let bad = result(Some("test"), Outcome::TimedOut, true);
        assert_eq!(
            Verdict::of(&[bad, lint]),
            Verdict::failed(RunReason::CheckFailed)
        );
        let setup = result(None, Outcome::Failed, true);
        let not_run = result(Some("test"), Outcome::NotRun, true);
        assert_eq!(
            Verdict::of(&[setup, not_run]),
            Verdict::failed(RunReason::SetupFailed)
        );
        assert_eq!(Verdict::of(&[]), Verdict::passed());
    }

    #[test]
    fn a_failure_message_names_the_check_and_ends_with_its_log() {
        let results = [
            result(Some("test"), Outcome::Failed, true),
            result(Some("lint"), Outcome::Passed, true),
        ];
        let text = result_message(&run(), &results, Verdict::failed(RunReason::CheckFailed));
        assert!(text.starts_with("The checks of T-12 (cr-4) failed: test failed (exit 2)."));
        assert!(text.contains("The end of the log of test:\nline 1\nline 2\nboom"));
        assert!(text.ends_with("Call get_check_run with cr-4 for every result."));

        let passed = result_message(
            &run(),
            &[
                results[1].clone(),
                result(Some("fmt"), Outcome::Failed, false),
            ],
            Verdict::passed(),
        );
        assert_eq!(
            passed,
            "The checks of T-12 (cr-4) passed: lint. Failed but not required: fmt failed (exit 2)."
        );
        let setup = result_message(
            &run(),
            &[result(None, Outcome::Failed, true)],
            Verdict::failed(RunReason::SetupFailed),
        );
        assert!(setup.contains("the setup failed, so no check ran"));
        assert!(setup.contains("The end of the log of setup:"));
        for reason in [RunReason::FilesChanged, RunReason::WorkspaceChanged] {
            let text = result_message(&run(), &[], Verdict::failed(reason));
            assert!(text.contains("run_checks again"), "{text}");
        }
        assert_eq!(
            result_message(&run(), &[], Verdict::passed()),
            "The checks of T-12 (cr-4) passed: no check ran."
        );
    }

    #[test]
    fn a_log_tail_keeps_the_last_lines_and_characters() {
        let log: String = (1..=50).map(|n| format!("line {n}\n")).collect();
        let tail = log_tail(log.as_bytes(), 3, 1000);
        assert_eq!(tail, "line 48\nline 49\nline 50");
        assert_eq!(log_tail(log.as_bytes(), 3, 4), "e 50");
        assert_eq!(log_tail(b"one", 30, 100), "one");
        assert_eq!(log_tail(b"", 30, 100), "");
    }
}
