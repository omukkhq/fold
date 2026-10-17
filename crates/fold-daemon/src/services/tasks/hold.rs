use fold_core::events::types;
use fold_core::{
    Actor, FoldError, HoldMark, HoldReason, LabelName, NewEvent, PolicyDenied, TaskId, TaskKind,
    TaskStatus, codes, policy, policy::TaskStart,
};
use fold_store::tasks::{self, TaskRow, claims};
use fold_store::{WriteTx, reviews, workspaces};
use serde_json::json;

use super::{TaskDetail, TaskError, TaskService, TaskView, areas_of, task_not_found};
use crate::services::agents::starter_of;
use crate::services::events::EventBuffer;

/// The hold mark, which the board labels Blocker. Each command is one write transaction that appends `task.hold_set` or `task.hold_cleared`; neither runs `recompute`, because the hold never changes a status.
impl TaskService {
    /// Puts a task that no agent has started on hold with `reason`, or changes the reason of its hold, by a human, the main agent or the lead of an area whose label the task carries. A task with a claim, a live task workspace or an open proposal, or one past Todo, is refused with `task_hold_denied` and the detail reason `started`. The same reason again changes nothing and appends no event. While the hold is on, `get_ready_tasks` leaves the task out, a claim is refused and no lead starts a task worker for it.
    pub async fn set_hold(
        &self,
        actor: &Actor,
        task: TaskId,
        reason: &str,
    ) -> Result<TaskView, TaskError> {
        let reason = HoldReason::parse(reason).map_err(|error| {
            FoldError::from(error)
                .with_details([("field", json!("reason"))])
                .with_hint("say in a line or two why the task must wait for a person")
        })?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let row = tasks::fetch(tx.conn(), task)
                    .await?
                    .ok_or_else(|| task_not_found(task))?;
                let starter = starter_of(tx, actor).await?;
                let labels = labels_of(tx, task).await?;
                let areas = areas_of(tx, &labels).await?;
                policy::can_set_hold(&starter, &areas).map_err(|denied| refusal(denied, task))?;
                if let Some(closed) = closed(&row, true) {
                    return Err(closed);
                }
                let start = TaskStart {
                    status: row.status,
                    claimed: claims::unreleased_for_task(tx.conn(), task)
                        .await?
                        .is_some(),
                    workspace: workspaces::live_task_workspace(tx.conn(), task)
                        .await?
                        .is_some(),
                    open_proposal: reviews::open_for_task(tx.conn(), task).await?.is_some(),
                };
                policy::can_hold_task(&start).map_err(|denied| {
                    TaskError::from(FoldError::from(denied).with_details([
                        ("task", json!(task.to_string())),
                        ("reason", json!("started")),
                    ]))
                })?;
                if row.hold.as_ref().is_some_and(|hold| hold.reason == reason) {
                    return Ok(());
                }
                let mark = HoldMark {
                    reason: reason.clone(),
                    set_by: actor.event_actor(),
                    set_at: now,
                };
                tasks::set_hold(tx, task, &mark, now).await?;
                let event = NewEvent::new(types::TASK_HOLD_SET, actor.event_actor())?
                    .task(task)
                    .data(json!({"reason": mark.reason.as_str()}))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, TaskError>(())
            })
            .await?;
        self.bus.publish(buffer);
        self.get(task).await
    }

    /// Takes a task off hold, by a human or the main agent. The event keeps the reason the hold had. A task without a hold changes nothing.
    pub async fn clear_hold(&self, actor: &Actor, task: TaskId) -> Result<TaskView, TaskError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let row = tasks::fetch(tx.conn(), task)
                    .await?
                    .ok_or_else(|| task_not_found(task))?;
                let starter = starter_of(tx, actor).await?;
                policy::can_clear_hold(&starter).map_err(|denied| refusal(denied, task))?;
                if let Some(closed) = closed(&row, false) {
                    return Err(closed);
                }
                let Some(previous) = row.hold else {
                    return Ok(());
                };
                tasks::clear_hold(tx, task, now).await?;
                let event = NewEvent::new(types::TASK_HOLD_CLEARED, actor.event_actor())?
                    .task(task)
                    .data(json!({"reason": previous.reason.as_str()}))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, TaskError>(())
            })
            .await?;
        self.bus.publish(buffer);
        self.get(task).await
    }
}

/// What a named human may do now with the blocker of a task and with the Done mark of a human task, as a task view offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HumanTaskRights {
    pub set_blocker: bool,
    pub clear_blocker: bool,
    pub mark_done: bool,
}

/// The rights of a named human on `detail`; a request that names no human gets none. A human sets a blocker on a task no agent has started, clears one on a task that is not archived and marks a human task Done while it is open; `complete_task` still checks the prerequisites.
pub fn human_task_rights(detail: &TaskDetail, named_human: bool) -> HumanTaskRights {
    let open = detail.task.archived_at.is_none();
    HumanTaskRights {
        set_blocker: named_human && detail.blocker_allowed,
        clear_blocker: named_human && open && detail.task.hold.is_some(),
        mark_done: named_human
            && open
            && detail.task.kind == TaskKind::Human
            && detail.task.status != TaskStatus::Done,
    }
}

/// Refuses agent work on a human task with `task_needs_human` and on a task on hold with `task_on_hold`: a claim, a task workspace, a proposal or a task worker started for it. `None` for a task an agent may take. The reason of the hold stays out of the error; `get_task` shows it.
pub(crate) fn agent_refusal(row: &TaskRow) -> Option<FoldError> {
    let task = json!(row.id.to_string());
    if row.kind == TaskKind::Human {
        return Some(
            FoldError::new(
                codes::TASK_NEEDS_HUMAN,
                format!("{} is a human task: a person does it outside fold", row.id),
            )
            .with_details([("task", task)])
            .with_hint(
                "no agent works on a human task; a human or the main agent marks it Done; call get_ready_tasks for other work",
            ),
        );
    }
    let hold = row.hold.as_ref()?;
    Some(
        FoldError::new(
            codes::TASK_ON_HOLD,
            format!("{} has a blocker and waits for a person", row.id),
        )
        .with_details([("task", task), ("set_by", json!(hold.set_by.as_str()))])
        .with_hint(
            "wait until a human or the main agent calls clear_blocker; get_task shows the reason; call get_ready_tasks for other work",
        ),
    )
}

/// `task_closed` for an archived task, and with `done` for a Done one too.
fn closed(row: &TaskRow, done: bool) -> Option<TaskError> {
    let reason = if row.archived_at.is_some() {
        "archived"
    } else if done && row.status == TaskStatus::Done {
        "done"
    } else {
        return None;
    };
    Some(
        FoldError::new(codes::TASK_CLOSED, format!("{} is {reason}", row.id))
            .with_details([("reason", json!(reason))])
            .with_hint("a Done or archived task needs no blocker")
            .into(),
    )
}

async fn labels_of(tx: &mut WriteTx, task: TaskId) -> Result<Vec<LabelName>, TaskError> {
    Ok(tasks::labels_for_conn(tx.conn(), task)
        .await?
        .into_iter()
        .map(|label| label.name)
        .collect())
}

fn refusal(denied: PolicyDenied, task: TaskId) -> TaskError {
    FoldError::from(denied)
        .with_details([("task", json!(task.to_string()))])
        .into()
}
