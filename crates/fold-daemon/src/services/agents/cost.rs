use fold_core::cost::{self, CostReport, TaskCostReport};
use fold_core::{TaskId, Timestamp};
use fold_store::StoreError;
use fold_store::agents::{self, CostSlot, CostTotals};

use super::{AgentError, AgentService};
use crate::services::tasks::task_missing;

/// The days before today that the week's total adds: today and the six days before it.
const WEEK_DAYS_BEFORE: i64 = 6;

const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// What the project's supervised workers cost since a time, with the cost by slot and role. The main agent is the user's own Claude Code session, which fold does not supervise, so none of its turns are here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCost {
    pub since: Option<Timestamp>,
    pub report: CostReport,
    pub slots: Vec<CostSlot>,
    pub slot_millis: i64,
}

impl AgentService {
    /// The cost of the turns that ended at or after `since`, of every turn without it. Costs are Claude Code's reported values only.
    pub async fn project_cost(&self, since: Option<Timestamp>) -> Result<ProjectCost, AgentError> {
        let pool = self.store.read();
        let slot_millis = cost::slot_millis(since, self.clock.now());
        let groups = agents::cost_groups(pool, since, None).await?;
        let slots = agents::cost_slots(pool, since, slot_millis).await?;
        Ok(ProjectCost {
            since,
            report: cost::report(&groups),
            slots,
            slot_millis,
        })
    }

    /// What one task cost: the turns of its task workers and of the reviewers of its proposals. An unknown task is `task_not_found`.
    pub async fn task_cost(&self, task: TaskId) -> Result<TaskCostReport, AgentError> {
        let pool = self.store.read();
        if fold_store::tasks::get_by_number(pool, task)
            .await?
            .is_none()
        {
            return Err(task_missing(task).into());
        }
        let groups = agents::cost_groups(pool, None, Some(task)).await?;
        Ok(cost::task_report(&groups))
    }
}

/// The totals of every turn, of the turns since `today` and of those since `week`. Without `today`, the day starts at midnight UTC of `now`; without `week`, the week starts six days before the day.
pub async fn cost_totals(
    store: &fold_store::Store,
    today: Option<Timestamp>,
    week: Option<Timestamp>,
    now: Timestamp,
) -> Result<CostTotals, StoreError> {
    let today = today.unwrap_or_else(|| {
        let millis = now.as_millis();
        Timestamp::from_millis(millis - millis.rem_euclid(DAY_MILLIS)).unwrap_or(now)
    });
    let week = week.unwrap_or_else(|| {
        Timestamp::from_millis(
            today
                .as_millis()
                .saturating_sub(WEEK_DAYS_BEFORE * DAY_MILLIS),
        )
        .unwrap_or(today)
    });
    agents::cost_totals(store.read(), today, week).await
}
