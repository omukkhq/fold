use std::collections::BTreeSet;

use fold_core::object::ObjectHash;
use fold_core::reviews::checks::{
    CHECKS_FILE, ChecksFile, Outcome, Purpose, RunReason, RunState, Stage, runs_any,
};
use fold_core::{
    Actor, AddressedAs, CheckRunId, EventActor, FoldError, IdKind, MessageBody, NewEvent,
    ProposalId, Recipient, ReviewBatchId, RevisionId, Target, TaskId, WorkerName, codes,
};
use fold_store::reviews::checks::{self, CheckRunRow, NewCheckRun};
use fold_store::reviews::{self};
use fold_store::{StoreError, WriteTx, counters};
use serde_json::json;

use super::runner::{self, CommandResult};
use super::{Verdict, end_run_in_tx, log_tail};
use crate::services::events::EventBuffer;
use crate::services::messages::send_from_fold_in_tx;
use crate::services::reviews::{ReviewError, ReviewService};
use crate::services::tasks::recompute;

/// The most lines of a failed integration check's log a result message carries.
const TAIL_LINES: usize = 30;

/// The most characters of a failed integration check's log a result message carries.
const TAIL_CHARS: usize = 3000;

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
    /// The run failed and the proposal went back to its author.
    Failed(CheckRunId),
    /// fold itself could not run the checks: the run stopped as `fold_failed`, and the proposal keeps its version, its approvals and its place with the reviewer.
    FoldFailed(CheckRunId),
    /// The checks file of main itself is not valid, so the fault is not the proposal's: no run starts, nothing is sent back, and the integration waits until main's file is fixed.
    Held,
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
    /// Runs the integration checks of the merged tree `merged` in fold's check directory, under the integration mutex the caller holds, so main cannot move meanwhile. The checks that run are the integration-stage checks of the merged tree's own `fold-checks.toml` whose `files` pattern matches a path the merge changes on main. A passed run of an earlier attempt on the same version, head and tree is reused from `carried`; one on another state ends as `superseded`. A passed run is left in `carried` for the caller's transaction to end; a failed one ends here, with the proposal sent back to its author.
    pub(crate) async fn integration_checks(
        &self,
        actor: &Actor,
        at: Integrating<'_>,
        carried: &mut Option<PassedRun>,
    ) -> Result<IntegrationChecks, ReviewError> {
        let input = self
            .workspaces
            .checks_input(at.head_tree, at.merged)
            .await?;
        let Some(text) = input.file else {
            self.end_carried(carried).await?;
            return Ok(IntegrationChecks::None);
        };
        let file = match ChecksFile::parse(&text) {
            Ok(file) => file,
            Err(error) => {
                self.end_carried(carried).await?;
                if let Some(problem) = self.main_checks_problem(at.head_tree).await? {
                    self.hold_for_main(at.head, &problem).await?;
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
                };
                let verdict = Verdict::failed(RunReason::ChecksFileInvalid);
                if at.batch.is_some() {
                    self.end_run_alone(&run, &[invalid], verdict).await?;
                } else {
                    self.send_back_for_checks(actor, at, &run, &[invalid], verdict)
                        .await?;
                }
                return Ok(IntegrationChecks::Failed(run.id));
            }
        };
        let plan = file.plan(Stage::Integration, &input.changed);
        if !runs_any(&plan) {
            self.end_carried(carried).await?;
            return Ok(IntegrationChecks::None);
        }
        if let Some(passed) = carried.as_ref() {
            let run = &passed.run;
            if run.proposal == Some(at.proposal)
                && run.version == Some(at.version)
                && run.base == at.head
                && run.tree == at.merged
            {
                return Ok(IntegrationChecks::Passed);
            }
        }
        self.end_carried(carried).await?;
        let run = self.start_integration_run(actor, at).await?;
        let specs = runner::commands(&file, &plan);
        let lock = self.checks.check_dir();
        let held = lock.lock().await;
        let permit = self.checks.permits().acquire().await;
        let fault = Verdict {
            state: RunState::Stopped,
            reason: Some(RunReason::FoldFailed),
        };
        let (results, verdict) = match self.workspaces.check_dir_to(at.merged).await {
            Ok(dir) => {
                let results =
                    runner::run_commands(&dir, specs, self.checks.limits(), &*self.clock).await;
                let verdict = if results.iter().any(|result| result.fault) {
                    tracing::warn!(run = %run.id, "a command of the integration checks could not start or be waited for");
                    fault
                } else {
                    match self.workspaces.check_dir_holds(at.merged).await {
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
        drop(held);
        if verdict == fault {
            self.end_run_alone(&run, &results, verdict).await?;
            return Ok(IntegrationChecks::FoldFailed(run.id));
        }
        if verdict.state == RunState::Passed {
            *carried = Some(PassedRun { run, results });
            return Ok(IntegrationChecks::Passed);
        }
        if at.batch.is_some() {
            self.end_run_alone(&run, &results, verdict).await?;
        } else {
            self.send_back_for_checks(actor, at, &run, &results, verdict)
                .await?;
        }
        Ok(IntegrationChecks::Failed(run.id))
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
        at: Integrating<'_>,
    ) -> Result<CheckRunRow, ReviewError> {
        let now = self.clock.now();
        let label = actor.event_actor();
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
                    purpose: Purpose::Integrate,
                    task: at.task,
                    workspace: None,
                    worker_id: None,
                    proposal: Some(at.proposal),
                    version: Some(at.version),
                    base: at.head,
                    tree: at.merged,
                    actor: label.as_str(),
                    title: None,
                    evidence: None,
                    batch: at.batch,
                };
                checks::insert_run(tx, &new, now).await?;
                if at.batch.is_some() {
                    checks::insert_run_members(tx, id, at.members).await?;
                }
                let mut data = json!({
                    "run": id.to_string(),
                    "stage": Stage::Integration.as_str(),
                    "purpose": Purpose::Integrate.as_str(),
                    "version": at.version,
                });
                if let Some(batch) = at.batch {
                    data["batch"] = json!(batch.to_string());
                }
                let event = NewEvent::new(fold_core::events::types::CHECKS_STARTED, label.clone())?
                    .task(at.task)
                    .proposal(at.proposal)
                    .revision(at.head)
                    .data(data)?;
                buffer.append(tx, &event, now).await?;
                checks::get_run(tx.conn(), id)
                    .await?
                    .ok_or(StoreError::Corrupt("check_runs.number"))
                    .map_err(ReviewError::from)
            })
            .await?;
        self.bus.publish(buffer);
        tracing::info!(run = %run.id, proposal = %at.proposal, version = at.version, "integration check run started");
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
            if let Err(error) = service.workspaces.check_dir_to(tree).await {
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
    let what = match verdict.reason {
        Some(RunReason::FilesChanged) => {
            "a check changed tracked files of the merged tree; put what it writes in .gitignore"
                .to_owned()
        }
        Some(RunReason::SetupFailed) => "the setup failed, so no check ran".to_owned(),
        Some(RunReason::ChecksFileInvalid) => {
            "the merged tree's fold-checks.toml is not valid".to_owned()
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
    };
    let mut text = format!(
        "{subject}: {what}. fold did not integrate it, and the proposal is back with its author: fix the work in the task workspace, sync it with main, and call update_proposal."
    );
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
            task: at.task,
            workspace: None,
            worker: None,
            proposal: Some(at.proposal),
            version: Some(2),
            base: at.head,
            tree: at.merged,
            actor: "fold".to_owned(),
            title: None,
            evidence: None,
            state: RunState::Running,
            reason: None,
            created_at: Timestamp::from_millis(0).unwrap(),
            finished_at: None,
            batch: None,
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
        }];
        let text = send_back_message(at, &run, &results, Verdict::failed(RunReason::CheckFailed));
        assert!(text.starts_with(
            "The integration checks of P-3 v2 (cr-9) failed on its merge with main r5: e2e failed (exit 1)."
        ));
        assert!(text.contains("call update_proposal"));
        assert!(text.contains("The end of the log of e2e:\nstep 1\nboom"));
        let changed = send_back_message(at, &run, &[], Verdict::failed(RunReason::FilesChanged));
        assert!(changed.contains("changed tracked files"));
    }
}
