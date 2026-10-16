use fold_core::container::completes_by_children;
use fold_core::dag::Dag;
use fold_core::events::types;
use fold_core::status::calculate;
use fold_core::{EventActor, NewEvent, TaskId, TaskStatus, Timestamp};
use fold_store::tasks::checklists;
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::{WriteTx, tasks};
use serde_json::json;

use super::TaskError;
use crate::services::events::EventBuffer;

/// What one recompute appended. The events are stored but not published: the calling command adds them to its own buffer and publishes after the commit.
#[derive(Debug, Default)]
pub struct Recomputed {
    pub events: EventBuffer,
    /// The tasks this call moved to Todo, in the order it reached them.
    pub became_todo: Vec<TaskId>,
}

/// Recalculates `tasks.status` and `tasks.blocked` for `seeds` and every task that needs one of them, directly or not, inside the caller's transaction. Every command that changes a fact the status depends on calls this before it commits, with the tasks whose facts it changed as seeds; `now` comes from the command's `Clock`.
///
/// Only the edges among the seeds and their dependents are read. Tasks are visited in dependency order and each task's facts are read on the writer's connection just before it is calculated, so each prerequisite's status is final before a dependent reads it, and one call cascades a completion down the whole chain. Before a task is calculated, its linked checklist items whose child is Done are ticked (`checklist.item_done`), and when `completes_by_children` holds it gets a `children` completion (`task.completed`) and calculates as Done; because a parent needs its linked children, a grandparent completes in the same call. Archived tasks keep the cache they had and are neither ticked nor completed. A row is written, and its events appended, only when the status or the blocked flag differs from the stored value. `task.blocked_changed` follows the stored flag, so its `blocked_by` is the unfinished set at the moment the flag changes; a list that shrinks or grows while the task stays blocked shows in the prerequisite's `task.status_changed` or in the `dependency.added` and `dependency.removed` events. The events carry the `fold` actor: the status is calculated, no agent sets it.
pub async fn recompute(
    tx: &mut WriteTx,
    seeds: &[TaskId],
    now: Timestamp,
) -> Result<Recomputed, TaskError> {
    let mut recomputed = Recomputed::default();
    if seeds.is_empty() {
        return Ok(recomputed);
    }
    let graph = Dag::from_edges(tasks::dependent_edges(tx.conn(), seeds).await?);
    let closure = graph.dependent_closure(seeds.iter().copied());
    for task in graph.topological_order(closure) {
        let Some(mut stored) = tasks::load_facts(tx.conn(), &[task]).await?.pop() else {
            continue;
        };
        if stored.archived {
            continue;
        }
        for (item, linked) in checklists::tick_done_children(tx, task, now).await? {
            let event = NewEvent::new(types::CHECKLIST_ITEM_DONE, EventActor::fold())?
                .task(task)
                .data(json!({"item": item.to_string(), "linked_task": linked.to_string()}))?;
            recomputed.events.append(tx, &event, now).await?;
        }
        if completes_by_children(&stored.container) {
            completions::insert(tx, task, CompletionKind::Children, None, now).await?;
            let event = NewEvent::new(types::TASK_COMPLETED, EventActor::fold())?
                .task(task)
                .data(json!({"kind": CompletionKind::Children.as_str()}))?;
            recomputed.events.append(tx, &event, now).await?;
            stored.facts.completed = true;
        }
        let calculated = calculate(&stored.facts);
        let blocked = calculated.blocked();
        if calculated.status == stored.status && blocked == stored.blocked {
            continue;
        }
        tasks::write_cache(tx, task, calculated.status, blocked).await?;
        if calculated.status != stored.status {
            if calculated.status == TaskStatus::Todo {
                recomputed.became_todo.push(task);
            }
            let event = NewEvent::new(types::TASK_STATUS_CHANGED, EventActor::fold())?
                .task(task)
                .data(json!({
                    "from": stored.status.as_str(),
                    "to": calculated.status.as_str(),
                }))?;
            recomputed.events.append(tx, &event, now).await?;
        }
        if blocked != stored.blocked {
            let blocked_by: Vec<String> = calculated
                .blocked_by
                .iter()
                .map(TaskId::to_string)
                .collect();
            let event = NewEvent::new(types::TASK_BLOCKED_CHANGED, EventActor::fold())?
                .task(task)
                .data(json!({"blocked": blocked, "blocked_by": blocked_by}))?;
            recomputed.events.append(tx, &event, now).await?;
        }
    }
    Ok(recomputed)
}
