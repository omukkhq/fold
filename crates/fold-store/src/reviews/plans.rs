use fold_core::reviews::plans::{
    PlanProposalFacts, PlanRelease, PlanState, RelatedProposal, Relation,
};
use fold_core::{
    AgentName, LabelName, ProposalId, ReviewBatchId, ReviewPlanId, TaskId, TaskKind, TaskStatus,
    TaskTitle, Timestamp,
};
use sqlx::{SqliteConnection, SqliteExecutor};

use crate::columns::{optional_time, time};
use crate::error::StoreError;
use crate::store::WriteTx;

/// One review plan with public names only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRow {
    pub id: ReviewPlanId,
    /// The review lead that made the plan.
    pub lead: AgentName,
    pub state: PlanState,
    pub created_at: Timestamp,
    pub deadline_at: Timestamp,
    /// When fold ended the wait at the deadline; `None` before, and for a plan started earlier.
    pub wait_ended_at: Option<Timestamp>,
    pub started_at: Option<Timestamp>,
    /// The reviewer the start gave the plan's proposals to.
    pub reviewer: Option<AgentName>,
    /// The batch the start made of two or more proposals.
    pub batch: Option<ReviewBatchId>,
    pub ended_at: Option<Timestamp>,
}

/// One task of a plan: awaited while `proposal` is `None`, joined with the version its proposal had then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanMemberRow {
    pub task: TaskId,
    pub proposal: Option<ProposalId>,
    pub version: Option<u32>,
    pub added_at: Timestamp,
    pub joined_at: Option<Timestamp>,
    /// `None` while the member is in the plan.
    pub release: Option<PlanRelease>,
    pub released_at: Option<Timestamp>,
}

impl PlanMemberRow {
    pub fn is_live(&self) -> bool {
        self.release.is_none()
    }

    /// In the plan and waiting for its task's proposal.
    pub fn is_awaited(&self) -> bool {
        self.is_live() && self.proposal.is_none()
    }
}

/// A member in an open plan, for the checks that keep each member eligible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveMember {
    pub plan: ReviewPlanId,
    pub task: TaskId,
    pub proposal: Option<ProposalId>,
}

/// Inserts an open plan of the review lead `lead`, in the caller's write.
pub async fn insert_plan(
    tx: &mut WriteTx,
    plan: ReviewPlanId,
    lead: &AgentName,
    created_at: Timestamp,
    deadline_at: Timestamp,
) -> Result<(), StoreError> {
    let (number, lead, created_at, deadline_at) = (
        plan.to_db(),
        lead.as_str(),
        created_at.as_millis(),
        deadline_at.as_millis(),
    );
    let inserted = sqlx::query!(
        "INSERT INTO review_plans (number, lead_agent_id, created_at, deadline_at)
         SELECT ?1, a.id, ?3, ?4 FROM agents a WHERE a.name = ?2",
        number,
        lead,
        created_at,
        deadline_at
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if inserted == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid("the lead agent does not exist"))
    }
}

/// Adds `task` to the open plan, awaited, or joined with `version` of its proposal `joined`, in the caller's write. A task already in a live plan fails on its partial unique index, which decides a race between two plans; a sixth live member fails on the plan's trigger.
pub async fn insert_member(
    tx: &mut WriteTx,
    plan: ReviewPlanId,
    task: TaskId,
    joined: Option<(ProposalId, u32)>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let (plan, task, now) = (plan.to_db(), task.to_db(), now.as_millis());
    let inserted = match joined {
        None => sqlx::query!(
            "INSERT INTO review_plan_members (plan_id, task_id, added_at)
             SELECT r.id, t.id, ?3 FROM review_plans r, tasks t WHERE r.number = ?1 AND t.number = ?2",
            plan,
            task,
            now
        )
        .execute(tx.conn())
        .await?
        .rows_affected(),
        Some((proposal, version)) => {
            let (proposal, version) = (proposal.to_db(), i64::from(version));
            sqlx::query!(
                "INSERT INTO review_plan_members (plan_id, task_id, proposal_id, version_id, added_at, joined_at)
                 SELECT r.id, t.id, p.id, v.id, ?5, ?5
                 FROM review_plans r, tasks t, proposals p JOIN proposal_versions v ON v.proposal_id = p.id
                 WHERE r.number = ?1 AND t.number = ?2 AND p.number = ?3 AND v.version = ?4",
                plan,
                task,
                proposal,
                version,
                now
            )
            .execute(tx.conn())
            .await?
            .rows_affected()
        }
    };
    if inserted == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid(
            "the plan, the task, the proposal or the version does not exist",
        ))
    }
}

/// Joins the awaited member of `task` with `version` of its proposal, in the caller's write. False, with nothing changed, when the task is not awaited by the plan.
pub async fn join_member(
    tx: &mut WriteTx,
    plan: ReviewPlanId,
    task: TaskId,
    proposal: ProposalId,
    version: u32,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (plan, task, proposal, version, now) = (
        plan.to_db(),
        task.to_db(),
        proposal.to_db(),
        i64::from(version),
        now.as_millis(),
    );
    let updated = sqlx::query!(
        "UPDATE review_plan_members
         SET proposal_id = (SELECT id FROM proposals WHERE number = ?3),
             version_id = (SELECT v.id FROM proposal_versions v JOIN proposals p ON p.id = v.proposal_id
                           WHERE p.number = ?3 AND v.version = ?4),
             joined_at = ?5
         WHERE released_at IS NULL AND proposal_id IS NULL
           AND plan_id = (SELECT id FROM review_plans WHERE number = ?1)
           AND task_id = (SELECT id FROM tasks WHERE number = ?2)",
        plan,
        task,
        proposal,
        version,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// Releases the live member of `task` with `reason`, in the caller's write. False, with nothing changed, when the task is not a live member of the plan.
pub async fn release_member(
    tx: &mut WriteTx,
    plan: ReviewPlanId,
    task: TaskId,
    reason: PlanRelease,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (plan, task, reason, now) = (plan.to_db(), task.to_db(), reason.as_str(), now.as_millis());
    let updated = sqlx::query!(
        "UPDATE review_plan_members SET released_at = ?4, release_reason = ?3
         WHERE released_at IS NULL
           AND plan_id = (SELECT id FROM review_plans WHERE number = ?1)
           AND task_id = (SELECT id FROM tasks WHERE number = ?2)",
        plan,
        task,
        reason,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// Records that the wait of an open plan ended, in the caller's write. False, with nothing changed, when the plan is not open or its wait ended already.
pub async fn end_wait(
    tx: &mut WriteTx,
    plan: ReviewPlanId,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (number, now) = (plan.to_db(), now.as_millis());
    let updated = sqlx::query!(
        "UPDATE review_plans SET wait_ended_at = ?2
         WHERE number = ?1 AND state = 'open' AND wait_ended_at IS NULL",
        number,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// Marks an open plan started by the reviewer `reviewer`, with the batch its proposals became, in the caller's write. False, with nothing changed, when the plan is not open.
pub async fn mark_started(
    tx: &mut WriteTx,
    plan: ReviewPlanId,
    reviewer: &AgentName,
    batch: Option<ReviewBatchId>,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (number, reviewer, batch, now) = (
        plan.to_db(),
        reviewer.as_str(),
        batch.map(ReviewBatchId::to_db),
        now.as_millis(),
    );
    let updated = sqlx::query!(
        "UPDATE review_plans
         SET state = 'started', started_at = ?4,
             started_agent_id = (SELECT id FROM agents WHERE name = ?2),
             review_batch_id = (SELECT id FROM review_batches WHERE number = ?3)
         WHERE number = ?1 AND state = 'open'",
        number,
        reviewer,
        batch,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// Ends an open plan that has no member left, in the caller's write. False, with nothing changed, when the plan is not open.
pub async fn mark_ended(
    tx: &mut WriteTx,
    plan: ReviewPlanId,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (number, now) = (plan.to_db(), now.as_millis());
    let updated = sqlx::query!(
        "UPDATE review_plans SET state = 'ended', ended_at = ?2 WHERE number = ?1 AND state = 'open'",
        number,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(updated == 1)
}

pub async fn get_plan(
    executor: impl SqliteExecutor<'_>,
    plan: ReviewPlanId,
) -> Result<Option<PlanRow>, StoreError> {
    Ok(select_plans(executor, Some(plan), false).await?.pop())
}

/// Plans in number order, only the open ones when `open_only`.
pub async fn list_plans(
    executor: impl SqliteExecutor<'_>,
    open_only: bool,
) -> Result<Vec<PlanRow>, StoreError> {
    select_plans(executor, None, open_only).await
}

async fn select_plans(
    executor: impl SqliteExecutor<'_>,
    plan: Option<ReviewPlanId>,
    open_only: bool,
) -> Result<Vec<PlanRow>, StoreError> {
    let plan = plan.map(ReviewPlanId::to_db);
    let rows = sqlx::query!(
        r#"SELECT r.number AS "number!: i64", lead.name AS lead, r.state, r.created_at, r.deadline_at,
                  r.wait_ended_at, r.started_at, reviewer.name AS "reviewer?: String",
                  b.number AS "batch?: i64", r.ended_at
           FROM review_plans r
           JOIN agents lead ON lead.id = r.lead_agent_id
           LEFT JOIN agents reviewer ON reviewer.id = r.started_agent_id
           LEFT JOIN review_batches b ON b.id = r.review_batch_id
           WHERE (?1 IS NULL OR r.number = ?1) AND (NOT ?2 OR r.state = 'open')
           ORDER BY r.number"#,
        plan,
        open_only
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            let agent =
                |name: &str| AgentName::parse(name).map_err(|_| StoreError::Corrupt("agents.name"));
            Ok(PlanRow {
                id: ReviewPlanId::try_from_db(row.number)
                    .ok_or(StoreError::Corrupt("review_plans.number"))?,
                lead: agent(&row.lead)?,
                state: PlanState::parse(&row.state)
                    .ok_or(StoreError::Corrupt("review_plans.state"))?,
                created_at: time(row.created_at, "review_plans.created_at")?,
                deadline_at: time(row.deadline_at, "review_plans.deadline_at")?,
                wait_ended_at: optional_time(row.wait_ended_at, "review_plans.wait_ended_at")?,
                started_at: optional_time(row.started_at, "review_plans.started_at")?,
                reviewer: row.reviewer.as_deref().map(agent).transpose()?,
                batch: row
                    .batch
                    .map(|number| {
                        ReviewBatchId::try_from_db(number)
                            .ok_or(StoreError::Corrupt("review_batches.number"))
                    })
                    .transpose()?,
                ended_at: optional_time(row.ended_at, "review_plans.ended_at")?,
            })
        })
        .collect()
}

/// Every member the plan has had, live or released, in the order they were added, ties by task number.
pub async fn members(
    executor: impl SqliteExecutor<'_>,
    plan: ReviewPlanId,
) -> Result<Vec<PlanMemberRow>, StoreError> {
    let number = plan.to_db();
    let rows = sqlx::query!(
        r#"SELECT t.number AS "task!: i64", p.number AS "proposal?: i64", v.version AS "version?: i64",
                  m.added_at, m.joined_at, m.released_at, m.release_reason
           FROM review_plan_members m
           JOIN review_plans r ON r.id = m.plan_id
           JOIN tasks t ON t.id = m.task_id
           LEFT JOIN proposals p ON p.id = m.proposal_id
           LEFT JOIN proposal_versions v ON v.id = m.version_id
           WHERE r.number = ?
           ORDER BY m.added_at, t.number"#,
        number
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(PlanMemberRow {
                task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
                proposal: row
                    .proposal
                    .map(|number| {
                        ProposalId::try_from_db(number)
                            .ok_or(StoreError::Corrupt("proposals.number"))
                    })
                    .transpose()?,
                version: row
                    .version
                    .map(|version| {
                        u32::try_from(version)
                            .map_err(|_| StoreError::Corrupt("proposal_versions.version"))
                    })
                    .transpose()?,
                added_at: time(row.added_at, "review_plan_members.added_at")?,
                joined_at: optional_time(row.joined_at, "review_plan_members.joined_at")?,
                release: row
                    .release_reason
                    .as_deref()
                    .map(|reason| {
                        PlanRelease::parse(reason)
                            .ok_or(StoreError::Corrupt("review_plan_members.release_reason"))
                    })
                    .transpose()?,
                released_at: optional_time(row.released_at, "review_plan_members.released_at")?,
            })
        })
        .collect()
}

/// Every live member of every open plan, by plan and task number.
pub async fn live_members(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<LiveMember>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT r.number AS "plan!: i64", t.number AS "task!: i64", p.number AS "proposal?: i64"
           FROM review_plan_members m
           JOIN review_plans r ON r.id = m.plan_id
           JOIN tasks t ON t.id = m.task_id
           LEFT JOIN proposals p ON p.id = m.proposal_id
           WHERE m.released_at IS NULL AND r.state = 'open'
           ORDER BY r.number, t.number"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(LiveMember {
                plan: ReviewPlanId::try_from_db(row.plan)
                    .ok_or(StoreError::Corrupt("review_plans.number"))?,
                task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
                proposal: row
                    .proposal
                    .map(|number| {
                        ProposalId::try_from_db(number)
                            .ok_or(StoreError::Corrupt("proposals.number"))
                    })
                    .transpose()?,
            })
        })
        .collect()
}

/// The open plan the task is a live member of, or `None`.
pub async fn live_plan_of_task(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
) -> Result<Option<ReviewPlanId>, StoreError> {
    let number = task.to_db();
    let found = sqlx::query_scalar!(
        r#"SELECT r.number AS "number!: i64"
           FROM review_plan_members m
           JOIN review_plans r ON r.id = m.plan_id
           WHERE m.task_id = (SELECT id FROM tasks WHERE number = ?) AND m.released_at IS NULL"#,
        number
    )
    .fetch_optional(executor)
    .await?;
    found
        .map(|value| {
            ReviewPlanId::try_from_db(value).ok_or(StoreError::Corrupt("review_plans.number"))
        })
        .transpose()
}

/// The open plan the proposal is a live member of, or `None`.
pub async fn live_plan_of_proposal(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<Option<ReviewPlanId>, StoreError> {
    let number = proposal.to_db();
    let found = sqlx::query_scalar!(
        r#"SELECT r.number AS "number!: i64"
           FROM review_plan_members m
           JOIN review_plans r ON r.id = m.plan_id
           WHERE m.proposal_id = (SELECT id FROM proposals WHERE number = ?) AND m.released_at IS NULL"#,
        number
    )
    .fetch_optional(executor)
    .await?;
    found
        .map(|value| {
            ReviewPlanId::try_from_db(value).ok_or(StoreError::Corrupt("review_plans.number"))
        })
        .transpose()
}

/// The open plans whose wait has not ended and whose deadline is at or before `now`, by number.
pub async fn due_plans(
    executor: impl SqliteExecutor<'_>,
    now: Timestamp,
) -> Result<Vec<ReviewPlanId>, StoreError> {
    let now = now.as_millis();
    let numbers = sqlx::query_scalar!(
        r#"SELECT number AS "number!: i64" FROM review_plans
           WHERE state = 'open' AND wait_ended_at IS NULL AND deadline_at <= ?
           ORDER BY number"#,
        now
    )
    .fetch_all(executor)
    .await?;
    numbers
        .into_iter()
        .map(|number| {
            ReviewPlanId::try_from_db(number).ok_or(StoreError::Corrupt("review_plans.number"))
        })
        .collect()
}

/// The plan whose start made `batch`; its members released as `started` are the batch's proposals.
pub async fn plan_of_batch(
    executor: impl SqliteExecutor<'_>,
    batch: ReviewBatchId,
) -> Result<Option<ReviewPlanId>, StoreError> {
    let number = batch.to_db();
    let found = sqlx::query_scalar!(
        r#"SELECT r.number AS "number!: i64"
           FROM review_plans r JOIN review_batches b ON b.id = r.review_batch_id
           WHERE b.number = ?"#,
        number
    )
    .fetch_optional(executor)
    .await?;
    found
        .map(|value| {
            ReviewPlanId::try_from_db(value).ok_or(StoreError::Corrupt("review_plans.number"))
        })
        .transpose()
}

/// Whether a version of the proposal has a review with a verdict; a review with only comments is none. A plan holds only proposals without one, so a version after a review is reviewed alone with its review scope.
pub async fn has_verdict(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<bool, StoreError> {
    let number = proposal.to_db();
    let found = sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM reviews r
               JOIN proposal_versions v ON v.id = r.version_id
               JOIN proposals p ON p.id = v.proposal_id
               WHERE p.number = ? AND r.verdict <> 'comment'
           ) AS "found!: bool""#,
        number
    )
    .fetch_one(executor)
    .await?;
    Ok(found)
}

/// A task related to a proposal's task, with the facts that decide whether a review plan can wait for it or take its open proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedTaskRow {
    pub task: TaskId,
    pub relation: Relation,
    pub title: TaskTitle,
    pub status: TaskStatus,
    pub kind: TaskKind,
    pub human_review: bool,
    pub blocker: bool,
    pub labels: Vec<LabelName>,
    /// The task's open proposal and its facts.
    pub open_proposal: Option<RelatedProposal>,
    /// The task is a member of a live plan, awaited or with its proposal.
    pub in_plan: bool,
}

/// The tasks related to `task` that are neither archived nor Done, each once per relation, in number order: its direct prerequisites, the other tasks a checklist parent of it links, the other tasks of the request whose approval created it, and the tasks with an open proposal that share an area label with it. One read gives every fact a plan reads.
pub async fn related_tasks(
    conn: &mut SqliteConnection,
    task: TaskId,
) -> Result<Vec<RelatedTaskRow>, StoreError> {
    let number = task.to_db();
    let rows = sqlx::query!(
        r#"WITH me AS (SELECT id, request_id FROM tasks WHERE number = ?1),
           related (id, relation) AS (
               SELECT d.needs_task_id, 'dependency'
               FROM dependencies d JOIN me ON d.task_id = me.id
               UNION
               SELECT other.linked_task_id, 'same_parent'
               FROM checklist_items mine
               JOIN me ON mine.linked_task_id = me.id
               JOIN checklists c ON c.id = mine.checklist_id
               JOIN checklists siblings ON siblings.task_id = c.task_id
               JOIN checklist_items other ON other.checklist_id = siblings.id
               WHERE other.linked_task_id <> me.id
               UNION
               SELECT t.id, 'same_request'
               FROM tasks t JOIN me ON t.request_id = me.request_id
               WHERE t.id <> me.id
               UNION
               SELECT other.task_id, 'same_area'
               FROM task_labels mine
               JOIN me ON mine.task_id = me.id
               JOIN areas a ON a.label_id = mine.label_id
               JOIN task_labels other ON other.label_id = mine.label_id
               JOIN proposals op ON op.task_id = other.task_id AND op.state = 'open'
               WHERE other.task_id <> me.id
           )
           SELECT t.number AS "task!: i64",
                  r.relation AS "relation!: String",
                  t.title AS "title!: String",
                  t.status AS "status!: String",
                  t.kind AS "kind!: String",
                  t.human_review AS "human_review!: bool",
                  t.hold_reason IS NOT NULL AS "blocker!: bool",
                  (SELECT json_group_array(l.name)
                   FROM task_labels tl JOIN labels l ON l.id = tl.label_id
                   WHERE tl.task_id = t.id) AS "labels!: String",
                  op.number AS "proposal?: i64",
                  EXISTS (SELECT 1 FROM reviews v
                          JOIN proposal_versions pv ON pv.id = v.version_id
                          WHERE pv.proposal_id = op.id AND v.verdict <> 'comment') AS "reviewed!: bool",
                  EXISTS (SELECT 1 FROM claims c
                          WHERE c.proposal_id = op.id AND c.kind = 'review'
                            AND c.released_at IS NULL) AS "claimed!: bool",
                  EXISTS (SELECT 1 FROM review_batch_members bm
                          WHERE bm.proposal_id = op.id AND bm.released_at IS NULL) AS "in_batch!: bool",
                  EXISTS (SELECT 1 FROM review_plan_members m
                          WHERE m.task_id = t.id AND m.released_at IS NULL) AS "in_plan!: bool"
           FROM related r
           JOIN tasks t ON t.id = r.id
           LEFT JOIN proposals op ON op.task_id = t.id AND op.state = 'open'
           WHERE t.archived_at IS NULL AND t.status <> 'done'
           ORDER BY t.number, r.relation"#,
        number
    )
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            let labels: Vec<String> = serde_json::from_str(&row.labels)
                .map_err(|_| StoreError::Corrupt("labels.name"))?;
            let open_proposal = row
                .proposal
                .map(|number| {
                    Ok::<_, StoreError>(RelatedProposal {
                        id: ProposalId::try_from_db(number)
                            .ok_or(StoreError::Corrupt("proposals.number"))?,
                        facts: PlanProposalFacts {
                            reviewed: row.reviewed,
                            claimed: row.claimed,
                            in_batch: row.in_batch,
                            human_review: row.human_review,
                        },
                    })
                })
                .transpose()?;
            Ok(RelatedTaskRow {
                task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
                relation: Relation::parse(&row.relation)
                    .ok_or(StoreError::Corrupt("related_tasks.relation"))?,
                title: TaskTitle::parse(&row.title)
                    .map_err(|_| StoreError::Corrupt("tasks.title"))?,
                status: row
                    .status
                    .parse()
                    .map_err(|_| StoreError::Corrupt("tasks.status"))?,
                kind: row
                    .kind
                    .parse()
                    .map_err(|_| StoreError::Corrupt("tasks.kind"))?,
                human_review: row.human_review,
                blocker: row.blocker,
                labels: labels
                    .iter()
                    .map(|name| {
                        LabelName::parse(name).map_err(|_| StoreError::Corrupt("labels.name"))
                    })
                    .collect::<Result<_, _>>()?,
                open_proposal,
                in_plan: row.in_plan,
            })
        })
        .collect()
}
