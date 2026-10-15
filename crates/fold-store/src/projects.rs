use fold_core::{HumanName, Timestamp};
use sqlx::{SqliteExecutor, SqlitePool};

use crate::error::StoreError;
use crate::store::WriteTx;

/// One row of the host registry. `root` is the project directory that holds `.fold/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    pub slug: String,
    pub name: String,
    pub root: String,
}

/// Every registered project, ordered by slug.
pub async fn list_projects(pool: &SqlitePool) -> Result<Vec<ProjectRow>, StoreError> {
    Ok(sqlx::query_as!(
        ProjectRow,
        "SELECT slug, name, root FROM projects ORDER BY slug"
    )
    .fetch_all(pool)
    .await?)
}

pub async fn find_project(pool: &SqlitePool, slug: &str) -> Result<Option<ProjectRow>, StoreError> {
    Ok(sqlx::query_as!(
        ProjectRow,
        "SELECT slug, name, root FROM projects WHERE slug = ?",
        slug
    )
    .fetch_optional(pool)
    .await?)
}

/// The project registered at `root`, which must be the canonical path `insert_project` stored.
pub async fn find_by_root(pool: &SqlitePool, root: &str) -> Result<Option<ProjectRow>, StoreError> {
    Ok(sqlx::query_as!(
        ProjectRow,
        "SELECT slug, name, root FROM projects WHERE root = ?",
        root
    )
    .fetch_optional(pool)
    .await?)
}

/// Every registered slug, read inside the write that is about to choose a new one.
pub async fn slugs(tx: &mut WriteTx) -> Result<Vec<String>, StoreError> {
    Ok(sqlx::query_scalar!("SELECT slug FROM projects")
        .fetch_all(tx.conn())
        .await?)
}

/// Registers a project. A repeated slug or root fails with a unique violation.
pub async fn insert_project(
    tx: &mut WriteTx,
    project: &ProjectRow,
    now: Timestamp,
) -> Result<(), StoreError> {
    let created_at = now.as_millis();
    sqlx::query!(
        "INSERT INTO projects (slug, name, root, created_at) VALUES (?, ?, ?, ?)",
        project.slug,
        project.name,
        project.root,
        created_at
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// A named human of the host registry. Humans belong to the host, not to one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanRow {
    pub id: i64,
    pub name: HumanName,
    pub created_at: Timestamp,
}

pub async fn find_human(
    executor: impl SqliteExecutor<'_>,
    name: &HumanName,
) -> Result<Option<HumanRow>, StoreError> {
    let name = name.as_str();
    let row = sqlx::query!(
        r#"SELECT id AS "id!: i64", name, created_at FROM humans WHERE name = ?"#,
        name
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(HumanRow {
            id: row.id,
            name: HumanName::parse(&row.name).map_err(|_| StoreError::Corrupt("humans.name"))?,
            created_at: Timestamp::from_millis(row.created_at)
                .map_err(|_| StoreError::Corrupt("humans.created_at"))?,
        })
    })
    .transpose()
}

/// Inserts a human. A repeated name fails with a unique violation.
pub async fn insert_human(
    tx: &mut WriteTx,
    name: &HumanName,
    now: Timestamp,
) -> Result<i64, StoreError> {
    let name = name.as_str();
    let created_at = now.as_millis();
    Ok(sqlx::query_scalar!(
        r#"INSERT INTO humans (name, created_at) VALUES (?, ?) RETURNING id AS "id!: i64""#,
        name,
        created_at
    )
    .fetch_one(tx.conn())
    .await?)
}

/// The human named `name`, inserted with `created_at = now` when it is new. An existing row keeps its first `created_at`.
pub async fn ensure_human(
    tx: &mut WriteTx,
    name: &HumanName,
    now: Timestamp,
) -> Result<HumanRow, StoreError> {
    let text = name.as_str();
    let created_at = now.as_millis();
    sqlx::query!(
        "INSERT INTO humans (name, created_at) VALUES (?, ?) ON CONFLICT (name) DO NOTHING",
        text,
        created_at
    )
    .execute(tx.conn())
    .await?;
    find_human(tx.conn(), name)
        .await?
        .ok_or(StoreError::Corrupt("humans.name"))
}
