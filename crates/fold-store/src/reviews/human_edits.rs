use fold_core::{HumanName, ProposalId, Timestamp};

use crate::error::StoreError;
use crate::store::WriteTx;

/// Copies onto version `to` every agent approval that stands on version `from` of the proposal: an approval by a worker after the last `request_changes` of that version that still counts. Each copy names the approval it copies, has the same reviewer and an empty body, and is written at `now`, so a later `request_changes` on `to` withdraws it as it would any approval. A human approval is never kept, so a human approves the edited version again. A human edit keeps its source's approvals only this way; every other new version starts without approvals. Returns how many approvals were kept.
pub async fn keep_approvals(
    tx: &mut WriteTx,
    proposal: ProposalId,
    from: u32,
    to: u32,
    now: Timestamp,
) -> Result<u32, StoreError> {
    let (number, from, to, now) = (
        proposal.to_db(),
        i64::from(from),
        i64::from(to),
        now.as_millis(),
    );
    let kept = sqlx::query!(
        "INSERT INTO reviews (version_id, reviewer_worker_id, reviewer_human, verdict, body, created_at, kept_from_review_id)
         SELECT nv.id, r.reviewer_worker_id, r.reviewer_human, 'approve', '', ?4, r.id
         FROM reviews r
         JOIN proposal_versions sv ON sv.id = r.version_id
         JOIN proposals p ON p.id = sv.proposal_id
         JOIN proposal_versions nv ON nv.proposal_id = p.id AND nv.version = ?3
         WHERE p.number = ?1 AND sv.version = ?2 AND r.verdict = 'approve' AND r.reviewer_worker_id IS NOT NULL
           AND (r.kept_from_review_id IS NULL
               OR NOT EXISTS (SELECT 1 FROM review_again_requests q WHERE q.version_id = r.version_id))
           AND NOT EXISTS (
               SELECT 1 FROM reviews later
               WHERE later.version_id = r.version_id AND later.verdict = 'request_changes'
                 AND (later.created_at > r.created_at OR (later.created_at = r.created_at AND later.id > r.id))
           )
         ORDER BY r.created_at, r.id",
        number,
        from,
        to,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    u32::try_from(kept).map_err(|_| StoreError::Corrupt("reviews.kept_from_review_id"))
}

/// Records that `human` sent `version` of the proposal for review again, so the approvals that version kept no longer count. False, with nothing written, when the version was sent before; an unknown version is refused.
pub async fn request_review_again(
    tx: &mut WriteTx,
    proposal: ProposalId,
    version: u32,
    human: &HumanName,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (number, version, human, now) = (
        proposal.to_db(),
        i64::from(version),
        human.as_str(),
        now.as_millis(),
    );
    let exists = sqlx::query_scalar!(
        r#"SELECT v.id AS "id!: i64" FROM proposal_versions v JOIN proposals p ON p.id = v.proposal_id
           WHERE p.number = ?1 AND v.version = ?2"#,
        number,
        version
    )
    .fetch_optional(tx.conn())
    .await?;
    let Some(version_id) = exists else {
        return Err(StoreError::Invalid("the proposal version does not exist"));
    };
    let inserted = sqlx::query!(
        "INSERT INTO review_again_requests (version_id, requested_by_human, created_at)
         VALUES (?1, ?2, ?3) ON CONFLICT (version_id) DO NOTHING",
        version_id,
        human,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(inserted == 1)
}
