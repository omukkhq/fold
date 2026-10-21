use fold_core::checkpoint_policy::{AutoReason, prune_set};
use fold_core::object::ObjectHash;
use fold_core::{CheckpointId, IdKind, RevisionId, Timestamp, WorkspaceId};
use sqlx::SqliteExecutor;

use super::columns::{number, timestamp, tree_hash};
use crate::counters;
use crate::error::StoreError;
use crate::store::WriteTx;

/// How many automatic checkpoints of one workspace stay unpruned when the project sets no other number.
pub const AUTO_CHECKPOINTS_KEPT: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointKind {
    Auto,
    Explicit,
}

impl CheckpointKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckpointKind::Auto => "auto",
            CheckpointKind::Explicit => "explicit",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "auto" => Some(CheckpointKind::Auto),
            "explicit" => Some(CheckpointKind::Explicit),
            _ => None,
        }
    }
}

/// Why a checkpoint was taken. An agent's own request, a human edit and a reviewer's fix are explicit checkpoints; every other reason belongs to an automatic checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointReason {
    Explicit,
    BeforeSync,
    BeforeRecord,
    BeforeProposal,
    BeforeProposalUpdate,
    BeforeRestore,
    /// The workspace was quiet for two minutes after a change.
    Quiet,
    /// The tree of a human's edit of a proposal version, which the task worker's next sync or restore writes into the workspace.
    HumanEdit,
    /// The tree of a reviewer's fix of a proposal version, made in its review checkout, which the task worker's next sync or restore writes into the workspace.
    ReviewFix,
    /// The files a closed workspace held that fold had not recorded, taken just before fold removes its directory. It is explicit, so the pruning of automatic checkpoints never removes the only record of those files; it goes with the workspace's other checkpoints after `closed_workspace_days`.
    BeforeRemoval,
}

impl CheckpointReason {
    pub const ALL: [CheckpointReason; 10] = [
        CheckpointReason::Explicit,
        CheckpointReason::BeforeSync,
        CheckpointReason::BeforeRecord,
        CheckpointReason::BeforeProposal,
        CheckpointReason::BeforeProposalUpdate,
        CheckpointReason::BeforeRestore,
        CheckpointReason::Quiet,
        CheckpointReason::HumanEdit,
        CheckpointReason::ReviewFix,
        CheckpointReason::BeforeRemoval,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            CheckpointReason::Explicit => "explicit",
            CheckpointReason::BeforeSync => "before_sync",
            CheckpointReason::BeforeRecord => "before_record",
            CheckpointReason::BeforeProposal => "before_proposal",
            CheckpointReason::BeforeProposalUpdate => "before_proposal_update",
            CheckpointReason::BeforeRestore => "before_restore",
            CheckpointReason::Quiet => "quiet",
            CheckpointReason::HumanEdit => "human_edit",
            CheckpointReason::ReviewFix => "review_fix",
            CheckpointReason::BeforeRemoval => "before_removal",
        }
    }

    /// A version a human or a reviewer made without the task worker: its tree waits as the workspace's pending write until the worker's next sync or restore writes it into the files.
    pub fn is_version_write(self) -> bool {
        matches!(
            self,
            CheckpointReason::HumanEdit | CheckpointReason::ReviewFix
        )
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.as_str() == text)
    }

    /// The only kind a checkpoint with this reason can have.
    pub fn kind(self) -> CheckpointKind {
        match self {
            CheckpointReason::Explicit
            | CheckpointReason::HumanEdit
            | CheckpointReason::ReviewFix
            | CheckpointReason::BeforeRemoval => CheckpointKind::Explicit,
            _ => CheckpointKind::Auto,
        }
    }
}

impl From<AutoReason> for CheckpointReason {
    fn from(reason: AutoReason) -> Self {
        match reason {
            AutoReason::BeforeSync => CheckpointReason::BeforeSync,
            AutoReason::BeforeRecord => CheckpointReason::BeforeRecord,
            AutoReason::BeforeProposal => CheckpointReason::BeforeProposal,
            AutoReason::BeforeProposalUpdate => CheckpointReason::BeforeProposalUpdate,
            AutoReason::Quiet => CheckpointReason::Quiet,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub id: CheckpointId,
    pub workspace: WorkspaceId,
    pub tree: ObjectHash,
    /// The workspace's base revision when the checkpoint was taken; `None` for a checkpoint older than this column.
    pub base: Option<RevisionId>,
    pub kind: CheckpointKind,
    pub reason: CheckpointReason,
    pub label: Option<String>,
    pub created_at: Timestamp,
    pub pruned_at: Option<Timestamp>,
}

/// Inserts one checkpoint row in the caller's write, with the workspace's base revision as it is now. `tree` must already be in the object store. A `reason` whose kind differs from `kind`, a workspace that does not exist and a review checkout are refused.
pub async fn insert_checkpoint(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    tree: ObjectHash,
    kind: CheckpointKind,
    reason: CheckpointReason,
    label: Option<&str>,
    now: Timestamp,
) -> Result<Checkpoint, StoreError> {
    if reason.kind() != kind {
        return Err(StoreError::Invalid(
            "the checkpoint reason does not fit its kind",
        ));
    }
    let workspace_number = workspace.to_db();
    let found = sqlx::query!(
        r#"SELECT w.id AS "id!: i64", w.base_revision_id AS "base_id!: i64", r.number AS "base!: i64",
                  w.kind = 'review' AS "review!: bool"
           FROM workspaces w JOIN revisions r ON r.id = w.base_revision_id
           WHERE w.number = ?"#,
        workspace_number
    )
    .fetch_optional(tx.conn())
    .await?
    .ok_or(StoreError::Invalid("the workspace does not exist"))?;
    if found.review {
        return Err(StoreError::Invalid("a review checkout takes no checkpoint"));
    }
    let id = counters::allocate(tx, IdKind::Checkpoint).await?;
    let id = CheckpointId::from_number(id).ok_or(StoreError::Corrupt("counters.next"))?;
    let number_value = id.to_db();
    let tree_text = tree.to_string();
    let kind_text = kind.as_str();
    let reason_text = reason.as_str();
    let created_at = now.as_millis();
    sqlx::query!(
        "INSERT INTO checkpoints (number, workspace_id, tree_hash, kind, reason, label, created_at, base_revision_id)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        number_value,
        found.id,
        tree_text,
        kind_text,
        reason_text,
        label,
        created_at,
        found.base_id
    )
    .execute(tx.conn())
    .await?;
    Ok(Checkpoint {
        id,
        workspace,
        tree,
        base: Some(number(
            found.base,
            RevisionId::try_from_db,
            "revisions.number",
        )?),
        kind,
        reason,
        label: label.map(str::to_owned),
        created_at: now,
        pruned_at: None,
    })
}

/// The checkpoints of one workspace, oldest first.
pub async fn list_checkpoints<'e>(
    db: impl SqliteExecutor<'e>,
    workspace: WorkspaceId,
    include_pruned: bool,
) -> Result<Vec<Checkpoint>, StoreError> {
    let workspace_number = workspace.to_db();
    let rows = sqlx::query_as!(
        Row,
        r#"SELECT c.number AS "number!: i64", w.number AS "workspace!: i64", c.tree_hash,
                  c.kind, c.reason, c.label, c.created_at, c.pruned_at, r.number AS "base?: i64"
           FROM checkpoints c JOIN workspaces w ON w.id = c.workspace_id
           LEFT JOIN revisions r ON r.id = c.base_revision_id
           WHERE w.number = ?1 AND (?2 OR c.pruned_at IS NULL)
           ORDER BY c.number ASC"#,
        workspace_number,
        include_pruned
    )
    .fetch_all(db)
    .await?;
    rows.into_iter().map(Row::into_checkpoint).collect()
}

/// The newest checkpoint of one workspace that is not pruned, of either kind.
pub async fn latest_checkpoint<'e>(
    db: impl SqliteExecutor<'e>,
    workspace: WorkspaceId,
) -> Result<Option<Checkpoint>, StoreError> {
    let workspace_number = workspace.to_db();
    let row = sqlx::query_as!(
        Row,
        r#"SELECT c.number AS "number!: i64", w.number AS "workspace!: i64", c.tree_hash,
                  c.kind, c.reason, c.label, c.created_at, c.pruned_at, r.number AS "base?: i64"
           FROM checkpoints c JOIN workspaces w ON w.id = c.workspace_id
           LEFT JOIN revisions r ON r.id = c.base_revision_id
           WHERE w.number = ? AND c.pruned_at IS NULL
           ORDER BY c.number DESC
           LIMIT 1"#,
        workspace_number
    )
    .fetch_optional(db)
    .await?;
    row.map(Row::into_checkpoint).transpose()
}

pub async fn get_checkpoint<'e>(
    db: impl SqliteExecutor<'e>,
    id: CheckpointId,
) -> Result<Option<Checkpoint>, StoreError> {
    let number = id.to_db();
    let row = sqlx::query_as!(
        Row,
        r#"SELECT c.number AS "number!: i64", w.number AS "workspace!: i64", c.tree_hash,
                  c.kind, c.reason, c.label, c.created_at, c.pruned_at, r.number AS "base?: i64"
           FROM checkpoints c JOIN workspaces w ON w.id = c.workspace_id
           LEFT JOIN revisions r ON r.id = c.base_revision_id
           WHERE c.number = ?"#,
        number
    )
    .fetch_optional(db)
    .await?;
    row.map(Row::into_checkpoint).transpose()
}

/// Marks the unpruned automatic checkpoints of the workspace that `prune_set` gives for `keep` as pruned at `now`, and returns the ones it marked. Explicit checkpoints and checkpoints already pruned are left alone, and already-pruned ones do not count toward `keep`. The checkpoint a workspace names as `sync_checkpoint_id` is left alone too: the next sync restores it, so its objects must stay.
pub async fn prune_auto(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    keep: u32,
    now: Timestamp,
) -> Result<Vec<CheckpointId>, StoreError> {
    let workspace_number = workspace.to_db();
    let found = sqlx::query!(
        r#"SELECT c.number AS "number!: i64",
                  c.id IS w.sync_checkpoint_id AS "marked!: bool"
           FROM checkpoints c JOIN workspaces w ON w.id = c.workspace_id
           WHERE w.number = ? AND c.kind = 'auto' AND c.pruned_at IS NULL
           ORDER BY c.number DESC"#,
        workspace_number
    )
    .fetch_all(tx.conn())
    .await?;
    let mut autos = Vec::with_capacity(found.len());
    let mut marked = None;
    for row in found {
        let id = number(row.number, CheckpointId::try_from_db, "checkpoints.number")?;
        if row.marked {
            marked = Some(id);
        }
        autos.push(id);
    }
    let mut pruned: Vec<CheckpointId> = prune_set(&autos, u64::from(keep))
        .into_iter()
        .filter(|id| Some(*id) != marked)
        .collect();
    let pruned_at = now.as_millis();
    let numbers = serde_json::to_string(&pruned.iter().map(|id| id.to_db()).collect::<Vec<_>>())
        .map_err(|_| StoreError::Invalid("checkpoint numbers"))?;
    sqlx::query!(
        "UPDATE checkpoints SET pruned_at = ? WHERE number IN (SELECT value FROM json_each(?))",
        pruned_at,
        numbers
    )
    .execute(tx.conn())
    .await?;
    pruned.sort();
    Ok(pruned)
}

/// Marks every unpruned checkpoint of the workspace, explicit ones included, as pruned at `now`, and returns the ones it marked.
pub async fn prune_workspace(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    now: Timestamp,
) -> Result<Vec<CheckpointId>, StoreError> {
    let pruned_at = now.as_millis();
    let workspace_number = workspace.to_db();
    let numbers = sqlx::query_scalar!(
        r#"UPDATE checkpoints SET pruned_at = ?1
           WHERE workspace_id = (SELECT id FROM workspaces WHERE number = ?2)
             AND pruned_at IS NULL
           RETURNING number AS "number!: i64""#,
        pruned_at,
        workspace_number
    )
    .fetch_all(tx.conn())
    .await?;
    sorted_ids(numbers)
}

fn sorted_ids(numbers: Vec<i64>) -> Result<Vec<CheckpointId>, StoreError> {
    let mut ids = numbers
        .into_iter()
        .map(|value| number(value, CheckpointId::try_from_db, "checkpoints.number"))
        .collect::<Result<Vec<_>, _>>()?;
    ids.sort();
    Ok(ids)
}

struct Row {
    number: i64,
    workspace: i64,
    tree_hash: String,
    kind: String,
    reason: Option<String>,
    label: Option<String>,
    created_at: i64,
    pruned_at: Option<i64>,
    base: Option<i64>,
}

impl Row {
    fn into_checkpoint(self) -> Result<Checkpoint, StoreError> {
        Ok(Checkpoint {
            id: number(self.number, CheckpointId::try_from_db, "checkpoints.number")?,
            workspace: number(
                self.workspace,
                WorkspaceId::try_from_db,
                "workspaces.number",
            )?,
            tree: tree_hash(&self.tree_hash, "checkpoints.tree_hash")?,
            base: self
                .base
                .map(|value| number(value, RevisionId::try_from_db, "revisions.number"))
                .transpose()?,
            kind: CheckpointKind::parse(&self.kind)
                .ok_or(StoreError::Corrupt("checkpoints.kind"))?,
            reason: self
                .reason
                .as_deref()
                .and_then(CheckpointReason::parse)
                .ok_or(StoreError::Corrupt("checkpoints.reason"))?,
            label: self.label,
            created_at: timestamp(self.created_at, "checkpoints.created_at")?,
            pruned_at: self
                .pruned_at
                .map(|millis| timestamp(millis, "checkpoints.pruned_at"))
                .transpose()?,
        })
    }
}
