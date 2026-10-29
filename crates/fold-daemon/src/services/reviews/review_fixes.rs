use std::collections::BTreeSet;

use fold_core::events::types;
use fold_core::object::ObjectHash;
use fold_core::policy::AgentStanding;
use fold_core::reviews::MAX_REASON_CHARS;
use fold_core::reviews::checks::{Purpose, RunReason, RunState};
use fold_core::{
    Actor, AgentActor, ChangesetId, CheckRunId, CheckpointId, FoldError, NewEvent, ProposalId,
    RevisionId, TaskId, WorkspaceId, codes, policy,
};
use fold_store::agents;
use fold_store::reviews::checks::CheckRunRow;
use fold_store::reviews::{self, NewVersion, ProposalRow, ReviewClaimRow};
use fold_store::workspaces::{self, CheckpointReason, WorkspaceRow};
use fold_store::{StoreError, WriteTx};
use serde_json::json;

use super::checks::{
    FixJob, GateDecision, GateJob, NewProposalRun, Verdict, Waiting, end_run_in_tx,
    insert_proposal_run, is_refusal, run_commands_for, tell_requester,
};
use super::human_edits::{busy, current_version, idle_checks};
use super::proposals::{ProposalDetail, mark_checks_change, require_open, usable_workspace};
use super::verdicts::require_version;
use super::{ReviewError, ReviewService};
use crate::services::agents::{actor_worker, worker_not_found};
use crate::services::events::EventBuffer;
use crate::services::tasks::{recompute, worker_row};
use crate::services::workspaces::{ReviewFixTree, checkpoint_in_tx, file_path};

/// The most files one fix names.
pub const MAX_FIXES: usize = 50;

/// The longest text of the checks a fix names, in characters.
pub const MAX_CHECKS_CHARS: usize = 20_000;

/// How many paths a `fixes_mismatch` refusal lists on each side; the counts give the rest.
const MAX_LISTED: usize = 20;

/// One file a reviewer changed in its review checkout, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFix {
    pub path: String,
    pub reason: String,
}

/// What a reviewer's fix wrote: the new version, the version it fixed, the changeset and the pending checkpoint in the task workspace, and the checkout that now holds the new version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedVersion {
    pub version: u32,
    pub from_version: u32,
    pub changeset: ChangesetId,
    pub checkpoint: CheckpointId,
    pub checkout: WorkspaceId,
    pub detail: ProposalDetail,
}

impl ReviewService {
    /// Makes the files of the actor worker's active review checkout of `version` the next version of an open proposal, authored by the reviewer, when the reviewer fixed what it found there (`policy::can_fix_in_review`: only the agent that holds the review claim). `version` is the version the checkout holds and the reviewer fixed; another current version is `stale_version`. `fixes` names every file that differs from `version`, each once with its reason, and nothing else (`invalid_request` with the reason `fixes_mismatch`); `checks` gives the commands the reviewer ran on the fixed files and their results. Both make the new version's evidence.
    ///
    /// The new version keeps the base of `version`, so a main that moved meanwhile is merged at integration as for any version, and it starts without approvals: the reviewer approves it with `submit_review` after its checks pass, and the review claim stays with the reviewer. The fix is recorded in the task workspace as a changeset and as a `review_fix` checkpoint marked as the workspace's pending write, without touching its files: fold writes into a workspace only on create, sync and restore. The worker's next sync writes the fix into the files first, and until then `record_changes` and `update_proposal` refuse with `workspace_write_unfinished`; the author's agent gets a push notice to sync. The checkout then holds the new version, so the reviewer builds, tests and approves it there, or fixes it again. A batch checkout mixes every member's work and is never one member's version, so a fix of a member is made in that member's own checkout.
    ///
    /// The fix is refused with `worker_busy` by the rules of a human edit: the task worker may be changing its workspace, or it holds work the version does not.
    pub async fn submit_review_fix(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        version: u32,
        fixes: &[ReviewFix],
        checks: &str,
    ) -> Result<FixOutcome, ReviewError> {
        let fixes = check_fixes(fixes)?;
        let checks = check_checks(checks)?;
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let row = require_open(reviews::get_proposal(&mut *conn, proposal).await?, proposal)?;
        let holder = reviews::unreleased_review_claim(&mut *conn, proposal).await?;
        let standing = agents::standing_of(&mut *conn, actor).await?;
        let reviewer = fix_gate(actor, standing, &row, holder.as_ref())?;
        let current = current_version(&mut conn, proposal).await?;
        require_version(proposal, version, current.version)?;
        let worker = actor_worker(&mut conn, reviewer)
            .await?
            .ok_or_else(worker_not_found)?;
        let checkout_id =
            workspaces::active_review_checkout(&mut *conn, proposal, version, worker.id)
                .await?
                .ok_or_else(|| no_checkout(proposal, version))?;
        let checkout = workspaces::get_workspace(&mut *conn, checkout_id)
            .await?
            .ok_or(StoreError::Corrupt("workspaces.number"))?;
        let live = workspaces::live_task_workspace(&mut *conn, row.task).await?;
        let workspace = usable_workspace(live, row.task, &row.author)?;
        idle_checks(&mut conn, &row, &workspace, &current).await?;
        drop(conn);
        let fixed = self
            .workspaces
            .snapshot_review_fix(workspace.id, &checkout, current.base, current.tree)
            .await?;
        if fixed.tree == current.tree {
            return Err(FoldError::new(
                codes::VERSION_UNCHANGED,
                format!(
                    "the checkout holds v{} of {proposal} unchanged",
                    current.version
                ),
            )
            .with_details([
                ("proposal", json!(proposal.to_string())),
                ("version", json!(current.version)),
                ("checkout", json!(checkout_id.to_string())),
            ])
            .with_hint(
                "fix the files in the review checkout, or review the version with submit_review",
            )
            .into());
        }
        match_fixes(&fixes, &fixed.changed)?;
        let evidence = evidence(&worker.name, current.version, &fixes, &checks);
        let fix = FixJob {
            version,
            checkout: checkout_id,
            files: fixes.len(),
        };
        let checks_changed = match self.gate_in(checkout_id, current.base, fixed.tree).await? {
            GateDecision::Ready { checks_changed } => checks_changed,
            GateDecision::Run(mut job) => {
                let tree = fixed.tree;
                drop(fixed);
                job.fix = Some(fix);
                return self
                    .start_fix_run(actor, &row, &checkout, current.base, tree, &evidence, job)
                    .await;
            }
        };
        let input = FixInput {
            actor,
            proposal,
            workspace: workspace.id,
            fixed: &fixed,
            evidence: &evidence,
            fix,
            checks_changed,
        };
        let mut buffer = EventBuffer::new();
        let made = self
            .store
            .write(async |tx| fix_in_tx(tx, &mut buffer, &input).await)
            .await?;
        drop(fixed);
        self.bus.publish(buffer);
        self.workspaces
            .proposal_changed(workspace.id, Some(proposal))
            .await;
        let detail = self.get_proposal(proposal, None).await?;
        Ok(FixOutcome::Fixed(Box::new(FixedVersion {
            version: made.version,
            from_version: made.from_version,
            changeset: made.changeset,
            checkpoint: made.checkpoint,
            checkout: checkout_id,
            detail,
        })))
    }

    /// Writes the running `review_fix` run of the fixed `tree` in the reviewer's checkout and runs it in the background; the call answers at once.
    #[expect(
        clippy::too_many_arguments,
        reason = "the step of a command, with the inputs its earlier steps produced"
    )]
    async fn start_fix_run(
        &self,
        actor: &Actor,
        row: &ProposalRow,
        checkout: &WorkspaceRow,
        base: RevisionId,
        tree: ObjectHash,
        evidence: &str,
        job: GateJob,
    ) -> Result<FixOutcome, ReviewError> {
        let fix = job.fix.ok_or(StoreError::Corrupt("check_runs.purpose"))?;
        let proposal = row.id;
        let now = self.clock.now();
        let label = actor.event_actor();
        let mut buffer = EventBuffer::new();
        let run = self
            .store
            .write(async |tx| {
                let row =
                    require_open(reviews::get_proposal(tx.conn(), proposal).await?, proposal)?;
                let holder = reviews::unreleased_review_claim(tx.conn(), proposal).await?;
                let standing = agents::standing_of(tx.conn(), actor).await?;
                let reviewer = fix_gate(actor, standing, &row, holder.as_ref())?;
                let current = current_version(tx.conn(), proposal).await?;
                require_version(proposal, fix.version, current.version)?;
                let worker = worker_row(tx, reviewer).await?;
                let found =
                    workspaces::active_review_checkout(tx.conn(), proposal, fix.version, worker.id)
                        .await?;
                if found != Some(checkout.id) {
                    return Err(no_checkout(proposal, fix.version));
                }
                let new = NewProposalRun {
                    purpose: Purpose::ReviewFix,
                    task: row.task,
                    workspace: checkout.id,
                    worker_id: Some(worker.id),
                    base,
                    tree,
                    actor: &label,
                    waiting: Waiting {
                        proposal: Some(proposal),
                        title: None,
                        evidence: Some(evidence),
                    },
                };
                insert_proposal_run(tx, &mut buffer, &new, now).await
            })
            .await?;
        self.bus.publish(buffer);
        self.workspaces.checks_changed().await;
        tracing::info!(run = %run.id, proposal = %proposal, checkout = %checkout.id, "check run of a review fix started");
        let checking = FixOutcome::Checking {
            run: run.id,
            task: run.task,
            checkout: checkout.id,
        };
        self.spawn_proposal_run(run, job, actor.clone());
        Ok(checking)
    }

    /// The end of a `review_fix` run: the commands run in the reviewer's checkout, the checkout is recorded again, and a passed run whose checkout still holds the checked tree makes the fix version in the transaction that stores the result. A refusal by then, such as a review claim that was released, ends the run as stopped.
    pub(super) async fn complete_fix_run(
        &self,
        run: CheckRunRow,
        job: GateJob,
        actor: Actor,
    ) -> Result<(), ReviewError> {
        let fix = job.fix.ok_or(StoreError::Corrupt("check_runs.purpose"))?;
        let proposal = run
            .proposal
            .ok_or(StoreError::Corrupt("check_runs.proposal_id"))?;
        let checkout = workspaces::get_workspace(self.store.read(), fix.checkout)
            .await?
            .ok_or(StoreError::Corrupt("check_runs.workspace_id"))?;
        let permit = self.checks.permits().acquire().await;
        let dir = self.workspaces.workspace_dir(&checkout);
        let owner = self
            .checks
            .owner(run.worker.as_ref().map(fold_core::WorkerName::agent));
        let results = run_commands_for(
            &owner,
            &dir,
            job.specs.clone(),
            self.checks.limits(),
            &*self.clock,
        )
        .await;
        drop(permit);
        let mut verdict = Verdict::of(&results);
        let live = workspaces::live_task_workspace(self.store.read(), run.task).await?;
        let snapshot = match live {
            Some(live) => {
                let current = current_version(
                    &mut *self
                        .store
                        .read()
                        .acquire()
                        .await
                        .map_err(StoreError::from)?,
                    proposal,
                )
                .await?;
                match self
                    .workspaces
                    .snapshot_review_fix(live.id, &checkout, run.base, current.tree)
                    .await
                {
                    Ok(snapshot) => Some((live, snapshot)),
                    Err(error) => {
                        tracing::debug!(run = %run.id, code = error.log_code(), "the review checkout could not be recorded after its checks");
                        None
                    }
                }
            }
            None => None,
        };
        let Some((live, snapshot)) = snapshot else {
            verdict = Verdict::failed(RunReason::WorkspaceChanged);
            return self.end_proposal_run(&run, &results, verdict, None).await;
        };
        if snapshot.tree != run.tree {
            drop(snapshot);
            verdict = Verdict::failed(RunReason::FilesChanged);
            return self.end_proposal_run(&run, &results, verdict, None).await;
        }
        if verdict.state != RunState::Passed {
            drop(snapshot);
            return self.end_proposal_run(&run, &results, verdict, None).await;
        }
        let evidence = run.evidence.clone().unwrap_or_default();
        let input = FixInput {
            actor: &actor,
            proposal,
            workspace: live.id,
            fixed: &snapshot,
            evidence: &evidence,
            fix,
            checks_changed: job.checks_changed,
        };
        let now = snapshot.taken_at;
        let mut buffer = EventBuffer::new();
        let made = self
            .store
            .write(async |tx| {
                if !end_run_in_tx(tx, &mut buffer, &run, &results, verdict, now).await? {
                    return Ok(None);
                }
                let made = fix_in_tx(tx, &mut buffer, &input).await?;
                let body = format!(
                    "fold made {proposal} v{} for {}: the checks of your fix passed ({}).",
                    made.version, run.task, run.id
                );
                tell_requester(tx, &mut buffer, &run, &body, now).await?;
                Ok::<_, ReviewError>(Some(made))
            })
            .await;
        drop(snapshot);
        match made {
            Ok(Some(_)) => {
                self.bus.publish(buffer);
                self.workspaces
                    .proposal_changed(live.id, Some(proposal))
                    .await;
                self.workspaces.checks_changed().await;
                tracing::info!(run = %run.id, proposal = %proposal, "check run passed and made the review fix version");
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(error) if is_refusal(&error) => {
                verdict = Verdict {
                    state: RunState::Stopped,
                    reason: Some(RunReason::Refused),
                };
                self.end_proposal_run(&run, &results, verdict, Some(&error))
                    .await
            }
            Err(error) => Err(error),
        }
    }
}

/// What the result of a reviewer's fix is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixOutcome {
    /// The fix version exists.
    Fixed(Box<FixedVersion>),
    /// fold runs the proposal checks on the fix in the reviewer's checkout and makes the version only when they pass; the result comes as a message from fold.
    Checking {
        run: CheckRunId,
        task: TaskId,
        checkout: WorkspaceId,
    },
}

impl FixOutcome {
    /// The fix version, when it was made at once.
    pub fn fixed(self) -> Option<FixedVersion> {
        match self {
            FixOutcome::Fixed(fixed) => Some(*fixed),
            FixOutcome::Checking { .. } => None,
        }
    }
}

/// The inputs of the transaction that makes a reviewer's fix version.
struct FixInput<'a> {
    actor: &'a Actor,
    proposal: ProposalId,
    /// The task workspace the snapshot was taken against.
    workspace: WorkspaceId,
    fixed: &'a ReviewFixTree,
    evidence: &'a str,
    fix: FixJob,
    checks_changed: bool,
}

/// What a fix version wrote.
struct MadeFix {
    from_version: u32,
    version: u32,
    changeset: ChangesetId,
    checkpoint: CheckpointId,
}

/// The writes of a fix version in the caller's transaction: the refusals again, the changeset and the pending `review_fix` checkpoint in the task workspace, the version under the reviewer's name, the moved checkout, `proposal.reviewer_fixed`, the mark of a changed checks file and the recalculated status.
async fn fix_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    input: &FixInput<'_>,
) -> Result<MadeFix, ReviewError> {
    let (actor, proposal, version) = (input.actor, input.proposal, input.fix.version);
    let (fixed, checkout_id, now) = (input.fixed, input.fix.checkout, input.fixed.taken_at);
    let row = require_open(reviews::get_proposal(tx.conn(), proposal).await?, proposal)?;
    let holder = reviews::unreleased_review_claim(tx.conn(), proposal).await?;
    let standing = agents::standing_of(tx.conn(), actor).await?;
    let reviewer = fix_gate(actor, standing, &row, holder.as_ref())?;
    let current = current_version(tx.conn(), proposal).await?;
    require_version(proposal, version, current.version)?;
    let worker = worker_row(tx, reviewer).await?;
    let found = workspaces::active_review_checkout(tx.conn(), proposal, version, worker.id).await?;
    if found != Some(checkout_id) {
        return Err(no_checkout(proposal, version));
    }
    let live = workspaces::live_task_workspace(tx.conn(), row.task).await?;
    let live = usable_workspace(live, row.task, &row.author)?;
    if live.id != input.workspace {
        return Err(busy(&live, "newer_work"));
    }
    idle_checks(tx.conn(), &row, &live, &current).await?;
    let by = actor.event_actor();
    let message = format!("Review fix of v{} by {by}", current.version);
    let changeset = workspaces::insert_changeset(
        tx,
        live.id,
        live.task,
        live.base,
        fixed.tree,
        &message,
        fixed.stats,
        by.as_str(),
        now,
    )
    .await?;
    let stats = fixed.stats;
    let files = stats
        .files_added
        .saturating_add(stats.files_modified)
        .saturating_add(stats.files_deleted);
    let recorded = NewEvent::new(types::CHANGESET_RECORDED, by.clone())?
        .task(live.task)
        .workspace(live.id)
        .revision(live.base)
        .data(json!({"changeset": changeset.id.to_string(), "files": files}))?;
    buffer.append(tx, &recorded, now).await?;
    let next = current.version.saturating_add(1);
    let label = format!("v{next} of {proposal} by {by}");
    let checkpoint = checkpoint_in_tx(
        tx,
        buffer,
        actor,
        &live,
        fixed.tree,
        CheckpointReason::ReviewFix,
        Some(&label),
        now,
    )
    .await?;
    workspaces::set_sync_checkpoint(tx, live.id, Some(checkpoint.id)).await?;
    let new = NewVersion {
        base: current.base,
        tree: fixed.tree,
        head_changeset: changeset.id,
        evidence: Some(input.evidence),
        created_by: worker.name.as_str(),
        fixed_in_checkout: Some(checkout_id),
    };
    reviews::add_version(tx, proposal, next, &new, now).await?;
    if !workspaces::move_review_checkout(tx, checkout_id, proposal, next).await? {
        return Err(StoreError::Corrupt("workspaces.version_id").into());
    }
    let event = NewEvent::new(types::PROPOSAL_REVIEWER_FIXED, by)?
        .task(row.task)
        .proposal(proposal)
        .workspace(live.id)
        .revision(current.base)
        .data(json!({
            "version": next,
            "from_version": current.version,
            "changeset": changeset.id.to_string(),
            "checkpoint": checkpoint.id.to_string(),
            "checkout": checkout_id.to_string(),
            "files": input.fix.files,
        }))?;
    buffer.append(tx, &event, now).await?;
    mark_checks_change(tx, buffer, row.task, input.checks_changed, now).await?;
    buffer.extend(recompute(tx, &[row.task], now).await?.events);
    Ok(MadeFix {
        from_version: current.version,
        version: next,
        changeset: changeset.id,
        checkpoint: checkpoint.id,
    })
}

fn fix_gate<'a>(
    actor: &'a Actor,
    standing: AgentStanding,
    row: &ProposalRow,
    holder: Option<&ReviewClaimRow>,
) -> Result<&'a AgentActor, ReviewError> {
    let holder = holder.map(|claim| &claim.worker);
    policy::can_fix_in_review(actor, standing, holder, &row.author).map_err(|denied| {
        FoldError::from(denied)
            .with_details([
                ("proposal", json!(row.id.to_string())),
                ("holder", json!(holder.map(|name| name.as_str()))),
            ])
            .into()
    })
}

/// The fixes with each path in its normal form, in the given order: 1 to [`MAX_FIXES`] files, each named once, each with a one-line reason.
fn check_fixes(fixes: &[ReviewFix]) -> Result<Vec<ReviewFix>, ReviewError> {
    if fixes.is_empty() || fixes.len() > MAX_FIXES {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("fixes must name 1 to {MAX_FIXES} files"),
        )
        .with_details([("field", json!("fixes"))])
        .with_hint("name each file you changed in the review checkout with the reason for the fix")
        .into());
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(fixes.len());
    for (index, fix) in fixes.iter().enumerate() {
        let path = file_path(&fix.path)?;
        let path = String::from_utf8_lossy(path.as_bytes()).into_owned();
        if !seen.insert(path.clone()) {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("fixes[{index}] names {path} a second time"),
            )
            .with_details([("field", json!(format!("fixes[{index}].path")))])
            .with_hint("name each file once and give all its reasons in one line")
            .into());
        }
        let reason = fold_core::text::one_line(&fix.reason, MAX_REASON_CHARS).map_err(|_| {
            FoldError::new(
                codes::INVALID_REQUEST,
                format!("fixes[{index}]: the reason must be one line of 1 to {MAX_REASON_CHARS} characters"),
            )
            .with_details([("field", json!(format!("fixes[{index}].reason")))])
            .with_hint("say in one short line what was wrong and how the fix and its test show it")
        })?;
        out.push(ReviewFix {
            path,
            reason: reason.to_owned(),
        });
    }
    Ok(out)
}

fn check_checks(checks: &str) -> Result<String, ReviewError> {
    let checks = checks.trim();
    if checks.is_empty() || checks.chars().count() > MAX_CHECKS_CHARS {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("checks must be 1 to {MAX_CHECKS_CHARS} characters"),
        )
        .with_details([("field", json!("checks"))])
        .with_hint("give the commands you ran on the fixed version and their results")
        .into());
    }
    Ok(checks.to_owned())
}

/// `fixes_mismatch` unless the fixes name exactly the files that differ from the fixed version, so every change of a reviewer's version has its reason.
fn match_fixes(fixes: &[ReviewFix], changed: &[String]) -> Result<(), ReviewError> {
    let listed: BTreeSet<&str> = fixes.iter().map(|fix| fix.path.as_str()).collect();
    let changed: BTreeSet<&str> = changed.iter().map(String::as_str).collect();
    let unlisted: Vec<&str> = changed.difference(&listed).copied().collect();
    let unchanged: Vec<&str> = listed.difference(&changed).copied().collect();
    if unlisted.is_empty() && unchanged.is_empty() {
        return Ok(());
    }
    Err(FoldError::new(
        codes::INVALID_REQUEST,
        "fixes must name exactly the files that differ from the version you fixed",
    )
    .with_details([
        ("reason", json!("fixes_mismatch")),
        ("unlisted", json!(&unlisted[..unlisted.len().min(MAX_LISTED)])),
        ("unlisted_count", json!(unlisted.len())),
        ("unchanged", json!(&unchanged[..unchanged.len().min(MAX_LISTED)])),
        ("unchanged_count", json!(unchanged.len())),
    ])
    .with_hint("add each changed file to fixes with its reason, undo a change that is no fix, and leave out files you did not change")
    .into())
}

/// The version's evidence: who fixed which version, each file with its reason, then the checks.
fn evidence(
    reviewer: &fold_core::WorkerName,
    version: u32,
    fixes: &[ReviewFix],
    checks: &str,
) -> String {
    let mut text = format!("Review fixes by {reviewer} on v{version}:\n");
    for fix in fixes {
        text.push_str(&format!("- {}: {}\n", fix.path, fix.reason));
    }
    text.push_str("Checks:\n");
    text.push_str(checks);
    text
}

fn no_checkout(proposal: ProposalId, version: u32) -> ReviewError {
    FoldError::new(
        codes::WORKSPACE_NOT_FOUND,
        format!("you have no active review checkout of v{version} of {proposal}"),
    )
    .with_details([
        ("reason", json!("no_checkout")),
        ("proposal", json!(proposal.to_string())),
        ("version", json!(version)),
    ])
    .with_hint("call create_review_checkout for the proposal and fix the files there; a batch checkout mixes every member, so fix a member in its own checkout")
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(path: &str, reason: &str) -> ReviewFix {
        ReviewFix {
            path: path.to_owned(),
            reason: reason.to_owned(),
        }
    }

    fn reason_of(error: ReviewError) -> (Option<&'static str>, serde_json::Value) {
        let ReviewError::Fold(error) = error else {
            panic!("not a refusal")
        };
        (Some(error.code()), json!(error.details()))
    }

    #[test]
    fn fixes_are_normal_paths_named_once_with_a_one_line_reason() {
        let checked = check_fixes(&[fix("src/lib.rs", " off by one ")]).unwrap();
        assert_eq!(checked, [fix("src/lib.rs", "off by one")]);
        assert!(check_fixes(&[]).is_err());
        let many: Vec<ReviewFix> = (0..=MAX_FIXES)
            .map(|n| fix(&format!("f{n}"), "r"))
            .collect();
        assert!(check_fixes(&many).is_err());
        let (code, details) = reason_of(check_fixes(&[fix("a", "r"), fix("a", "s")]).unwrap_err());
        assert_eq!(code, Some(codes::INVALID_REQUEST));
        assert_eq!(details["field"], "fixes[1].path");
        let (_, details) = reason_of(check_fixes(&[fix("a", "two\nlines")]).unwrap_err());
        assert_eq!(details["field"], "fixes[0].reason");
        assert!(check_fixes(&[fix("../a", "r")]).is_err());
        assert!(check_fixes(&[fix("./a", "r")]).is_err());
        assert!(check_checks("  ").is_err());
        assert!(check_checks(&"x".repeat(MAX_CHECKS_CHARS + 1)).is_err());
        assert_eq!(
            check_checks(" cargo test: ok \n").unwrap(),
            "cargo test: ok"
        );
    }

    #[test]
    fn a_mismatch_names_both_sides_and_caps_each_list() {
        let fixes = [fix("a", "r"), fix("b", "r")];
        assert!(match_fixes(&fixes, &["a".to_owned(), "b".to_owned()]).is_ok());
        let changed: Vec<String> = (0..30)
            .map(|n| format!("out/{n:02}"))
            .chain(["a".to_owned()])
            .collect();
        let (_, details) = reason_of(match_fixes(&fixes, &changed).unwrap_err());
        assert_eq!(details["reason"], "fixes_mismatch");
        assert_eq!(details["unlisted_count"], 30);
        assert_eq!(details["unlisted"].as_array().unwrap().len(), MAX_LISTED);
        assert_eq!(details["unchanged"], json!(["b"]));
        assert_eq!(details["unchanged_count"], 1);
    }

    #[test]
    fn the_evidence_lists_each_fix_then_the_checks() {
        let reviewer = fold_core::WorkerName::parse("review").unwrap();
        let text = evidence(
            &reviewer,
            2,
            &[fix("src/lib.rs", "off by one; regression test added")],
            "cargo test: 12 passed",
        );
        assert_eq!(
            text,
            "Review fixes by review on v2:\n- src/lib.rs: off by one; regression test added\nChecks:\ncargo test: 12 passed"
        );
    }
}
