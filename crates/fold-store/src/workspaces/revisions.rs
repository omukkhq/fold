use fold_core::object::ObjectHash;
use fold_core::revision::{Revision, StoredRevision};
use fold_core::{IdKind, IntegrationId, ProposalId, RevisionId, TaskId, Timestamp};
use sqlx::{Executor, Sqlite, SqlitePool};

use super::refs;
use crate::counters;
use crate::error::StoreError;
use crate::store::{Store, WriteTx};

/// The most revisions one `list_recent` or `ancestors` call returns.
pub const MAX_PAGE: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionRow {
    pub number: RevisionId,
    pub hash: ObjectHash,
    pub tree_hash: ObjectHash,
    pub parent: Option<RevisionId>,
    pub created_at: Timestamp,
    /// Where `r1` came from; `None` on every other revision.
    pub import_source: Option<String>,
    pub integration: Option<IntegrationInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationKind {
    Proposal,
    Pull,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationInfo {
    /// The number the revision encoding records.
    pub number: IntegrationId,
    pub kind: IntegrationKind,
    pub actor: String,
    /// The Git commit a pull brought in.
    pub remote_commit: Option<String>,
    pub proposal: Option<IntegratedProposal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegratedProposal {
    pub number: ProposalId,
    pub title: String,
    pub task: TaskId,
    pub agent: String,
    pub worker: String,
}

/// The initial revision for `tree`: no parent, no integration. The caller stores it with `fold-vcs` before `create_initial_revision`.
pub fn initial_revision(tree: ObjectHash, now: Timestamp) -> Result<Revision, StoreError> {
    Ok(Revision {
        tree,
        parent: None,
        number: RevisionId::from_number(1).ok_or(StoreError::Corrupt("revision number 1"))?,
        integration: None,
        created_at: now,
    })
}

/// Inserts `stored` in the caller's write transaction. Its number must be the next revision number and the parent's number plus one, and its integration must already have a row.
pub async fn insert_revision(
    tx: &mut WriteTx,
    stored: &StoredRevision,
) -> Result<RevisionRow, StoreError> {
    insert(tx, stored, None).await
}

async fn insert(
    tx: &mut WriteTx,
    stored: &StoredRevision,
    import_source: Option<&str>,
) -> Result<RevisionRow, StoreError> {
    let revision = stored.revision();
    let parent_id = match &revision.parent {
        None => None,
        Some(parent_hash) => {
            let parent_hash = parent_hash.to_string();
            let parent = sqlx::query!(
                r#"SELECT id AS "id!: i64", number FROM revisions WHERE hash = ?"#,
                parent_hash
            )
            .fetch_optional(tx.conn())
            .await?
            .ok_or(StoreError::Invalid("the parent revision does not exist"))?;
            if parent.number + 1 != revision.number.to_db() {
                return Err(StoreError::Invalid(
                    "the revision number is not the parent's number plus one",
                ));
            }
            Some(parent.id)
        }
    };
    let integration_id = match revision.integration {
        None => None,
        Some(integration) => {
            let number = integration.to_db();
            let id = sqlx::query_scalar!(
                r#"SELECT id AS "id!: i64" FROM integrations WHERE number = ?"#,
                number
            )
            .fetch_optional(tx.conn())
            .await?
            .ok_or(StoreError::Invalid("the integration does not exist"))?;
            Some(id)
        }
    };
    let allocated = counters::allocate(tx, IdKind::Revision).await?;
    if allocated != revision.number.number() {
        return Err(StoreError::Invalid(
            "the revision number is not the next revision number",
        ));
    }
    let number = revision.number.to_db();
    let hash_text = stored.hash().to_string();
    let tree_text = revision.tree.to_string();
    let created_at = revision.created_at.as_millis();
    sqlx::query!(
        "INSERT INTO revisions (number, hash, tree_hash, parent_id, integration_id, import_source, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        number,
        hash_text,
        tree_text,
        parent_id,
        integration_id,
        import_source,
        created_at
    )
    .execute(tx.conn())
    .await?;
    one(tx.conn(), revision.number)
        .await?
        .ok_or(StoreError::Corrupt("revisions.number"))
}

/// Creates `r1` and the `main` ref in one transaction. `stored` holds `initial_revision(..)`, and the caller wrote its tree and its object first. `import_source` is the canonical path of an imported directory or Git repository, `None` for a plain init.
pub async fn create_initial_revision(
    store: &Store,
    stored: &StoredRevision,
    import_source: Option<&str>,
) -> Result<RevisionRow, StoreError> {
    let revision = stored.revision();
    if revision.parent.is_some() || revision.integration.is_some() {
        return Err(StoreError::Invalid(
            "the initial revision has no parent and no integration",
        ));
    }
    store
        .write(async |tx| {
            let row = insert(tx, stored, import_source).await?;
            refs::create_main(tx, row.number).await?;
            Ok(row)
        })
        .await
}

/// The revision the `main` ref names, or `None` before `r1` exists.
pub async fn head(pool: &SqlitePool) -> Result<Option<RevisionRow>, StoreError> {
    let number = sqlx::query_scalar!(
        r#"SELECT r.number AS "number!: i64"
           FROM refs JOIN revisions r ON r.id = refs.revision_id
           WHERE refs.name = 'main'"#
    )
    .fetch_optional(pool)
    .await?;
    match number {
        None => Ok(None),
        Some(number) => one(pool, revision_id(number)?).await,
    }
}

pub async fn get_by_number(
    pool: &SqlitePool,
    number: RevisionId,
) -> Result<RevisionRow, StoreError> {
    one(pool, number)
        .await?
        .ok_or(StoreError::RevisionNotFound(number))
}

pub async fn get_by_hash(
    pool: &SqlitePool,
    hash: &ObjectHash,
) -> Result<Option<RevisionRow>, StoreError> {
    let hash = hash.to_string();
    let number = sqlx::query_scalar!(
        r#"SELECT number AS "number!: i64" FROM revisions WHERE hash = ?"#,
        hash
    )
    .fetch_optional(pool)
    .await?;
    match number {
        None => Ok(None),
        Some(number) => one(pool, revision_id(number)?).await,
    }
}

/// Newest first, at most `limit` (capped at [`MAX_PAGE`]). `before` pages: it returns the revisions numbered below it.
pub async fn list_recent(
    pool: &SqlitePool,
    limit: usize,
    before: Option<RevisionId>,
) -> Result<Vec<RevisionRow>, StoreError> {
    let below = before.map_or(i64::MAX, |before| before.to_db() - 1);
    newest_first(pool, below, page(limit)).await
}

/// Every revision numbered above `after`, oldest first, for the Git export.
pub async fn list_after(
    pool: &SqlitePool,
    after: RevisionId,
) -> Result<Vec<RevisionRow>, StoreError> {
    oldest_first(pool, after.to_db() + 1, i64::MAX, NO_LIMIT).await
}

/// The revisions below `from`, newest first, at most `limit`. Every revision's parent is the revision numbered one lower, so numbers are the ancestry.
pub async fn ancestors(
    pool: &SqlitePool,
    from: RevisionId,
    limit: usize,
) -> Result<Vec<RevisionRow>, StoreError> {
    get_by_number(pool, from).await?;
    list_recent(pool, limit, Some(from)).await
}

/// SQLite reads a negative `LIMIT` as no limit.
const NO_LIMIT: i64 = -1;

fn page(limit: usize) -> i64 {
    i64::try_from(limit.min(MAX_PAGE)).unwrap_or(0)
}

fn revision_id(number: i64) -> Result<RevisionId, StoreError> {
    RevisionId::try_from_db(number).ok_or(StoreError::Corrupt("revisions.number"))
}

/// The columns of one revision with its integration, shared by the two ordered reads.
struct RawRow {
    number: i64,
    hash: String,
    tree_hash: String,
    parent: Option<i64>,
    created_at: i64,
    import_source: Option<String>,
    integration_number: Option<i64>,
    integration_kind: Option<String>,
    integration_actor: Option<String>,
    remote_commit: Option<String>,
    proposal_number: Option<i64>,
    proposal_title: Option<String>,
    task_number: Option<i64>,
    agent: Option<String>,
    worker: Option<String>,
}

async fn one<'e, E>(executor: E, number: RevisionId) -> Result<Option<RevisionRow>, StoreError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let number = number.to_db();
    Ok(oldest_first(executor, number, number, 1)
        .await?
        .into_iter()
        .next())
}

// The two reads differ only in their sort order; both walk the unique index on `number`.
async fn oldest_first<'e, E>(
    executor: E,
    from: i64,
    to: i64,
    limit: i64,
) -> Result<Vec<RevisionRow>, StoreError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let rows = sqlx::query_as!(
        RawRow,
        r#"SELECT r.number AS "number!: i64", r.hash AS "hash!", r.tree_hash AS "tree_hash!",
                  p.number AS "parent?: i64", r.created_at AS "created_at!: i64", r.import_source,
                  i.number AS "integration_number?: i64", i.kind AS "integration_kind?",
                  i.actor AS "integration_actor?", i.remote_commit AS "remote_commit?",
                  pr.number AS "proposal_number?: i64", pr.title AS "proposal_title?",
                  t.number AS "task_number?: i64", a.name AS "agent?", w.name AS "worker?"
           FROM revisions r
           LEFT JOIN revisions p ON p.id = r.parent_id
           LEFT JOIN integrations i ON i.id = r.integration_id
           LEFT JOIN proposals pr ON pr.id = i.proposal_id
           LEFT JOIN tasks t ON t.id = pr.task_id
           LEFT JOIN workers w ON w.id = pr.author_worker_id
           LEFT JOIN agents a ON a.id = w.agent_id
           WHERE r.number BETWEEN ?1 AND ?2
           ORDER BY r.number ASC
           LIMIT ?3"#,
        from,
        to,
        limit
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(row).collect()
}

async fn newest_first<'e, E>(
    executor: E,
    to: i64,
    limit: i64,
) -> Result<Vec<RevisionRow>, StoreError>
where
    E: Executor<'e, Database = Sqlite>,
{
    let rows = sqlx::query_as!(
        RawRow,
        r#"SELECT r.number AS "number!: i64", r.hash AS "hash!", r.tree_hash AS "tree_hash!",
                  p.number AS "parent?: i64", r.created_at AS "created_at!: i64", r.import_source,
                  i.number AS "integration_number?: i64", i.kind AS "integration_kind?",
                  i.actor AS "integration_actor?", i.remote_commit AS "remote_commit?",
                  pr.number AS "proposal_number?: i64", pr.title AS "proposal_title?",
                  t.number AS "task_number?: i64", a.name AS "agent?", w.name AS "worker?"
           FROM revisions r
           LEFT JOIN revisions p ON p.id = r.parent_id
           LEFT JOIN integrations i ON i.id = r.integration_id
           LEFT JOIN proposals pr ON pr.id = i.proposal_id
           LEFT JOIN tasks t ON t.id = pr.task_id
           LEFT JOIN workers w ON w.id = pr.author_worker_id
           LEFT JOIN agents a ON a.id = w.agent_id
           WHERE r.number <= ?1
           ORDER BY r.number DESC
           LIMIT ?2"#,
        to,
        limit
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(row).collect()
}

fn row(raw: RawRow) -> Result<RevisionRow, StoreError> {
    let integration = match (raw.integration_number, raw.integration_kind) {
        (Some(number), Some(kind)) => Some(IntegrationInfo {
            number: IntegrationId::try_from_db(number)
                .ok_or(StoreError::Corrupt("integrations.number"))?,
            kind: match kind.as_str() {
                "proposal" => IntegrationKind::Proposal,
                "pull" => IntegrationKind::Pull,
                _ => return Err(StoreError::Corrupt("integrations.kind")),
            },
            actor: raw
                .integration_actor
                .ok_or(StoreError::Corrupt("integrations.actor"))?,
            remote_commit: raw.remote_commit,
            proposal: match (
                raw.proposal_number,
                raw.proposal_title,
                raw.task_number,
                raw.agent,
                raw.worker,
            ) {
                (Some(number), Some(title), Some(task), Some(agent), Some(worker)) => {
                    Some(IntegratedProposal {
                        number: ProposalId::try_from_db(number)
                            .ok_or(StoreError::Corrupt("proposals.number"))?,
                        title,
                        task: TaskId::try_from_db(task)
                            .ok_or(StoreError::Corrupt("tasks.number"))?,
                        agent,
                        worker,
                    })
                }
                _ => None,
            },
        }),
        _ => None,
    };
    Ok(RevisionRow {
        number: revision_id(raw.number)?,
        hash: parse_hash(&raw.hash, "revisions.hash")?,
        tree_hash: parse_hash(&raw.tree_hash, "revisions.tree_hash")?,
        parent: match raw.parent {
            None => None,
            Some(parent) => Some(
                RevisionId::try_from_db(parent)
                    .ok_or(StoreError::Corrupt("revisions.parent_id"))?,
            ),
        },
        created_at: Timestamp::from_millis(raw.created_at)
            .map_err(|_| StoreError::Corrupt("revisions.created_at"))?,
        import_source: raw.import_source,
        integration,
    })
}

fn parse_hash(text: &str, column: &'static str) -> Result<ObjectHash, StoreError> {
    text.parse().map_err(|_| StoreError::Corrupt(column))
}
