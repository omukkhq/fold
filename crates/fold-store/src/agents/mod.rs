use std::collections::HashMap;

use fold_core::policy::AgentStanding;
use fold_core::roles::Role;
use fold_core::{
    Actor, AgentName, CheckpointId, ProposalId, RevisionId, Tag, TaskId, Timestamp, WorkerName,
    WorkspaceId,
};
use sqlx::{SqliteConnection, SqliteExecutor};

use crate::error::StoreError;
use crate::workspaces::WorkspaceState;

mod handoffs;
mod questions;
mod workers;

pub use handoffs::{
    HandoffRow, OpenProposalRow, OpenTaskRow, handoff_asked, insert_handoff, latest_handoff,
    open_area_proposals, open_area_tasks, refresh_session, session_handoff, set_handoff_asked,
};
pub use questions::{
    AnswerRow, NewQuestion, QuestionKind, QuestionRow, asked_among, asker_answered_by,
    close_open_questions, find_question, insert_answer, insert_question, insert_question_message,
    open_ask_of_thread, open_questions, question_by_id, question_messages, questions_of_worker,
};
pub use workers::{
    AreaRow, NewWorkerProcess, TurnTotals, WorkerProcessRow, bind_proposal, bind_task,
    coders_of_task, find_area, find_worker_process, insert_area, insert_worker_process, label_id,
    list_areas, list_worker_processes, mark_worker_working, proposals_without_verdict, record_exit,
    record_turn, restart_worker_process, reviewers_of_proposal, running_counts, set_area_lead,
    set_area_profile, set_worker_context, set_worker_pid, set_worker_state,
    tasks_awaiting_proposal,
};
pub use workers::{PendingResume, pending_resumes};

/// Row ids of these tables stay inside the daemon; the public handles are the names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    pub id: i64,
    pub name: AgentName,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRow {
    pub id: i64,
    pub agent_id: i64,
    pub name: WorkerName,
    pub parent_id: Option<i64>,
    pub last_call_at: Option<Timestamp>,
    pub activity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub id: i64,
    pub agent_id: Option<i64>,
    pub pid: i64,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub last_call_at: Option<Timestamp>,
}

/// A session that is still open and has an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSession {
    pub id: i64,
    pub agent: AgentName,
}

fn time(millis: i64, column: &'static str) -> Result<Timestamp, StoreError> {
    Timestamp::from_millis(millis).map_err(|_| StoreError::Corrupt(column))
}

fn optional_time(
    millis: Option<i64>,
    column: &'static str,
) -> Result<Option<Timestamp>, StoreError> {
    millis.map(|value| time(value, column)).transpose()
}

pub async fn insert_agent(
    executor: impl SqliteExecutor<'_>,
    name: &AgentName,
    now: Timestamp,
) -> Result<i64, StoreError> {
    let name = name.as_str();
    let created_at = now.as_millis();
    Ok(sqlx::query_scalar!(
        r#"INSERT INTO agents (name, created_at) VALUES (?, ?) RETURNING id AS "id!: i64""#,
        name,
        created_at
    )
    .fetch_one(executor)
    .await?)
}

pub async fn find_agent(
    executor: impl SqliteExecutor<'_>,
    name: &AgentName,
) -> Result<Option<AgentRow>, StoreError> {
    let name = name.as_str();
    let row = sqlx::query!(
        r#"SELECT id AS "id!: i64", name, created_at FROM agents WHERE name = ?"#,
        name
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(AgentRow {
            id: row.id,
            name: AgentName::parse(&row.name).map_err(|_| StoreError::Corrupt("agents.name"))?,
            created_at: time(row.created_at, "agents.created_at")?,
        })
    })
    .transpose()
}

pub async fn agent_name(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
) -> Result<Option<AgentName>, StoreError> {
    let name = sqlx::query_scalar!("SELECT name FROM agents WHERE id = ?", agent_id)
        .fetch_optional(executor)
        .await?;
    name.map(|name| AgentName::parse(&name).map_err(|_| StoreError::Corrupt("agents.name")))
        .transpose()
}

/// True when the tag was new. `granted_by` is the actor label of the granting human; a tag that is already present keeps its first granter.
pub async fn add_tag(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
    tag: &Tag,
    granted_by: Option<&str>,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let tag = tag.as_str();
    let created_at = now.as_millis();
    let result = sqlx::query!(
        "INSERT INTO agent_tags (agent_id, tag, created_at, granted_by) VALUES (?, ?, ?, ?)
         ON CONFLICT (agent_id, tag) DO NOTHING",
        agent_id,
        tag,
        created_at,
        granted_by
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn tags(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
) -> Result<Vec<Tag>, StoreError> {
    let rows = sqlx::query_scalar!(
        "SELECT tag FROM agent_tags WHERE agent_id = ? ORDER BY tag",
        agent_id
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|tag| Tag::parse(&tag).map_err(|_| StoreError::Corrupt("agent_tags.tag")))
        .collect()
}

pub async fn insert_worker(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
    name: &WorkerName,
    parent_id: Option<i64>,
    now: Timestamp,
) -> Result<i64, StoreError> {
    let name = name.as_str();
    let created_at = now.as_millis();
    Ok(sqlx::query_scalar!(
        r#"INSERT INTO workers (agent_id, name, parent_worker_id, created_at)
           VALUES (?, ?, ?, ?) RETURNING id AS "id!: i64""#,
        agent_id,
        name,
        parent_id,
        created_at
    )
    .fetch_one(executor)
    .await?)
}

/// The worker `name` of agent `agent_id`; a worker of another agent is not found.
pub async fn find_worker(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
    name: &WorkerName,
) -> Result<Option<WorkerRow>, StoreError> {
    let name = name.as_str();
    let row = sqlx::query!(
        r#"SELECT id AS "id!: i64", agent_id, name, parent_worker_id, last_call_at, activity
           FROM workers WHERE agent_id = ? AND name = ?"#,
        agent_id,
        name
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(WorkerRow {
            id: row.id,
            agent_id: row.agent_id,
            name: WorkerName::parse(&row.name).map_err(|_| StoreError::Corrupt("workers.name"))?,
            parent_id: row.parent_worker_id,
            last_call_at: optional_time(row.last_call_at, "workers.last_call_at")?,
            activity: row.activity,
        })
    })
    .transpose()
}

/// The row ids of every worker of agent `agent_id`, in id order.
pub async fn worker_ids(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
) -> Result<Vec<i64>, StoreError> {
    Ok(sqlx::query_scalar!(
        r#"SELECT id AS "id!: i64" FROM workers WHERE agent_id = ? ORDER BY id"#,
        agent_id
    )
    .fetch_all(executor)
    .await?)
}

pub async fn main_worker(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
) -> Result<Option<WorkerRow>, StoreError> {
    let row = sqlx::query!(
        r#"SELECT id AS "id!: i64", agent_id, name, parent_worker_id, last_call_at, activity
           FROM workers WHERE agent_id = ? AND parent_worker_id IS NULL"#,
        agent_id
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(WorkerRow {
            id: row.id,
            agent_id: row.agent_id,
            name: WorkerName::parse(&row.name).map_err(|_| StoreError::Corrupt("workers.name"))?,
            parent_id: row.parent_worker_id,
            last_call_at: optional_time(row.last_call_at, "workers.last_call_at")?,
            activity: row.activity,
        })
    })
    .transpose()
}

pub async fn insert_session(
    executor: impl SqliteExecutor<'_>,
    pid: u32,
    cwd: Option<&str>,
    now: Timestamp,
) -> Result<i64, StoreError> {
    let pid = i64::from(pid);
    let started_at = now.as_millis();
    Ok(sqlx::query_scalar!(
        r#"INSERT INTO sessions (pid, started_at, last_call_at, cwd) VALUES (?, ?, ?, ?)
           RETURNING id AS "id!: i64""#,
        pid,
        started_at,
        started_at,
        cwd
    )
    .fetch_one(executor)
    .await?)
}

pub async fn find_session(
    executor: impl SqliteExecutor<'_>,
    id: i64,
) -> Result<Option<SessionRow>, StoreError> {
    let row = sqlx::query!(
        r#"SELECT id AS "id!: i64", agent_id, pid, started_at, ended_at, last_call_at
           FROM sessions WHERE id = ?"#,
        id
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(SessionRow {
            id: row.id,
            agent_id: row.agent_id,
            pid: row.pid,
            started_at: time(row.started_at, "sessions.started_at")?,
            ended_at: optional_time(row.ended_at, "sessions.ended_at")?,
            last_call_at: optional_time(row.last_call_at, "sessions.last_call_at")?,
        })
    })
    .transpose()
}

/// Binds a session that has no agent yet. False when it was already bound or does not exist.
pub async fn bind_session(
    executor: impl SqliteExecutor<'_>,
    id: i64,
    agent_id: i64,
) -> Result<bool, StoreError> {
    let result = sqlx::query!(
        "UPDATE sessions SET agent_id = ? WHERE id = ? AND agent_id IS NULL",
        agent_id,
        id
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Sets `ended_at` once. False when the session was already ended or does not exist.
pub async fn end_session(
    executor: impl SqliteExecutor<'_>,
    id: i64,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let ended_at = now.as_millis();
    let result = sqlx::query!(
        "UPDATE sessions SET ended_at = ? WHERE id = ? AND ended_at IS NULL",
        ended_at,
        id
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn touch_session(
    executor: impl SqliteExecutor<'_>,
    id: i64,
    now: Timestamp,
) -> Result<(), StoreError> {
    let at = now.as_millis();
    sqlx::query!("UPDATE sessions SET last_call_at = ? WHERE id = ?", at, id)
        .execute(executor)
        .await?;
    Ok(())
}

pub async fn touch_worker(
    executor: impl SqliteExecutor<'_>,
    id: i64,
    now: Timestamp,
    activity: &str,
) -> Result<(), StoreError> {
    let at = now.as_millis();
    sqlx::query!(
        "UPDATE workers SET last_call_at = ?, activity = ? WHERE id = ?",
        at,
        activity,
        id
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Open sessions that have an agent, oldest first.
pub async fn open_bound_sessions(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<OpenSession>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT s.id AS "id!: i64", a.name AS "name!"
           FROM sessions s JOIN agents a ON a.id = s.agent_id
           WHERE s.ended_at IS NULL ORDER BY s.id"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(OpenSession {
                id: row.id,
                agent: AgentName::parse(&row.name)
                    .map_err(|_| StoreError::Corrupt("agents.name"))?,
            })
        })
        .collect()
}

/// Ends every open session at `now` and returns how many ended.
pub async fn end_open_sessions(
    executor: impl SqliteExecutor<'_>,
    now: Timestamp,
) -> Result<u64, StoreError> {
    let ended_at = now.as_millis();
    let result = sqlx::query!(
        "UPDATE sessions SET ended_at = ? WHERE ended_at IS NULL",
        ended_at
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected())
}

/// Every agent in registration order.
pub async fn list_agents(executor: impl SqliteExecutor<'_>) -> Result<Vec<AgentRow>, StoreError> {
    let rows = sqlx::query!(r#"SELECT id AS "id!: i64", name, created_at FROM agents ORDER BY id"#)
        .fetch_all(executor)
        .await?;
    rows.into_iter()
        .map(|row| {
            Ok(AgentRow {
                id: row.id,
                name: AgentName::parse(&row.name)
                    .map_err(|_| StoreError::Corrupt("agents.name"))?,
                created_at: time(row.created_at, "agents.created_at")?,
            })
        })
        .collect()
}

/// Every tag of every agent as `(agent_id, tag)`, ordered by agent and tag.
pub async fn all_tags(executor: impl SqliteExecutor<'_>) -> Result<Vec<(i64, Tag)>, StoreError> {
    let rows = sqlx::query!("SELECT agent_id, tag FROM agent_tags ORDER BY agent_id, tag")
        .fetch_all(executor)
        .await?;
    rows.into_iter()
        .map(|row| {
            let tag = Tag::parse(&row.tag).map_err(|_| StoreError::Corrupt("agent_tags.tag"))?;
            Ok((row.agent_id, tag))
        })
        .collect()
}

/// The acting agent's standing for the rules that read more than its tags; a human or the system acts with [`AgentStanding::NO_RIGHTS`], because the rules decide for them without one.
pub async fn standing_of(
    executor: impl SqliteExecutor<'_>,
    actor: &Actor,
) -> Result<AgentStanding, StoreError> {
    match actor {
        Actor::Agent(agent) => standing(executor, &agent.agent).await,
        Actor::Human { .. } | Actor::System => Ok(AgentStanding::NO_RIGHTS),
    }
}

/// Whether `agent` has the `main` tag and the role of its worker process. An unknown agent stands as one that fold does not supervise.
async fn standing(
    executor: impl SqliteExecutor<'_>,
    agent: &AgentName,
) -> Result<AgentStanding, StoreError> {
    let name = agent.as_str();
    let row = sqlx::query!(
        r#"SELECT EXISTS (
                      SELECT 1 FROM agent_tags t JOIN agents g ON g.id = t.agent_id
                      WHERE g.name = ?1 AND t.tag = 'main'
                  ) AS "holds_main!: bool",
                  (SELECT w.role FROM worker_processes w JOIN agents g ON g.id = w.agent_id
                   WHERE g.name = ?1) AS "role?: String""#,
        name
    )
    .fetch_one(executor)
    .await?;
    let role = row
        .role
        .as_deref()
        .map(|role| Role::parse(role).ok_or(StoreError::Corrupt("worker_processes.role")))
        .transpose()?;
    Ok(AgentStanding {
        holds_main: row.holds_main,
        role,
    })
}

/// Every worker of every agent, ordered by name.
pub async fn all_workers(executor: impl SqliteExecutor<'_>) -> Result<Vec<WorkerRow>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT id AS "id!: i64", agent_id, name, parent_worker_id, last_call_at, activity
           FROM workers ORDER BY name"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(WorkerRow {
                id: row.id,
                agent_id: row.agent_id,
                name: WorkerName::parse(&row.name)
                    .map_err(|_| StoreError::Corrupt("workers.name"))?,
                parent_id: row.parent_worker_id,
                last_call_at: optional_time(row.last_call_at, "workers.last_call_at")?,
                activity: row.activity,
            })
        })
        .collect()
}

/// True when the tag was present.
pub async fn remove_tag(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
    tag: &Tag,
) -> Result<bool, StoreError> {
    let tag = tag.as_str();
    let result = sqlx::query!(
        "DELETE FROM agent_tags WHERE agent_id = ? AND tag = ?",
        agent_id,
        tag
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// The sessions of one agent as presence and the agent overview read them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentSessions {
    /// The `last_call_at` of every open session.
    pub open: Vec<Option<Timestamp>>,
    /// The newest `last_call_at` of any session, open or ended.
    pub newest_call: Option<Timestamp>,
}

/// The sessions of every agent that has one, by agent row id. Sessions that never bound an agent are left out.
pub async fn sessions_by_agent(
    conn: &mut SqliteConnection,
) -> Result<HashMap<i64, AgentSessions>, StoreError> {
    let newest = sqlx::query!(
        r#"SELECT agent_id AS "agent_id!: i64", MAX(last_call_at) AS "newest: i64"
           FROM sessions WHERE agent_id IS NOT NULL
           GROUP BY agent_id"#
    )
    .fetch_all(&mut *conn)
    .await?;
    let open = sqlx::query!(
        r#"SELECT agent_id AS "agent_id!: i64", last_call_at
           FROM sessions WHERE agent_id IS NOT NULL AND ended_at IS NULL
           ORDER BY id"#
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut sessions = HashMap::with_capacity(newest.len());
    for row in newest {
        let entry = AgentSessions {
            open: Vec::new(),
            newest_call: optional_time(row.newest, "sessions.last_call_at")?,
        };
        sessions.insert(row.agent_id, entry);
    }
    for row in open {
        let at = optional_time(row.last_call_at, "sessions.last_call_at")?;
        sessions.entry(row.agent_id).or_default().open.push(at);
    }
    Ok(sessions)
}

/// An open, bound session as the agents overview lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSessionRow {
    pub pid: i64,
    pub started_at: Timestamp,
    pub cwd: Option<String>,
}

/// The open sessions of every agent that has one, by agent row id, oldest first.
pub async fn open_sessions_by_agent(
    executor: impl SqliteExecutor<'_>,
) -> Result<HashMap<i64, Vec<OpenSessionRow>>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT agent_id AS "agent_id!: i64", pid, started_at, cwd
           FROM sessions WHERE agent_id IS NOT NULL AND ended_at IS NULL
           ORDER BY id"#
    )
    .fetch_all(executor)
    .await?;
    let mut sessions: HashMap<i64, Vec<OpenSessionRow>> = HashMap::new();
    for row in rows {
        sessions
            .entry(row.agent_id)
            .or_default()
            .push(OpenSessionRow {
                pid: row.pid,
                started_at: time(row.started_at, "sessions.started_at")?,
                cwd: row.cwd,
            });
    }
    Ok(sessions)
}

/// The `last_call_at` of every open session of one agent, which is what its presence reads.
pub async fn open_session_calls(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
) -> Result<Vec<Option<Timestamp>>, StoreError> {
    let rows = sqlx::query_scalar!(
        r#"SELECT last_call_at FROM sessions WHERE agent_id = ? AND ended_at IS NULL"#,
        agent_id
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|at| optional_time(at, "sessions.last_call_at"))
        .collect()
}

/// The `last_call_at` of every open, bound session, with its agent's name. An agent without an open session is absent.
pub async fn open_session_calls_by_agent(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<(AgentName, Option<Timestamp>)>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT a.name, s.last_call_at
           FROM sessions s JOIN agents a ON a.id = s.agent_id
           WHERE s.ended_at IS NULL"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            let name =
                AgentName::parse(&row.name).map_err(|_| StoreError::Corrupt("agents.name"))?;
            Ok((
                name,
                optional_time(row.last_call_at, "sessions.last_call_at")?,
            ))
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimKind {
    Implementation,
    Review,
}

impl ClaimKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ClaimKind::Implementation => "implementation",
            ClaimKind::Review => "review",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "implementation" => Ok(ClaimKind::Implementation),
            "review" => Ok(ClaimKind::Review),
            _ => Err(StoreError::Corrupt("claims.kind")),
        }
    }
}

/// An unreleased claim of either kind. A review claim names a proposal, and `task` is that proposal's task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldClaim {
    pub worker_id: i64,
    pub kind: ClaimKind,
    pub task: TaskId,
    pub acquired_at: Timestamp,
    pub renewed_at: Timestamp,
    /// The lease sweeper marked the claim (`stale_at` is set).
    pub marked_stale: bool,
}

/// Every unreleased claim, oldest first.
pub async fn held_claims(executor: impl SqliteExecutor<'_>) -> Result<Vec<HeldClaim>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT c.worker_id, c.kind, COALESCE(t.number, pt.number) AS "task!: i64",
                  c.acquired_at, c.renewed_at, c.stale_at IS NOT NULL AS "marked_stale!: bool"
           FROM claims c
           LEFT JOIN tasks t ON t.id = c.task_id
           LEFT JOIN proposals p ON p.id = c.proposal_id
           LEFT JOIN tasks pt ON pt.id = p.task_id
           WHERE c.released_at IS NULL
           ORDER BY c.acquired_at, c.id"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(HeldClaim {
                worker_id: row.worker_id,
                kind: ClaimKind::parse(&row.kind)?,
                task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
                acquired_at: time(row.acquired_at, "claims.acquired_at")?,
                renewed_at: time(row.renewed_at, "claims.renewed_at")?,
                marked_stale: row.marked_stale,
            })
        })
        .collect()
}

/// A task workspace in state `active` or `missing` with an owner. `changed_files` and `last_activity_at` are the row's cached values, so they read the same with or without the watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedWorkspace {
    pub worker_id: i64,
    pub id: WorkspaceId,
    pub task: TaskId,
    pub state: WorkspaceState,
    pub base: RevisionId,
    pub changed_files: u32,
    pub last_activity_at: Option<Timestamp>,
    /// The newest checkpoint that is not pruned.
    pub latest_checkpoint: Option<CheckpointId>,
}

/// Every live task workspace that has an owner, by workspace number. A review checkout holds no work in progress, so the overview leaves it out.
pub async fn owned_live_workspaces(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<OwnedWorkspace>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT w.worker_id AS "worker_id!: i64", w.number AS "number!: i64",
                  t.number AS "task!: i64", w.state, r.number AS "base!: i64",
                  w.changed_files, w.last_activity_at,
                  (SELECT MAX(c.number) FROM checkpoints c
                   WHERE c.workspace_id = w.id AND c.pruned_at IS NULL) AS "checkpoint: i64"
           FROM workspaces w
           JOIN tasks t ON t.id = w.task_id
           JOIN revisions r ON r.id = w.base_revision_id
           WHERE w.kind = 'task' AND w.state IN ('active', 'missing') AND w.worker_id IS NOT NULL
           ORDER BY w.number"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(OwnedWorkspace {
                worker_id: row.worker_id,
                id: WorkspaceId::try_from_db(row.number)
                    .ok_or(StoreError::Corrupt("workspaces.number"))?,
                task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
                state: WorkspaceState::parse(&row.state)?,
                base: RevisionId::try_from_db(row.base)
                    .ok_or(StoreError::Corrupt("revisions.number"))?,
                changed_files: u32::try_from(row.changed_files)
                    .map_err(|_| StoreError::Corrupt("workspaces.changed_files"))?,
                last_activity_at: optional_time(
                    row.last_activity_at,
                    "workspaces.last_activity_at",
                )?,
                latest_checkpoint: row
                    .checkpoint
                    .map(|number| {
                        CheckpointId::try_from_db(number)
                            .ok_or(StoreError::Corrupt("checkpoints.number"))
                    })
                    .transpose()?,
            })
        })
        .collect()
}

/// An open proposal and the worker that wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoredProposal {
    pub author_worker_id: i64,
    pub id: ProposalId,
    pub task: TaskId,
    pub title: String,
    pub conflicted: bool,
}

/// Every open proposal, by proposal number.
pub async fn open_proposals(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<AuthoredProposal>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT p.author_worker_id, p.number AS "number!: i64", t.number AS "task!: i64",
                  p.title, p.conflicted AS "conflicted: bool"
           FROM proposals p
           JOIN tasks t ON t.id = p.task_id
           WHERE p.state = 'open'
           ORDER BY p.number"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(AuthoredProposal {
                author_worker_id: row.author_worker_id,
                id: ProposalId::try_from_db(row.number)
                    .ok_or(StoreError::Corrupt("proposals.number"))?,
                task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
                title: row.title,
                conflicted: row.conflicted,
            })
        })
        .collect()
}

/// The pid of every worker process that runs or may run now, with its agent's name. A socket peer under one of these processes acts only as that agent.
pub async fn live_worker_pids(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<(u32, AgentName)>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT w.pid AS "pid!: i64", g.name
           FROM worker_processes w JOIN agents g ON g.id = w.agent_id
           WHERE w.pid IS NOT NULL AND w.state NOT IN ('stopped', 'failed')"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            let pid =
                u32::try_from(row.pid).map_err(|_| StoreError::Corrupt("worker_processes.pid"))?;
            let name =
                AgentName::parse(&row.name).map_err(|_| StoreError::Corrupt("agents.name"))?;
            Ok((pid, name))
        })
        .collect()
}
