use std::time::Duration;

use fold_core::object::ObjectHash;
use fold_core::{Timestamp, WorkspaceId};
use sqlx::SqliteExecutor;

use crate::error::StoreError;
use crate::store::WriteTx;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Every object hash a row refers to: each revision's own hash and tree, the trees of every changeset, every proposal version and every unpruned checkpoint, the blob of every request attachment, both trees of every human change, and the project folder's checkout record. The bases of workspaces are revisions, so they are covered. One statement reads all of them, so the list is one consistent snapshot.
pub async fn gc_roots<'e>(db: impl SqliteExecutor<'e>) -> Result<Vec<ObjectHash>, StoreError> {
    let rows = sqlx::query_scalar!(
        r#"SELECT hash AS "hash!: String" FROM revisions
           UNION SELECT tree_hash FROM revisions
           UNION SELECT tree_hash FROM changesets
           UNION SELECT tree_hash FROM proposal_versions
           UNION SELECT tree_hash FROM checkpoints WHERE pruned_at IS NULL
           UNION SELECT object FROM request_attachments
           UNION SELECT base_tree_hash FROM human_changes
           UNION SELECT tree_hash FROM human_changes
           UNION SELECT tree_hash FROM checkout"#
    )
    .fetch_all(db)
    .await?;
    rows.iter()
        .map(|text| text.parse().map_err(|_| StoreError::Corrupt("gc_roots")))
        .collect()
}

/// The workspaces holding more than `keep` unpruned automatic checkpoints, the only ones `prune_auto` would change.
pub async fn workspaces_over_auto_limit<'e>(
    db: impl SqliteExecutor<'e>,
    keep: u32,
) -> Result<Vec<WorkspaceId>, StoreError> {
    let keep = i64::from(keep);
    let numbers = sqlx::query_scalar!(
        r#"SELECT w.number AS "number!: i64"
           FROM checkpoints c JOIN workspaces w ON w.id = c.workspace_id
           WHERE c.kind = 'auto' AND c.pruned_at IS NULL
           GROUP BY w.number
           HAVING COUNT(*) > ?
           ORDER BY w.number"#,
        keep
    )
    .fetch_all(db)
    .await?;
    numbers
        .into_iter()
        .map(|number| {
            WorkspaceId::try_from_db(number).ok_or(StoreError::Corrupt("workspaces.number"))
        })
        .collect()
}

/// An archived or abandoned workspace whose retention has passed. Review checkouts are workspaces too and follow the same rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedWorkspace {
    pub id: WorkspaceId,
    /// The directory as `workspaces.path` holds it.
    pub path: String,
    pub closed_at: Timestamp,
}

/// The archived or abandoned workspaces closed more than `closed_workspace_days` days before `now` whose directory gc has not removed yet, by id.
pub async fn closed_workspaces_due<'e>(
    db: impl SqliteExecutor<'e>,
    now: Timestamp,
    closed_workspace_days: u64,
) -> Result<Vec<ClosedWorkspace>, StoreError> {
    let retention = u32::try_from(closed_workspace_days)
        .ok()
        .and_then(|days| DAY.checked_mul(days));
    let Some(cutoff) = retention.and_then(|retention| now.checked_sub(retention)) else {
        return Ok(Vec::new());
    };
    let cutoff = cutoff.as_millis();
    let rows = sqlx::query!(
        r#"SELECT number AS "number!: i64", path, closed_at AS "closed_at!: i64"
           FROM workspaces
           WHERE state IN ('archived', 'abandoned') AND closed_at IS NOT NULL AND closed_at < ?
             AND directory_removed_at IS NULL
           ORDER BY number"#,
        cutoff
    )
    .fetch_all(db)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ClosedWorkspace {
                id: WorkspaceId::try_from_db(row.number)
                    .ok_or(StoreError::Corrupt("workspaces.number"))?,
                path: row.path,
                closed_at: Timestamp::from_millis(row.closed_at)
                    .map_err(|_| StoreError::Corrupt("workspaces.closed_at"))?,
            })
        })
        .collect()
}

/// Records that gc removed the directory of the closed workspace, so `closed_workspaces_due` leaves it out from then on. False when no closed workspace has this id.
pub async fn mark_directory_removed(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (workspace, now) = (workspace.to_db(), now.as_millis());
    let changed = sqlx::query!(
        "UPDATE workspaces SET directory_removed_at = ?2
         WHERE number = ?1 AND state IN ('archived', 'abandoned') AND directory_removed_at IS NULL",
        workspace,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(changed == 1)
}
