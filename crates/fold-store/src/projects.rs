use fold_core::Timestamp;
use sqlx::SqlitePool;

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
