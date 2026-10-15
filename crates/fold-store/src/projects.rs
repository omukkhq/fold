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

/// Every human of the host registry, ordered by name.
pub async fn list_humans(executor: impl SqliteExecutor<'_>) -> Result<Vec<HumanName>, StoreError> {
    let names = sqlx::query_scalar!("SELECT name FROM humans ORDER BY name")
        .fetch_all(executor)
        .await?;
    names
        .iter()
        .map(|name| HumanName::parse(name).map_err(|_| StoreError::Corrupt("humans.name")))
        .collect()
}

/// A web sign-in token of the host registry. The registry holds only the token's BLAKE3 hash, never the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebTokenRow {
    pub id: i64,
    pub token_hash: Vec<u8>,
    pub human: HumanName,
    pub last_used_at: Option<Timestamp>,
}

pub async fn insert_web_token(
    tx: &mut WriteTx,
    token_hash: &[u8],
    human_id: i64,
    label: Option<&str>,
    now: Timestamp,
) -> Result<i64, StoreError> {
    let created_at = now.as_millis();
    Ok(sqlx::query_scalar!(
        r#"INSERT INTO web_tokens (token_hash, human_id, label, created_at) VALUES (?, ?, ?, ?) RETURNING id AS "id!: i64""#,
        token_hash,
        human_id,
        label,
        created_at
    )
    .fetch_one(tx.conn())
    .await?)
}

/// The token whose hash is `token_hash`, with the name of the human it signs in.
pub async fn find_web_token(
    executor: impl SqliteExecutor<'_>,
    token_hash: &[u8],
) -> Result<Option<WebTokenRow>, StoreError> {
    let row = sqlx::query!(
        r#"SELECT t.id AS "id!: i64", t.token_hash, h.name, t.last_used_at
           FROM web_tokens t JOIN humans h ON h.id = t.human_id
           WHERE t.token_hash = ?"#,
        token_hash
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(WebTokenRow {
            id: row.id,
            token_hash: row.token_hash,
            human: HumanName::parse(&row.name).map_err(|_| StoreError::Corrupt("humans.name"))?,
            last_used_at: row
                .last_used_at
                .map(Timestamp::from_millis)
                .transpose()
                .map_err(|_| StoreError::Corrupt("web_tokens.last_used_at"))?,
        })
    })
    .transpose()
}

/// Sets `last_used_at` to `now` unless it is already later than `stale_before`. True when the row changed.
pub async fn touch_web_token(
    tx: &mut WriteTx,
    id: i64,
    now: Timestamp,
    stale_before: Timestamp,
) -> Result<bool, StoreError> {
    let now = now.as_millis();
    let stale_before = stale_before.as_millis();
    let result = sqlx::query!(
        "UPDATE web_tokens SET last_used_at = ? WHERE id = ? AND (last_used_at IS NULL OR last_used_at <= ?)",
        now,
        id,
        stale_before
    )
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}
