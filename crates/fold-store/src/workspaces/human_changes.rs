use fold_core::merge::ConflictKind;
use fold_core::object::ObjectHash;
use fold_core::{IntegrationId, Timestamp, WorkspaceId};
use sqlx::{SqliteConnection, SqliteExecutor};

use super::columns::{number, timestamp, tree_hash};
use super::conflicts::ConflictRow;
use crate::error::StoreError;
use crate::store::WriteTx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HumanChangeState {
    Recorded,
    Conflicted,
    Resolved,
    Superseded,
}

impl HumanChangeState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recorded => "recorded",
            Self::Conflicted => "conflicted",
            Self::Resolved => "resolved",
            Self::Superseded => "superseded",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "recorded" => Self::Recorded,
            "conflicted" => Self::Conflicted,
            "resolved" => Self::Resolved,
            "superseded" => Self::Superseded,
            _ => return None,
        })
    }
}

/// A human's commit on the project's Git branch with the two trees its merge needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewHumanChange<'a> {
    /// The full Git object id.
    pub commit: &'a str,
    pub human: &'a str,
    pub base_tree: ObjectHash,
    pub tree: ObjectHash,
    pub now: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanChangeRow {
    pub commit: String,
    pub human: String,
    pub base_tree: ObjectHash,
    pub tree: ObjectHash,
    pub state: HumanChangeState,
    pub integration: Option<IntegrationId>,
    /// The main workspace that holds the merge of a conflicted change.
    pub workspace: Option<WorkspaceId>,
    pub created_at: Timestamp,
    /// The paths that conflicted when the change was recorded, in path byte order.
    pub conflicts: Vec<ConflictRow>,
}

/// Closes the conflicted human change, when one waits, as `superseded`: a newer human change replaces it. Returns its commit.
pub async fn supersede_conflicted(
    tx: &mut WriteTx,
    now: Timestamp,
) -> Result<Option<String>, StoreError> {
    let closed_at = now.as_millis();
    let commit = sqlx::query_scalar!(
        r#"UPDATE human_changes SET state = 'superseded', closed_at = ?
           WHERE state = 'conflicted'
           RETURNING commit_id AS "commit!""#,
        closed_at
    )
    .fetch_optional(tx.conn())
    .await?;
    Ok(commit)
}

/// Inserts a human change that the `pull` integration `integration` brought onto main.
pub async fn insert_recorded_human_change(
    tx: &mut WriteTx,
    change: &NewHumanChange<'_>,
    integration: IntegrationId,
) -> Result<(), StoreError> {
    let integration = integration.to_db();
    let (base, tree, created_at) = (
        change.base_tree.to_string(),
        change.tree.to_string(),
        change.now.as_millis(),
    );
    sqlx::query!(
        r#"INSERT INTO human_changes (commit_id, human, base_tree_hash, tree_hash, state, integration_id, created_at, closed_at)
           VALUES (?, ?, ?, ?, 'recorded', (SELECT id FROM integrations WHERE number = ?), ?, ?)"#,
        change.commit,
        change.human,
        base,
        tree,
        integration,
        created_at,
        created_at
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Inserts a human change whose merge with main conflicted, with its conflicted paths. A second conflicted change is refused by the unique index; supersede the first one before.
pub async fn insert_conflicted_human_change(
    tx: &mut WriteTx,
    change: &NewHumanChange<'_>,
    conflicts: &[ConflictRow],
) -> Result<(), StoreError> {
    let (base, tree, created_at) = (
        change.base_tree.to_string(),
        change.tree.to_string(),
        change.now.as_millis(),
    );
    let id = sqlx::query_scalar!(
        r#"INSERT INTO human_changes (commit_id, human, base_tree_hash, tree_hash, state, created_at)
           VALUES (?, ?, ?, ?, 'conflicted', ?)
           RETURNING id AS "id!: i64""#,
        change.commit,
        change.human,
        base,
        tree,
        created_at
    )
    .fetch_one(tx.conn())
    .await?;
    for conflict in conflicts {
        let kind = conflict.kind.as_str();
        sqlx::query!(
            "INSERT INTO human_change_conflicts (human_change_id, path, kind) VALUES (?, ?, ?)",
            id,
            conflict.path,
            kind
        )
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

/// Names `workspace`, a main workspace, as the one that holds the merge of the conflicted human change. False when no change is conflicted.
pub async fn set_human_change_workspace(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
) -> Result<bool, StoreError> {
    let workspace = workspace.to_db();
    let updated = sqlx::query!(
        "UPDATE human_changes SET workspace_id = (SELECT id FROM workspaces WHERE number = ?)
         WHERE state = 'conflicted'",
        workspace
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// Closes the conflicted human change of `commit` as `resolved` by `integration`, the integration that brought its changes onto main: the direct change of the merge the main agent prepared, or any other integration whose tree holds them. False when that change no longer waits.
pub async fn resolve_human_change(
    tx: &mut WriteTx,
    commit: &str,
    integration: IntegrationId,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (integration, closed_at) = (integration.to_db(), now.as_millis());
    let resolved = sqlx::query!(
        r#"UPDATE human_changes
           SET state = 'resolved', closed_at = ?,
               integration_id = (SELECT id FROM integrations WHERE number = ?)
           WHERE state = 'conflicted' AND commit_id = ?"#,
        closed_at,
        integration,
        commit
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(resolved == 1)
}

/// The human change that waits for the main agent: the conflicted one.
pub async fn conflicted_human_change(
    conn: &mut SqliteConnection,
) -> Result<Option<HumanChangeRow>, StoreError> {
    let row = sqlx::query!(
        r#"SELECT hc.id AS "id!: i64", hc.commit_id, hc.human, hc.base_tree_hash, hc.tree_hash, hc.state,
                  i.number AS "integration?: i64", w.number AS "workspace?: i64",
                  hc.created_at AS "created_at!: i64"
           FROM human_changes hc
           LEFT JOIN integrations i ON i.id = hc.integration_id
           LEFT JOIN workspaces w ON w.id = hc.workspace_id
           WHERE hc.state = 'conflicted'"#
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let fields = Fields {
        commit: row.commit_id,
        human: row.human,
        base_tree: row.base_tree_hash,
        tree: row.tree_hash,
        state: row.state,
        integration: row.integration,
        workspace: row.workspace,
        created_at: row.created_at,
    };
    Ok(Some(fields.into_row(conflicts_of(conn, row.id).await?)?))
}

/// The newest human change whose merge `workspace` holds, in any state.
pub async fn human_change_of_workspace(
    conn: &mut SqliteConnection,
    workspace: WorkspaceId,
) -> Result<Option<HumanChangeRow>, StoreError> {
    let workspace = workspace.to_db();
    let row = sqlx::query!(
        r#"SELECT hc.id AS "id!: i64", hc.commit_id, hc.human, hc.base_tree_hash, hc.tree_hash, hc.state,
                  i.number AS "integration?: i64", w.number AS "workspace?: i64",
                  hc.created_at AS "created_at!: i64"
           FROM human_changes hc
           JOIN workspaces w ON w.id = hc.workspace_id
           LEFT JOIN integrations i ON i.id = hc.integration_id
           WHERE w.number = ?
           ORDER BY hc.id DESC
           LIMIT 1"#,
        workspace
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let fields = Fields {
        commit: row.commit_id,
        human: row.human,
        base_tree: row.base_tree_hash,
        tree: row.tree_hash,
        state: row.state,
        integration: row.integration,
        workspace: row.workspace,
        created_at: row.created_at,
    };
    Ok(Some(fields.into_row(conflicts_of(conn, row.id).await?)?))
}

struct Fields {
    commit: String,
    human: String,
    base_tree: String,
    tree: String,
    state: String,
    integration: Option<i64>,
    workspace: Option<i64>,
    created_at: i64,
}

impl Fields {
    fn into_row(self, conflicts: Vec<ConflictRow>) -> Result<HumanChangeRow, StoreError> {
        Ok(HumanChangeRow {
            commit: self.commit,
            human: self.human,
            base_tree: tree_hash(&self.base_tree, "human_changes.base_tree_hash")?,
            tree: tree_hash(&self.tree, "human_changes.tree_hash")?,
            state: HumanChangeState::parse(&self.state)
                .ok_or(StoreError::Corrupt("human_changes.state"))?,
            integration: self
                .integration
                .map(|value| number(value, IntegrationId::try_from_db, "integrations.number"))
                .transpose()?,
            workspace: self
                .workspace
                .map(|value| number(value, WorkspaceId::try_from_db, "workspaces.number"))
                .transpose()?,
            created_at: timestamp(self.created_at, "human_changes.created_at")?,
            conflicts,
        })
    }
}

async fn conflicts_of<'e>(
    executor: impl SqliteExecutor<'e>,
    id: i64,
) -> Result<Vec<ConflictRow>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT path AS "path!: Vec<u8>", kind FROM human_change_conflicts
           WHERE human_change_id = ? ORDER BY path"#,
        id
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ConflictRow {
                path: row.path,
                kind: ConflictKind::parse(&row.kind)
                    .ok_or(StoreError::Corrupt("human_change_conflicts.kind"))?,
            })
        })
        .collect()
}

/// The commit and tree of the newest human change, or of the newest one that main holds (`recorded` or `resolved`) when `on_main` is true.
pub async fn newest_human_change(
    conn: &mut SqliteConnection,
    on_main: bool,
) -> Result<Option<(String, ObjectHash)>, StoreError> {
    let row = sqlx::query!(
        r#"SELECT commit_id, tree_hash FROM human_changes
           WHERE NOT ?1 OR state IN ('recorded', 'resolved')
           ORDER BY id DESC
           LIMIT 1"#,
        on_main
    )
    .fetch_optional(&mut *conn)
    .await?;
    row.map(|row| {
        Ok((
            row.commit_id,
            tree_hash(&row.tree_hash, "human_changes.tree_hash")?,
        ))
    })
    .transpose()
}
