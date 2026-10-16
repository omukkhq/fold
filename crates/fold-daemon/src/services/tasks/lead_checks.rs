use fold_core::events::types;
use fold_core::{
    Actor, AreaName, FoldError, LabelName, NewEvent, TaskId, TaskKind, Timestamp, WorkerName,
    codes, policy,
};
use fold_store::tasks::TaskRow;
use fold_store::tasks::claims::{self, ReleaseReason};
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::tasks::reports::{self, ReportRow, Resolution};
use fold_store::{WriteTx, agents, tasks};
use serde_json::json;

use super::{
    TaskError, TaskService, TaskView, closed_refusal, recompute, task_not_found, worker_row,
};
use crate::services::agents::starter_of;
use crate::services::events::EventBuffer;

/// How many reports a task read returns.
pub const REPORT_LIMIT: u32 = 20;

/// The longest report or send-back note, in characters.
pub const MAX_NOTE_CHARS: usize = 20_000;

/// A coder's report on a task without a proposal as the task shows it, with the lead check that answered it once there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportView {
    pub by: WorkerName,
    pub note: String,
    pub at: Timestamp,
    pub resolution: Option<Resolution>,
    /// A worker name or `human:<name>`.
    pub resolved_by: Option<String>,
    pub resolved_at: Option<Timestamp>,
    pub send_back_note: Option<String>,
}

impl From<ReportRow> for ReportView {
    fn from(row: ReportRow) -> Self {
        Self {
            by: row.worker,
            note: row.note,
            at: row.created_at,
            resolution: row.resolution,
            resolved_by: row.resolved_by,
            resolved_at: row.resolved_at,
            send_back_note: row.send_back_note,
        }
    }
}

/// The lead check of tasks without a proposal: the coder reports the work finished, and the lead of the task's area, the main agent or a human completes the task or sends it back. Each command is one write transaction that runs `recompute`, because the claim and the open report decide the status of such a task.
impl TaskService {
    /// Reports the work on `task` finished with `note`, by the worker that holds its implementation claim. The task moves to Review and waits for a lead check; the claim stays with the coder, so a send-back returns the task to it.
    pub async fn report_task(
        &self,
        actor: &Actor,
        task: TaskId,
        note: &str,
    ) -> Result<TaskView, TaskError> {
        let claimant = policy::can_claim(actor).map_err(FoldError::from)?;
        let note = check_note(note)?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let worker = worker_row(tx, claimant).await?;
                let row = open_task(tx, task).await?;
                if row.kind != TaskKind::NoProposal {
                    return Err(FoldError::new(
                        codes::TASK_KIND_MISMATCH,
                        format!("{task} needs a proposal"),
                    )
                    .with_details([("kind", json!(row.kind.as_str()))])
                    .with_hint("send the finished work with create_proposal")
                    .into());
                }
                let holder = claims::unreleased_for_task(tx.conn(), task).await?;
                if holder.as_ref().map(|claim| claim.worker_id) != Some(worker.id) {
                    return Err(FoldError::new(
                        codes::NOT_CLAIM_HOLDER,
                        format!("{} does not hold the claim on {task}", worker.name),
                    )
                    .with_details([(
                        "holder",
                        json!(holder.as_ref().map(|claim| claim.worker.as_str())),
                    )])
                    .with_hint("only the claim holder reports a task; call claim_task first")
                    .into());
                }
                match reports::insert(tx, task, worker.id, &note, now).await {
                    Ok(()) => {}
                    Err(error) if error.unique_violation() => {
                        return Err(FoldError::new(
                            codes::TASK_ALREADY_REPORTED,
                            format!("{task} is already reported and waits for a lead check"),
                        )
                        .with_hint(
                            "wait for the lead check; fold resumes you if the task is sent back",
                        )
                        .into());
                    }
                    Err(error) => return Err(error.into()),
                }
                let event = NewEvent::new(types::TASK_REPORTED, actor.event_actor())?
                    .task(task)
                    .data(json!({"worker": worker.name.as_str()}))?;
                buffer.append(tx, &event, now).await?;
                buffer.extend(recompute(tx, &[task], now).await?.events);
                Ok::<_, TaskError>(())
            })
            .await?;
        self.bus.publish(buffer);
        self.get(task).await
    }

    /// Completes a reported task without a proposal: the open report is answered `completed`, the claim is released and the completion record is written, so the task is Done and its dependents unblock in the same transaction. It is refused with `prerequisites_not_done` while a prerequisite is not Done, as an integration is, and with `human_approval_missing` when an agent completes a task with the human review mark, which only a human's check passes.
    pub async fn complete_task(&self, actor: &Actor, task: TaskId) -> Result<TaskView, TaskError> {
        let now = self.clock.now();
        let by = actor.event_actor();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let (row, _) = checked_report(tx, actor, task).await?;
                policy::human_check_done(row.human_review.as_ref(), matches!(actor, Actor::Human { .. }))
                    .map_err(|refusal| {
                        FoldError::from(refusal).with_hint(
                            "the task has the human review mark, so a human completes it; ask the named human, or any human when none is named",
                        )
                    })?;
                let prerequisites = tasks::load_facts(tx.conn(), &[task])
                    .await?
                    .pop()
                    .map(|loaded| loaded.facts.prerequisites)
                    .unwrap_or_default();
                policy::prerequisites_done(&prerequisites).map_err(|refusal| {
                    FoldError::from(refusal).with_hint(
                        "call get_task on each prerequisite in the details; complete this task once they are Done",
                    )
                })?;
                reports::resolve(tx, task, Resolution::Completed, by.as_str(), None, now).await?;
                if let Some(claim) = claims::unreleased_for_task(tx.conn(), task).await? {
                    let reason = ReleaseReason::Completed;
                    claims::release(tx, task, claim.worker_id, reason, now).await?;
                    let event = NewEvent::new(types::CLAIM_RELEASED, by.clone())?
                        .task(task)
                        .data(json!({
                            "kind": "implementation",
                            "worker": claim.worker.as_str(),
                            "reason": reason.as_str(),
                        }))?;
                    buffer.append(tx, &event, now).await?;
                }
                completions::insert(tx, task, CompletionKind::Check, None, now).await?;
                let event = NewEvent::new(types::TASK_COMPLETED, by.clone())?
                    .task(task)
                    .data(json!({"kind": CompletionKind::Check.as_str()}))?;
                buffer.append(tx, &event, now).await?;
                buffer.extend(recompute(tx, &[task], now).await?.events);
                Ok::<_, TaskError>(())
            })
            .await?;
        self.bus.publish(buffer);
        self.get(task).await
    }

    /// Sends a reported task without a proposal back to its coder with `note`: the open report is answered `sent_back` and the task returns to In Progress while the coder holds the claim. The supervisor resumes a stopped task coder with the note.
    pub async fn send_back_task(
        &self,
        actor: &Actor,
        task: TaskId,
        note: &str,
    ) -> Result<TaskView, TaskError> {
        let note = check_note(note)?;
        let now = self.clock.now();
        let by = actor.event_actor();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let (_, report) = checked_report(tx, actor, task).await?;
                reports::resolve(
                    tx,
                    task,
                    Resolution::SentBack,
                    by.as_str(),
                    Some(&note),
                    now,
                )
                .await?;
                let event = NewEvent::new(types::TASK_SENT_BACK, by.clone())?
                    .task(task)
                    .data(json!({"worker": report.worker.as_str()}))?;
                buffer.append(tx, &event, now).await?;
                buffer.extend(recompute(tx, &[task], now).await?.events);
                Ok::<_, TaskError>(())
            })
            .await?;
        self.bus.publish(buffer);
        self.get(task).await
    }
}

/// Refuses a task without a proposal where a command needs a task workspace or a proposal; `None` for a task with a proposal.
pub(crate) fn proposal_kind_refusal(row: &TaskRow) -> Option<TaskError> {
    (row.kind == TaskKind::NoProposal).then(|| {
        FoldError::new(
            codes::TASK_KIND_MISMATCH,
            format!("{} needs no proposal, so it has no workspace", row.id),
        )
        .with_details([("kind", json!(row.kind.as_str()))])
        .with_hint("do the work through the tools the task names, then call report_task")
        .into()
    })
}

/// The areas whose label is among `labels`.
pub(crate) async fn areas_of(
    tx: &mut WriteTx,
    labels: &[LabelName],
) -> Result<Vec<AreaName>, TaskError> {
    Ok(agents::list_areas(tx.conn())
        .await?
        .into_iter()
        .filter(|area| labels.contains(&area.label))
        .map(|area| area.name)
        .collect())
}

/// The rules a lead check shares: the task exists, the actor may check it, it is open, it needs no proposal and its coder's report waits. Returns the task and that report.
async fn checked_report(
    tx: &mut WriteTx,
    actor: &Actor,
    task: TaskId,
) -> Result<(TaskRow, ReportRow), TaskError> {
    let row = tasks::fetch(tx.conn(), task)
        .await?
        .ok_or_else(|| task_not_found(task))?;
    let starter = starter_of(tx, actor).await?;
    let labels: Vec<LabelName> = tasks::labels_for_conn(tx.conn(), task)
        .await?
        .into_iter()
        .map(|label| label.name)
        .collect();
    let areas = areas_of(tx, &labels).await?;
    policy::can_check_task(&starter, &areas).map_err(FoldError::from)?;
    if let Some(refusal) = closed_refusal(&row) {
        return Err(refusal);
    }
    if row.kind != TaskKind::NoProposal {
        return Err(FoldError::new(
            codes::TASK_KIND_MISMATCH,
            format!("{task} needs a proposal"),
        )
        .with_details([("kind", json!(row.kind.as_str()))])
        .with_hint("review its proposal instead; integration completes it")
        .into());
    }
    let report = reports::open_for_task(tx.conn(), task)
        .await?
        .ok_or_else(|| -> TaskError {
            FoldError::new(
                codes::TASK_NOT_REPORTED,
                format!("{task} has no report that waits for a check"),
            )
            .with_hint("wait until its coder calls report_task")
            .into()
        })?;
    Ok((row, report))
}

/// The task, refused with `task_closed` when it is Done or archived.
async fn open_task(tx: &mut WriteTx, task: TaskId) -> Result<TaskRow, TaskError> {
    let row = tasks::fetch(tx.conn(), task)
        .await?
        .ok_or_else(|| task_not_found(task))?;
    match closed_refusal(&row) {
        Some(refusal) => Err(refusal),
        None => Ok(row),
    }
}

fn check_note(note: &str) -> Result<String, TaskError> {
    let note = note.trim();
    if note.is_empty() || note.chars().count() > MAX_NOTE_CHARS {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("the note must be 1 to {MAX_NOTE_CHARS} characters"),
        )
        .with_details([("field", json!("note"))])
        .with_hint("say what was done, or what to change, in the note")
        .into());
    }
    Ok(note.to_owned())
}
