use fold_core::object::ObjectHash;
use fold_core::reviews::Verdict;
use fold_core::{
    ProposalId, ReviewBatchId, RevisionId, TaskId, Timestamp, WorkerName, WorkspaceId,
};
use sqlx::SqliteExecutor;

use super::{ReviewerRef, parse_verdict, time};
use crate::error::StoreError;
use crate::store::WriteTx;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchState {
    Open,
    Submitted,
    /// Every member left or ended without a batch review.
    Closed,
}

impl BatchState {
    pub fn as_str(self) -> &'static str {
        match self {
            BatchState::Open => "open",
            BatchState::Submitted => "submitted",
            BatchState::Closed => "closed",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "open" => Ok(BatchState::Open),
            "submitted" => Ok(BatchState::Submitted),
            "closed" => Ok(BatchState::Closed),
            _ => Err(StoreError::Corrupt("review_batches.state")),
        }
    }
}

/// Why a member left its batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberRelease {
    Removed,
    Submitted,
    Closed,
}

impl MemberRelease {
    pub fn as_str(self) -> &'static str {
        match self {
            MemberRelease::Removed => "removed",
            MemberRelease::Submitted => "submitted",
            MemberRelease::Closed => "closed",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "removed" => Ok(MemberRelease::Removed),
            "submitted" => Ok(MemberRelease::Submitted),
            "closed" => Ok(MemberRelease::Closed),
            _ => Err(StoreError::Corrupt("review_batch_members.release_reason")),
        }
    }
}

/// One review batch. `holder_worker_id` is a row id and stays inside the daemon; the public handle of the holder is `holder`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchRow {
    pub id: ReviewBatchId,
    pub holder_worker_id: i64,
    pub holder: WorkerName,
    pub state: BatchState,
    pub created_at: Timestamp,
    pub submitted_at: Option<Timestamp>,
    pub closed_at: Option<Timestamp>,
}

/// One member of a batch with the version it joined at and that version's two trees. `member_id` and `version_id` are row ids and stay inside the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRow {
    pub member_id: i64,
    pub proposal: ProposalId,
    pub task: TaskId,
    pub position: u32,
    pub version_id: i64,
    pub version: u32,
    pub base_tree: ObjectHash,
    pub tree: ObjectHash,
    pub release: Option<MemberRelease>,
    pub released_at: Option<Timestamp>,
}

impl MemberRow {
    pub fn is_open(&self) -> bool {
        self.release.is_none()
    }
}

/// Inserts an open batch held by `holder_worker_id` in the caller's write.
pub async fn insert_batch(
    tx: &mut WriteTx,
    batch: ReviewBatchId,
    holder_worker_id: i64,
    now: Timestamp,
) -> Result<(), StoreError> {
    let (number, now) = (batch.to_db(), now.as_millis());
    sqlx::query!(
        "INSERT INTO review_batches (number, holder_worker_id, created_at) VALUES (?, ?, ?)",
        number,
        holder_worker_id,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Adds `version` of the proposal to the batch at `position`, in the caller's write. A proposal already in an open batch fails on `review_batch_members.proposal_id`, so the partial unique index decides a race between two batches. A batch, proposal or version that does not exist inserts nothing and fails with `StoreError::Invalid`.
pub async fn insert_member(
    tx: &mut WriteTx,
    batch: ReviewBatchId,
    proposal: ProposalId,
    version: u32,
    position: u32,
) -> Result<(), StoreError> {
    let (batch, proposal, version, position) = (
        batch.to_db(),
        proposal.to_db(),
        i64::from(version),
        i64::from(position),
    );
    let inserted = sqlx::query!(
        "INSERT INTO review_batch_members (batch_id, proposal_id, version_id, position)
         SELECT b.id, p.id, v.id, ?4
         FROM review_batches b, proposals p JOIN proposal_versions v ON v.proposal_id = p.id
         WHERE b.number = ?1 AND p.number = ?2 AND v.version = ?3",
        batch,
        proposal,
        version,
        position
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if inserted == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid(
            "the batch, the proposal or the version does not exist",
        ))
    }
}

/// The open batch the proposal is a member of, or `None`.
pub async fn open_batch_of(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<Option<ReviewBatchId>, StoreError> {
    let number = proposal.to_db();
    let found = sqlx::query_scalar!(
        r#"SELECT b.number AS "number!: i64"
           FROM review_batch_members m
           JOIN review_batches b ON b.id = m.batch_id
           JOIN proposals p ON p.id = m.proposal_id
           WHERE p.number = ? AND m.released_at IS NULL"#,
        number
    )
    .fetch_optional(executor)
    .await?;
    found
        .map(|value| {
            ReviewBatchId::try_from_db(value).ok_or(StoreError::Corrupt("review_batches.number"))
        })
        .transpose()
}

pub async fn get_batch(
    executor: impl SqliteExecutor<'_>,
    batch: ReviewBatchId,
) -> Result<Option<BatchRow>, StoreError> {
    Ok(select_batches(executor, Some(batch), false).await?.pop())
}

/// Batches in number order, only the open ones when `open_only`.
pub async fn list_batches(
    executor: impl SqliteExecutor<'_>,
    open_only: bool,
) -> Result<Vec<BatchRow>, StoreError> {
    select_batches(executor, None, open_only).await
}

async fn select_batches(
    executor: impl SqliteExecutor<'_>,
    batch: Option<ReviewBatchId>,
    open_only: bool,
) -> Result<Vec<BatchRow>, StoreError> {
    let batch = batch.map(ReviewBatchId::to_db);
    let rows = sqlx::query!(
        r#"SELECT b.number AS "number!: i64", b.holder_worker_id, w.name AS holder, b.state,
                  b.created_at, b.submitted_at, b.closed_at
           FROM review_batches b
           JOIN workers w ON w.id = b.holder_worker_id
           WHERE (?1 IS NULL OR b.number = ?1) AND (NOT ?2 OR b.state = 'open')
           ORDER BY b.number"#,
        batch,
        open_only
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(BatchRow {
                id: ReviewBatchId::try_from_db(row.number)
                    .ok_or(StoreError::Corrupt("review_batches.number"))?,
                holder_worker_id: row.holder_worker_id,
                holder: WorkerName::parse(&row.holder)
                    .map_err(|_| StoreError::Corrupt("workers.name"))?,
                state: BatchState::parse(&row.state)?,
                created_at: time(row.created_at, "review_batches.created_at")?,
                submitted_at: row
                    .submitted_at
                    .map(|millis| time(millis, "review_batches.submitted_at"))
                    .transpose()?,
                closed_at: row
                    .closed_at
                    .map(|millis| time(millis, "review_batches.closed_at"))
                    .transpose()?,
            })
        })
        .collect()
}

/// Every member the batch has had, open or released, in merge order.
pub async fn members(
    executor: impl SqliteExecutor<'_>,
    batch: ReviewBatchId,
) -> Result<Vec<MemberRow>, StoreError> {
    let number = batch.to_db();
    let rows = sqlx::query!(
        r#"SELECT m.id AS "member_id!: i64", p.number AS "proposal!: i64", t.number AS "task!: i64",
                  m.position, v.id AS "version_id!: i64", v.version, r.tree_hash AS base_tree,
                  v.tree_hash, m.released_at, m.release_reason
           FROM review_batch_members m
           JOIN review_batches b ON b.id = m.batch_id
           JOIN proposals p ON p.id = m.proposal_id
           JOIN tasks t ON t.id = p.task_id
           JOIN proposal_versions v ON v.id = m.version_id
           JOIN revisions r ON r.id = v.base_revision_id
           WHERE b.number = ?
           ORDER BY m.position"#,
        number
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(MemberRow {
                member_id: row.member_id,
                proposal: ProposalId::try_from_db(row.proposal)
                    .ok_or(StoreError::Corrupt("proposals.number"))?,
                task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
                position: u32::try_from(row.position)
                    .map_err(|_| StoreError::Corrupt("review_batch_members.position"))?,
                version_id: row.version_id,
                version: u32::try_from(row.version)
                    .map_err(|_| StoreError::Corrupt("proposal_versions.version"))?,
                base_tree: row
                    .base_tree
                    .parse()
                    .map_err(|_| StoreError::Corrupt("revisions.tree_hash"))?,
                tree: row
                    .tree_hash
                    .parse()
                    .map_err(|_| StoreError::Corrupt("proposal_versions.tree_hash"))?,
                release: row
                    .release_reason
                    .as_deref()
                    .map(MemberRelease::parse)
                    .transpose()?,
                released_at: row
                    .released_at
                    .map(|millis| time(millis, "review_batch_members.released_at"))
                    .transpose()?,
            })
        })
        .collect()
}

/// Releases the open membership of the proposal in the batch as `removed`, in the caller's write. False, with nothing changed, when the proposal is not an open member.
pub async fn remove_member(
    tx: &mut WriteTx,
    batch: ReviewBatchId,
    proposal: ProposalId,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (batch, proposal, now) = (batch.to_db(), proposal.to_db(), now.as_millis());
    let updated = sqlx::query!(
        "UPDATE review_batch_members SET released_at = ?3, release_reason = 'removed'
         WHERE released_at IS NULL
           AND batch_id = (SELECT id FROM review_batches WHERE number = ?1)
           AND proposal_id = (SELECT id FROM proposals WHERE number = ?2)",
        batch,
        proposal,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// Marks an open batch submitted and releases its open members as `submitted`, in the caller's write. False, with nothing changed, when the batch is not open.
pub async fn submit_batch(
    tx: &mut WriteTx,
    batch: ReviewBatchId,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (number, now) = (batch.to_db(), now.as_millis());
    let updated = sqlx::query!(
        "UPDATE review_batches SET state = 'submitted', submitted_at = ?2
         WHERE number = ?1 AND state = 'open'",
        number,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if updated != 1 {
        return Ok(false);
    }
    sqlx::query!(
        "UPDATE review_batch_members SET released_at = ?2, release_reason = 'submitted'
         WHERE released_at IS NULL AND batch_id = (SELECT id FROM review_batches WHERE number = ?1)",
        number,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(true)
}

/// Marks an open batch closed and releases its open members as `closed`, in the caller's write. False, with nothing changed, when the batch is not open.
pub async fn close_batch(
    tx: &mut WriteTx,
    batch: ReviewBatchId,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (number, now) = (batch.to_db(), now.as_millis());
    let updated = sqlx::query!(
        "UPDATE review_batches SET state = 'closed', closed_at = ?2
         WHERE number = ?1 AND state = 'open'",
        number,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if updated != 1 {
        return Ok(false);
    }
    sqlx::query!(
        "UPDATE review_batch_members SET released_at = ?2, release_reason = 'closed'
         WHERE released_at IS NULL AND batch_id = (SELECT id FROM review_batches WHERE number = ?1)",
        number,
        now
    )
    .execute(tx.conn())
    .await?;
    Ok(true)
}

/// Whether some open member of the batch still has an open proposal to review.
pub async fn has_open_member_to_review(
    executor: impl SqliteExecutor<'_>,
    batch: ReviewBatchId,
) -> Result<bool, StoreError> {
    let number = batch.to_db();
    let found = sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM review_batch_members m
               JOIN review_batches b ON b.id = m.batch_id
               JOIN proposals p ON p.id = m.proposal_id
               WHERE b.number = ? AND m.released_at IS NULL AND p.state = 'open'
           ) AS "found!: bool""#,
        number
    )
    .fetch_one(executor)
    .await?;
    Ok(found)
}

/// Inserts a comment of the batch about members together with the next number of the batch, in the caller's write, and returns its row id and number.
pub async fn insert_batch_comment(
    tx: &mut WriteTx,
    batch: ReviewBatchId,
    body: &str,
    now: Timestamp,
) -> Result<(i64, u32), StoreError> {
    let (batch, now) = (batch.to_db(), now.as_millis());
    let row = sqlx::query!(
        r#"INSERT INTO review_batch_comments (batch_id, number, body, created_at)
           SELECT b.id,
                  COALESCE((SELECT MAX(c.number) FROM review_batch_comments c WHERE c.batch_id = b.id), 0) + 1,
                  ?2, ?3
           FROM review_batches b WHERE b.number = ?1
           RETURNING id AS "id!: i64", number AS "number!: i64""#,
        batch,
        body,
        now
    )
    .fetch_optional(tx.conn())
    .await?
    .ok_or(StoreError::Invalid("the batch does not exist"))?;
    let number = u32::try_from(row.number)
        .map_err(|_| StoreError::Corrupt("review_batch_comments.number"))?;
    Ok((row.id, number))
}

/// A comment of the batch about members together, with the members it was put on in number order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchCommentRow {
    pub number: u32,
    pub body: String,
    pub members: Vec<ProposalId>,
    pub created_at: Timestamp,
}

/// The batch's comments about members together, in number order.
pub async fn batch_comments(
    executor: impl SqliteExecutor<'_>,
    batch: ReviewBatchId,
) -> Result<Vec<BatchCommentRow>, StoreError> {
    let number = batch.to_db();
    let rows = sqlx::query!(
        r#"SELECT bc.number, bc.body, bc.created_at,
                  (SELECT group_concat(p.number)
                   FROM review_comments c
                   JOIN proposal_versions v ON v.id = c.version_id
                   JOIN proposals p ON p.id = v.proposal_id
                   WHERE c.batch_comment_id = bc.id) AS "members?: String"
           FROM review_batch_comments bc
           JOIN review_batches b ON b.id = bc.batch_id
           WHERE b.number = ?
           ORDER BY bc.number"#,
        number
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(BatchCommentRow {
                number: u32::try_from(row.number)
                    .map_err(|_| StoreError::Corrupt("review_batch_comments.number"))?,
                body: row.body,
                members: proposal_list(row.members.as_deref())?,
                created_at: time(row.created_at, "review_batch_comments.created_at")?,
            })
        })
        .collect()
}

/// The proposals of a `group_concat` of proposal numbers, in number order.
pub(super) fn proposal_list(numbers: Option<&str>) -> Result<Vec<ProposalId>, StoreError> {
    let mut members = numbers
        .unwrap_or("")
        .split(',')
        .filter(|text| !text.is_empty())
        .map(|text| {
            text.parse::<i64>()
                .ok()
                .and_then(ProposalId::try_from_db)
                .ok_or(StoreError::Corrupt("proposals.number"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    members.sort_unstable();
    members.dedup();
    Ok(members)
}

/// A batch checkout to insert. `task` is the task of the first member in merge order, because every workspace belongs to a task; `base` is main's head the members were merged onto.
#[derive(Debug, Clone)]
pub struct NewBatchCheckout<'a> {
    pub id: WorkspaceId,
    pub batch: ReviewBatchId,
    pub task: TaskId,
    pub worker_id: i64,
    pub path: &'a str,
    pub base: RevisionId,
    pub now: Timestamp,
}

/// Inserts an active review workspace that holds the batch's merged members. A second active checkout of the batch fails on `workspaces.review_batch_id`, and a reused directory on `workspaces.path`. A batch, task or revision that does not exist inserts nothing and fails with `StoreError::Invalid`.
pub async fn insert_batch_checkout(
    tx: &mut WriteTx,
    new: &NewBatchCheckout<'_>,
) -> Result<(), StoreError> {
    let (id, task, base, batch, now) = (
        new.id.to_db(),
        new.task.to_db(),
        new.base.to_db(),
        new.batch.to_db(),
        new.now.as_millis(),
    );
    let inserted = sqlx::query!(
        "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, review_batch_id, state, last_activity_at, created_at)
         SELECT ?1, t.id, ?3, 'review', ?4, r.id, b.id, 'active', ?7, ?7
         FROM tasks t, revisions r, review_batches b
         WHERE t.number = ?2 AND r.number = ?5 AND b.number = ?6",
        id,
        task,
        new.worker_id,
        new.path,
        base,
        batch,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if inserted == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid(
            "the batch, the task or the revision does not exist",
        ))
    }
}

/// The active checkout of the batch; the partial unique index allows at most one.
pub async fn active_batch_checkout(
    executor: impl SqliteExecutor<'_>,
    batch: ReviewBatchId,
) -> Result<Option<WorkspaceId>, StoreError> {
    let number = batch.to_db();
    let found = sqlx::query_scalar!(
        r#"SELECT w.number AS "number!: i64"
           FROM workspaces w JOIN review_batches b ON b.id = w.review_batch_id
           WHERE b.number = ? AND w.state = 'active'"#,
        number
    )
    .fetch_optional(executor)
    .await?;
    found
        .map(|value| {
            WorkspaceId::try_from_db(value).ok_or(StoreError::Corrupt("workspaces.number"))
        })
        .transpose()
}

/// The directories of every checkout the batch has had, in any state, so a new checkout never takes an earlier one's name.
pub async fn batch_checkout_paths(
    executor: impl SqliteExecutor<'_>,
    batch: ReviewBatchId,
) -> Result<Vec<String>, StoreError> {
    let number = batch.to_db();
    let paths = sqlx::query_scalar!(
        "SELECT w.path
         FROM workspaces w JOIN review_batches b ON b.id = w.review_batch_id
         WHERE b.number = ?
         ORDER BY w.number",
        number
    )
    .fetch_all(executor)
    .await?;
    Ok(paths)
}

/// A run of lines of `path` in a batch checkout that the member `proposal` brought in, starting at `member_start_line` in the member's version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewLineOrigin<'a> {
    pub proposal: ProposalId,
    pub path: &'a str,
    pub start_line: u32,
    pub end_line: u32,
    pub member_start_line: u32,
}

/// Records the line origins of the batch checkout `workspace`, in the caller's write. Each names an open member of `batch`; another proposal, or a workspace that is not a checkout of `batch`, inserts nothing and fails with `StoreError::Invalid`.
pub async fn insert_line_origins(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    batch: ReviewBatchId,
    origins: &[NewLineOrigin<'_>],
) -> Result<(), StoreError> {
    let (workspace, batch) = (workspace.to_db(), batch.to_db());
    for origin in origins {
        let proposal = origin.proposal.to_db();
        let (start, end, member_start) = (
            i64::from(origin.start_line),
            i64::from(origin.end_line),
            i64::from(origin.member_start_line),
        );
        let inserted = sqlx::query!(
            "INSERT INTO review_batch_lines (workspace_id, member_id, path, start_line, end_line, member_start_line)
             SELECT w.id, m.id, ?4, ?5, ?6, ?7
             FROM workspaces w, review_batch_members m
             JOIN review_batches b ON b.id = m.batch_id
             JOIN proposals p ON p.id = m.proposal_id
             WHERE w.number = ?1 AND w.review_batch_id = b.id AND b.number = ?2 AND p.number = ?3
               AND m.released_at IS NULL",
            workspace,
            batch,
            proposal,
            origin.path,
            start,
            end,
            member_start
        )
        .execute(tx.conn())
        .await?
        .rows_affected();
        if inserted != 1 {
            return Err(StoreError::Invalid(
                "the workspace or the open member does not exist",
            ));
        }
    }
    Ok(())
}

/// The run of a batch checkout's lines that holds `line` of `path`, with the member and version that brought it. `version_id` is a row id and stays inside the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineOriginRow {
    pub proposal: ProposalId,
    pub version_id: i64,
    pub version: u32,
    pub start_line: u32,
    pub end_line: u32,
    pub member_start_line: u32,
}

/// The run of `path` in the checkout `workspace` that holds `line`, or `None` when no member brought that line in.
pub async fn line_origin(
    executor: impl SqliteExecutor<'_>,
    workspace: WorkspaceId,
    path: &str,
    line: u32,
) -> Result<Option<LineOriginRow>, StoreError> {
    let (workspace, line) = (workspace.to_db(), i64::from(line));
    let row = sqlx::query!(
        r#"SELECT p.number AS "proposal!: i64", v.id AS "version_id!: i64", v.version,
                  l.start_line, l.end_line, l.member_start_line
           FROM review_batch_lines l
           JOIN workspaces w ON w.id = l.workspace_id
           JOIN review_batch_members m ON m.id = l.member_id
           JOIN proposals p ON p.id = m.proposal_id
           JOIN proposal_versions v ON v.id = m.version_id
           WHERE w.number = ?1 AND l.path = ?2 AND l.start_line <= ?3 AND l.end_line >= ?3"#,
        workspace,
        path,
        line
    )
    .fetch_optional(executor)
    .await?;
    let line = |value: i64| {
        u32::try_from(value).map_err(|_| StoreError::Corrupt("review_batch_lines.start_line"))
    };
    row.map(|row| {
        Ok(LineOriginRow {
            proposal: ProposalId::try_from_db(row.proposal)
                .ok_or(StoreError::Corrupt("proposals.number"))?,
            version_id: row.version_id,
            version: u32::try_from(row.version)
                .map_err(|_| StoreError::Corrupt("proposal_versions.version"))?,
            start_line: line(row.start_line)?,
            end_line: line(row.end_line)?,
            member_start_line: line(row.member_start_line)?,
        })
    })
    .transpose()
}

/// Inserts a review of the version row `version_id` that came with the batch, in the caller's write, and returns its row id.
pub async fn insert_batch_review(
    tx: &mut WriteTx,
    batch: ReviewBatchId,
    version_id: i64,
    reviewer: ReviewerRef<'_>,
    verdict: Verdict,
    body: &str,
    now: Timestamp,
) -> Result<i64, StoreError> {
    let (worker, human) = reviewer.columns();
    let (batch, verdict, now) = (batch.to_db(), verdict.as_str(), now.as_millis());
    let id = sqlx::query_scalar!(
        r#"INSERT INTO reviews (version_id, reviewer_worker_id, reviewer_human, verdict, body, created_at, review_batch_id)
           SELECT ?1, ?2, ?3, ?4, ?5, ?6, b.id FROM review_batches b WHERE b.number = ?7
           RETURNING id AS "id!: i64""#,
        version_id,
        worker,
        human,
        verdict,
        body,
        now,
        batch
    )
    .fetch_optional(tx.conn())
    .await?;
    id.ok_or(StoreError::Invalid("the batch does not exist"))
}

/// A verdict the batch gave, on the member version it reviewed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchReviewRow {
    pub proposal: ProposalId,
    pub version: u32,
    pub verdict: Verdict,
    pub created_at: Timestamp,
}

/// The reviews submitted with the batch, in the order they were written.
pub async fn batch_reviews(
    executor: impl SqliteExecutor<'_>,
    batch: ReviewBatchId,
) -> Result<Vec<BatchReviewRow>, StoreError> {
    let number = batch.to_db();
    let rows = sqlx::query!(
        r#"SELECT p.number AS "proposal!: i64", v.version, r.verdict, r.created_at
           FROM reviews r
           JOIN review_batches b ON b.id = r.review_batch_id
           JOIN proposal_versions v ON v.id = r.version_id
           JOIN proposals p ON p.id = v.proposal_id
           WHERE b.number = ?
           ORDER BY r.id"#,
        number
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(BatchReviewRow {
                proposal: ProposalId::try_from_db(row.proposal)
                    .ok_or(StoreError::Corrupt("proposals.number"))?,
                version: u32::try_from(row.version)
                    .map_err(|_| StoreError::Corrupt("proposal_versions.version"))?,
                verdict: parse_verdict(&row.verdict)?,
                created_at: time(row.created_at, "reviews.created_at")?,
            })
        })
        .collect()
}
