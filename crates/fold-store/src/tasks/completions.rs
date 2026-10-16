use fold_core::{IntegrationId, TaskId, Timestamp};

use crate::error::StoreError;
use crate::store::WriteTx;

/// What completed a task: the integration of its proposal, the container rule once every child is Done, or the lead check of a task without a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Integration,
    Children,
    Check,
}

impl CompletionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CompletionKind::Integration => "integration",
            CompletionKind::Children => "children",
            CompletionKind::Check => "check",
        }
    }
}

/// Records that `task` is done. This is the only writer of `completions`; the table refuses `UPDATE` and `DELETE`, a second row for the task fails on `completions.task_id`, and `completions_integration_matches_kind` refuses an integration that does not match `kind`. A task or an integration number that does not exist inserts nothing and fails with `StoreError::Invalid`. The caller runs recompute in the same transaction so the task shows Done.
pub async fn insert(
    tx: &mut WriteTx,
    task: TaskId,
    kind: CompletionKind,
    integration: Option<IntegrationId>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = task.to_db();
    let kind = kind.as_str();
    let integration = integration.map(IntegrationId::to_db);
    let now = now.as_millis();
    let result = sqlx::query!(
        "INSERT INTO completions (task_id, kind, integration_id, created_at)
         SELECT t.id, ?2, i.id, ?4
         FROM tasks t LEFT JOIN integrations i ON i.number = ?3
         WHERE t.number = ?1 AND (?3 IS NULL OR i.id IS NOT NULL)",
        number,
        kind,
        integration,
        now
    )
    .execute(tx.conn())
    .await?;
    if result.rows_affected() == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid(
            "the task or the integration does not exist",
        ))
    }
}
