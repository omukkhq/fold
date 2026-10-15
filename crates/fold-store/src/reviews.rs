use fold_core::object::ObjectHash;
use fold_core::tree::ChangeCounts;
use fold_core::{ChangesetId, ProposalId, RevisionId, TaskId, Timestamp, WorkerName, WorkspaceId};
use sqlx::SqliteExecutor;

use crate::error::StoreError;
use crate::store::WriteTx;
use crate::workspaces::parse_stats;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalState {
    Open,
    Integrated,
    Closed,
}

impl ProposalState {
    pub fn as_str(self) -> &'static str {
        match self {
            ProposalState::Open => "open",
            ProposalState::Integrated => "integrated",
            ProposalState::Closed => "closed",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "open" => Some(ProposalState::Open),
            "integrated" => Some(ProposalState::Integrated),
            "closed" => Some(ProposalState::Closed),
            _ => None,
        }
    }
}

/// The two states an open proposal ends in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetiredState {
    Integrated,
    Closed,
}

impl RetiredState {
    pub fn as_str(self) -> &'static str {
        match self {
            RetiredState::Integrated => ProposalState::Integrated.as_str(),
            RetiredState::Closed => ProposalState::Closed.as_str(),
        }
    }
}

/// One proposal with the facts its readers decide on. `author_worker_id` is a row id and stays inside the daemon; the public handle of the author is `author`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalRow {
    pub id: ProposalId,
    pub task: TaskId,
    pub author_worker_id: i64,
    pub author: WorkerName,
    pub title: String,
    pub state: ProposalState,
    pub conflicted: bool,
    pub close_reason: Option<String>,
    /// The highest version number.
    pub current_version: u32,
    /// The latest verdict on the current version is `request_changes`, read with the rule `tasks::load_facts` uses for the status, so the board and the proposal never disagree.
    pub changes_requested: bool,
    /// The worker that holds the unreleased review claim.
    pub review_claim: Option<WorkerName>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub closed_at: Option<Timestamp>,
}

/// One immutable version of a proposal, with the file counts of its head changeset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionRow {
    pub version: u32,
    pub base: RevisionId,
    pub tree: ObjectHash,
    pub head_changeset: ChangesetId,
    pub stats: ChangeCounts,
    pub evidence: Option<String>,
    pub created_by: String,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone)]
pub struct NewProposal<'a> {
    pub id: ProposalId,
    pub task: TaskId,
    pub author_worker_id: i64,
    pub title: &'a str,
}

/// A version to insert. Every object of `tree` must already be in the object store.
#[derive(Debug, Clone)]
pub struct NewVersion<'a> {
    pub base: RevisionId,
    pub tree: ObjectHash,
    pub head_changeset: ChangesetId,
    pub evidence: Option<&'a str>,
    pub created_by: &'a str,
}

/// Inserts an open proposal and its version 1 in the caller's write. A second open proposal for the task fails on `proposals.task_id`, so the partial unique index decides a race. A task, base revision or changeset that does not exist is refused.
pub async fn open_proposal(
    tx: &mut WriteTx,
    new: &NewProposal<'_>,
    first: &NewVersion<'_>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let (number, task, now_ms) = (new.id.to_db(), new.task.to_db(), now.as_millis());
    let inserted = sqlx::query!(
        "INSERT INTO proposals (number, task_id, author_worker_id, title, created_at, updated_at)
         SELECT ?1, t.id, ?3, ?4, ?5, ?5 FROM tasks t WHERE t.number = ?2",
        number,
        task,
        new.author_worker_id,
        new.title,
        now_ms
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if inserted != 1 {
        return Err(StoreError::Invalid("the task does not exist"));
    }
    insert_version(tx, new.id, 1, first, now).await
}

/// Adds version `version` to an open proposal and clears its conflict flag, in the caller's write: a new version answers the conflict report it was made for. Earlier versions are never touched; the unique `(proposal, version)` index refuses a number that exists.
pub async fn add_version(
    tx: &mut WriteTx,
    proposal: ProposalId,
    version: u32,
    new: &NewVersion<'_>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let (number, now_ms) = (proposal.to_db(), now.as_millis());
    let updated = sqlx::query!(
        "UPDATE proposals SET conflicted = 0, updated_at = ?2 WHERE number = ?1 AND state = 'open'",
        number,
        now_ms
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if updated != 1 {
        return Err(StoreError::Invalid("the proposal is not open"));
    }
    insert_version(tx, proposal, version, new, now).await
}

async fn insert_version(
    tx: &mut WriteTx,
    proposal: ProposalId,
    version: u32,
    new: &NewVersion<'_>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let (proposal, version) = (proposal.to_db(), i64::from(version));
    let (base, changeset, now) = (
        new.base.to_db(),
        new.head_changeset.to_db(),
        now.as_millis(),
    );
    let tree = new.tree.to_string();
    let inserted = sqlx::query!(
        "INSERT INTO proposal_versions (proposal_id, version, base_revision_id, tree_hash,
                                        head_changeset_id, evidence, created_by, created_at)
         SELECT p.id, ?2, r.id, ?4, c.id, ?6, ?7, ?8
         FROM proposals p, revisions r, changesets c
         WHERE p.number = ?1 AND r.number = ?3 AND c.number = ?5",
        proposal,
        version,
        base,
        tree,
        changeset,
        new.evidence,
        new.created_by,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if inserted == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid(
            "the proposal, base revision or changeset does not exist",
        ))
    }
}

/// Ends an open proposal: sets its state and close reason and stamps `closed_at`. A closed proposal names its reason and an integrated one has none. False, with nothing changed, when the proposal is not open.
pub async fn retire(
    tx: &mut WriteTx,
    proposal: ProposalId,
    state: RetiredState,
    reason: Option<&str>,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (number, state, now) = (proposal.to_db(), state.as_str(), now.as_millis());
    let updated = sqlx::query!(
        "UPDATE proposals SET state = ?2, close_reason = ?3, closed_at = ?4, updated_at = ?4
         WHERE number = ?1 AND state = 'open'",
        number,
        state,
        reason,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// Releases the unreleased review claim on the proposal with `reason` stored as `release_reason`, and returns its holder; `None`, with nothing changed, when nobody holds one.
pub async fn release_review_claim(
    tx: &mut WriteTx,
    proposal: ProposalId,
    reason: &str,
    now: Timestamp,
) -> Result<Option<WorkerName>, StoreError> {
    let number = proposal.to_db();
    let holder = sqlx::query!(
        r#"SELECT c.id AS "id!: i64", w.name AS worker
           FROM claims c
           JOIN proposals p ON p.id = c.proposal_id
           JOIN workers w ON w.id = c.worker_id
           WHERE p.number = ? AND c.kind = 'review' AND c.released_at IS NULL"#,
        number
    )
    .fetch_optional(tx.conn())
    .await?;
    let Some(holder) = holder else {
        return Ok(None);
    };
    let now = now.as_millis();
    sqlx::query!(
        "UPDATE claims SET released_at = ?2, release_reason = ?3 WHERE id = ?1",
        holder.id,
        now,
        reason
    )
    .execute(tx.conn())
    .await?;
    WorkerName::parse(&holder.worker)
        .map(Some)
        .map_err(|_| StoreError::Corrupt("workers.name"))
}

/// The review checkouts of the proposal that are still live, oldest first.
pub async fn live_review_workspaces(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<Vec<WorkspaceId>, StoreError> {
    let number = proposal.to_db();
    let rows = sqlx::query_scalar!(
        r#"SELECT w.number AS "number!: i64"
           FROM workspaces w JOIN proposals p ON p.id = w.proposal_id
           WHERE p.number = ? AND w.kind = 'review' AND w.state IN ('active', 'missing')
           ORDER BY w.number"#,
        number
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|value| {
            WorkspaceId::try_from_db(value).ok_or(StoreError::Corrupt("workspaces.number"))
        })
        .collect()
}

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

/// The open proposal of the task, or `None`.
pub async fn open_for_task(
    executor: impl SqliteExecutor<'_>,
    task: TaskId,
) -> Result<Option<ProposalId>, StoreError> {
    let number = task.to_db();
    let found = sqlx::query_scalar!(
        r#"SELECT p.number AS "number!: i64"
           FROM proposals p JOIN tasks t ON t.id = p.task_id
           WHERE t.number = ? AND p.state = 'open'"#,
        number
    )
    .fetch_optional(executor)
    .await?;
    found
        .map(|value| ProposalId::try_from_db(value).ok_or(StoreError::Corrupt("proposals.number")))
        .transpose()
}

pub async fn get_proposal(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<Option<ProposalRow>, StoreError> {
    Ok(select(executor, Some(proposal), None, false).await?.pop())
}

/// Proposals in number order, only those in `state` when it is given, and only those without an unreleased review claim when `unclaimed`.
pub async fn list_proposals(
    executor: impl SqliteExecutor<'_>,
    state: Option<ProposalState>,
    unclaimed: bool,
) -> Result<Vec<ProposalRow>, StoreError> {
    select(executor, None, state, unclaimed).await
}

/// One statement serves every read so the rows always map the same way.
async fn select(
    executor: impl SqliteExecutor<'_>,
    proposal: Option<ProposalId>,
    state: Option<ProposalState>,
    unclaimed: bool,
) -> Result<Vec<ProposalRow>, StoreError> {
    let proposal = proposal.map(ProposalId::to_db);
    let state = state.map(ProposalState::as_str);
    let rows = sqlx::query!(
        r#"SELECT p.number AS "number!: i64",
                  t.number AS "task!: i64",
                  p.author_worker_id,
                  w.name AS author,
                  p.title,
                  p.state,
                  p.conflicted AS "conflicted: bool",
                  p.close_reason,
                  (SELECT MAX(v.version) FROM proposal_versions v WHERE v.proposal_id = p.id) AS "current_version?: i64",
                  (
                      SELECT r.verdict FROM reviews r
                      WHERE r.version_id = (
                                SELECT v.id FROM proposal_versions v
                                WHERE v.proposal_id = p.id
                                ORDER BY v.version DESC
                                LIMIT 1
                            )
                        AND r.verdict <> 'comment'
                      ORDER BY r.created_at DESC, r.id DESC
                      LIMIT 1
                  ) AS "verdict?: String",
                  (
                      SELECT k.name FROM claims c JOIN workers k ON k.id = c.worker_id
                      WHERE c.proposal_id = p.id AND c.kind = 'review' AND c.released_at IS NULL
                  ) AS "review_claim?: String",
                  p.created_at,
                  p.updated_at,
                  p.closed_at
           FROM proposals p
           JOIN tasks t ON t.id = p.task_id
           JOIN workers w ON w.id = p.author_worker_id
           WHERE (?1 IS NULL OR p.number = ?1)
             AND (?2 IS NULL OR p.state = ?2)
             AND (NOT ?3 OR NOT EXISTS (
                     SELECT 1 FROM claims c
                     WHERE c.proposal_id = p.id AND c.kind = 'review' AND c.released_at IS NULL
                 ))
           ORDER BY p.number"#,
        proposal,
        state,
        unclaimed
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            let worker = |name: &str| {
                WorkerName::parse(name).map_err(|_| StoreError::Corrupt("workers.name"))
            };
            let current_version = row
                .current_version
                .and_then(|value| u32::try_from(value).ok())
                .ok_or(StoreError::Corrupt("proposal_versions.version"))?;
            Ok(ProposalRow {
                id: ProposalId::try_from_db(row.number)
                    .ok_or(StoreError::Corrupt("proposals.number"))?,
                task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
                author_worker_id: row.author_worker_id,
                author: worker(&row.author)?,
                title: row.title,
                state: ProposalState::parse(&row.state)
                    .ok_or(StoreError::Corrupt("proposals.state"))?,
                conflicted: row.conflicted,
                close_reason: row.close_reason,
                current_version,
                changes_requested: row.verdict.as_deref() == Some("request_changes"),
                review_claim: row.review_claim.as_deref().map(worker).transpose()?,
                created_at: time(row.created_at, "proposals.created_at")?,
                updated_at: time(row.updated_at, "proposals.updated_at")?,
                closed_at: row
                    .closed_at
                    .map(|millis| time(millis, "proposals.closed_at"))
                    .transpose()?,
            })
        })
        .collect()
}

/// The versions of the proposal, oldest first.
pub async fn list_versions(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<Vec<VersionRow>, StoreError> {
    let number = proposal.to_db();
    let rows = sqlx::query!(
        r#"SELECT v.version, r.number AS "base!: i64", v.tree_hash,
                  c.number AS "changeset!: i64", c.stats, v.evidence, v.created_by, v.created_at
           FROM proposal_versions v
           JOIN proposals p ON p.id = v.proposal_id
           JOIN revisions r ON r.id = v.base_revision_id
           JOIN changesets c ON c.id = v.head_changeset_id
           WHERE p.number = ?
           ORDER BY v.version"#,
        number
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(VersionRow {
                version: u32::try_from(row.version)
                    .map_err(|_| StoreError::Corrupt("proposal_versions.version"))?,
                base: RevisionId::try_from_db(row.base)
                    .ok_or(StoreError::Corrupt("revisions.number"))?,
                tree: row
                    .tree_hash
                    .parse()
                    .map_err(|_| StoreError::Corrupt("proposal_versions.tree_hash"))?,
                head_changeset: ChangesetId::try_from_db(row.changeset)
                    .ok_or(StoreError::Corrupt("changesets.number"))?,
                stats: parse_stats(&row.stats).ok_or(StoreError::Corrupt("changesets.stats"))?,
                evidence: row.evidence,
                created_by: row.created_by,
                created_at: time(row.created_at, "proposal_versions.created_at")?,
            })
        })
        .collect()
}

fn time(millis: i64, column: &'static str) -> Result<Timestamp, StoreError> {
    Timestamp::from_millis(millis).map_err(|_| StoreError::Corrupt(column))
}
