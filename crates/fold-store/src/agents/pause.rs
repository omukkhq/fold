use fold_core::Timestamp;
use sqlx::SqliteExecutor;

use crate::columns::optional_time;
use crate::error::StoreError;
use crate::store::WriteTx;

/// The pause state of the project's agents. `paused_at` and `paused_by` are set while paused; `resumed_at` is the end of the latest pause.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PauseRow {
    pub paused_at: Option<Timestamp>,
    pub paused_by: Option<String>,
    pub resumed_at: Option<Timestamp>,
}

impl PauseRow {
    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }
}

/// The pause state; a project that was never paused has the empty one.
pub async fn pause_state(executor: impl SqliteExecutor<'_>) -> Result<PauseRow, StoreError> {
    let row =
        sqlx::query!(r#"SELECT paused_at, paused_by, resumed_at FROM project_pause WHERE id = 1"#)
            .fetch_optional(executor)
            .await?;
    let Some(row) = row else {
        return Ok(PauseRow::default());
    };
    Ok(PauseRow {
        paused_at: optional_time(row.paused_at, "project_pause.paused_at")?,
        paused_by: row.paused_by,
        resumed_at: optional_time(row.resumed_at, "project_pause.resumed_at")?,
    })
}

/// Pauses the project's agents. Returns false and changes nothing when they are paused already.
pub async fn set_paused(tx: &mut WriteTx, by: &str, now: Timestamp) -> Result<bool, StoreError> {
    let now = now.as_millis();
    let result = sqlx::query!(
        "INSERT INTO project_pause (id, paused_at, paused_by) VALUES (1, ?1, ?2)
         ON CONFLICT (id) DO UPDATE SET paused_at = ?1, paused_by = ?2 WHERE paused_at IS NULL",
        now,
        by
    )
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Ends the pause and returns when it began; `None` when the agents were not paused, and then nothing changes.
pub async fn set_resumed(
    tx: &mut WriteTx,
    now: Timestamp,
) -> Result<Option<Timestamp>, StoreError> {
    let before = pause_state(tx.conn()).await?;
    let Some(paused_at) = before.paused_at else {
        return Ok(None);
    };
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE project_pause SET paused_at = NULL, paused_by = NULL, resumed_at = ? WHERE id = 1",
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(Some(paused_at))
}
