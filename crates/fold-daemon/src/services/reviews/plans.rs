use std::collections::{BTreeMap, BTreeSet};

use fold_core::dag::Dag;
use fold_core::events::types;
use fold_core::reviews::plans::{
    AwaitFacts, MAX_PLAN_TASKS, PlanProposalFacts, PlanRelease, PlanState, await_issue,
    depends_on_member, plan_deadline, proposal_issue,
};
use fold_core::{
    Actor, AgentName, EventActor, FoldError, IdKind, NewEvent, ProposalId, ReviewBatchId,
    ReviewPlanId, TaskId, TaskStatus, Timestamp, WorkerName, codes, policy,
};
use fold_store::reviews::batches;
use fold_store::reviews::plans::{self as store, LiveMember, PlanMemberRow, PlanRow};
use fold_store::reviews::{self, ProposalRow, ProposalState};
use fold_store::tasks::claims;
use fold_store::{SqliteConnection, StoreError, WriteTx, agents, counters, tasks};
use serde_json::{Value, json};

use super::proposals::require_open;
use super::{ReviewError, ReviewService};
use crate::services::agents::starter_of;
use crate::services::events::EventBuffer;
use crate::services::tasks::{TaskError, task_in_main_area, task_not_found};

/// A review plan as callers see it: the plan, every member it has had, and the tasks it still waits for with who works on each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPlanView {
    pub plan: PlanRow,
    pub members: Vec<PlanMemberRow>,
    pub awaited: Vec<AwaitedTask>,
}

impl ReviewPlanView {
    /// The open proposals in the plan, in the order they were added.
    pub fn proposals(&self) -> Vec<ProposalId> {
        self.members
            .iter()
            .filter(|member| member.is_live())
            .filter_map(|member| member.proposal)
            .collect()
    }
}

/// A task the plan waits for, with the task worker that holds its claim and the leads of its area, whom the review lead asks to start a task that nobody works on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwaitedTask {
    pub task: TaskId,
    pub status: TaskStatus,
    pub worker: Option<WorkerName>,
    pub area_leads: Vec<AgentName>,
}

/// The plan an open proposal waits in, as its proposal view and its board card show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalPlan {
    pub plan: ReviewPlanId,
    pub deadline_at: Timestamp,
    /// The wait ended at the deadline, so the plan waits only for the review lead to start it.
    pub wait_ended: bool,
    pub awaited: Vec<TaskId>,
    /// Every open proposal of the plan, this one included.
    pub proposals: Vec<ProposalId>,
}

/// The plan each live member proposal of an open plan waits in, read in one go for a board.
pub async fn proposal_plans(
    conn: &mut SqliteConnection,
) -> Result<BTreeMap<ProposalId, ProposalPlan>, StoreError> {
    let live = store::live_members(&mut *conn).await?;
    if live.is_empty() {
        return Ok(BTreeMap::new());
    }
    let plans: BTreeMap<ReviewPlanId, PlanRow> = store::list_plans(&mut *conn, true)
        .await?
        .into_iter()
        .map(|plan| (plan.id, plan))
        .collect();
    let mut grouped: BTreeMap<ReviewPlanId, Vec<LiveMember>> = BTreeMap::new();
    for member in live {
        grouped.entry(member.plan).or_default().push(member);
    }
    let mut found = BTreeMap::new();
    for (id, members) in grouped {
        let Some(plan) = plans.get(&id) else {
            continue;
        };
        let summary = ProposalPlan {
            plan: id,
            deadline_at: plan.deadline_at,
            wait_ended: plan.wait_ended_at.is_some(),
            awaited: members
                .iter()
                .filter(|member| member.proposal.is_none())
                .map(|member| member.task)
                .collect(),
            proposals: members
                .iter()
                .filter_map(|member| member.proposal)
                .collect(),
        };
        for proposal in &summary.proposals {
            found.insert(*proposal, summary.clone());
        }
    }
    Ok(found)
}

/// The plan the open proposal waits in, or `None`.
pub async fn proposal_plan(
    conn: &mut SqliteConnection,
    proposal: ProposalId,
) -> Result<Option<ProposalPlan>, StoreError> {
    if store::live_plan_of_proposal(&mut *conn, proposal)
        .await?
        .is_none()
    {
        return Ok(None);
    }
    Ok(proposal_plans(conn).await?.remove(&proposal))
}

/// Refuses to give a reviewer `proposal` on its own while it waits in a plan, inside the caller's write: the plan's start gives it to a reviewer with the others. The start of the plan takes its members out first, so it passes.
pub(crate) async fn refuse_planned(
    tx: &mut WriteTx,
    proposal: ProposalId,
) -> Result<(), ReviewError> {
    let Some(plan) = store::live_plan_of_proposal(tx.conn(), proposal).await? else {
        return Ok(());
    };
    Err(FoldError::new(
        codes::ALREADY_IN_PLAN,
        format!("{proposal} waits in {plan} to be reviewed with the plan's other proposals"),
    )
    .with_details([
        ("proposal", json!(proposal.to_string())),
        ("plan", json!(plan.to_string())),
    ])
    .with_hint(format!(
        "start the reviewer with start_worker and plan {plan}, or take the proposal's task out with change_review_plan"
    ))
    .into())
}

/// Takes the open members of `plan` out of it for its start with `start_worker`, inside the caller's write, and returns its open proposals: those members leave as `started`, and the tasks it still waits for leave as `not_opened`, so their proposals are reviewed on their own later. A plan that is not open, or has no open proposal yet, refuses the start.
pub(crate) async fn take_plan_for_start(
    tx: &mut WriteTx,
    plan: ReviewPlanId,
    now: Timestamp,
) -> Result<PlanStart, ReviewError> {
    let row = open_plan(tx, plan).await?;
    let members = store::members(tx.conn(), plan).await?;
    let proposals: Vec<(TaskId, ProposalId)> = members
        .iter()
        .filter(|member| member.is_live())
        .filter_map(|member| member.proposal.map(|proposal| (member.task, proposal)))
        .collect();
    if proposals.is_empty() {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("{plan} has no open proposal yet"),
        )
        .with_details([("plan", json!(plan.to_string()))])
        .with_hint("wait for its awaited tasks to open their proposals; read the plan with get_review_plan")
        .into());
    }
    let mut not_opened = Vec::new();
    for member in members.iter().filter(|member| member.is_awaited()) {
        store::release_member(tx, plan, member.task, PlanRelease::NotOpened, now).await?;
        not_opened.push(member.task);
    }
    for (task, _) in &proposals {
        store::release_member(tx, plan, *task, PlanRelease::Started, now).await?;
    }
    Ok(PlanStart {
        plan: row.id,
        proposals: proposals
            .into_iter()
            .map(|(_, proposal)| proposal)
            .collect(),
        not_opened,
    })
}

/// What [`take_plan_for_start`] took from a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanStart {
    pub plan: ReviewPlanId,
    pub proposals: Vec<ProposalId>,
    pub not_opened: Vec<TaskId>,
}

/// Records the start of a plan whose members [`take_plan_for_start`] took, inside the same write, once the reviewer `reviewer` holds their reviews: `batch` is the batch they became, `None` for one proposal reviewed alone. Appends `review.plan_started`.
pub(crate) async fn mark_plan_started(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    by: &EventActor,
    start: &PlanStart,
    reviewer: &AgentName,
    batch: Option<ReviewBatchId>,
    now: Timestamp,
) -> Result<(), ReviewError> {
    if !store::mark_started(tx, start.plan, reviewer, batch, now).await? {
        return Err(StoreError::Corrupt("review_plans.state").into());
    }
    let event = NewEvent::new(types::REVIEW_PLAN_STARTED, by.clone())?.data(json!({
        "plan": start.plan.to_string(),
        "reviewer": reviewer.as_str(),
        "batch": batch.map(|batch| batch.to_string()),
        "proposals": ids(&start.proposals),
        "not_opened": ids(&start.not_opened),
    }))?;
    buffer.append(tx, &event, now).await?;
    Ok(())
}

/// Keeps every open plan true to its members inside the caller's write; every command that changes a fact a member's eligibility reads runs it, and `recompute` runs it at its end. An awaited task whose proposal opened joins the plan with that version; an awaited task that stopped being eligible, by the rule that refuses it at the plan's creation, leaves with that reason; a joined proposal that closed, or whose task got the human review mark or came into main, leaves. Each change appends `review.plan_joined` or `review.plan_left` by fold with the plan's open proposals and awaited tasks after it, and a plan with no member left ends with `review.plan_ended`.
pub(crate) async fn settle_plans(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    now: Timestamp,
) -> Result<(), TaskError> {
    let live = store::live_members(tx.conn()).await?;
    if live.is_empty() {
        return Ok(());
    }
    let mut grouped: BTreeMap<ReviewPlanId, Vec<LiveMember>> = BTreeMap::new();
    for member in live {
        grouped.entry(member.plan).or_default().push(member);
    }
    for (plan, members) in grouped {
        settle_plan(tx, buffer, plan, &members, now).await?;
    }
    Ok(())
}

/// The open proposals and awaited tasks of one plan while [`settle_plans`] changes it.
struct Standing {
    awaited: BTreeSet<TaskId>,
    joined: BTreeMap<TaskId, ProposalId>,
}

impl Standing {
    fn data(&self) -> (Value, Value) {
        (
            json!(ids(&self.joined.values().copied().collect::<Vec<_>>())),
            json!(ids(&self.awaited.iter().copied().collect::<Vec<_>>())),
        )
    }
}

async fn settle_plan(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    plan: ReviewPlanId,
    members: &[LiveMember],
    now: Timestamp,
) -> Result<(), TaskError> {
    let mut standing = Standing {
        awaited: members
            .iter()
            .filter(|member| member.proposal.is_none())
            .map(|member| member.task)
            .collect(),
        joined: members
            .iter()
            .filter_map(|member| member.proposal.map(|proposal| (member.task, proposal)))
            .collect(),
    };
    let tasks_of_plan: Vec<TaskId> = members.iter().map(|member| member.task).collect();
    let dag = if standing.awaited.is_empty() {
        Dag::default()
    } else {
        Dag::from_edges(tasks::dependent_edges(tx.conn(), &tasks_of_plan).await?)
    };
    let mut changed = false;
    for member in members {
        let outcome = match member.proposal {
            None => settle_awaited(tx, member.task, &dag, &tasks_of_plan).await?,
            Some(proposal) => settle_joined(tx, member.task, proposal).await?,
        };
        let event = match outcome {
            Settled::Stays => continue,
            Settled::Joins(row) => {
                store::join_member(tx, plan, member.task, row.id, row.current_version, now).await?;
                standing.awaited.remove(&member.task);
                standing.joined.insert(member.task, row.id);
                let (proposals, awaited) = standing.data();
                NewEvent::new(types::REVIEW_PLAN_JOINED, EventActor::fold())?
                    .task(member.task)
                    .proposal(row.id)
                    .data(json!({
                        "plan": plan.to_string(),
                        "task": member.task.to_string(),
                        "proposal": row.id.to_string(),
                        "version": row.current_version,
                        "proposals": proposals,
                        "awaited": awaited,
                    }))?
            }
            Settled::Leaves(reason) => {
                store::release_member(tx, plan, member.task, reason, now).await?;
                standing.awaited.remove(&member.task);
                standing.joined.remove(&member.task);
                let (proposals, awaited) = standing.data();
                let mut event =
                    NewEvent::new(types::REVIEW_PLAN_LEFT, EventActor::fold())?.task(member.task);
                if let Some(proposal) = member.proposal {
                    event = event.proposal(proposal);
                }
                event.data(json!({
                    "plan": plan.to_string(),
                    "task": member.task.to_string(),
                    "proposal": member.proposal.map(|proposal| proposal.to_string()),
                    "reason": reason.as_str(),
                    "proposals": proposals,
                    "awaited": awaited,
                }))?
            }
        };
        changed = true;
        buffer.append(tx, &event, now).await?;
    }
    if changed && standing.awaited.is_empty() && standing.joined.is_empty() {
        end_plan(tx, buffer, &EventActor::fold(), plan, "no_member", now).await?;
    }
    Ok(())
}

enum Settled {
    Stays,
    Joins(Box<ProposalRow>),
    Leaves(PlanRelease),
}

async fn settle_awaited(
    tx: &mut WriteTx,
    task: TaskId,
    dag: &Dag,
    tasks_of_plan: &[TaskId],
) -> Result<Settled, TaskError> {
    if let Some(proposal) = reviews::open_for_task(tx.conn(), task).await?
        && let Some(row) = reviews::get_proposal(tx.conn(), proposal).await?
    {
        if row.human_review.is_some() {
            let main = task_in_main_area(tx.conn(), task).await?;
            return Ok(Settled::Leaves(if main {
                PlanRelease::MainArea
            } else {
                PlanRelease::HumanReview
            }));
        }
        return Ok(Settled::Joins(Box::new(row)));
    }
    let row = tasks::fetch(tx.conn(), task)
        .await?
        .ok_or_else(|| task_not_found(task))?;
    let facts = AwaitFacts {
        status: row.status,
        kind: row.kind,
        archived: row.archived_at.is_some(),
        human_review: row.human_review.is_some(),
        main_area: task_in_main_area(tx.conn(), task).await?,
        blocker: row.hold.is_some(),
        depends_on_member: depends_on_member(dag, task, tasks_of_plan),
    };
    Ok(match await_issue(&facts) {
        Some(issue) => Settled::Leaves(issue.into()),
        None => Settled::Stays,
    })
}

async fn settle_joined(
    tx: &mut WriteTx,
    task: TaskId,
    proposal: ProposalId,
) -> Result<Settled, TaskError> {
    let row = reviews::get_proposal(tx.conn(), proposal).await?;
    let Some(row) = row.filter(|row| row.state == ProposalState::Open) else {
        return Ok(Settled::Leaves(PlanRelease::Closed));
    };
    if task_in_main_area(tx.conn(), task).await? {
        return Ok(Settled::Leaves(PlanRelease::MainArea));
    }
    if row.human_review.is_some() {
        return Ok(Settled::Leaves(PlanRelease::HumanReview));
    }
    Ok(Settled::Stays)
}

async fn end_plan(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    by: &EventActor,
    plan: ReviewPlanId,
    reason: &str,
    now: Timestamp,
) -> Result<(), TaskError> {
    if !store::mark_ended(tx, plan, now).await? {
        return Ok(());
    }
    let event = NewEvent::new(types::REVIEW_PLAN_ENDED, by.clone())?
        .data(json!({"plan": plan.to_string(), "reason": reason}))?;
    buffer.append(tx, &event, now).await?;
    Ok(())
}

fn ids<T: ToString>(items: &[T]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

fn plan_missing(plan: ReviewPlanId) -> ReviewError {
    FoldError::new(
        codes::REVIEW_PLAN_NOT_FOUND,
        format!("{plan} does not exist"),
    )
    .with_details([("plan", json!(plan.to_string()))])
    .with_hint("call list_review_plans to see the plans")
    .into()
}

/// The plan, refused with `review_plan_not_open` once it started or ended.
async fn open_plan(tx: &mut WriteTx, plan: ReviewPlanId) -> Result<PlanRow, ReviewError> {
    let row = store::get_plan(tx.conn(), plan)
        .await?
        .ok_or_else(|| plan_missing(plan))?;
    if row.state == PlanState::Open {
        return Ok(row);
    }
    Err(FoldError::new(
        codes::REVIEW_PLAN_NOT_OPEN,
        format!("{plan} is {}", row.state.as_str()),
    )
    .with_details([
        ("plan", json!(plan.to_string())),
        ("state", json!(row.state.as_str())),
    ])
    .with_hint("read it with get_review_plan; give proposals that still wait to a reviewer with start_worker")
    .into())
}

/// The review lead's agent, refused with `review_plan_denied` for every other actor.
async fn plan_lead(tx: &mut WriteTx, actor: &Actor) -> Result<AgentName, ReviewError> {
    let starter = starter_of(tx, actor).await?;
    policy::can_plan_reviews(&starter).map_err(FoldError::from)?;
    match actor {
        Actor::Agent(agent) => Ok(agent.agent.clone()),
        // Only an agent is ever the review lead, so the policy refused everyone else.
        Actor::Human { .. } | Actor::System => {
            Err(StoreError::Corrupt("worker_processes.role").into())
        }
    }
}

fn too_large(count: usize) -> ReviewError {
    FoldError::new(
        codes::REVIEW_PLAN_TOO_LARGE,
        format!("a plan holds at most {MAX_PLAN_TASKS} tasks, the tasks of its proposals included; this one would hold {count}"),
    )
    .with_details([("tasks", json!(count)), ("max", json!(MAX_PLAN_TASKS))])
    .with_hint("keep the most closely related tasks; review the others alone or in another plan")
    .into()
}

fn named_once<T: Ord + Copy + ToString>(items: &[T], field: &str) -> Result<(), ReviewError> {
    let mut seen = BTreeSet::new();
    for item in items {
        if !seen.insert(*item) {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("{} is named twice", item.to_string()),
            )
            .with_details([("field", json!(field))])
            .with_hint("name each proposal and task once")
            .into());
        }
    }
    Ok(())
}

/// An open first version that may wait in a plan, by [`proposal_issue`], not of main and not in another plan.
async fn plannable(tx: &mut WriteTx, proposal: ProposalId) -> Result<ProposalRow, ReviewError> {
    let row = require_open(reviews::get_proposal(tx.conn(), proposal).await?, proposal)?;
    let main_task = task_in_main_area(tx.conn(), row.task).await?;
    policy::can_assign_review(main_task).map_err(|denied| {
        FoldError::from(denied).with_details([("proposal", json!(proposal.to_string()))])
    })?;
    if let Some(plan) = store::live_plan_of_proposal(tx.conn(), proposal).await? {
        return Err(in_plan(&format!("{proposal}"), plan));
    }
    let facts = PlanProposalFacts {
        current_version: row.current_version,
        reviewed: row.latest_verdict.is_some(),
        claimed: row.review_claim.is_some(),
        in_batch: batches::open_batch_of(tx.conn(), proposal).await?.is_some(),
        human_review: row.human_review.is_some(),
    };
    if let Some(issue) = proposal_issue(&facts) {
        return Err(FoldError::new(
            codes::PROPOSAL_NOT_PLANNABLE,
            format!("{proposal} {}", issue.message()),
        )
        .with_details([
            ("proposal", json!(proposal.to_string())),
            ("reason", json!(issue.as_str())),
        ])
        .with_hint("leave it out of the plan and give it to a reviewer alone with start_worker")
        .into());
    }
    Ok(row)
}

fn in_plan(what: &str, plan: ReviewPlanId) -> ReviewError {
    FoldError::new(codes::ALREADY_IN_PLAN, format!("{what} is in {plan}"))
        .with_details([("plan", json!(plan.to_string()))])
        .with_hint(format!(
            "read {plan} with get_review_plan, or take it out of that plan with change_review_plan first"
        ))
        .into()
}

/// A task a plan with the tasks `tasks_of_plan` may wait for: it exists, has no open proposal (that proposal goes in the plan instead), is in no other plan and passes [`await_issue`].
async fn awaitable(
    tx: &mut WriteTx,
    task: TaskId,
    tasks_of_plan: &[TaskId],
) -> Result<(), ReviewError> {
    let row = tasks::fetch(tx.conn(), task)
        .await?
        .ok_or_else(|| task_not_found(task))?;
    if let Some(proposal) = reviews::open_for_task(tx.conn(), task).await? {
        return Err(FoldError::new(
            codes::TASK_NOT_AWAITABLE,
            format!("{task} has the open proposal {proposal}"),
        )
        .with_details([
            ("task", json!(task.to_string())),
            ("reason", json!("proposal_open")),
        ])
        .with_hint(format!(
            "put {proposal} in the plan's proposals instead of waiting for {task}"
        ))
        .into());
    }
    if let Some(plan) = store::live_plan_of_task(tx.conn(), task).await? {
        return Err(in_plan(&format!("{task}"), plan));
    }
    let dag = Dag::from_edges(tasks::dependent_edges(tx.conn(), tasks_of_plan).await?);
    let facts = AwaitFacts {
        status: row.status,
        kind: row.kind,
        archived: row.archived_at.is_some(),
        human_review: row.human_review.is_some(),
        main_area: task_in_main_area(tx.conn(), task).await?,
        blocker: row.hold.is_some(),
        depends_on_member: depends_on_member(&dag, task, tasks_of_plan),
    };
    if let Some(issue) = await_issue(&facts) {
        return Err(FoldError::new(
            codes::TASK_NOT_AWAITABLE,
            format!("{task} {}", issue.message()),
        )
        .with_details([
            ("task", json!(task.to_string())),
            ("reason", json!(issue.as_str())),
        ])
        .with_hint(
            "leave the task out of the plan; its proposal is reviewed on its own once it opens",
        )
        .into());
    }
    Ok(())
}

/// Adds `task` to the plan inside the caller's write; the partial unique index decides a race with another plan.
async fn add_member(
    tx: &mut WriteTx,
    plan: ReviewPlanId,
    task: TaskId,
    joined: Option<(ProposalId, u32)>,
    now: Timestamp,
) -> Result<(), ReviewError> {
    match store::insert_member(tx, plan, task, joined, now).await {
        Ok(()) => Ok(()),
        Err(error) if error.unique_violation() => {
            let other = store::live_plan_of_task(tx.conn(), task)
                .await?
                .ok_or(error)?;
            Err(in_plan(&format!("{task}"), other))
        }
        Err(error) => Err(error.into()),
    }
}

/// Review plans: open first versions that wait up to an hour for related tasks to open their proposals, so the review lead gives them all to one reviewer as one batch.
impl ReviewService {
    /// Makes a plan of the review lead for the open first versions `proposals` that waits for the related `tasks`, in one transaction with `review.plan_created`. Its wait ends an hour after now. fold refuses a plan of more than five tasks (`review_plan_too_large`), a proposal that is not an unreviewed, unclaimed first version outside a batch and without the human review mark (`proposal_not_plannable`), a proposal of main (`main_area`), and a task the plan cannot wait for (`task_not_awaitable`): one with an open proposal, one that is not a To do or In progress agent task of kind proposal, one with the human review mark or a blocker, one of main, and one that needs the task of another member directly or not. A proposal or task in another live plan is `already_in_plan`.
    pub async fn create_review_plan(
        &self,
        actor: &Actor,
        proposals: Vec<ProposalId>,
        awaited: Vec<TaskId>,
    ) -> Result<ReviewPlanView, ReviewError> {
        if proposals.is_empty() {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                "a plan needs the open proposal that waits",
            )
            .with_details([("field", json!("proposals"))])
            .with_hint("pass the new proposal in proposals, such as [\"P-3\"]")
            .into());
        }
        if awaited.is_empty() {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                "a plan needs at least one task to wait for",
            )
            .with_details([("field", json!("tasks"))])
            .with_hint("pass the related tasks in tasks; with none, give the proposals to a reviewer at once with start_worker")
            .into());
        }
        named_once(&proposals, "proposals")?;
        named_once(&awaited, "tasks")?;
        let count = proposals.len() + awaited.len();
        if count > MAX_PLAN_TASKS {
            return Err(too_large(count));
        }
        let now = self.clock.now();
        let deadline = plan_deadline(now).ok_or(StoreError::Corrupt("review_plans.deadline_at"))?;
        let mut buffer = EventBuffer::new();
        let plan =
            self.store
                .write(async |tx| {
                    let lead = plan_lead(tx, actor).await?;
                    let mut rows = Vec::with_capacity(proposals.len());
                    for &proposal in &proposals {
                        rows.push(plannable(tx, proposal).await?);
                    }
                    let mut tasks_of_plan: Vec<TaskId> = rows.iter().map(|row| row.task).collect();
                    tasks_of_plan.extend(awaited.iter().copied());
                    named_once(&tasks_of_plan, "tasks")?;
                    for &task in &awaited {
                        awaitable(tx, task, &tasks_of_plan).await?;
                    }
                    let number = counters::allocate(tx, IdKind::ReviewPlan).await?;
                    let plan = ReviewPlanId::from_number(number)
                        .ok_or(StoreError::Corrupt("counters.next"))?;
                    store::insert_plan(tx, plan, &lead, now, deadline).await?;
                    for row in &rows {
                        add_member(tx, plan, row.task, Some((row.id, row.current_version)), now)
                            .await?;
                    }
                    for &task in &awaited {
                        add_member(tx, plan, task, None, now).await?;
                    }
                    let event = NewEvent::new(types::REVIEW_PLAN_CREATED, actor.event_actor())?
                        .data(json!({
                            "plan": plan.to_string(),
                            "lead": lead.as_str(),
                            "deadline_at": deadline.as_millis(),
                            "proposals": ids(&proposals),
                            "awaited": ids(&awaited),
                        }))?;
                    buffer.append(tx, &event, now).await?;
                    Ok::<_, ReviewError>(plan)
                })
                .await?;
        self.bus.publish(buffer);
        self.get_review_plan(plan).await
    }

    /// Changes an open plan of the review lead in one transaction with `review.plan_changed`: `remove` takes tasks out, awaited or joined, as `removed`, so a removed proposal is reviewed on its own; `add_proposals` and `add_tasks` add members by the rules of [`ReviewService::create_review_plan`], tasks only before the wait ended. The deadline stays. A plan left without members ends with `review.plan_ended`.
    pub async fn change_review_plan(
        &self,
        actor: &Actor,
        plan: ReviewPlanId,
        add_proposals: Vec<ProposalId>,
        add_tasks: Vec<TaskId>,
        remove: Vec<TaskId>,
    ) -> Result<ReviewPlanView, ReviewError> {
        if add_proposals.is_empty() && add_tasks.is_empty() && remove.is_empty() {
            return Err(
                FoldError::new(codes::INVALID_REQUEST, "the change names nothing")
                    .with_details([("field", json!("add_tasks"))])
                    .with_hint("pass add_proposals, add_tasks or remove")
                    .into(),
            );
        }
        named_once(&add_proposals, "add_proposals")?;
        let mut named: Vec<TaskId> = add_tasks.clone();
        named.extend(remove.iter().copied());
        named_once(&named, "tasks")?;
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                plan_lead(tx, actor).await?;
                let row = open_plan(tx, plan).await?;
                if !add_tasks.is_empty() && (row.wait_ended_at.is_some() || now >= row.deadline_at)
                {
                    return Err(FoldError::new(
                        codes::REVIEW_PLAN_NOT_OPEN,
                        format!("the wait of {plan} ended, so it takes no task to wait for"),
                    )
                    .with_details([("plan", json!(plan.to_string()))])
                    .with_hint(format!("start the plan with start_worker and plan {plan}"))
                    .into());
                }
                for &task in &remove {
                    if !store::release_member(tx, plan, task, PlanRelease::Removed, now).await? {
                        return Err(FoldError::new(
                            codes::INVALID_REQUEST,
                            format!("{task} is not in {plan}"),
                        )
                        .with_details([
                            ("field", json!("remove")),
                            ("task", json!(task.to_string())),
                        ])
                        .with_hint(format!("read {plan} with get_review_plan"))
                        .into());
                    }
                }
                let live: Vec<TaskId> = store::members(tx.conn(), plan)
                    .await?
                    .into_iter()
                    .filter(PlanMemberRow::is_live)
                    .map(|member| member.task)
                    .collect();
                let count = live.len() + add_proposals.len() + add_tasks.len();
                if count > MAX_PLAN_TASKS {
                    return Err(too_large(count));
                }
                let mut rows = Vec::with_capacity(add_proposals.len());
                for &proposal in &add_proposals {
                    rows.push(plannable(tx, proposal).await?);
                }
                let mut tasks_of_plan = live.clone();
                tasks_of_plan.extend(rows.iter().map(|row| row.task));
                tasks_of_plan.extend(add_tasks.iter().copied());
                named_once(&tasks_of_plan, "tasks")?;
                for &task in &add_tasks {
                    awaitable(tx, task, &tasks_of_plan).await?;
                }
                for row in &rows {
                    add_member(tx, plan, row.task, Some((row.id, row.current_version)), now)
                        .await?;
                }
                for &task in &add_tasks {
                    add_member(tx, plan, task, None, now).await?;
                }
                let event = NewEvent::new(types::REVIEW_PLAN_CHANGED, actor.event_actor())?.data(
                    json!({
                        "plan": plan.to_string(),
                        "added_proposals": ids(&add_proposals),
                        "added_tasks": ids(&add_tasks),
                        "removed": ids(&remove),
                    }),
                )?;
                buffer.append(tx, &event, now).await?;
                if tasks_of_plan.is_empty() {
                    end_plan(tx, &mut buffer, &actor.event_actor(), plan, "removed", now).await?;
                }
                Ok::<_, ReviewError>(())
            })
            .await?;
        self.bus.publish(buffer);
        self.get_review_plan(plan).await
    }

    pub async fn get_review_plan(&self, plan: ReviewPlanId) -> Result<ReviewPlanView, ReviewError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let row = store::get_plan(&mut *conn, plan)
            .await?
            .ok_or_else(|| plan_missing(plan))?;
        plan_view(&mut conn, row).await
    }

    /// Plans in number order, only the open ones when `open_only`.
    pub async fn list_review_plans(
        &self,
        open_only: bool,
    ) -> Result<Vec<ReviewPlanView>, ReviewError> {
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let mut views = Vec::new();
        for row in store::list_plans(&mut *conn, open_only).await? {
            views.push(plan_view(&mut conn, row).await?);
        }
        Ok(views)
    }

    /// Ends the wait of every open plan whose deadline passed by the clock's time, each in its own transaction: the tasks it still waits for leave as `deadline`, so their proposals are reviewed on their own later, and `review.plan_wait_ended` by fold names the open proposals the review lead now gives to one reviewer. A plan without an open proposal ends with `review.plan_ended`. Returns how many plans it ended the wait of.
    pub async fn end_due_plan_waits(&self) -> Result<usize, ReviewError> {
        let now = self.clock.now();
        let due = store::due_plans(self.store.read(), now).await?;
        let mut ended = 0;
        for plan in due {
            let mut buffer = EventBuffer::new();
            let done = self
                .store
                .write(async |tx| {
                    if !store::end_wait(tx, plan, now).await? {
                        return Ok::<_, ReviewError>(false);
                    }
                    let members = store::members(tx.conn(), plan).await?;
                    let mut left = Vec::new();
                    for member in members.iter().filter(|member| member.is_awaited()) {
                        store::release_member(tx, plan, member.task, PlanRelease::Deadline, now)
                            .await?;
                        left.push(member.task);
                    }
                    let proposals: Vec<ProposalId> = members
                        .iter()
                        .filter(|member| member.is_live())
                        .filter_map(|member| member.proposal)
                        .collect();
                    if proposals.is_empty() {
                        end_plan(tx, &mut buffer, &EventActor::fold(), plan, "deadline", now)
                            .await?;
                        return Ok(true);
                    }
                    let event = NewEvent::new(types::REVIEW_PLAN_WAIT_ENDED, EventActor::fold())?
                        .data(json!({
                        "plan": plan.to_string(),
                        "proposals": ids(&proposals),
                        "left": ids(&left),
                    }))?;
                    buffer.append(tx, &event, now).await?;
                    Ok(true)
                })
                .await?;
            self.bus.publish(buffer);
            ended += usize::from(done);
        }
        Ok(ended)
    }
}

async fn plan_view(
    conn: &mut SqliteConnection,
    plan: PlanRow,
) -> Result<ReviewPlanView, ReviewError> {
    let members = store::members(&mut *conn, plan.id).await?;
    let mut awaited = Vec::new();
    if plan.state == PlanState::Open {
        let areas = agents::list_areas(&mut *conn).await?;
        for member in members.iter().filter(|member| member.is_awaited()) {
            let Some(task) = tasks::fetch(&mut *conn, member.task).await? else {
                continue;
            };
            let labels = tasks::labels_for_conn(&mut *conn, member.task).await?;
            let area_leads = areas
                .iter()
                .filter(|area| labels.iter().any(|label| label.name == area.label))
                .filter_map(|area| area.lead.clone())
                .collect();
            awaited.push(AwaitedTask {
                task: member.task,
                status: task.status,
                worker: claims::unreleased_for_task(&mut *conn, member.task)
                    .await?
                    .map(|claim| claim.worker),
                area_leads,
            });
        }
    }
    Ok(ReviewPlanView {
        plan,
        members,
        awaited,
    })
}
