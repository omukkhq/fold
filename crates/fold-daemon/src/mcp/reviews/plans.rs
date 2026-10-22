use fold_core::reviews::plans::{MAX_PLAN_TASKS, PlanState};
use fold_core::{Actor, FoldError, ReviewPlanId, TaskId};
use rmcp::handler::server::common::schema_for_type;
use rmcp::model::{JsonObject, Tool};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::error::from_review;
use super::super::parse;
use super::{id, proposal_id, version};
use crate::services::reviews::{ProposalPlan, ReviewPlanView, ReviewService};

pub const CREATE_REVIEW_PLAN: &str = "create_review_plan";
pub const CHANGE_REVIEW_PLAN: &str = "change_review_plan";
pub const GET_REVIEW_PLAN: &str = "get_review_plan";
pub const LIST_REVIEW_PLANS: &str = "list_review_plans";

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateReviewPlanInput {
    /// The open proposals that wait, such as ["P-3"]: the new proposal, and any open proposal of a related task that nobody reviewed or claimed yet.
    pub proposals: Vec<String>,
    /// The related tasks to wait for, such as ["T-7", "T-8"]: To do or In progress tasks whose proposals belong in the same batch. Together with the proposals' tasks at most 5.
    pub tasks: Vec<String>,
    /// The worker that makes this call, such as review/plan. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ChangeReviewPlanInput {
    /// The review plan, such as rp-2.
    pub id: String,
    /// Open proposals without a review to add, such as ["P-9"].
    #[serde(default)]
    pub add_proposals: Vec<String>,
    /// Related tasks to wait for too, such as ["T-9"]; only before the plan's wait ended.
    #[serde(default)]
    pub add_tasks: Vec<String>,
    /// Tasks to take out of the plan, awaited or with their proposal, such as ["T-8"]. A removed proposal is reviewed on its own.
    #[serde(default)]
    pub remove: Vec<String>,
    /// The worker that makes this call, such as review/plan. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReviewPlanIdInput {
    /// The review plan, such as rp-2.
    pub id: String,
    /// The worker that makes this call, such as review/plan. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListReviewPlansInput {
    /// Only the plans that are open: waiting for tasks or for their start.
    #[serde(default)]
    pub open: bool,
    /// The worker that makes this call, such as review/plan. Leave it out to act as the main worker.
    #[serde(rename = "as", default)]
    pub as_worker: Option<String>,
}

pub fn tools() -> Vec<Tool> {
    vec![
        Tool::new(
            CREATE_REVIEW_PLAN,
            "Review lead only: when a proposal opens and related To do or In progress tasks will open proposals that belong in the same review, make a plan that waits for them, so one reviewer reviews all of them as one batch. At most 5 tasks, the proposals' tasks included. The wait ends an hour after the plan is made. fold adds each awaited task's proposal when it opens and tells you when it joins, when a member leaves (such as at a verdict or a review claim), when every awaited task opened one, when a task stops being eligible and leaves, and when the wait ends; then call start_worker with role reviewer and plan. Refusals: review_plan_too_large, proposal_not_plannable (a verdict on any version, a review claim, a batch or the human review mark: review it alone), task_not_awaitable (details.reason, such as blocker, backlog or depends_on_member: a task that needs a member's task would wait for the plan itself), main_area, already_in_plan and review_plan_denied.",
            schema_for_type::<CreateReviewPlanInput>(),
        ),
        Tool::new(
            CHANGE_REVIEW_PLAN,
            "Review lead only: add open proposals without a review or awaited tasks to an open plan, or take tasks out of it. The deadline stays. A plan left without members ends. Refusals as for create_review_plan, and review_plan_not_open.",
            schema_for_type::<ChangeReviewPlanInput>(),
        ),
        Tool::new(
            GET_REVIEW_PLAN,
            "Read one review plan: its open proposals, the tasks it still waits for with their status, task worker and area leads, its deadline, and after its start the reviewer and the batch.",
            schema_for_type::<ReviewPlanIdInput>(),
        ),
        Tool::new(
            LIST_REVIEW_PLANS,
            "List review plans with their state, proposals and awaited tasks.",
            schema_for_type::<ListReviewPlansInput>(),
        ),
    ]
}

/// Runs a plan tool. `None` when `tool` is not one.
pub async fn run(
    reviews: &ReviewService,
    tool: &str,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Option<Value>, FoldError> {
    let value = match tool {
        CREATE_REVIEW_PLAN => create_review_plan(reviews, actor, arguments).await?,
        CHANGE_REVIEW_PLAN => change_review_plan(reviews, actor, arguments).await?,
        GET_REVIEW_PLAN => {
            let input: ReviewPlanIdInput = parse(arguments)?;
            let plan = reviews
                .get_review_plan(plan_id(&input.id)?)
                .await
                .map_err(from_review)?;
            plan_json(&plan)
        }
        LIST_REVIEW_PLANS => {
            let input: ListReviewPlansInput = parse(arguments)?;
            let plans = reviews
                .list_review_plans(input.open)
                .await
                .map_err(from_review)?;
            json!({"plans": plans.iter().map(plan_json).collect::<Vec<_>>()})
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

fn plan_id(text: &str) -> Result<ReviewPlanId, FoldError> {
    id(
        text,
        "pass a review plan id such as rp-2; call list_review_plans to see them",
    )
}

fn task_ids(texts: &[String]) -> Result<Vec<TaskId>, FoldError> {
    texts
        .iter()
        .map(|text| id(text, "pass task ids such as T-12"))
        .collect()
}

async fn create_review_plan(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: CreateReviewPlanInput = parse(arguments)?;
    let proposals = input
        .proposals
        .iter()
        .map(|text| proposal_id(text))
        .collect::<Result<Vec<_>, _>>()?;
    let plan = reviews
        .create_review_plan(actor, proposals, task_ids(&input.tasks)?)
        .await
        .map_err(from_review)?;
    Ok(plan_json(&plan))
}

async fn change_review_plan(
    reviews: &ReviewService,
    actor: &Actor,
    arguments: &JsonObject,
) -> Result<Value, FoldError> {
    let input: ChangeReviewPlanInput = parse(arguments)?;
    let add_proposals = input
        .add_proposals
        .iter()
        .map(|text| proposal_id(text))
        .collect::<Result<Vec<_>, _>>()?;
    let plan = reviews
        .change_review_plan(
            actor,
            plan_id(&input.id)?,
            add_proposals,
            task_ids(&input.add_tasks)?,
            task_ids(&input.remove)?,
        )
        .await
        .map_err(from_review)?;
    Ok(plan_json(&plan))
}

/// What the review lead does next with an open plan.
fn next_step(view: &ReviewPlanView) -> Option<String> {
    let id = view.plan.id;
    if view.plan.state != PlanState::Open {
        return None;
    }
    let idle: Vec<String> = view
        .awaited
        .iter()
        .filter(|task| task.worker.is_none())
        .map(|task| task.task.to_string())
        .collect();
    Some(if view.awaited.is_empty() {
        format!(
            "no task is awaited: start one reviewer with start_worker, role reviewer and plan {id}"
        )
    } else if idle.is_empty() {
        format!(
            "fold tells you when each awaited task opens its proposal and when the wait ends; then call start_worker with plan {id}"
        )
    } else {
        format!(
            "no task worker works on {} yet: ask the area lead in area_leads with send_message to start it next; fold tells you when each awaited task opens its proposal",
            idle.join(", ")
        )
    })
}

pub(super) fn plan_json(view: &ReviewPlanView) -> Value {
    let plan = &view.plan;
    let awaited: Vec<Value> = view
        .awaited
        .iter()
        .map(|task| {
            json!({
                "task": task.task,
                "status": task.status.as_str(),
                "worker": task.worker.as_ref().map(|worker| worker.as_str()),
                "area_leads": task.area_leads.iter().map(|lead| lead.as_str()).collect::<Vec<_>>(),
            })
        })
        .collect();
    let members: Vec<Value> = view
        .members
        .iter()
        .map(|member| {
            json!({
                "task": member.task,
                "proposal": member.proposal,
                "version": member.version.map(version),
                "in_plan": member.is_live(),
                "left": member.release.map(|release| release.as_str()),
            })
        })
        .collect();
    let mut value = json!({
        "id": plan.id,
        "lead": plan.lead.as_str(),
        "state": plan.state.as_str(),
        "created_at": plan.created_at.as_millis(),
        "deadline_at": plan.deadline_at.as_millis(),
        "wait_ended": plan.wait_ended_at.is_some(),
        "max_tasks": MAX_PLAN_TASKS,
        "proposals": view.proposals(),
        "awaited": awaited,
        "members": members,
        "reviewer": plan.reviewer.as_ref().map(|agent| agent.as_str()),
        "batch": plan.batch,
    });
    if let Some(next) = next_step(view) {
        value["next"] = json!(next);
    }
    value
}

/// The plan a proposal waits in, as `get_proposal` shows it.
pub(super) fn proposal_plan_json(plan: &ProposalPlan) -> Value {
    json!({
        "id": plan.plan,
        "deadline_at": plan.deadline_at.as_millis(),
        "wait_ended": plan.wait_ended,
        "awaited": plan.awaited,
        "proposals": plan.proposals,
    })
}
