use fold_core::events::types;
use fold_core::reviews::HumanReviewMark;
use fold_core::{HumanName, ProposalId, TaskId, Timestamp};
use sqlx::SqliteExecutor;

use super::{expect_one, mark, timestamp};
use crate::error::StoreError;
use crate::store::WriteTx;

/// Turns the mark on, or replaces what it says, and stamps `updated_at`. The task must exist.
pub async fn set_human_review(
    tx: &mut WriteTx,
    task: TaskId,
    mark: &HumanReviewMark,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = task.to_db();
    let reviewer = mark.reviewer.as_ref().map(HumanName::as_str);
    let reason = mark.reason.as_str();
    let set_by = mark.set_by.as_str();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE tasks
         SET human_review = 1, human_reviewer = ?2, human_review_reason = ?3,
             human_review_set_by = ?4, updated_at = ?5
         WHERE number = ?1",
        number,
        reviewer,
        reason,
        set_by,
        now
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

/// Turns the mark off with every column back to null, and stamps `updated_at`. The task must exist.
pub async fn clear_human_review(
    tx: &mut WriteTx,
    task: TaskId,
    now: Timestamp,
) -> Result<(), StoreError> {
    let number = task.to_db();
    let now = now.as_millis();
    let result = sqlx::query!(
        "UPDATE tasks
         SET human_review = 0, human_reviewer = NULL, human_review_reason = NULL,
             human_review_set_by = NULL, updated_at = ?2
         WHERE number = ?1",
        number,
        now
    )
    .execute(tx.conn())
    .await?;
    expect_one(result.rows_affected())
}

/// An open proposal on a task whose mark is on, with its current version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkedProposal {
    pub proposal: ProposalId,
    pub task: TaskId,
    pub mark: HumanReviewMark,
    pub version: u32,
    /// The proposal is with its author: its merge conflicted or its integration checks failed.
    pub conflicted: bool,
    /// When the current version was added.
    pub version_at: Timestamp,
    /// When the mark was last set or changed; `None` for a mark that no event records.
    pub marked_at: Option<Timestamp>,
}

/// Every open proposal on a marked task, in proposal number order.
pub async fn marked_open_proposals(
    executor: impl SqliteExecutor<'_>,
) -> Result<Vec<MarkedProposal>, StoreError> {
    let set = types::TASK_HUMAN_REVIEW_SET.as_str();
    let rows = sqlx::query!(
        r#"SELECT p.number AS "proposal!: i64",
                  t.number AS "task!: i64",
                  t.human_reviewer,
                  t.human_review_reason,
                  t.human_review_set_by,
                  (p.conflicted OR p.checks_failed) AS "conflicted: bool",
                  v.version,
                  v.created_at AS version_at,
                  (
                      SELECT MAX(e.created_at) FROM events e
                      WHERE e.task_id = t.number AND e.type = ?1
                  ) AS "marked_at?: i64"
           FROM proposals p
           JOIN tasks t ON t.id = p.task_id
           JOIN proposal_versions v ON v.proposal_id = p.id
           WHERE p.state = 'open'
             AND t.human_review = 1
             AND v.version = (
                     SELECT MAX(c.version) FROM proposal_versions c WHERE c.proposal_id = p.id
                 )
           ORDER BY p.number"#,
        set
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(MarkedProposal {
                proposal: ProposalId::try_from_db(row.proposal)
                    .ok_or(StoreError::Corrupt("proposals.number"))?,
                task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
                mark: mark(
                    true,
                    row.human_reviewer,
                    row.human_review_reason,
                    row.human_review_set_by,
                )?
                .ok_or(StoreError::Corrupt("tasks.human_review"))?,
                version: u32::try_from(row.version)
                    .map_err(|_| StoreError::Corrupt("proposal_versions.version"))?,
                conflicted: row.conflicted,
                version_at: timestamp(row.version_at, "proposal_versions.created_at")?,
                marked_at: row
                    .marked_at
                    .map(|millis| timestamp(millis, "events.created_at"))
                    .transpose()?,
            })
        })
        .collect()
}
