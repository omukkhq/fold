use fold_core::{HumanName, Timestamp, WebTokenId};
use sqlx::{SqliteExecutor, SqlitePool};

use crate::columns::{optional_time, time};
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
    pub human_id: i64,
    pub human: HumanName,
    pub last_used_at: Option<Timestamp>,
}

pub async fn insert_web_token(
    tx: &mut WriteTx,
    token_hash: &[u8],
    human_id: i64,
    label: Option<&str>,
    now: Timestamp,
) -> Result<WebTokenId, StoreError> {
    let created_at = now.as_millis();
    // The registry has no counters table; every registry write goes through its one writer, so `MAX + 1` inside this transaction is the next free number. A revoked token keeps its row and so its number.
    let number = sqlx::query_scalar!(
        r#"INSERT INTO web_tokens (number, token_hash, human_id, label, created_at)
           VALUES ((SELECT COALESCE(MAX(number), 0) + 1 FROM web_tokens), ?, ?, ?, ?)
           RETURNING number AS "number!: i64""#,
        token_hash,
        human_id,
        label,
        created_at
    )
    .fetch_one(tx.conn())
    .await?;
    WebTokenId::try_from_db(number).ok_or(StoreError::Corrupt("web_tokens.number"))
}

/// The unrevoked token whose hash is `token_hash`, with the human it signs in.
pub async fn find_web_token(
    executor: impl SqliteExecutor<'_>,
    token_hash: &[u8],
) -> Result<Option<WebTokenRow>, StoreError> {
    let row = sqlx::query!(
        r#"SELECT t.id AS "id!: i64", t.token_hash, t.human_id, h.name, t.last_used_at
           FROM web_tokens t JOIN humans h ON h.id = t.human_id
           WHERE t.token_hash = ? AND t.revoked_at IS NULL"#,
        token_hash
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(WebTokenRow {
            id: row.id,
            token_hash: row.token_hash,
            human_id: row.human_id,
            human: HumanName::parse(&row.name).map_err(|_| StoreError::Corrupt("humans.name"))?,
            last_used_at: optional_time(row.last_used_at, "web_tokens.last_used_at")?,
        })
    })
    .transpose()
}

/// What `fld token list` shows of a token: never its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebTokenEntry {
    pub id: WebTokenId,
    pub human: HumanName,
    pub label: Option<String>,
    pub created_at: Timestamp,
    pub last_used_at: Option<Timestamp>,
}

/// Every unrevoked token, oldest first.
pub async fn list_web_tokens(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<WebTokenEntry>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT t.number, h.name, t.label, t.created_at, t.last_used_at
           FROM web_tokens t JOIN humans h ON h.id = t.human_id
           WHERE t.revoked_at IS NULL
           ORDER BY t.number"#
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(WebTokenEntry {
                id: WebTokenId::try_from_db(row.number)
                    .ok_or(StoreError::Corrupt("web_tokens.number"))?,
                human: HumanName::parse(&row.name)
                    .map_err(|_| StoreError::Corrupt("humans.name"))?,
                label: row.label,
                created_at: time(row.created_at, "web_tokens.created_at")?,
                last_used_at: optional_time(row.last_used_at, "web_tokens.last_used_at")?,
            })
        })
        .collect()
}

/// Marks the token revoked and deletes every session made from it. False when no unrevoked token has this id. The row stays, so its number is never reused.
pub async fn revoke_web_token(
    tx: &mut WriteTx,
    id: WebTokenId,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let number = id.to_db();
    let now = now.as_millis();
    let Some(row_id) = sqlx::query_scalar!(
        r#"UPDATE web_tokens SET revoked_at = ? WHERE number = ? AND revoked_at IS NULL
           RETURNING id AS "id!: i64""#,
        now,
        number
    )
    .fetch_optional(tx.conn())
    .await?
    else {
        return Ok(false);
    };
    sqlx::query!("DELETE FROM web_sessions WHERE token_id = ?", row_id)
        .execute(tx.conn())
        .await?;
    Ok(true)
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

/// A web sign-in session. The registry holds only the session id's BLAKE3 hash, never the id the cookie carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSessionRow {
    pub id: i64,
    pub session_hash: Vec<u8>,
    pub token_id: i64,
    pub human: HumanName,
    pub last_used_at: Timestamp,
    pub expires_at: Timestamp,
}

/// The fields of a new session row.
#[derive(Debug, Clone, Copy)]
pub struct NewWebSession<'a> {
    pub session_hash: &'a [u8],
    pub token_id: i64,
    pub human_id: i64,
    pub now: Timestamp,
    pub expires_at: Timestamp,
}

pub async fn insert_web_session(
    tx: &mut WriteTx,
    session: NewWebSession<'_>,
) -> Result<i64, StoreError> {
    let now = session.now.as_millis();
    let expires_at = session.expires_at.as_millis();
    Ok(sqlx::query_scalar!(
        r#"INSERT INTO web_sessions (session_hash, token_id, human_id, created_at, last_used_at, expires_at)
           VALUES (?, ?, ?, ?, ?, ?) RETURNING id AS "id!: i64""#,
        session.session_hash,
        session.token_id,
        session.human_id,
        now,
        now,
        expires_at
    )
    .fetch_one(tx.conn())
    .await?)
}

/// The session whose hash is `session_hash`, while its token is unrevoked. Expiry is the caller's decision, so the row comes back even when `expires_at` has passed.
pub async fn find_web_session(
    executor: impl SqliteExecutor<'_>,
    session_hash: &[u8],
) -> Result<Option<WebSessionRow>, StoreError> {
    let row = sqlx::query!(
        r#"SELECT s.id AS "id!: i64", s.session_hash, s.token_id, h.name, s.last_used_at, s.expires_at
           FROM web_sessions s
           JOIN web_tokens t ON t.id = s.token_id
           JOIN humans h ON h.id = s.human_id
           WHERE s.session_hash = ? AND t.revoked_at IS NULL"#,
        session_hash
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(WebSessionRow {
            id: row.id,
            session_hash: row.session_hash,
            token_id: row.token_id,
            human: HumanName::parse(&row.name).map_err(|_| StoreError::Corrupt("humans.name"))?,
            last_used_at: time(row.last_used_at, "web_sessions.last_used_at")?,
            expires_at: time(row.expires_at, "web_sessions.expires_at")?,
        })
    })
    .transpose()
}

/// Sets `last_used_at` of the session and of its token to `now`, each unless it is already later than `stale_before`.
pub async fn touch_web_session(
    tx: &mut WriteTx,
    session: &WebSessionRow,
    now: Timestamp,
    stale_before: Timestamp,
) -> Result<(), StoreError> {
    let now = now.as_millis();
    let stale_before = stale_before.as_millis();
    sqlx::query!(
        "UPDATE web_sessions SET last_used_at = ? WHERE id = ? AND last_used_at <= ?",
        now,
        session.id,
        stale_before
    )
    .execute(tx.conn())
    .await?;
    sqlx::query!(
        "UPDATE web_tokens SET last_used_at = ? WHERE id = ? AND (last_used_at IS NULL OR last_used_at <= ?)",
        now,
        session.token_id,
        stale_before
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Deletes the session whose hash is `session_hash`. True when a row went.
pub async fn delete_web_session(tx: &mut WriteTx, session_hash: &[u8]) -> Result<bool, StoreError> {
    let result = sqlx::query!(
        "DELETE FROM web_sessions WHERE session_hash = ?",
        session_hash
    )
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Deletes every session whose `expires_at` is not later than `now`, and returns how many went.
pub async fn delete_expired_web_sessions(
    tx: &mut WriteTx,
    now: Timestamp,
) -> Result<u64, StoreError> {
    let now = now.as_millis();
    let result = sqlx::query!("DELETE FROM web_sessions WHERE expires_at <= ?", now)
        .execute(tx.conn())
        .await?;
    Ok(result.rows_affected())
}
