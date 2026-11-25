use std::collections::BTreeMap;

use sqlx::SqliteExecutor;

use crate::error::StoreError;
use crate::store::WriteTx;

/// Stores the schema hash of each `(tool, hash)` pair under `fingerprint`. A row that exists stays as it is: a fingerprint names one binary, so its tools never change.
pub async fn record_tool_schema_hashes(
    tx: &mut WriteTx,
    fingerprint: &str,
    tools: &[(String, String)],
) -> Result<(), StoreError> {
    for (tool, hash) in tools {
        sqlx::query!(
            "INSERT OR IGNORE INTO tool_schema_hashes (fingerprint, tool, hash) VALUES (?, ?, ?)",
            fingerprint,
            tool,
            hash
        )
        .execute(tx.conn())
        .await?;
    }
    Ok(())
}

/// The schema hash of each tool recorded for `fingerprint`, by tool name. A fingerprint nothing recorded gives an empty map.
pub async fn tool_schema_hashes(
    executor: impl SqliteExecutor<'_>,
    fingerprint: &str,
) -> Result<BTreeMap<String, String>, StoreError> {
    let rows = sqlx::query!(
        "SELECT tool, hash FROM tool_schema_hashes WHERE fingerprint = ?",
        fingerprint
    )
    .fetch_all(executor)
    .await?;
    Ok(rows.into_iter().map(|row| (row.tool, row.hash)).collect())
}
