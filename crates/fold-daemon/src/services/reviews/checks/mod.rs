use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::reviews::ProposalIntent;
use fold_core::reviews::checks::{
    ChecksFile, Outcome, Purpose, RunReason, RunState, Stage, checks_file_changed, runs_any,
};
use fold_core::{
    Actor, AddressedAs, CheckRunId, Clock, EventActor, FoldError, IdKind, MessageBody, NewEvent,
    ProposalId, Recipient, Target, TaskId, Timestamp, WorkerName, WorkspaceId, codes, policy,
};
use fold_store::reviews::checks::{self, CheckResultRow, CheckRunRow, NewCheckResult, NewCheckRun};
use fold_store::workspaces::{self, WorkspaceRow, WorkspaceState};
use fold_store::{SqliteConnection, Store, StoreError, WriteTx, counters, tasks};
use serde_json::json;

use super::proposals::{
    Proposed, VersionInput, add_version_in_tx, no_task_workspace, open_in_tx, usable_workspace,
};
use super::{ReviewError, ReviewService};
use crate::services::events::{EventBuffer, EventBus};
use crate::services::messages::send_from_fold_in_tx;
use crate::services::tasks::{TaskError, closed_refusal, proposal_kind_refusal, task_not_found};
use crate::services::workspaces::ProposalSnapshot;

mod integration;
mod runner;

pub(crate) use integration::{Integrating, IntegrationChecks, PassedRun, fold_failed_refusal};
pub(crate) use runner::HeldRequest;
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
    pub(super) fn passed() -> Self {
        Self {
            state: RunState::Passed,
            reason: None,
        }
    }

    pub(super) fn failed(reason: RunReason) -> Self {
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

/// What a run carries to its end: the commands, whether the checked tree changes the checks file, and the actor whose command waits.
#[derive(Debug, Clone)]
pub(super) struct GateJob {
    pub specs: Vec<CommandSpec>,
    pub checks_changed: bool,
    /// The reviewer's fix a `review_fix` run waits to make.
    pub fix: Option<FixJob>,
}

/// What a reviewer's fix needs at the end of its run, beyond the stored run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FixJob {
    /// The version the reviewer fixed, which its checkout holds.
    pub version: u32,
    pub checkout: WorkspaceId,
    /// How many files the fix names.
    pub files: usize,
}

/// What the proposal checks decide for a version about to be made.
#[derive(Debug, Clone)]
pub(super) enum GateDecision {
    /// Make the version now: the tree has no proposal check to run, or a passed run of the same files on the same base stands.
    Ready { checks_changed: bool },
    /// Run the checks first.
    Run(GateJob),
}

impl ReviewService {
    /// Starts a run of the proposal checks in the actor's own task workspace of `task`, by the worker that owns it: the workspace's recorded files are snapshotted, and that tree on the workspace's base is what is checked. The call returns at once; fold runs the setup and the checks in the workspace and sends the result to the worker as a message from fold. A tree without `fold-checks.toml`, or whose change no proposal check runs for, starts nothing. An invalid file is refused with `checks_file_invalid`, and a second run while one goes in the workspace with `checks_running`.
    pub async fn run_checks(&self, actor: &Actor, task: TaskId) -> Result<Started, ReviewError> {
        let workspace = self.checks_workspace(actor, task).await?;
        let snapshot = self.workspaces.snapshot_for_proposal(&workspace).await?;
        let tree = snapshot.tree;
        drop(snapshot);
        let (job, _) = self.proposal_checks(workspace.base, tree).await?;
        let Some(job) = job else {
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
        self.spawn_proposal_run(run, job, actor.clone());
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

    /// The commands of a proposal-stage run of `tree` on `base`, or `None` when the tree has no checks file or no proposal check runs for the change, and whether the change adds, changes or removes the checks file.
    async fn proposal_checks(
        &self,
        base: fold_core::RevisionId,
        tree: ObjectHash,
    ) -> Result<(Option<GateJob>, bool), ReviewError> {
        let base_tree = workspaces::get_by_number(self.store.read(), base)
            .await?
            .tree_hash;
        let input = self.workspaces.checks_input(base_tree, tree).await?;
        let checks_changed =
            checks_file_changed(input.base_entry.as_ref(), input.tree_entry.as_ref());
        let Some(text) = input.file else {
            return Ok((None, checks_changed));
        };
        let file = ChecksFile::parse(&text)?;
        let plan = file.plan(Stage::Proposal, &input.changed);
        if !runs_any(&plan) {
            return Ok((None, checks_changed));
        }
        let job = GateJob {
            specs: runner::commands(&file, &plan),
            checks_changed,
            fix: None,
        };
        Ok((Some(job), checks_changed))
    }

    /// The proposal checks of a version about to be made from `snapped` with `tree`. The tracker is the workspace's newest ended run: when its checks passed on the workspace's base and this tree, the same files were checked already and nothing runs again. That holds also for a run that passed but whose command was refused by then, such as one whose claim was lost meanwhile.
    pub(super) async fn proposal_gate(
        &self,
        snapped: &WorkspaceRow,
        tree: ObjectHash,
    ) -> Result<GateDecision, ReviewError> {
        self.gate_in(snapped.id, snapped.base, tree).await
    }

    /// [`ReviewService::proposal_gate`] of `tree` on `base`, with the tracker of the directory `workspace` the checks run in.
    pub(super) async fn gate_in(
        &self,
        workspace: WorkspaceId,
        base: fold_core::RevisionId,
        tree: ObjectHash,
    ) -> Result<GateDecision, ReviewError> {
        let (job, checks_changed) = self.proposal_checks(base, tree).await?;
        let Some(job) = job else {
            return Ok(GateDecision::Ready { checks_changed });
        };
        let last = checks::last_finished_proposal_run(self.store.read(), workspace).await?;
        let reused = last.is_some_and(|last| {
            let green = last.state == RunState::Passed
                || (last.state == RunState::Stopped && last.reason == Some(RunReason::Refused));
            green && last.base == base && last.tree == tree
        });
        if reused {
            return Ok(GateDecision::Ready { checks_changed });
        }
        Ok(GateDecision::Run(job))
    }

    /// Starts the run a proposal command waits on and answers at once.
    pub(super) async fn start_gate(
        &self,
        actor: &Actor,
        snapped: &WorkspaceRow,
        tree: ObjectHash,
        purpose: Purpose,
        waiting: Waiting<'_>,
        job: GateJob,
    ) -> Result<Proposed, ReviewError> {
        let run = self
            .start_proposal_run(actor, snapped, tree, purpose, waiting)
            .await?;
        let checking = Proposed::Checking {
            run: run.id,
            task: run.task,
            workspace: snapped.id,
        };
        self.spawn_proposal_run(run, job, actor.clone());
        Ok(checking)
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
                let new = NewProposalRun {
                    purpose,
                    task,
                    workspace: live.id,
                    worker_id: live.worker_id,
                    base: live.base,
                    tree,
                    actor: &label,
                    waiting,
                };
                insert_proposal_run(tx, &mut buffer, &new, now).await
            })
            .await?;
        self.bus.publish(buffer);
        self.workspaces.checks_changed().await;
        tracing::info!(run = %run.id, task = %task, workspace = %snapped.id, purpose = purpose.as_str(), "check run started");
        Ok(run)
    }

    /// Runs the commands of `run` in the background and ends it.
    pub(super) fn spawn_proposal_run(&self, run: CheckRunRow, job: GateJob, actor: Actor) {
        let service = self.clone();
        self.checks.spawn(async move {
            let id = run.id;
            if let Err(error) = service.complete_proposal_run(run, job, actor).await {
                tracing::warn!(run = %id, code = error.log_code(), "a check run could not end; the next start of the daemon ends it as stopped");
            }
        });
    }

    /// Takes a machine-wide permit, runs the commands in the workspace and snapshots it again. The run fails when the snapshot differs from the checked tree, or when the workspace was replaced, synced or restored meanwhile. A passed run of a proposal command then makes its version; a run whose command is refused by then, such as a claim that was taken over, ends as stopped with the refusal in its message.
    async fn complete_proposal_run(
        &self,
        run: CheckRunRow,
        job: GateJob,
        actor: Actor,
    ) -> Result<(), ReviewError> {
        if run.purpose == Purpose::ReviewFix {
            return self.complete_fix_run(run, job, actor).await;
        }
        let workspace = run
            .workspace
            .ok_or(StoreError::Corrupt("check_runs.workspace_id"))?;
        let row = workspaces::get_workspace(self.store.read(), workspace)
            .await?
            .ok_or(StoreError::Corrupt("check_runs.workspace_id"))?;
        let permit = self.checks.permits().acquire().await;
        let dir = self.workspaces.workspace_dir(&row);
        let owner = self
            .checks
            .owner(run.worker.as_ref().map(WorkerName::agent));
        let results = run_commands_for(
            &owner,
            &dir,
            job.specs.clone(),
            self.checks.limits(),
            &*self.clock,
        )
        .await;
        drop(permit);
        let (mut verdict, checked) = self.recheck_workspace(&run, &results).await?;
        let mut refused = None;
        if verdict.state == RunState::Passed
            && matches!(
                run.purpose,
                Purpose::CreateProposal | Purpose::UpdateProposal
            )
            && let Some((live, snapshot)) = checked
        {
            let gated = Gated {
                run: &run,
                results: &results,
                job: &job,
                actor: &actor,
                snapped: &live,
            };
            match self.make_version(gated, snapshot).await {
                Ok(()) => return Ok(()),
                Err(error) if is_refusal(&error) => {
                    verdict = Verdict {
                        state: RunState::Stopped,
                        reason: Some(RunReason::Refused),
                    };
                    refused = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        self.end_proposal_run(&run, &results, verdict, refused.as_ref())
            .await
    }

    /// The verdict of a finished run once the workspace is snapshotted again, with that workspace and snapshot when the snapshot equals the checked tree.
    async fn recheck_workspace(
        &self,
        run: &CheckRunRow,
        results: &[CommandResult],
    ) -> Result<(Verdict, Option<(WorkspaceRow, ProposalSnapshot)>), ReviewError> {
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
        Ok((Verdict::of(results), Some((live, snapshot))))
    }

    /// One write transaction: the passed run's results and end, the proposal or version it waited for exactly as the command makes it at once, and a message to the author. The snapshot is held until the commit, as a proposal command holds it.
    async fn make_version(
        &self,
        gated: Gated<'_>,
        snapshot: ProposalSnapshot,
    ) -> Result<(), ReviewError> {
        let Gated {
            run,
            results,
            job,
            actor,
            snapped,
        } = gated;
        let input = VersionInput {
            actor,
            snapped,
            recorded: &snapshot,
            evidence: run.evidence.as_deref(),
            intent: run.intent.as_ref(),
            checks_changed: job.checks_changed,
            now: snapshot.taken_at,
        };
        let now = input.now;
        let mut buffer = EventBuffer::new();
        let made = self
            .store
            .write(async |tx| {
                if !end_run_in_tx(tx, &mut buffer, run, results, Verdict::passed(), now).await? {
                    return Ok(None);
                }
                #[cfg(feature = "failpoints")]
                fail::fail_point!("checks_before_version", |_| Err(ReviewError::from(
                    crate::services::fired("checks_before_version")
                )));
                let (proposal, version) = match (run.purpose, run.proposal) {
                    (Purpose::UpdateProposal, Some(proposal)) => (
                        proposal,
                        add_version_in_tx(tx, &mut buffer, proposal, &input).await?,
                    ),
                    _ => {
                        let title = run
                            .title
                            .as_deref()
                            .ok_or(StoreError::Corrupt("check_runs.title"))?;
                        (
                            open_in_tx(tx, &mut buffer, run.task, title, &input).await?,
                            1,
                        )
                    }
                };
                let body = format!(
                    "fold made {proposal} v{version} for {}: its checks passed ({}).",
                    run.task, run.id
                );
                tell_requester(tx, &mut buffer, run, &body, now).await?;
                Ok::<_, ReviewError>(Some(proposal))
            })
            .await?;
        drop(snapshot);
        let Some(proposal) = made else {
            return Ok(());
        };
        self.bus.publish(buffer);
        self.workspaces
            .proposal_changed(snapped.id, Some(proposal))
            .await;
        self.workspaces.checks_changed().await;
        tracing::info!(run = %run.id, proposal = %proposal, "check run passed and made its version");
        Ok(())
    }

    /// One write transaction: the results, the run's end, `checks.finished` and the result message to the requesting worker. `refused` is the refusal of the command a passed run waited for.
    pub(super) async fn end_proposal_run(
        &self,
        run: &CheckRunRow,
        results: &[CommandResult],
        verdict: Verdict,
        refused: Option<&ReviewError>,
    ) -> Result<(), ReviewError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let ended = self
            .store
            .write(async |tx| {
                if !end_run_in_tx(tx, &mut buffer, run, results, verdict, now).await? {
                    return Ok(false);
                }
                let body = match refused {
                    Some(error) => refused_message(run, error),
                    None => result_message(run, results, verdict),
                };
                tell_requester(tx, &mut buffer, run, &body, now).await?;
                Ok::<_, ReviewError>(true)
            })
            .await?;
        if ended {
            self.bus.publish(buffer);
            self.workspaces.checks_changed().await;
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

/// A running run that makes the next version of a proposal when its checks pass: the author's update (`update_proposal`) or a reviewer's fix (`review_fix`). It has no version until then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionRun {
    pub run: CheckRunId,
    pub purpose: Purpose,
}

/// The run of [`VersionRun`] of `proposal` of the task `task`, if one goes. An approval or integration of the current version waits for it: the older version's integration would end it as `workspace_changed`.
pub(super) async fn next_version_run(
    conn: &mut SqliteConnection,
    task: TaskId,
    proposal: ProposalId,
) -> Result<Option<VersionRun>, StoreError> {
    Ok(checks::running_version_run(&mut *conn, task, proposal)
        .await?
        .map(|run| VersionRun {
            run: run.id,
            purpose: run.purpose,
        }))
}

/// A proposal-stage run to write as running.
pub(super) struct NewProposalRun<'a> {
    pub purpose: Purpose,
    pub task: TaskId,
    /// Where the commands run: the task workspace, or a reviewer's review checkout.
    pub workspace: WorkspaceId,
    pub worker_id: Option<i64>,
    pub base: fold_core::RevisionId,
    pub tree: ObjectHash,
    pub actor: &'a EventActor,
    pub waiting: Waiting<'a>,
}

/// Writes a running proposal-stage run with `checks.started` in the caller's write, and refuses a second one of the same workspace with `checks_running`, which the run's unique index also decides.
pub(super) async fn insert_proposal_run(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    new: &NewProposalRun<'_>,
    now: Timestamp,
) -> Result<CheckRunRow, ReviewError> {
    let workspace = new.workspace;
    if let Some(going) = checks::running_in_workspace(tx.conn(), workspace).await? {
        return Err(checks_running(going.id, workspace));
    }
    let number = counters::allocate(tx, IdKind::CheckRun).await?;
    let id = CheckRunId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
    let row = NewCheckRun {
        id,
        stage: Stage::Proposal,
        purpose: new.purpose,
        task: new.task,
        workspace: Some(workspace),
        worker_id: new.worker_id,
        proposal: new.waiting.proposal,
        version: None,
        base: new.base,
        tree: new.tree,
        actor: new.actor.as_str(),
        title: new.waiting.title,
        evidence: new.waiting.evidence,
        intent: new.waiting.intent,
        batch: None,
    };
    match checks::insert_run(tx, &row, now).await {
        Ok(()) => {}
        Err(error) if error.unique_violation() => {
            return match checks::running_in_workspace(tx.conn(), workspace).await? {
                Some(going) => Err(checks_running(going.id, workspace)),
                None => Err(error.into()),
            };
        }
        Err(error) => return Err(error.into()),
    }
    let mut event = NewEvent::new(types::CHECKS_STARTED, new.actor.clone())?
        .task(new.task)
        .workspace(workspace)
        .revision(new.base);
    if let Some(proposal) = new.waiting.proposal {
        event = event.proposal(proposal);
    }
    let event = event.data(json!({
        "run": id.to_string(),
        "stage": Stage::Proposal.as_str(),
        "purpose": new.purpose.as_str(),
    }))?;
    buffer.append(tx, &event, now).await?;
    Ok(checks::get_run(tx.conn(), id)
        .await?
        .ok_or(StoreError::Corrupt("check_runs.number"))?)
}

/// A passed run of a proposal command with what its version needs.
struct Gated<'a> {
    run: &'a CheckRunRow,
    results: &'a [CommandResult],
    job: &'a GateJob,
    actor: &'a Actor,
    /// The workspace as the second snapshot found it: the checked one, on the checked base.
    snapped: &'a WorkspaceRow,
}

/// A refusal of the command a passed run waited for, which ends the run as stopped, as opposed to a failure that leaves it for the next daemon start.
pub(super) fn is_refusal(error: &ReviewError) -> bool {
    match error {
        ReviewError::Fold(error) | ReviewError::Task(TaskError::Fold(error)) => {
            error.code() != codes::FAILPOINT
        }
        _ => false,
    }
}

/// The message of a passed run whose command was refused when its version was to be made.
fn refused_message(run: &CheckRunRow, error: &ReviewError) -> String {
    let (message, hint) = match error {
        ReviewError::Fold(error) | ReviewError::Task(TaskError::Fold(error)) => (
            error.message().to_owned(),
            error.hint().unwrap_or_default().to_owned(),
        ),
        _ => (String::new(), String::new()),
    };
    let what = match run.proposal {
        Some(proposal) => format!("fold added no version to {proposal}"),
        None => "fold made no proposal".to_owned(),
    };
    let mut text = format!(
        "The checks of {} ({}) passed, but {what}: {message}.",
        run.task, run.id
    );
    if !hint.is_empty() {
        text.push_str(&format!(" Next: {hint}."));
    }
    text
}

/// What a proposal-stage run carries for the proposal or version it waits to make.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Waiting<'a> {
    pub proposal: Option<ProposalId>,
    pub title: Option<&'a str>,
    pub evidence: Option<&'a str>,
    pub intent: Option<&'a ProposalIntent>,
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
            if let Some(gate) = gate_sentence(run) {
                text.push(' ');
                text.push_str(&gate);
            }
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

/// What a failed run of a proposal command did not make, and how to ask again.
fn gate_sentence(run: &CheckRunRow) -> Option<String> {
    match (run.purpose, run.proposal) {
        (Purpose::CreateProposal, _) => Some(
            "fold made no proposal; fix the work in the workspace and call create_proposal again."
                .to_owned(),
        ),
        (Purpose::UpdateProposal, Some(proposal)) => Some(format!(
            "fold added no version to {proposal}; fix the work in the workspace and call update_proposal again."
        )),
        (Purpose::ReviewFix, Some(proposal)) => Some(format!(
            "fold added no fix version to {proposal}; fix the files in your review checkout and call submit_review_fix again."
        )),
        _ => None,
    }
}

/// The tool that asks for the same run again.
fn again_tool(purpose: Purpose) -> &'static str {
    match purpose {
        Purpose::CreateProposal => "create_proposal",
        Purpose::UpdateProposal => "update_proposal",
        Purpose::ReviewFix => "submit_review_fix",
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
            fault: false,
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
            intent: None,
            state: RunState::Running,
            reason: None,
            created_at: Timestamp::from_millis(0).unwrap(),
            finished_at: None,
            batch: None,
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
