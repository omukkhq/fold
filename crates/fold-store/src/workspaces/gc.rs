use std::time::Duration;

use fold_core::object::ObjectHash;
use fold_core::{Timestamp, WorkspaceId};
use sqlx::SqliteExecutor;

use crate::error::StoreError;
use crate::store::WriteTx;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Every object hash a row refers to: each revision's own hash and tree, the trees of every changeset, every proposal version and every unpruned checkpoint, the blob of every request attachment, both trees of every human change and of every entry on the stash stack, and the project folder's checkout record. The bases of workspaces are revisions, so they are covered. One statement reads all of them, so the list is one consistent snapshot.
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
           UNION SELECT tree_hash FROM checkout
           UNION SELECT base_tree_hash FROM folder_stashes WHERE state IN ('stashed', 'conflicted')
           UNION SELECT tree_hash FROM folder_stashes WHERE state IN ('stashed', 'conflicted')"#
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

/// An archived or abandoned workspace whose directory fold has not removed yet. Review checkouts are workspaces too and follow the same rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedWorkspace {
    pub id: WorkspaceId,
    /// The directory as `workspaces.path` holds it.
    pub path: String,
    pub closed_at: Timestamp,
    /// fold started to remove the directory (`start_directory_removal`), so what is left of it is only deleted.
    pub removal_started: bool,
}

/// The archived or abandoned workspaces whose directory fold has not removed yet, by id, or only `workspace` when it is one of them. A workspace whose owning agent has a worker process that has not ended is left out until it ends, so fold never removes the files a running worker can still use.
pub async fn closed_directories_due<'e>(
    db: impl SqliteExecutor<'e>,
    workspace: Option<WorkspaceId>,
) -> Result<Vec<ClosedWorkspace>, StoreError> {
    let only = workspace.map(WorkspaceId::to_db);
    let rows = sqlx::query!(
        r#"SELECT w.number AS "number!: i64", w.path, w.closed_at AS "closed_at!: i64",
                  w.removal_started_at IS NOT NULL AS "removal_started!: bool"
           FROM workspaces w
           WHERE w.state IN ('archived', 'abandoned') AND w.closed_at IS NOT NULL
             AND w.directory_removed_at IS NULL
             AND (?1 IS NULL OR w.number = ?1)
             AND NOT EXISTS (
               SELECT 1 FROM workers k JOIN worker_processes p ON p.agent_id = k.agent_id
               WHERE k.id = w.worker_id AND p.state NOT IN ('stopped', 'failed'))
           ORDER BY w.number"#,
        only
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
                removal_started: row.removal_started,
            })
        })
        .collect()
}

/// The archived or abandoned workspaces whose directory fold removed, with an unpruned checkpoint, whose close and directory removal both came more than `closed_workspace_days` days before `now`, by id. Counting from the later of the two keeps the checkpoint a late removal takes for the whole retention.
pub async fn closed_checkpoints_due<'e>(
    db: impl SqliteExecutor<'e>,
    now: Timestamp,
    closed_workspace_days: u64,
) -> Result<Vec<WorkspaceId>, StoreError> {
    let retention = u32::try_from(closed_workspace_days)
        .ok()
        .and_then(|days| DAY.checked_mul(days));
    let Some(cutoff) = retention.and_then(|retention| now.checked_sub(retention)) else {
        return Ok(Vec::new());
    };
    let cutoff = cutoff.as_millis();
    let numbers = sqlx::query_scalar!(
        r#"SELECT DISTINCT w.number AS "number!: i64"
           FROM workspaces w JOIN checkpoints c ON c.workspace_id = w.id
           WHERE w.state IN ('archived', 'abandoned') AND c.pruned_at IS NULL
             AND w.closed_at < ?1 AND w.directory_removed_at < ?1
           ORDER BY w.number"#,
        cutoff
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

/// Whether fold already holds `tree` for the workspace: as the tree of its base revision, of an unpruned checkpoint, of one of its changesets, or of a version of a proposal of its task. A closed workspace whose files are such a tree loses nothing when its directory goes.
pub async fn tree_recorded<'e>(
    db: impl SqliteExecutor<'e>,
    workspace: WorkspaceId,
    tree: &ObjectHash,
) -> Result<bool, StoreError> {
    let (workspace, tree) = (workspace.to_db(), tree.to_string());
    let recorded = sqlx::query_scalar!(
        r#"SELECT EXISTS (
             SELECT 1 FROM workspaces w JOIN revisions r ON r.id = w.base_revision_id
             WHERE w.number = ?1 AND r.tree_hash = ?2
             UNION ALL
             SELECT 1 FROM workspaces w JOIN checkpoints c ON c.workspace_id = w.id
             WHERE w.number = ?1 AND c.pruned_at IS NULL AND c.tree_hash = ?2
             UNION ALL
             SELECT 1 FROM workspaces w JOIN changesets c ON c.workspace_id = w.id
             WHERE w.number = ?1 AND c.tree_hash = ?2
             UNION ALL
             SELECT 1 FROM workspaces w JOIN proposals p ON p.task_id = w.task_id
               JOIN proposal_versions v ON v.proposal_id = p.id
             WHERE w.number = ?1 AND v.tree_hash = ?2
           ) AS "recorded!: bool""#,
        workspace,
        tree
    )
    .fetch_one(db)
    .await?;
    Ok(recorded)
}

/// Records that fold started to remove the directory of the closed workspace, in the write that commits its final checkpoint when it needs one. False when no closed workspace whose directory is still there has this id, or the start is already recorded.
pub async fn start_directory_removal(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (workspace, now) = (workspace.to_db(), now.as_millis());
    let changed = sqlx::query!(
        "UPDATE workspaces SET removal_started_at = ?2
         WHERE number = ?1 AND state IN ('archived', 'abandoned')
           AND directory_removed_at IS NULL AND removal_started_at IS NULL",
        workspace,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(changed == 1)
}

/// Records that fold removed the directory of the closed workspace, so `closed_directories_due` leaves it out from then on and `closed_checkpoints_due` counts its retention. False when no closed workspace has this id.
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
