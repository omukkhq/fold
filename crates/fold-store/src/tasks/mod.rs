use fold_core::{LabelColor, LabelName, TaskDescription, TaskId, TaskStatus, TaskTitle, Timestamp};
use sqlx::{SqliteConnection, SqlitePool};

use crate::error::StoreError;
use crate::store::WriteTx;

pub mod checklists;
pub mod claims;
pub mod completions;
mod dependencies;
mod ready;
mod recompute;
mod removal;

pub use dependencies::{EdgeRow, delete_edge, edges, edges_conn, fetch_edge, insert_edge};
pub use ready::{ReadyRow, ReadySnapshot, ready_snapshot};
pub use recompute::{LoadedFacts, load_facts, write_cache};
pub use removal::{
    TaskHistory, delete, delete_edges_touching, delete_linked_items, history, linked_item_owners,
    set_archived,
};

/// A task as stored. The row id stays inside this crate; callers address a task by its public number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: TaskId,
    pub title: TaskTitle,
    pub description: TaskDescription,
    pub created_by: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub archived_at: Option<Timestamp>,
    pub human_review: bool,
    pub human_reviewer: Option<String>,
    pub human_review_reason: Option<String>,
    pub human_review_set_by: Option<String>,
    pub status: TaskStatus,
    pub blocked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelRow {
    pub name: LabelName,
    pub color: LabelColor,
    pub created_at: Timestamp,
}

/// The values of a new task. `status` and `blocked` take the column defaults.
#[derive(Debug, Clone)]
pub struct NewTask<'a> {
    pub number: TaskId,
    pub title: &'a TaskTitle,
    pub description: &'a TaskDescription,
    pub created_by: &'a str,
    pub now: Timestamp,
}

struct RawTask {
    number: i64,
    title: String,
    description: String,
    created_by: String,
    created_at: i64,
    updated_at: i64,
    archived_at: Option<i64>,
    human_review: bool,
    human_reviewer: Option<String>,
    human_review_reason: Option<String>,
    human_review_set_by: Option<String>,
    status: String,
    blocked: bool,
}

impl RawTask {
    fn into_row(self) -> Result<TaskRow, StoreError> {
        Ok(TaskRow {
            id: TaskId::try_from_db(self.number).ok_or(StoreError::Corrupt("tasks.number"))?,
            title: TaskTitle::parse(&self.title).map_err(|_| StoreError::Corrupt("tasks.title"))?,
            description: TaskDescription::new(self.description),
            created_by: self.created_by,
            created_at: timestamp(self.created_at, "tasks.created_at")?,
            updated_at: timestamp(self.updated_at, "tasks.updated_at")?,
            archived_at: self
                .archived_at
                .map(|millis| timestamp(millis, "tasks.archived_at"))
                .transpose()?,
            human_review: self.human_review,
            human_reviewer: self.human_reviewer,
            human_review_reason: self.human_review_reason,
            human_review_set_by: self.human_review_set_by,
            status: self
                .status
                .parse()
                .map_err(|_| StoreError::Corrupt("tasks.status"))?,
            blocked: self.blocked,
        })
    }
}

fn label_row(name: &str, color: &str, created_at: i64) -> Result<LabelRow, StoreError> {
    Ok(LabelRow {
        name: LabelName::parse(name).map_err(|_| StoreError::Corrupt("labels.name"))?,
        color: color
            .parse()
            .map_err(|_| StoreError::Corrupt("labels.color"))?,
        created_at: timestamp(created_at, "labels.created_at")?,
    })
}

fn timestamp(millis: i64, column: &'static str) -> Result<Timestamp, StoreError> {
    Timestamp::from_millis(millis).map_err(|_| StoreError::Corrupt(column))
}

/// Inserts a task at `status = backlog`, `blocked = 0`. A number that already exists fails on `tasks.number`.
pub async fn insert(tx: &mut WriteTx, task: &NewTask<'_>) -> Result<(), StoreError> {
    let number = task.number.to_db();
    let title = task.title.as_str();
    let description = task.description.as_str();
    let now = task.now.as_millis();
    sqlx::query!(
        "INSERT INTO tasks (number, title, description, created_by, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        number,
        title,
        description,
        task.created_by,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Replaces the title and the description and stamps `updated_at`. The task must exist.
pub async fn update_text(
    tx: &mut WriteTx,
    task: TaskId,
    title: &TaskTitle,
    description: &TaskDescription,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = task.to_db();
    let title = title.as_str();
    let description = description.as_str();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE tasks SET title = ?2, description = ?3, updated_at = ?4 WHERE number = ?1",
        number,
        title,
        description,
        now
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

/// Stamps `updated_at` without changing anything else.
pub async fn touch(tx: &mut WriteTx, task: TaskId, now: Timestamp) -> Result<(), StoreError> {
    let number = task.to_db();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE tasks SET updated_at = ?2 WHERE number = ?1",
        number,
        now
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

fn expect_one(affected: u64) -> Result<(), StoreError> {
    if affected == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid("the task does not exist"))
    }
}

pub async fn get_by_number(pool: &SqlitePool, task: TaskId) -> Result<Option<TaskRow>, StoreError> {
    let mut conn = pool.acquire().await?;
    fetch(&mut conn, task).await
}

/// The same read on the writer's connection, so a command sees its own uncommitted changes.
pub async fn fetch(
    conn: &mut SqliteConnection,
    task: TaskId,
) -> Result<Option<TaskRow>, StoreError> {
    let number = task.to_db();
    sqlx::query_as!(
        RawTask,
        r#"SELECT number, title, description, created_by, created_at, updated_at, archived_at,
                  human_review AS "human_review: bool", human_reviewer, human_review_reason,
                  human_review_set_by, status, blocked AS "blocked: bool"
           FROM tasks WHERE number = ?"#,
        number
    )
    .fetch_optional(conn)
    .await?
    .map(RawTask::into_row)
    .transpose()
}

/// Tasks in number order. Archived tasks are left out unless `include_archived`.
pub async fn list(pool: &SqlitePool, include_archived: bool) -> Result<Vec<TaskRow>, StoreError> {
    sqlx::query_as!(
        RawTask,
        r#"SELECT number, title, description, created_by, created_at, updated_at, archived_at,
                  human_review AS "human_review: bool", human_reviewer, human_review_reason,
                  human_review_set_by, status, blocked AS "blocked: bool"
           FROM tasks WHERE ?1 OR archived_at IS NULL ORDER BY number"#,
        include_archived
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(RawTask::into_row)
    .collect()
}

/// Inserts a label. A name that exists fails on `labels.name`.
pub async fn insert_label(
    tx: &mut WriteTx,
    name: &LabelName,
    color: LabelColor,
    now: Timestamp,
) -> Result<(), StoreError> {
    let name = name.as_str();
    let color = color.as_str();
    let now = now.as_millis();
    sqlx::query!(
        "INSERT INTO labels (name, color, created_at) VALUES (?, ?, ?)",
        name,
        color,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The names in `names` that no label carries, in the given order.
pub async fn missing_labels(
    conn: &mut SqliteConnection,
    names: &[LabelName],
) -> Result<Vec<LabelName>, StoreError> {
    let mut missing = Vec::new();
    for name in names {
        let text = name.as_str();
        let found = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "count!: i64" FROM labels WHERE name = ?"#,
            text
        )
        .fetch_one(&mut *conn)
        .await?;
        if found == 0 {
            missing.push(name.clone());
        }
    }
    Ok(missing)
}

/// Makes the label set of `task` exactly `names`. Every name must exist.
pub async fn replace_labels(
    tx: &mut WriteTx,
    task: TaskId,
    names: &[LabelName],
) -> Result<(), StoreError> {
    let number = task.to_db();
    sqlx::query!(
        "DELETE FROM task_labels WHERE task_id = (SELECT id FROM tasks WHERE number = ?)",
        number
    )
    .execute(tx.conn())
    .await?;
    for name in names {
        let text = name.as_str();
        sqlx::query!(
            "INSERT OR IGNORE INTO task_labels (task_id, label_id)
             SELECT t.id, l.id FROM tasks t, labels l WHERE t.number = ? AND l.name = ?",
            number,
            text
        )
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

/// The labels of `task` in name order.
pub async fn labels_for(pool: &SqlitePool, task: TaskId) -> Result<Vec<LabelRow>, StoreError> {
    let mut conn = pool.acquire().await?;
    labels_for_conn(&mut conn, task).await
}

pub async fn labels_for_conn(
    conn: &mut SqliteConnection,
    task: TaskId,
) -> Result<Vec<LabelRow>, StoreError> {
    let number = task.to_db();
    let rows = sqlx::query!(
        "SELECT l.name, l.color, l.created_at
         FROM task_labels tl
         JOIN tasks t ON t.id = tl.task_id
         JOIN labels l ON l.id = tl.label_id
         WHERE t.number = ?
         ORDER BY l.name",
        number
    )
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|row| label_row(&row.name, &row.color, row.created_at))
        .collect()
}

/// Every label in name order.
pub async fn list_labels(pool: &SqlitePool) -> Result<Vec<LabelRow>, StoreError> {
    let rows = sqlx::query!("SELECT name, color, created_at FROM labels ORDER BY name")
        .fetch_all(pool)
        .await?;
    rows.into_iter()
        .map(|row| label_row(&row.name, &row.color, row.created_at))
        .collect()
}
