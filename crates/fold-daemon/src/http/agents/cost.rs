use axum::Json;
use axum::extract::Path;
use fold_core::cost::{Amount, CostPart, TaskCostReport};
use fold_core::{TaskId, Timestamp};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::super::Project;
use super::super::error::{ApiError, ApiQuery, ErrorEnvelope};
use super::super::tasks::id;
use super::workers::{WorkerProcessState, WorkerRole};
use crate::daemon::AppState;
use crate::services::agents::ProjectCost;

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(project_cost))
        .routes(routes!(task_cost))
}

/// A time of a query string, in UTC milliseconds.
pub(crate) fn time_param(millis: i64) -> Result<Timestamp, ApiError> {
    Timestamp::from_millis(millis)
        .map_err(|_| ApiError::invalid_request("a time is UTC milliseconds within range"))
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CostQuery {
    /// UTC milliseconds: count only the turns that ended at or after it, such as the start of the viewer's day; every turn without it.
    pub since: Option<i64>,
}

/// A sum of turns.
#[derive(Debug, Serialize, ToSchema)]
pub struct CostAmount {
    /// Millionths of a US dollar, as Claude Code reported them.
    pub cost_micro_usd: u64,
    pub turns: u64,
    /// How many workers ran the turns.
    pub workers: u64,
}

impl From<Amount> for CostAmount {
    fn from(amount: Amount) -> Self {
        Self {
            cost_micro_usd: amount.cost_micro_usd,
            turns: amount.turns,
            workers: amount.workers,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RoleCost {
    pub role: WorkerRole,
    pub cost_micro_usd: u64,
    pub turns: u64,
    pub workers: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AreaCost {
    /// Null for the review lead and the reviewers, who review the work of every area.
    #[schema(required = true, example = "web")]
    pub area: Option<String>,
    pub cost_micro_usd: u64,
    pub turns: u64,
    pub workers: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelCost {
    /// Null for turns that ended before Claude Code named a model.
    #[schema(required = true, example = "claude-sonnet-5")]
    pub model: Option<String>,
    pub cost_micro_usd: u64,
    pub turns: u64,
    pub workers: u64,
}

/// One worker's turns in the range. A stopped, failed or restarted worker keeps its turns.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkerCost {
    #[schema(example = "web-worker")]
    pub name: String,
    pub role: WorkerRole,
    /// Null for the review lead and reviewers.
    #[schema(required = true)]
    pub area: Option<String>,
    pub state: WorkerProcessState,
    /// The model of the worker's newest turn in the range.
    #[schema(required = true)]
    pub model: Option<String>,
    pub turns: u64,
    pub cost_micro_usd: u64,
}

/// What one task cost in the range: `build` is its task workers' turns, `review` the turns of the reviewers of its proposals.
#[derive(Debug, Serialize, ToSchema)]
pub struct TaskCost {
    #[schema(example = "T-12")]
    pub task: String,
    pub title: String,
    /// The area of the task workers that built it; null while only reviewers worked on it.
    #[schema(required = true)]
    pub area: Option<String>,
    pub build_micro_usd: u64,
    pub review_micro_usd: u64,
    pub total_micro_usd: u64,
    pub turns: u64,
}

/// The cost of one role's turns in one slot of `slot_minutes`.
#[derive(Debug, Serialize, ToSchema)]
pub struct CostSlot {
    /// UTC milliseconds of the start of the slot.
    #[schema(value_type = i64)]
    pub start: Timestamp,
    pub role: WorkerRole,
    pub cost_micro_usd: u64,
}

/// What the project's supervised workers cost since `since`, from the values Claude Code reported for each turn; fold keeps no price table. The main agent is the user's own Claude Code session, which fold does not supervise, so its cost is not here.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProjectCostView {
    /// The `since` of the request; null for all time.
    #[schema(value_type = Option<i64>, required = true)]
    pub since: Option<Timestamp>,
    pub total: CostAmount,
    /// UTC milliseconds of the end of the oldest turn in the range; null without a turn.
    #[schema(value_type = Option<i64>, required = true)]
    pub first_turn_at: Option<Timestamp>,
    /// Every role, a role without a turn with 0.
    pub by_role: Vec<RoleCost>,
    /// The most expensive first.
    pub by_area: Vec<AreaCost>,
    /// The most expensive first.
    pub by_model: Vec<ModelCost>,
    /// The most expensive first.
    pub workers: Vec<WorkerCost>,
    /// Every task a turn in the range counts for, the most expensive first.
    pub tasks: Vec<TaskCost>,
    /// Oldest first; a slot without a turn is left out. Every time zone's offset is a whole number of slots, so a viewer adds them up into its own hours and days.
    pub slots: Vec<CostSlot>,
    #[schema(example = 15)]
    pub slot_minutes: u32,
}

impl From<ProjectCost> for ProjectCostView {
    fn from(cost: ProjectCost) -> Self {
        let ProjectCost {
            since,
            report,
            slots,
        } = cost;
        let name = |area: Option<fold_core::AreaName>| area.map(|area| area.to_string());
        Self {
            since,
            total: report.total.into(),
            first_turn_at: report.first_turn_at,
            by_role: report
                .by_role
                .into_iter()
                .map(|(role, amount)| RoleCost {
                    role: role.into(),
                    cost_micro_usd: amount.cost_micro_usd,
                    turns: amount.turns,
                    workers: amount.workers,
                })
                .collect(),
            by_area: report
                .by_area
                .into_iter()
                .map(|(area, amount)| AreaCost {
                    area: name(area),
                    cost_micro_usd: amount.cost_micro_usd,
                    turns: amount.turns,
                    workers: amount.workers,
                })
                .collect(),
            by_model: report
                .by_model
                .into_iter()
                .map(|(model, amount)| ModelCost {
                    model,
                    cost_micro_usd: amount.cost_micro_usd,
                    turns: amount.turns,
                    workers: amount.workers,
                })
                .collect(),
            workers: report
                .workers
                .into_iter()
                .map(|worker| WorkerCost {
                    name: worker.agent.to_string(),
                    role: worker.role.into(),
                    area: name(worker.area),
                    state: worker.state.into(),
                    model: worker.model,
                    turns: worker.turns,
                    cost_micro_usd: worker.cost_micro_usd,
                })
                .collect(),
            tasks: report
                .tasks
                .into_iter()
                .map(|task| TaskCost {
                    total_micro_usd: task.total_micro_usd(),
                    task: task.task.to_string(),
                    title: task.title,
                    area: name(task.area),
                    build_micro_usd: task.build_micro_usd,
                    review_micro_usd: task.review_micro_usd,
                    turns: task.turns,
                })
                .collect(),
            slots: slots
                .into_iter()
                .map(|slot| CostSlot {
                    start: slot.start,
                    role: slot.role.into(),
                    cost_micro_usd: slot.cost_micro_usd,
                })
                .collect(),
            slot_minutes: u32::try_from(fold_store::agents::SLOT_MILLIS / 60_000).unwrap_or(15),
        }
    }
}

/// What the project's supervised workers cost: the total, the splits by role, area, model, worker and task, and the cost by slot and role.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/cost",
    operation_id = "project_cost",
    params(("slug" = String, Path, description = "The project slug"), CostQuery),
    responses(
        (status = 200, description = "The project's cost", body = ProjectCostView),
        (status = 400, description = "`invalid_request`: `since` is not a time", body = ErrorEnvelope),
        (status = 404, description = "`project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn project_cost(
    project: Project,
    ApiQuery(query): ApiQuery<CostQuery>,
) -> Result<Json<ProjectCostView>, ApiError> {
    let since = query.since.map(time_param).transpose()?;
    let cost = project.0.agents().project_cost(since).await?;
    Ok(Json(cost.into()))
}

/// Which part of a task's cost a worker's turns are: `build` for a task worker's, `review` for a reviewer's.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CostPartKind {
    Build,
    Review,
}

/// One worker's turns for the task.
#[derive(Debug, Serialize, ToSchema)]
pub struct TaskWorkerCost {
    #[schema(example = "web-worker")]
    pub name: String,
    pub role: WorkerRole,
    pub part: CostPartKind,
    /// The model of the worker's newest turn for the task.
    #[schema(required = true)]
    pub model: Option<String>,
    /// The proposals of the task a reviewer reviewed, such as `P-3`, in number order; empty for a build.
    pub proposals: Vec<String>,
    pub turns: u64,
    pub cost_micro_usd: u64,
}

/// What one task cost in all: its task workers' turns and the turns of the reviewers of its proposals, from the values Claude Code reported.
#[derive(Debug, Serialize, ToSchema)]
pub struct TaskCostView {
    #[schema(example = "T-12")]
    pub task: String,
    pub build_micro_usd: u64,
    pub review_micro_usd: u64,
    pub total_micro_usd: u64,
    pub turns: u64,
    /// The builders first, then the reviewers, each in the order they first worked on the task.
    pub workers: Vec<TaskWorkerCost>,
}

fn task_cost_view(task: TaskId, report: TaskCostReport) -> TaskCostView {
    TaskCostView {
        task: task.to_string(),
        build_micro_usd: report.build_micro_usd,
        review_micro_usd: report.review_micro_usd,
        total_micro_usd: report
            .build_micro_usd
            .saturating_add(report.review_micro_usd),
        turns: report.turns,
        workers: report
            .workers
            .into_iter()
            .map(|row| TaskWorkerCost {
                name: row.agent.to_string(),
                role: row.role.into(),
                part: match row.part {
                    CostPart::Build => CostPartKind::Build,
                    CostPart::Review => CostPartKind::Review,
                },
                model: row.model,
                proposals: row.proposals.iter().map(ToString::to_string).collect(),
                turns: row.turns,
                cost_micro_usd: row.cost_micro_usd,
            })
            .collect(),
    }
}

/// What one task cost, with one row per worker that built it or reviewed one of its proposals.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{slug}/cost/tasks/{id}",
    operation_id = "task_cost",
    params(
        ("slug" = String, Path, description = "The project slug"),
        ("id" = String, Path, description = "The task id, such as T-12"),
    ),
    responses(
        (status = 200, description = "The task's cost", body = TaskCostView),
        (status = 400, description = "`invalid_id`", body = ErrorEnvelope),
        (status = 404, description = "`task_not_found` or `project_not_found`", body = ErrorEnvelope),
        (status = 503, description = "`project_unavailable`", body = ErrorEnvelope),
        (status = 500, description = "Internal error", body = ErrorEnvelope),
    )
)]
pub async fn task_cost(
    project: Project,
    Path((_slug, task)): Path<(String, String)>,
) -> Result<Json<TaskCostView>, ApiError> {
    let task: TaskId = id(&task)?;
    let report = project.0.agents().task_cost(task).await?;
    Ok(Json(task_cost_view(task, report)))
}
