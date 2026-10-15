use fold_core::{AgentName, Tag, Timestamp, WorkerName};
use sqlx::SqliteExecutor;

use crate::error::StoreError;

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

/// True when the tag was new.
pub async fn add_tag(
    executor: impl SqliteExecutor<'_>,
    agent_id: i64,
    tag: &Tag,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let tag = tag.as_str();
    let created_at = now.as_millis();
    let result = sqlx::query!(
        "INSERT OR IGNORE INTO agent_tags (agent_id, tag, created_at) VALUES (?, ?, ?)",
        agent_id,
        tag,
        created_at
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
    now: Timestamp,
) -> Result<i64, StoreError> {
    let pid = i64::from(pid);
    let started_at = now.as_millis();
    Ok(sqlx::query_scalar!(
        r#"INSERT INTO sessions (pid, started_at, last_call_at) VALUES (?, ?, ?)
           RETURNING id AS "id!: i64""#,
        pid,
        started_at,
        started_at
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

/// Every agent, ordered by name.
pub async fn list_agents(executor: impl SqliteExecutor<'_>) -> Result<Vec<AgentRow>, StoreError> {
    let rows =
        sqlx::query!(r#"SELECT id AS "id!: i64", name, created_at FROM agents ORDER BY name"#)
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
