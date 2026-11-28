use std::collections::BTreeSet;
use tracing::Instrument as _;

use fold_core::object::ObjectHash;
use fold_core::reviews::checks::{
    CHECKS_FILE, ChecksFile, Outcome, Purpose, RunReason, RunState, Stage, runs_any,
};
use fold_core::{
    Actor, AddressedAs, ChangesetId, CheckRunId, EventActor, FoldError, IdKind, MessageBody,
    NewEvent, ProposalId, Recipient, ReviewBatchId, RevisionId, Target, TaskId, WorkerName,
    WorkspaceId, codes,
};
use fold_store::reviews::checks::{self, CheckRunRow, NewCheckRun};
use fold_store::reviews::{self};
use fold_store::{StoreError, WriteTx, counters};
use serde_json::json;

use super::runner::{self, CommandResult};
use super::{TAIL_CHARS, TAIL_LINES, Verdict, end_run_in_tx, end_run_with_in_tx, log_tail};
use crate::services::events::EventBuffer;
use crate::services::messages::send_from_fold_in_tx;
use crate::services::reviews::conflicts::{Fixer, Handoff, conflict_fixer, handoff_notice_in_tx};
use crate::services::reviews::lanes::lane_moved_note;
use crate::services::reviews::{ReviewError, ReviewService};
use crate::services::tasks::recompute;

/// A passed integration run whose end waits for the integration's own transaction, so a revision never exists without it and a stop before that leaves the run without an end.
#[derive(Debug, Clone)]
pub(crate) struct PassedRun {
    pub run: CheckRunRow,
    pub results: Vec<CommandResult>,
}

/// What the integration checks of one attempt decided.
#[derive(Debug)]
pub(crate) enum IntegrationChecks {
    /// The merged tree's checks file has no integration check that runs for the change.
    None,
    Passed,
    /// The run failed and the proposal went back to its author, or for a member of a lane that moved since its version's base, to the reviewer that approved it; a direct change stays in the main workspace and fold tells the main agent.
    Failed(CheckRunId),
    /// fold itself could not run the checks: the run stopped as `fold_failed`, and the proposal keeps its version, its approvals and its place with the reviewer.
    FoldFailed(CheckRunId),
    /// The checks file of main itself is not valid, so the fault is not the proposal's: no run starts, nothing is sent back, and the integration waits until main's file is fixed.
    Held,
}

/// The direct change of the main agent an attempt integrates, as the checks need it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DirectIntegrating {
    pub changeset: ChangesetId,
    /// The main workspace of the changeset, which stays open when the checks fail.
    pub workspace: WorkspaceId,
    pub head: RevisionId,
    pub head_tree: ObjectHash,
    pub merged: ObjectHash,
}

/// What the integration checks of an attempt check: a proposal version or a batch, or a direct change.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Checked<'a> {
    Proposal(Integrating<'a>),
    Direct(DirectIntegrating),
}

impl Checked<'_> {
    fn head(&self) -> RevisionId {
        match self {
            Checked::Proposal(at) => at.head,
            Checked::Direct(at) => at.head,
        }
    }

    fn head_tree(&self) -> ObjectHash {
        match self {
            Checked::Proposal(at) => at.head_tree,
            Checked::Direct(at) => at.head_tree,
        }
    }

    fn merged(&self) -> ObjectHash {
        match self {
            Checked::Proposal(at) => at.merged,
            Checked::Direct(at) => at.merged,
        }
    }

    /// Whether `run` is a run of this same change on the same main and merged tree.
    fn ran(&self, run: &CheckRunRow) -> bool {
        let same = match self {
            Checked::Proposal(at) => {
                run.proposal == Some(at.proposal) && run.version == Some(at.version)
            }
            Checked::Direct(at) => run.changeset == Some(at.changeset),
        };
        same && run.base == self.head() && run.tree == self.merged()
    }
}

/// The proposal version an attempt integrates, as the checks need it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Integrating<'a> {
    pub proposal: ProposalId,
    pub task: TaskId,
    pub version: u32,
    /// The head of main the merge ran onto, and its tree.
    pub head: RevisionId,
    pub head_tree: ObjectHash,
    pub merged: ObjectHash,
    /// The submitted batch whose members merge together into `merged`; the run is stored on `proposal`, the last of them, and names the batch. A failed run of a batch sends nobody back: the caller integrates the members one at a time.
    pub batch: Option<ReviewBatchId>,
    /// The version row ids of the batch's members in `merged`, which the run names so each member's page lists it; empty for one proposal.
    pub members: &'a [i64],
}

impl ReviewService {
    /// Runs the integration checks of the merged tree in fold's check directory, under the integration mutex the caller holds, so main cannot move meanwhile. The checks that run are the integration-stage checks of the merged tree's own `fold-checks.toml` whose `files` pattern matches a path the merge changes on main. A passed run of an earlier attempt on the same change, head and tree is reused from `carried`; one on another state ends as `superseded`. A passed run is left in `carried` for the caller's transaction to end; a failed one ends here, with the proposal sent back to its author, or given to its reviewer when it is a member of a lane that moved since its version's base, and for a direct change with a message to the main agent. A direct change is never held for main's invalid checks file: the main agent waits for its answer, so its run fails on the merged tree's file instead.
    pub(crate) async fn integration_checks(
        &self,
        actor: &Actor,
        at: Checked<'_>,
        carried: &mut Option<PassedRun>,
    ) -> Result<IntegrationChecks, ReviewError> {
        let input = self
            .workspaces
            .checks_input(at.head_tree(), at.merged())
            .await?;
        let Some(text) = input.file else {
            self.end_carried(carried).await?;
            return Ok(IntegrationChecks::None);
        };
        let file = match ChecksFile::parse(&text) {
            Ok(file) => file,
            Err(error) => {
                self.end_carried(carried).await?;
                if let Checked::Proposal(proposal) = at
                    && let Some(problem) = self.main_checks_problem(proposal.head_tree).await?
                {
                    self.hold_for_main(proposal.head, &problem).await?;
                    return Ok(IntegrationChecks::Held);
                }
                let run = self.start_integration_run(actor, at).await?;
                let message = format!("{}\n", error.message()).into_bytes();
                let invalid = CommandResult {
                    check: Some(CHECKS_FILE.to_owned()),
                    required: true,
                    outcome: Outcome::Failed,
                    exit_code: None,
                    duration_ms: 0,
                    log_bytes: message.len() as u64,
                    log: message,
                    fault: false,
                    killed: None,
                    no_build_slot: false,
                };
                let verdict = Verdict::failed(RunReason::ChecksFileInvalid);
                self.end_failed(actor, at, &run, &[invalid], verdict)
                    .await?;
                return Ok(IntegrationChecks::Failed(run.id));
            }
        };
        let plan = file.plan(Stage::Integration, &input.changed);
        if !runs_any(&plan) {
            self.end_carried(carried).await?;
            return Ok(IntegrationChecks::None);
        }
        if carried.as_ref().is_some_and(|passed| at.ran(&passed.run)) {
            return Ok(IntegrationChecks::Passed);
        }
        self.end_carried(carried).await?;
        let run = self.start_integration_run(actor, at).await?;
        let specs = runner::commands(&file, &plan);
        let lock = self.checks.check_dir();
        let held = lock.lock().await;
        // An integration holds the integration mutex, so it counts against `[runs] max_parallel_runs` but never waits for a place.
        let slot = self.checks.slot_ticket(None);
        let permit = self.checks.start_permit(run.id, &*self.clock).await;
        let fault = Verdict {
            state: RunState::Stopped,
            reason: Some(RunReason::FoldFailed),
        };
        let (results, verdict) = match self
            .workspaces
            .check_dir_to(at.merged(), self.checks.dir_work().await)
            .await
        {
            Ok(dir) => {
                let results = runner::run_gated_commands(
                    &self.checks.gate(run.id),
                    &dir,
                    specs,
                    self.checks.limits(),
                    &*self.clock,
                )
                .instrument(tracing::info_span!("check_run", run = %run.id))
                .await;
                let verdict = if results.iter().any(|result| result.fault) {
                    tracing::warn!(run = %run.id, "a command of the integration checks could not start or be waited for");
                    fault
                } else {
                    match self
                        .workspaces
                        .check_dir_holds(at.merged(), self.checks.dir_work().await)
                        .await
                    {
                        Ok(true) => Verdict::of(&results),
                        Ok(false) => Verdict::failed(RunReason::FilesChanged),
                        Err(error) => {
                            tracing::warn!(run = %run.id, code = error.log_code(), "the check directory could not be read after its checks");
                            fault
                        }
                    }
                };
                (results, verdict)
            }
            Err(error) => {
                tracing::warn!(run = %run.id, code = error.log_code(), "the check directory could not take the merged tree");
                (Vec::new(), fault)
            }
        };
        drop(permit);
        drop(slot);
        drop(held);
        if verdict == fault {
            self.end_run_alone(&run, &results, verdict).await?;
            return Ok(IntegrationChecks::FoldFailed(run.id));
        }
        if verdict.state == RunState::Passed {
            *carried = Some(PassedRun { run, results });
            return Ok(IntegrationChecks::Passed);
        }
        self.end_failed(actor, at, &run, &results, verdict).await?;
        Ok(IntegrationChecks::Failed(run.id))
    }

    /// Ends a failed run: a batch's alone, as its caller integrates the members one at a time; a proposal's with its send-back; a direct change's with a message to the main agent.
    async fn end_failed(
        &self,
        actor: &Actor,
        at: Checked<'_>,
        run: &CheckRunRow,
        results: &[CommandResult],
        verdict: Verdict,
    ) -> Result<(), ReviewError> {
        match at {
            Checked::Proposal(at) if at.batch.is_some() => {
                self.end_run_alone(run, results, verdict).await
            }
            Checked::Proposal(at) => {
                self.send_back_for_checks(actor, at, run, results, verdict)
                    .await
            }
            Checked::Direct(at) => {
                self.direct_checks_failed(actor, at, run, results, verdict)
                    .await
            }
        }
    }

    /// One write transaction: the failed run's results and end, and a message from fold to the main agent's worker that names the changeset. Nothing is published to main and the main workspace stays open, so the main agent fixes the change there, records it again and submits the new changeset.
    async fn direct_checks_failed(
        &self,
        actor: &Actor,
        at: DirectIntegrating,
        run: &CheckRunRow,
        results: &[CommandResult],
        verdict: Verdict,
    ) -> Result<(), ReviewError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let ended = self
            .store
            .write(async |tx| {
                if !end_run_in_tx(tx, &mut buffer, run, results, verdict, now).await? {
                    return Ok(false);
                }
                if let Actor::Agent(agent) = actor {
                    let body =
                        MessageBody::parse(direct_failed_message(at, run, results, verdict))?;
                    let recipients = vec![Recipient {
                        target: Target::Worker(agent.worker.clone()),
                        addressed_as: AddressedAs::Worker,
                    }];
                    send_from_fold_in_tx(tx, &mut buffer, recipients, (None, None), &body, now)
                        .await?;
                }
                Ok::<_, ReviewError>(true)
            })
            .await?;
        if ended {
            self.bus.publish(buffer);
            tracing::info!(run = %run.id, changeset = %at.changeset, reason = verdict.reason.map(RunReason::as_str), "integration checks of a direct change failed; main did not move");
        }
        Ok(())
    }

    /// The problem of main's own checks file in the tree `head_tree`, as its parse states it with the file and the line; `None` when main has no checks file or a valid one.
    async fn main_checks_problem(
        &self,
        head_tree: ObjectHash,
    ) -> Result<Option<String>, ReviewError> {
        let input = self.workspaces.checks_input(head_tree, head_tree).await?;
        Ok(input
            .file
            .and_then(|text| ChecksFile::parse(&text).err())
            .map(|error| error.message().to_owned()))
    }

    /// Records that integrations wait for main's checks file with `integration.held` on main's head `head`, once per head: the event tells the main agent and puts the problem in Needs you. Called under the integration mutex, so `head` is main's current head.
    async fn hold_for_main(&self, head: RevisionId, problem: &str) -> Result<(), ReviewError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let held = self
            .store
            .write(async |tx| {
                if checks::held_by_main(tx.conn()).await?.is_some() {
                    return Ok(false);
                }
                let event = NewEvent::new(
                    fold_core::events::types::INTEGRATION_HELD,
                    EventActor::fold(),
                )?
                .revision(head)
                .data(json!({"file": CHECKS_FILE, "problem": problem}))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, ReviewError>(true)
            })
            .await?;
        if held {
            self.bus.publish(buffer);
            tracing::info!(revision = %head, "integrations wait until the checks file on main is valid");
        }
        Ok(())
    }

    /// Ends a run in a write of its own, with nobody sent back: a run that fold itself could not complete, stopped as `fold_failed`, whose caller tells whoever asked for the integration, who asks again once the fault is fixed, or the failed run of a batch, whose caller integrates the members one at a time.
    async fn end_run_alone(
        &self,
        run: &CheckRunRow,
        results: &[CommandResult],
        verdict: Verdict,
    ) -> Result<(), ReviewError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let ended = self
            .store
            .write(async |tx| end_run_in_tx(tx, &mut buffer, run, results, verdict, now).await)
            .await?;
        if ended {
            self.bus.publish(buffer);
        }
        Ok(())
    }

    /// Ends a passed run that no attempt took into its transaction as `superseded`: the integration started again on a newer state, or ended without publishing.
    pub(crate) async fn end_carried(
        &self,
        carried: &mut Option<PassedRun>,
    ) -> Result<(), ReviewError> {
        let Some(passed) = carried.take() else {
            return Ok(());
        };
        let now = self.clock.now();
        let verdict = Verdict {
            state: RunState::Stopped,
            reason: Some(RunReason::Superseded),
        };
        let mut buffer = EventBuffer::new();
        let ended = self
            .store
            .write(async |tx| {
                end_run_in_tx(tx, &mut buffer, &passed.run, &passed.results, verdict, now).await
            })
            .await?;
        if ended {
            self.bus.publish(buffer);
        }
        Ok(())
    }

    async fn start_integration_run(
        &self,
        actor: &Actor,
        at: Checked<'_>,
    ) -> Result<CheckRunRow, ReviewError> {
        let now = self.clock.now();
        let label = actor.event_actor();
        let (purpose, task, proposal, version, batch, changeset) = match at {
            Checked::Proposal(at) => (
                Purpose::Integrate,
                Some(at.task),
                Some(at.proposal),
                Some(at.version),
                at.batch,
                None,
            ),
            Checked::Direct(at) => (
                Purpose::IntegrateDirect,
                None,
                None,
                None,
                None,
                Some(at.changeset),
            ),
        };
        let mut buffer = EventBuffer::new();
        let run = self
            .store
            .write(async |tx| {
                let number = counters::allocate(tx, IdKind::CheckRun).await?;
                let id =
                    CheckRunId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
                let new = NewCheckRun {
                    id,
                    stage: Stage::Integration,
                    purpose,
                    task,
                    workspace: None,
                    worker_id: None,
                    proposal,
                    version,
                    base: at.head(),
                    tree: at.merged(),
                    actor: label.as_str(),
                    title: None,
                    evidence: None,
                    intent: None,
                    batch,
                    changeset,
                };
                checks::insert_run(tx, &new, now).await?;
                if let Checked::Proposal(at) = at
                    && at.batch.is_some()
                {
                    checks::insert_run_members(tx, id, at.members).await?;
                }
                let mut data = json!({
                    "run": id.to_string(),
                    "stage": Stage::Integration.as_str(),
                    "purpose": purpose.as_str(),
                    "version": version,
                });
                if let Some(batch) = batch {
                    data["batch"] = json!(batch.to_string());
                }
                let mut event =
                    NewEvent::new(fold_core::events::types::CHECKS_STARTED, label.clone())?
                        .task(task)
                        .revision(at.head());
                if let Some(proposal) = proposal {
                    event = event.proposal(proposal);
                }
                if let Checked::Direct(at) = at {
                    data["changeset"] = json!(at.changeset.to_string());
                    event = event.workspace(at.workspace);
                }
                let event = event.data(data)?;
                buffer.append(tx, &event, now).await?;
                checks::get_run(tx.conn(), id)
                    .await?
                    .ok_or(StoreError::Corrupt("check_runs.number"))
                    .map_err(ReviewError::from)
            })
            .await?;
        self.bus.publish(buffer);
        tracing::info!(run = %run.id, subject = %run.subject(), "integration check run started");
        Ok(run)
    }

    /// One write transaction: the failed run's results and end, the proposal's failed-checks flag, which sends it back to its author until a new version, a message from fold to the author's worker and to the integrating agent, and the recalculated status. Nothing is published to main.
    async fn send_back_for_checks(
        &self,
        actor: &Actor,
        at: Integrating<'_>,
        run: &CheckRunRow,
        results: &[CommandResult],
        verdict: Verdict,
    ) -> Result<(), ReviewError> {
        if let Some(lane) = self.moved_lane(at.proposal, at.head_tree).await? {
            return self
                .lane_checks_failed(actor, at, run, results, verdict, &lane)
                .await;
        }
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let ended = self
            .store
            .write(async |tx| {
                if !end_run_in_tx(tx, &mut buffer, run, results, verdict, now).await? {
                    return Ok(false);
                }
                reviews::mark_checks_failed(tx, at.proposal, now).await?;
                let author = reviews::get_proposal(tx.conn(), at.proposal)
                    .await?
                    .ok_or(StoreError::Corrupt("proposals.number"))?
                    .author;
                let mut to = BTreeSet::from([author]);
                if let Actor::Agent(agent) = actor {
                    to.insert(agent.worker.clone());
                }
                let body = send_back_message(at, run, results, verdict);
                tell(tx, &mut buffer, &to, at, &body, now).await?;
                buffer.extend(recompute(tx, &[at.task], now).await?.events);
                Ok::<_, ReviewError>(true)
            })
            .await?;
        if ended {
            self.bus.publish(buffer);
            tracing::info!(run = %run.id, proposal = %at.proposal, reason = verdict.reason.map(RunReason::as_str), "integration checks failed; the proposal went back to its author");
        }
        Ok(())
    }

    /// [`Self::send_back_for_checks`] for a member of `lane` that main moved in since its version's base: the failure is the lane's move, not the author's work, so it goes the way of a conflict. One write transaction ends the run with `checks.finished`, whose `lane` names the lane and `fixer` the reviewer agent that approved the version and holds the review claim, flags the proposal as conflicted, so the reviewer's checkout holds the version merged onto main and the task stays in Review, tells the integrating agent, and, without such a reviewer, tells the review lead or for a task of main the main agent to have the failure fixed. The author gets no notice. The worker supervisor gives the failure to the named reviewer.
    async fn lane_checks_failed(
        &self,
        actor: &Actor,
        at: Integrating<'_>,
        run: &CheckRunRow,
        results: &[CommandResult],
        verdict: Verdict,
        lane: &str,
    ) -> Result<(), ReviewError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let ended = self
            .store
            .write(async |tx| {
                let fixer = conflict_fixer(tx, actor, at.proposal, at.version).await?;
                let fixer_name = match &fixer {
                    Fixer::Reviewer(agent) => Some(agent.as_str().to_owned()),
                    Fixer::ReviewLead(_) => None,
                };
                let extra = json!({"lane": lane, "fixer": fixer_name});
                if !end_run_with_in_tx(tx, &mut buffer, run, results, verdict, &extra, now).await? {
                    return Ok(false);
                }
                reviews::mark_conflicted(tx, at.proposal, now).await?;
                if let Actor::Agent(agent) = actor {
                    let body = lane_send_back_message(at, run, results, verdict, lane);
                    tell(
                        tx,
                        &mut buffer,
                        &BTreeSet::from([agent.worker.clone()]),
                        at,
                        &body,
                        now,
                    )
                    .await?;
                }
                if let Fixer::ReviewLead(why) = &fixer {
                    let handoff = Handoff::LaneChecks {
                        proposal: at.proposal,
                        version: at.version,
                        run: run.id,
                        main: at.head,
                    };
                    handoff_notice_in_tx(tx, &mut buffer, &handoff, at.task, why, Some(lane), now)
                        .await?;
                }
                buffer.extend(recompute(tx, &[at.task], now).await?.events);
                Ok::<_, ReviewError>(true)
            })
            .await?;
        if ended {
            self.bus.publish(buffer);
            tracing::info!(run = %run.id, proposal = %at.proposal, reason = verdict.reason.map(RunReason::as_str), "integration checks of a lane member failed after its lane moved; the proposal went to its reviewer");
        }
        Ok(())
    }

    /// Whether integrating `proposal` now can run integration checks: the checks file of its current version's tree or of main names an integration check. Read before the integration mutex, to decide whether the integration waits in the queue in the background.
    pub(crate) async fn integration_checks_may_run(
        &self,
        proposal: ProposalId,
    ) -> Result<bool, ReviewError> {
        let Some(target) = reviews::review_target(self.store.read(), proposal).await? else {
            return Ok(false);
        };
        let head = fold_store::workspaces::head(self.store.read())
            .await?
            .ok_or(StoreError::Corrupt("refs.main"))?;
        for tree in [target.tree, head.tree_hash] {
            let input = self.workspaces.checks_input(tree, tree).await?;
            let Some(text) = input.file else {
                continue;
            };
            match ChecksFile::parse(&text) {
                Ok(file) if file.has_stage(Stage::Integration) => return Ok(true),
                Ok(_) => {}
                // An invalid file fails at integration, where its error is reported.
                Err(_) => return Ok(true),
            }
        }
        Ok(false)
    }

    /// Brings fold's check directory to main's tree `tree` in the background after a new revision, when the tree names an integration check, so the next integration's setup finds the dependencies of main installed.
    pub(crate) fn follow_main_in_check_dir(&self, tree: ObjectHash) {
        let service = self.clone();
        self.checks.spawn(async move {
            let has_checks = match service.workspaces.checks_input(tree, tree).await {
                Ok(input) => input
                    .file
                    .and_then(|text| ChecksFile::parse(&text).ok())
                    .is_some_and(|file| file.has_stage(Stage::Integration)),
                Err(_) => false,
            };
            if !has_checks {
                return;
            }
            let lock = service.checks.check_dir();
            let _held = lock.lock().await;
            let work = service.checks.dir_work().await;
            if let Err(error) = service.workspaces.check_dir_to(tree, work).await {
                tracing::warn!(
                    code = error.log_code(),
                    "the check directory could not follow main"
                );
            }
        });
    }
}

/// The answer to an integration whose checks fold itself could not run: nothing was published and the version keeps its place, so the caller asks again once the fault is fixed.
pub(crate) fn fold_failed_refusal(at: Integrating<'_>, run: CheckRunId) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_IO,
        format!(
            "fold could not run the integration checks of {} v{} ({run}) on its merge with main {}, so it did not integrate it",
            at.proposal, at.version, at.head
        ),
    )
    .with_details([
        ("reason", json!(RunReason::FoldFailed.as_str())),
        ("proposal", json!(at.proposal.to_string())),
        ("version", json!(at.version)),
        ("run", json!(run.to_string())),
    ])
    .with_hint("the fault is fold's, not the proposal's: integrate again once it is fixed; the daemon log names its code")
}

/// [`fold_failed_refusal`] for a direct change: nothing was published and the main workspace stays open.
pub(crate) fn direct_fold_failed_refusal(at: DirectIntegrating, run: CheckRunId) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_IO,
        format!(
            "fold could not run the integration checks of the direct change {} ({run}) on its merge with main {}, so it did not integrate it",
            at.changeset, at.head
        ),
    )
    .with_details([
        ("reason", json!(RunReason::FoldFailed.as_str())),
        ("changeset", json!(at.changeset.to_string())),
        ("run", json!(run.to_string())),
    ])
    .with_hint("the fault is fold's, not the change's: call submit_main_change again once it is fixed; the daemon log names its code")
}

/// Sends `body` from fold to each worker of `to`, linked to the task and proposal.
async fn tell(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    to: &BTreeSet<WorkerName>,
    at: Integrating<'_>,
    body: &str,
    now: fold_core::Timestamp,
) -> Result<(), ReviewError> {
    let body = MessageBody::parse(body)?;
    let recipients = to
        .iter()
        .map(|worker| Recipient {
            target: Target::Worker(worker.clone()),
            addressed_as: AddressedAs::Worker,
        })
        .collect();
    send_from_fold_in_tx(
        tx,
        buffer,
        recipients,
        (at.task, Some(at.proposal)),
        &body,
        now,
    )
    .await?;
    Ok(())
}

/// The message of failed integration checks: what failed on which merge, and the end of the first failed log.
fn send_back_message(
    at: Integrating<'_>,
    run: &CheckRunRow,
    results: &[CommandResult],
    verdict: Verdict,
) -> String {
    let subject = format!(
        "The integration checks of {} v{} ({}) failed on its merge with main {}",
        at.proposal, at.version, run.id, at.head
    );
    let mut text = format!(
        "{subject}: {}. fold did not integrate it, and the proposal is back with its author: fix the work in the task workspace, sync it with main, and call update_proposal.",
        failure_text(results, verdict)
    );
    push_log_and_run(&mut text, run, results);
    text
}

/// What failed in a run, in a few words.
fn failure_text(results: &[CommandResult], verdict: Verdict) -> String {
    match verdict.reason {
        Some(RunReason::FilesChanged) => {
            "a check changed tracked files of the merged tree; put what it writes in .gitignore"
                .to_owned()
        }
        Some(RunReason::SetupFailed) => "the setup failed, so no check ran".to_owned(),
        Some(RunReason::ChecksFileInvalid) => {
            "the merged tree's fold-checks.toml is not valid".to_owned()
        }
        Some(RunReason::Killed) => {
            "a check was killed: out of memory or by a signal, not a failure of the change"
                .to_owned()
        }
        Some(RunReason::NoBuildSlot) => {
            "a check found no free build slot on the machine in time, not a failure of the change"
                .to_owned()
        }
        _ => results
            .iter()
            .filter(|result| result.blocks())
            .map(|result| {
                let name = result.check.as_deref().unwrap_or("setup");
                match result.exit_code {
                    Some(code) => format!("{name} failed (exit {code})"),
                    None => format!("{name} failed"),
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// Appends the end of the first failed log and the run to read for every result.
fn push_log_and_run(text: &mut String, run: &CheckRunRow, results: &[CommandResult]) {
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
}

/// The message to the main agent of failed integration checks of its direct change: the same facts as [`send_back_message`], and what to do in the main workspace.
fn direct_failed_message(
    at: DirectIntegrating,
    run: &CheckRunRow,
    results: &[CommandResult],
    verdict: Verdict,
) -> String {
    let mut text = format!(
        "The integration checks of the direct change {} ({}) failed on its merge with main {}: {}. fold did not integrate it, and {} stays open: fix the change there, call record_changes and submit_main_change with the new changeset.",
        at.changeset,
        run.id,
        at.head,
        failure_text(results, verdict),
        at.workspace
    );
    push_log_and_run(&mut text, run, results);
    text
}

/// The message to the integrating agent of failed integration checks of a lane member after its lane moved: the same facts as [`send_back_message`], but the version goes to its reviewer.
fn lane_send_back_message(
    at: Integrating<'_>,
    run: &CheckRunRow,
    results: &[CommandResult],
    verdict: Verdict,
    lane: &str,
) -> String {
    send_back_message(at, run, results, verdict).replacen(
        "fold did not integrate it, and the proposal is back with its author: fix the work in the task workspace, sync it with main, and call update_proposal.",
        &format!(
            "fold did not integrate it. {} The reviewer that approved it fixes it on the new main, not the author, and fold integrates it in its turn in the lane.",
            lane_moved_note(lane)
        ),
        1,
    )
}

#[cfg(test)]
mod tests {
    use fold_core::Timestamp;

    use super::*;

    #[test]
    fn the_send_back_names_the_merge_and_the_failed_check() {
        let at = Integrating {
            proposal: ProposalId::from_number(3).unwrap(),
            task: TaskId::from_number(12).unwrap(),
            version: 2,
            head: RevisionId::from_number(5).unwrap(),
            head_tree: ObjectHash::from_bytes([1; 32]),
            merged: ObjectHash::from_bytes([2; 32]),
            batch: None,
            members: &[],
        };
        let run = CheckRunRow {
            id: CheckRunId::from_number(9).unwrap(),
            stage: Stage::Integration,
            purpose: Purpose::Integrate,
            task: Some(at.task),
            workspace: None,
            worker: None,
            proposal: Some(at.proposal),
            version: Some(2),
            base: at.head,
            tree: at.merged,
            actor: "fold".to_owned(),
            title: None,
            evidence: None,
            intent: None,
            state: RunState::Running,
            reason: None,
            created_at: Timestamp::from_millis(0).unwrap(),
            finished_at: None,
            batch: None,
            changeset: None,
        };
        let results = [CommandResult {
            check: Some("e2e".to_owned()),
            required: true,
            outcome: Outcome::Failed,
            exit_code: Some(1),
            duration_ms: 3,
            log: b"step 1\nboom\n".to_vec(),
            log_bytes: 12,
            fault: false,
            killed: None,
            no_build_slot: false,
        }];
        let text = send_back_message(at, &run, &results, Verdict::failed(RunReason::CheckFailed));
        assert!(text.starts_with(
            "The integration checks of P-3 v2 (cr-9) failed on its merge with main r5: e2e failed (exit 1)."
        ));
        assert!(text.contains("call update_proposal"));
        assert!(text.contains("The end of the log of e2e:\nstep 1\nboom"));
        let changed = send_back_message(at, &run, &[], Verdict::failed(RunReason::FilesChanged));
        assert!(changed.contains("changed tracked files"));
        let no_slot = send_back_message(at, &run, &[], Verdict::failed(RunReason::NoBuildSlot));
        assert!(no_slot.contains("found no free build slot on the machine in time"));
        let lane = lane_send_back_message(
            at,
            &run,
            &results,
            Verdict::failed(RunReason::CheckFailed),
            "migrations",
        );
        assert!(
            lane.contains("e2e failed (exit 1). fold did not integrate it. Lane migrations moved")
        );
        assert!(
            lane.contains("The reviewer that approved it fixes it on the new main, not the author")
        );
        assert!(!lane.contains("update_proposal"));
        assert!(lane.contains("The end of the log of e2e:"));
    }
}
