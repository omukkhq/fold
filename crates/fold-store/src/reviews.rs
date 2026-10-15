use fold_core::ProposalId;
use sqlx::SqliteExecutor;

use crate::error::StoreError;

/// Whether a proposal with this number exists, in any state.
pub async fn proposal_exists(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<bool, StoreError> {
    let number = proposal.to_db();
    let found = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM proposals WHERE number = ?) AS "found!: bool""#,
        number
    )
    .fetch_one(executor)
    .await?;
    Ok(found)
}
