use fold_core::{ProposalId, Timestamp, WorkerName};
use sqlx::SqliteExecutor;

use super::time;
use crate::error::StoreError;
use crate::store::WriteTx;

/// An unreleased review claim. `worker_id` is a row id and stays inside the daemon; the public handle of the holder is `worker`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewClaimRow {
    pub proposal: ProposalId,
    pub worker_id: i64,
    pub worker: WorkerName,
    pub acquired_at: Timestamp,
    pub renewed_at: Timestamp,
    pub stale_at: Option<Timestamp>,
}

/// Inserts a review claim on an open proposal for `worker_id`, acquired and renewed at `now`. An unreleased review claim on the proposal fails on `claims.proposal_id`, so the partial unique index decides between two reviewers. A proposal that does not exist or is not open inserts nothing and fails with `StoreError::Invalid`.
pub async fn insert_review_claim(
    tx: &mut WriteTx,
    proposal: ProposalId,
    worker_id: i64,
    now: Timestamp,
) -> Result<(), StoreError> {
    let (number, now) = (proposal.to_db(), now.as_millis());
    let inserted = sqlx::query!(
        "INSERT INTO claims (kind, proposal_id, worker_id, acquired_at, renewed_at)
         SELECT 'review', p.id, ?2, ?3, ?3 FROM proposals p WHERE p.number = ?1 AND p.state = 'open'",
        number,
        worker_id,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if inserted == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid("the proposal is not open"))
    }
}

/// The unreleased review claim on `proposal` with its holder's name, or `None`.
pub async fn unreleased_review_claim(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<Option<ReviewClaimRow>, StoreError> {
    let number = proposal.to_db();
    let row = sqlx::query!(
        r#"SELECT c.worker_id, w.name AS worker, c.acquired_at, c.renewed_at, c.stale_at
           FROM claims c
           JOIN proposals p ON p.id = c.proposal_id
           JOIN workers w ON w.id = c.worker_id
           WHERE p.number = ? AND c.kind = 'review' AND c.released_at IS NULL"#,
        number
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(ReviewClaimRow {
            proposal,
            worker_id: row.worker_id,
            worker: WorkerName::parse(&row.worker)
                .map_err(|_| StoreError::Corrupt("workers.name"))?,
            acquired_at: time(row.acquired_at, "claims.acquired_at")?,
            renewed_at: time(row.renewed_at, "claims.renewed_at")?,
            stale_at: row
                .stale_at
                .map(|millis| time(millis, "claims.stale_at"))
                .transpose()?,
        })
    })
    .transpose()
}

/// The worker whose review claim the proposal's integration released, or `None` when nobody held one at that moment.
pub async fn integrated_review_claim_holder(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<Option<WorkerName>, StoreError> {
    let number = proposal.to_db();
    let worker = sqlx::query_scalar!(
        r#"SELECT w.name AS "name!: String"
           FROM claims c
           JOIN proposals p ON p.id = c.proposal_id
           JOIN workers w ON w.id = c.worker_id
           WHERE p.number = ? AND c.kind = 'review' AND c.release_reason = 'integrated'
           ORDER BY c.released_at DESC, c.id DESC
           LIMIT 1"#,
        number
    )
    .fetch_optional(executor)
    .await?;
    worker
        .map(|name| WorkerName::parse(&name).map_err(|_| StoreError::Corrupt("workers.name")))
        .transpose()
}
