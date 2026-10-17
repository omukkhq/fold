use std::collections::{BTreeMap, BTreeSet};

use fold_core::events::types;
use fold_core::reviews::{HumanReviewMark, human_review_reason};
use fold_core::task::{PlanRef, PlannedTask, check_task_plan};
use fold_core::{
    Actor, ChecklistName, DependencyOrigin, FoldError, HoldMark, HoldReason, HumanName, NewEvent,
    TaskId, TaskKind, codes, policy,
};
use fold_store::{Store, StoreError, projects, tasks};
use serde_json::json;

use super::{
    CreateTask, ItemContent, NewChecklistItem, TaskError, TaskService, TaskView, add_edge_in_tx,
    areas_of, checklists, create_in_tx, recompute, task_not_found,
};
use crate::services::agents::starter_of;
use crate::services::events::EventBuffer;

impl TaskService {
    /// Creates a plan's tasks, edges, parent links, review marks and blockers in one transaction, so a planned blocker is on before any lead can start the task. A task that names the same prerequisite twice gets one edge. Notices are published only after all task statuses have been calculated. `registry` names the humans eligible for a named review mark.
    pub async fn create_task_plan(
        &self,
        actor: &Actor,
        plan: Vec<PlannedTask>,
        registry: Option<&Store>,
    ) -> Result<Vec<(String, TaskView)>, TaskError> {
        check_task_plan(&plan)?;
        for task in &plan {
            if let Some(review) = &task.human_review
                && let Some(name) = &review.reviewer
            {
                let known = match registry {
                    Some(registry) => projects::find_human(registry.read(), name).await?.is_some(),
                    None => false,
                };
                if !known {
                    return Err(FoldError::new(codes::UNKNOWN_HUMAN, format!("{name} is not a known human"))
                        .with_details([("reviewer", json!(name.as_str()))])
                        .with_hint("use a registered human as reviewer or omit reviewer, then call create_task_plan again").into());
                }
            }
        }
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let created = self.store.write(async |tx| {
            let starter = starter_of(tx, actor).await?;
            for task in &plan {
                let areas = areas_of(tx, &task.labels).await?;
                policy::can_publish_task_plan(&starter, &areas).map_err(FoldError::from)?;
                for reference in task.needs.iter().chain(task.parent.iter()) {
                    if let PlanRef::Existing(id) = reference
                        && tasks::fetch(tx.conn(), *id).await?.is_none()
                    {
                        return Err(task_not_found(*id));
                    }
                }
                if let Some(PlanRef::Existing(parent)) = &task.parent {
                    let labels = tasks::labels_for_conn(tx.conn(), *parent).await?;
                    let names: Vec<_> = labels.into_iter().map(|label| label.name).collect();
                    let areas = areas_of(tx, &names).await?;
                    policy::can_publish_task_plan(&starter, &areas).map_err(FoldError::from)?;
                }
            }
            let mut by_key = BTreeMap::new();
            let mut created = Vec::with_capacity(plan.len());
            for task in &plan {
                let input = CreateTask {
                    title: task.title.clone(),
                    description: task.description.clone(),
                    labels: task.labels.clone(),
                };
                if task.kind != TaskKind::Proposal {
                    let areas = areas_of(tx, &task.labels).await?;
                    policy::can_choose_no_proposal(&starter, &areas).map_err(FoldError::from)?;
                }
                let id = create_in_tx(tx, &mut buffer, actor, &input, task.kind, now).await?;
                by_key.insert(task.key.as_str(), id);
                created.push((task.key.clone(), id));
                if let Some(review) = &task.human_review {
                    let reason = human_review_reason(&review.reason)?;
                    policy::can_set_human_review(actor, None, review.reviewer.as_ref(), &reason).map_err(FoldError::from)?;
                    let mark = HumanReviewMark {
                        reviewer: review.reviewer.clone(), reason, set_by: actor.event_actor(),
                    };
                    tasks::set_human_review(tx, id, &mark, now).await?;
                    let event = NewEvent::new(types::TASK_HUMAN_REVIEW_SET, actor.event_actor())?
                        .task(id).data(json!({"reviewer": mark.reviewer.as_ref().map(HumanName::as_str), "reason": mark.reason, "set_by": mark.set_by.as_str()}))?;
                    buffer.append(tx, &event, now).await?;
                }
                if let Some(reason) = &task.blocker {
                    let reason = HoldReason::parse(reason).map_err(FoldError::from)?;
                    let areas = areas_of(tx, &task.labels).await?;
                    policy::can_set_hold(&starter, &areas).map_err(FoldError::from)?;
                    let mark = HoldMark { reason, set_by: actor.event_actor(), set_at: now };
                    tasks::set_hold(tx, id, &mark, now).await?;
                    let event = NewEvent::new(types::TASK_HOLD_SET, actor.event_actor())?
                        .task(id).data(json!({"reason": mark.reason.as_str()}))?;
                    buffer.append(tx, &event, now).await?;
                }
            }
            #[cfg(feature = "failpoints")]
            fail::fail_point!("create_task_plan_in_transaction", |_| Err(crate::services::fired("create_task_plan_in_transaction").into()));
            let mut seeds: Vec<TaskId> = created.iter().map(|(_, id)| *id).collect();
            for (task, (_, id)) in plan.iter().zip(&created) {
                if let Some(parent) = &task.parent {
                    let parent = resolve(parent, &by_key)?;
                    let item = NewChecklistItem { content: ItemContent::Linked(*id), checklist: Some(ChecklistName::subtasks()) };
                    checklists::add_item_in_tx(tx, &mut buffer, actor, parent, &item, now).await?;
                    seeds.push(parent);
                }
                let mut seen = BTreeSet::new();
                for needs in &task.needs {
                    let needs = resolve(needs, &by_key)?;
                    if seen.insert(needs) {
                        add_edge_in_tx(tx, &mut buffer, *id, needs, DependencyOrigin::Direct, actor, now).await?;
                    }
                }
            }
            buffer.extend(recompute(tx, &seeds, now).await?.events);
            Ok::<_, TaskError>(created)
        }).await?;
        self.bus.publish(buffer);
        let mut views = Vec::with_capacity(created.len());
        for (key, id) in created {
            views.push((key, self.get(id).await?));
        }
        Ok(views)
    }
}

fn resolve(reference: &PlanRef, by_key: &BTreeMap<&str, TaskId>) -> Result<TaskId, TaskError> {
    match reference {
        PlanRef::Existing(id) => Ok(*id),
        PlanRef::Key(key) => by_key
            .get(key.as_str())
            .copied()
            .ok_or(StoreError::Corrupt("task_plan.key").into()),
    }
}
