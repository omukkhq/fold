use fold_core::Timestamp;
use sqlx::SqliteConnection;

use crate::columns::time;
use crate::error::StoreError;

/// Whether the agent has an open session, and when its last ended session ended: what decides whether its drafter may be replaced.
pub async fn agent_activity(
    conn: &mut SqliteConnection,
    agent_id: i64,
) -> Result<(bool, Option<Timestamp>), StoreError> {
    let row = sqlx::query!(
        r#"SELECT EXISTS (SELECT 1 FROM sessions WHERE agent_id = ?1 AND ended_at IS NULL) AS "open!: bool",
                  (SELECT MAX(ended_at) FROM sessions WHERE agent_id = ?1) AS "last_end?: i64""#,
        agent_id
    )
    .fetch_one(conn)
    .await?;
    let last_end = row
        .last_end
        .map(|millis| time(millis, "sessions.ended_at"))
        .transpose()?;
    Ok((row.open, last_end))
}
