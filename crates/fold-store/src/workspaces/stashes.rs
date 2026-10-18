use fold_core::merge::ConflictKind;
use fold_core::object::ObjectHash;
use fold_core::{RevisionId, Timestamp};
use sqlx::SqliteConnection;

use super::columns::{number, timestamp, tree_hash};
use super::conflicts::ConflictRow;
use crate::error::StoreError;
use crate::store::WriteTx;

/// An entry of the stack, which only these two states keep there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StashState {
    Stashed,
    /// An unstash wrote conflict markers into the folder; the entry stays until a record finds none left.
    Conflicted,
}

/// One entry of the human's stash of the project folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashRow {
    /// The row id, which the daemon keeps to itself.
    pub id: i64,
    /// The checkout record's tree when the entry was made.
    pub base_tree: ObjectHash,
    /// The folder's tree when the entry was made.
    pub tree: ObjectHash,
    /// The main revision the folder held when the entry was made.
    pub revision: RevisionId,
    /// The event actor label of the human who made it.
    pub actor: String,
    pub state: StashState,
    pub created_at: Timestamp,
    /// When an unstash of the entry conflicted.
    pub conflicted_at: Option<Timestamp>,
    /// The paths of that conflict, in path byte order.
    pub conflicts: Vec<ConflictRow>,
}

/// A new entry; both trees are in the object store already.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewStash<'a> {
    pub base_tree: ObjectHash,
    pub tree: ObjectHash,
    pub revision: RevisionId,
    pub actor: &'a str,
    pub now: Timestamp,
}

/// Pushes an entry onto the stack in the caller's write transaction and returns its row id. A revision that does not exist is refused.
pub async fn insert_stash(tx: &mut WriteTx, stash: &NewStash<'_>) -> Result<i64, StoreError> {
    let (base, tree, revision, created_at) = (
        stash.base_tree.to_string(),
        stash.tree.to_string(),
        stash.revision.to_db(),
        stash.now.as_millis(),
    );
    let id = sqlx::query_scalar!(
        r#"INSERT INTO folder_stashes (number, base_tree_hash, tree_hash, revision_id, actor, state, created_at)
           SELECT (SELECT COALESCE(MAX(number), 0) + 1 FROM folder_stashes), ?1, ?2, r.id, ?4, 'stashed', ?5
           FROM revisions r WHERE r.number = ?3
           RETURNING id AS "id!: i64""#,
        base,
        tree,
        revision,
        stash.actor,
        created_at
    )
    .fetch_optional(tx.conn())
    .await?;
    id.ok_or(StoreError::RevisionNotFound(stash.revision))
}

/// The stack, newest entry first: `stash@{0}` is the first.
pub async fn stash_stack(conn: &mut SqliteConnection) -> Result<Vec<StashRow>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT s.id AS "id!: i64", s.base_tree_hash, s.tree_hash, r.number AS "revision!: i64",
                  s.actor, s.state, s.created_at AS "created_at!: i64", s.conflicted_at AS "conflicted_at?: i64"
           FROM folder_stashes s JOIN revisions r ON r.id = s.revision_id
           WHERE s.state IN ('stashed', 'conflicted')
           ORDER BY s.number DESC"#
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut stack = Vec::with_capacity(rows.len());
    for row in rows {
        let state = match row.state.as_str() {
            "stashed" => StashState::Stashed,
            "conflicted" => StashState::Conflicted,
            _ => return Err(StoreError::Corrupt("folder_stashes.state")),
        };
        let conflicts = conflicts_of(conn, row.id).await?;
        stack.push(StashRow {
            id: row.id,
            base_tree: tree_hash(&row.base_tree_hash, "folder_stashes.base_tree_hash")?,
            tree: tree_hash(&row.tree_hash, "folder_stashes.tree_hash")?,
            revision: number(
                row.revision,
                RevisionId::try_from_db,
                "folder_stashes.revision_id",
            )?,
            actor: row.actor,
            state,
            created_at: timestamp(row.created_at, "folder_stashes.created_at")?,
            conflicted_at: row
                .conflicted_at
                .map(|at| timestamp(at, "folder_stashes.conflicted_at"))
                .transpose()?,
            conflicts,
        });
    }
    Ok(stack)
}

async fn conflicts_of(
    conn: &mut SqliteConnection,
    id: i64,
) -> Result<Vec<ConflictRow>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT path, kind FROM folder_stash_conflicts WHERE stash_id = ? ORDER BY path"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ConflictRow {
                path: row.path,
                kind: ConflictKind::parse(&row.kind)
                    .ok_or(StoreError::Corrupt("folder_stash_conflicts.kind"))?,
            })
        })
        .collect()
}

/// Closes the stashed entry `id` as `applied`: an unstash merged it into the folder cleanly. False when it is no longer stashed.
pub async fn apply_stash(tx: &mut WriteTx, id: i64, now: Timestamp) -> Result<bool, StoreError> {
    let closed_at = now.as_millis();
    let applied = sqlx::query!(
        "UPDATE folder_stashes SET state = 'applied', closed_at = ? WHERE id = ? AND state = 'stashed'",
        closed_at,
        id
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(applied == 1)
}

/// Keeps the stashed entry `id` on the stack as `conflicted`, with the paths where its unstash conflicted. False when it is no longer stashed.
pub async fn conflict_stash(
    tx: &mut WriteTx,
    id: i64,
    conflicts: &[ConflictRow],
    now: Timestamp,
) -> Result<bool, StoreError> {
    let at = now.as_millis();
    let moved = sqlx::query!(
        "UPDATE folder_stashes SET state = 'conflicted', conflicted_at = ? WHERE id = ? AND state = 'stashed'",
        at,
        id
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if moved != 1 {
        return Ok(false);
    }
    for conflict in conflicts {
        let kind = conflict.kind.as_str();
        sqlx::query!(
            "INSERT INTO folder_stash_conflicts (stash_id, path, kind) VALUES (?, ?, ?)",
            id,
            conflict.path,
            kind
        )
        .execute(tx.conn())
        .await?;
    }
    Ok(true)
}

/// Closes the conflicted entry `id` as `dropped`: the folder holds no marker of its unstash any more. False when it is no longer conflicted.
pub async fn drop_stash(tx: &mut WriteTx, id: i64, now: Timestamp) -> Result<bool, StoreError> {
    let closed_at = now.as_millis();
    let dropped = sqlx::query!(
        "UPDATE folder_stashes SET state = 'dropped', closed_at = ? WHERE id = ? AND state = 'conflicted'",
        closed_at,
        id
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(dropped == 1)
}
