use std::fmt::Write as _;

use fold_core::events::types;
use fold_core::merge::{Conflict, ConflictReport, ReportTrees};
use fold_core::reviews::{Approver, Verdict};
use fold_core::{
    Actor, AgentName, HumanName, IdKind, IntegrationId, NewEvent, ProposalId, RevisionId, Role,
    TaskId,
};
use fold_store::reviews::{self, IntegrationSubject, NewIntegration};
use fold_store::{StoreError, WriteTx, agents, counters, tasks, workspaces};
use serde_json::json;

use super::integrate::{caller_facts, decide};
use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;
use crate::services::messages::tell_review_decider_in_tx;
use crate::services::tasks::recompute;

/// A conflicting merge of the current version of a proposal onto main, as the integration procedure found it.
pub(super) struct FoundConflict<'a> {
    pub proposal: ProposalId,
    pub task: TaskId,
    pub version: u32,
    /// The head the merge ran onto.
    pub main: RevisionId,
    pub trees: ReportTrees,
    pub conflicts: &'a [Conflict],
    /// The version the caller asked to integrate, and whether the daemon integrates on its own: the gates read both again.
    pub expected_version: Option<u32>,
    pub automatic: bool,
}

impl ReviewService {
    /// Step 4 of the integration procedure, under the integration mutex. One write transaction records the `conflicted` integration with its report and `integration.conflicted`, sets the proposal's conflict flag and recalculates the task, which stays in Review while the proposal stays open. The conflict goes to the reviewer agent that approved the version and still holds its review claim: the event names it as `fixer`, and the worker supervisor gives it the conflict report. Without one, because a human or an agent that is not a reviewer approved the version or the reviewer lost its claim, the same transaction sends the review lead a message from `fold` to start a reviewer for the conflict. The author gets no conflict notice. Nothing is published: no revision, no move of main, no completion. False, with nothing written, when main moved, the proposal left that version or the gates refuse the caller, so the procedure starts again and its gate step reports why.
    pub(super) async fn record_conflict(
        &self,
        actor: &Actor,
        found: FoundConflict<'_>,
    ) -> Result<bool, ReviewError> {
        let FoundConflict {
            proposal,
            task,
            version,
            main,
            trees,
            conflicts,
            expected_version,
            automatic,
        } = found;
        let label = actor.event_actor();
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let recorded = self
            .store
            .write(async |tx| {
                if workspaces::main_head(tx.conn()).await? != Some(main) {
                    return Ok(None);
                }
                let Some(row) = reviews::get_proposal(tx.conn(), proposal).await? else {
                    return Ok(None);
                };
                if row.current_version != version {
                    return Ok(None);
                }
                // The gates again, as the clean path reads them before it publishes: a conflict recorded for a caller the gates refuse would message the author for nothing.
                let Some(facts) = reviews::integration_facts(tx.conn(), proposal).await? else {
                    return Ok(None);
                };
                let standing = agents::standing_of(tx.conn(), actor).await?;
                let caller = caller_facts(actor, standing, automatic)?;
                if decide(&facts, &caller, expected_version).is_err()
                    || !reviews::mark_conflicted(tx, proposal, now).await?
                {
                    return Ok(None);
                }
                let base_revision = reviews::list_versions(tx.conn(), proposal)
                    .await?
                    .into_iter()
                    .find(|row| row.version == version)
                    .ok_or(StoreError::Corrupt("proposal_versions.version"))?
                    .base;
                let report =
                    ConflictReport::new(proposal, version, base_revision, main, trees, conflicts);
                let number = counters::allocate(tx, IdKind::Integration).await?;
                let integration = IntegrationId::from_number(number)
                    .ok_or(StoreError::Corrupt("counters.next"))?;
                let new = NewIntegration {
                    number: integration,
                    subject: IntegrationSubject::Proposal {
                        proposal,
                        version,
                        remote_commit: None,
                        batch: None,
                    },
                    actor: label.as_str(),
                    created_at: now,
                };
                reviews::insert_conflicted(tx, &new, &report).await?;
                let fixer = conflict_fixer(tx, actor, proposal, version).await?;
                let fixer_name = match &fixer {
                    Fixer::Reviewer(agent) => Some(agent.as_str()),
                    Fixer::ReviewLead(_) => None,
                };
                let event = NewEvent::new(types::INTEGRATION_CONFLICTED, label.clone())?
                    .task(task)
                    .proposal(proposal)
                    .revision(main)
                    .data(json!({
                        "integration": integration.to_string(),
                        "proposal": proposal.to_string(),
                        "version": version,
                        "main_revision": main.to_string(),
                        "files": report.files.len(),
                        "fixer": fixer_name,
                    }))?;
                buffer.append(tx, &event, now).await?;
                if let Fixer::ReviewLead(why) = &fixer {
                    conflict_notice_in_tx(tx, &mut buffer, &report, task, why, now).await?;
                }
                buffer.extend(recompute(tx, &[task], now).await?.events);
                Ok::<_, ReviewError>(Some((integration, report.files.len())))
            })
            .await?;
        let Some((integration, files)) = recorded else {
            return Ok(false);
        };
        self.bus.publish(buffer);
        tracing::info!(proposal = %proposal, version, revision = %main, integration = %integration, files, "proposal conflicts with main");
        Ok(true)
    }
}

/// Who fixes a conflict of an approved version at integration.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Fixer {
    /// The reviewer agent that approved the version and holds the proposal's review claim.
    Reviewer(AgentName),
    /// No reviewer agent takes it on its own, for this reason; the review lead starts one.
    ReviewLead(NoReviewer),
}

/// Why no reviewer agent takes a conflict on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoReviewer {
    /// A human approved the version.
    Human(HumanName),
    /// An agent that is not a reviewer, such as the main agent, approved the version.
    Agent(AgentName),
    /// The reviewer that approved the version no longer holds the review claim.
    ClaimGone,
    /// The reviewer that approved the version could not be resumed in its session.
    NotResumed(AgentName),
}

/// The fixer of a conflict of `version`: the newest approval of the version by a reviewer worker whose agent holds the unreleased review claim, else the review lead with the reason of the newest approval. A human who integrates approves in the same action, so without a stored approval the integrating human is the approver.
async fn conflict_fixer(
    tx: &mut WriteTx,
    actor: &Actor,
    proposal: ProposalId,
    version: u32,
) -> Result<Fixer, StoreError> {
    let facts = reviews::review_facts(tx.conn(), proposal, version).await?;
    let holder = reviews::unreleased_review_claim(tx.conn(), proposal)
        .await?
        .map(|claim| claim.worker.agent());
    let mut fallback = None;
    for fact in facts
        .iter()
        .rev()
        .filter(|fact| fact.verdict == Verdict::Approve)
    {
        let why = match &fact.approver {
            Approver::Human(name) => NoReviewer::Human(name.clone()),
            Approver::DecidingAgent | Approver::OtherAgent => {
                let Some(agent) = fact
                    .reviewer
                    .agent()
                    .and_then(|name| AgentName::parse(name).ok())
                else {
                    continue;
                };
                let role = agents::find_worker_process(tx.conn(), &agent)
                    .await?
                    .map(|row| row.role);
                if role != Some(Role::Reviewer) {
                    NoReviewer::Agent(agent)
                } else if holder.as_ref() == Some(&agent) {
                    return Ok(Fixer::Reviewer(agent));
                } else {
                    NoReviewer::ClaimGone
                }
            }
        };
        fallback.get_or_insert(why);
    }
    let integrator = match actor {
        Actor::Human { name } => NoReviewer::Human(name.clone()),
        Actor::Agent(_) | Actor::System => NoReviewer::ClaimGone,
    };
    Ok(Fixer::ReviewLead(fallback.unwrap_or(integrator)))
}

/// The conflicting paths of `report`, the first few by name and the count of the rest.
fn conflict_paths(report: &ConflictReport) -> String {
    const NAMED: usize = 5;
    let named: Vec<&str> = report
        .files
        .iter()
        .take(NAMED)
        .map(|file| file.path.as_str())
        .collect();
    let mut text = format!(
        "Conflicting paths ({}): {}",
        report.files.len(),
        named.join(", ")
    );
    let rest = report.files.len().saturating_sub(NAMED);
    if rest > 0 {
        let _ = write!(text, " and {rest} more");
    }
    text.push('.');
    text
}

/// How a reviewer fixes a conflict: the same for the reviewer that approved the version and for one the review lead starts.
fn fix_steps(report: &ConflictReport) -> String {
    let (proposal, version) = (report.proposal, report.version);
    format!(
        "Call create_review_checkout for {proposal}: the checkout holds v{version} merged onto the current main, with conflict markers in each conflicted text file and main's side of each other conflict beside it as <path>.fold-main. Resolve each conflicted path, remove each .fold-main file, run the checks, send the fix with submit_review_fix, then approve the new version with submit_review, and fold integrates it. When the conflict needs the author's decision, call submit_review with the verdict request_changes instead."
    )
}

/// The turn of the reviewer that approved `report`'s version and fixes its conflict.
pub(crate) fn reviewer_conflict_turn(report: &ConflictReport, task: TaskId) -> String {
    let (proposal, version) = (report.proposal, report.version);
    format!(
        "{proposal} v{version} for {task}, which you approved, conflicts with main at {} and was not integrated. You fix the conflict, not the author: the task stays in Review and the author gets no conflict notice.\n{}\n{}",
        report.main_revision,
        conflict_paths(report),
        fix_steps(report)
    )
}

/// Sends the notice of a conflict that no reviewer agent takes on its own, in the caller's write: to the main agent for a task of the area main, with [`main_conflict_notice`], else to the review lead with [`review_lead_conflict_notice`].
pub(crate) async fn conflict_notice_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    report: &ConflictReport,
    task: TaskId,
    why: &NoReviewer,
    now: fold_core::Timestamp,
) -> Result<(), ReviewError> {
    let main_task = tasks::labels_for_conn(tx.conn(), task)
        .await?
        .iter()
        .any(|label| label.name.is_main());
    let text = if main_task {
        main_conflict_notice(report, task, why)
    } else {
        review_lead_conflict_notice(report, task, why)
    };
    tell_review_decider_in_tx(tx, buffer, task, report.proposal, text, main_task, now).await?;
    Ok(())
}

/// Why no reviewer agent takes the conflict, as the first sentence of a notice.
fn no_reviewer_reason(report: &ConflictReport, why: &NoReviewer) -> String {
    let (proposal, version) = (report.proposal, report.version);
    match why {
        NoReviewer::Human(name) => format!("The human @{name} approved v{version}"),
        NoReviewer::Agent(agent) => format!("@{agent} approved v{version} and is not a reviewer"),
        NoReviewer::ClaimGone => {
            format!("No reviewer that approved v{version} holds the review claim of {proposal}")
        }
        NoReviewer::NotResumed(agent) => {
            format!("@{agent} approved v{version}, but fold could not resume it")
        }
    }
}

/// The notice to the main agent for a conflict of a task of the area main, whose reviews only it or a human decides, so no reviewer is started for it.
pub(crate) fn main_conflict_notice(
    report: &ConflictReport,
    task: TaskId,
    why: &NoReviewer,
) -> String {
    let (proposal, version) = (report.proposal, report.version);
    format!(
        "{proposal} v{version} for {task} conflicts with main at {} and was not integrated. {}. {task} is a task of main, so you or a human fix the conflict: claim the review of {proposal} with claim_review, or ask the humans. The task stays in Review and its author gets no conflict notice.\n{}\nThe steps: {}",
        report.main_revision,
        no_reviewer_reason(report, why),
        conflict_paths(report),
        fix_steps(report)
    )
}

/// The notice to the review lead for a conflict that no reviewer agent takes on its own.
pub(crate) fn review_lead_conflict_notice(
    report: &ConflictReport,
    task: TaskId,
    why: &NoReviewer,
) -> String {
    let (proposal, version) = (report.proposal, report.version);
    let reason = no_reviewer_reason(report, why);
    format!(
        "{proposal} v{version} for {task} conflicts with main at {} and was not integrated. {reason}, so the conflict waits for a reviewer: start one for {proposal} with start_worker. The task stays in Review and its author gets no conflict notice.\n{}\nThe reviewer's steps: {}",
        report.main_revision,
        conflict_paths(report),
        fix_steps(report)
    )
}

#[cfg(test)]
mod tests {
    use fold_core::merge::{ReportFile, ReportKind};
    use fold_core::object::{ObjectKind, hash_object};

    use super::*;

    fn report(paths: &[&str]) -> ConflictReport {
        let hash = |text: &str| hash_object(ObjectKind::Tree, text.as_bytes());
        ConflictReport {
            proposal: ProposalId::from_number(2).unwrap(),
            version: 3,
            base_revision: RevisionId::from_number(1).unwrap(),
            main_revision: RevisionId::from_number(4).unwrap(),
            trees: ReportTrees {
                base: hash("base"),
                main: hash("main"),
                version: hash("version"),
            },
            files: paths
                .iter()
                .map(|path| ReportFile {
                    path: (*path).to_owned(),
                    kind: ReportKind::Text,
                    base: None,
                    main: None,
                    version: None,
                    hunks: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn the_reviewer_turn_says_who_fixes_the_conflict_and_how() {
        let task = TaskId::from_number(7).unwrap();
        let turn = reviewer_conflict_turn(&report(&["a.rs"]), task);
        for wanted in [
            "P-2 v3 for T-7, which you approved, conflicts with main at r4",
            "You fix the conflict, not the author",
            "Conflicting paths (1): a.rs.",
            "create_review_checkout for P-2",
            "submit_review_fix",
            "request_changes",
        ] {
            assert!(turn.contains(wanted), "{wanted}: {turn}");
        }
    }

    #[test]
    fn a_conflict_of_a_main_task_asks_the_main_agent_and_starts_no_reviewer() {
        let task = TaskId::from_number(7).unwrap();
        let why = NoReviewer::Agent(AgentName::parse("main").unwrap());
        let notice = main_conflict_notice(&report(&["a.rs"]), task, &why);
        for wanted in [
            "P-2 v3 for T-7 conflicts with main at r4",
            "@main approved v3 and is not a reviewer",
            "T-7 is a task of main, so you or a human fix the conflict",
            "claim_review",
            "create_review_checkout for P-2",
        ] {
            assert!(notice.contains(wanted), "{wanted}: {notice}");
        }
        assert!(!notice.contains("start_worker"), "{notice}");
    }

    #[test]
    fn the_review_lead_notice_names_why_no_reviewer_takes_the_conflict() {
        let task = TaskId::from_number(7).unwrap();
        let many = report(&["a", "b", "c", "d", "e", "f", "g"]);
        let agent = AgentName::parse("reviewer-2").unwrap();
        for (why, wanted) in [
            (
                NoReviewer::Human(HumanName::parse("ana").unwrap()),
                "The human @ana approved v3",
            ),
            (
                NoReviewer::Agent(AgentName::parse("main").unwrap()),
                "@main approved v3 and is not a reviewer",
            ),
            (
                NoReviewer::ClaimGone,
                "No reviewer that approved v3 holds the review claim of P-2",
            ),
            (
                NoReviewer::NotResumed(agent.clone()),
                "@reviewer-2 approved v3, but fold could not resume it",
            ),
        ] {
            let notice = review_lead_conflict_notice(&many, task, &why);
            assert!(notice.contains(wanted), "{wanted}: {notice}");
            assert!(
                notice.contains("Conflicting paths (7): a, b, c, d, e and 2 more."),
                "{notice}"
            );
            assert!(
                notice.contains("start one for P-2 with start_worker"),
                "{notice}"
            );
        }
    }
}
