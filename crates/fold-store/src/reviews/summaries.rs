use fold_core::reviews::Verdict;
use fold_core::{EventActor, ProposalId, RevisionId, TaskId, Timestamp};
use sqlx::SqliteConnection;

use super::{actor, parse_verdict, time};
use crate::error::StoreError;
use crate::workspaces::parse_stats;

/// The integration that published a proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationRecord {
    pub revision: RevisionId,
    /// A worker name or `human:<name>`.
    pub actor: String,
    pub at: Timestamp,
    /// The Git commit the revision became; `None` until it is written.
    pub commit: Option<String>,
}

/// The proposal a task shows: its newest proposal that is open or integrated. A closed proposal is left out, so a task whose proposal was closed shows none until a new one opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalSummary {
    pub id: ProposalId,
    pub task: TaskId,
    pub title: String,
    /// The worker that holds the unreleased review claim.
    pub review_claim: Option<String>,
    /// The files the head changeset of the current version adds, modifies and deletes.
    pub files: u32,
    pub integrated: bool,
    pub conflicted: bool,
    /// The integration checks of the current version failed, which sends the proposal back to its author until a new version.
    pub checks_failed: bool,
    /// The highest version number.
    pub version: u32,
    /// The latest verdict on the current version that is not a comment.
    pub verdict: Option<Verdict>,
    /// Set once the proposal is integrated.
    pub integration: Option<IntegrationRecord>,
}

/// One review as the task read lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewEntry {
    pub reviewer: EventActor,
    pub verdict: Verdict,
    pub version: u32,
    pub at: Timestamp,
}

/// The open proposal of a task as the stall rule reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTurn {
    pub task: TaskId,
    pub proposal: ProposalId,
    /// The time of the latest verdict on the current version when that verdict requests changes.
    pub changes_requested_at: Option<Timestamp>,
    /// While the proposal is flagged as conflicting or failing its integration checks: the last change of its row, which is the flag or a later handover to a new author.
    pub flagged_at: Option<Timestamp>,
}

/// The [`OpenTurn`] of every open proposal.
pub async fn open_turns(conn: &mut SqliteConnection) -> Result<Vec<OpenTurn>, StoreError> {
    let rows = sqlx::query!(
        r#"SELECT t.number AS "task!: i64", p.number AS "proposal!: i64",
                  CASE WHEN p.conflicted OR p.checks_failed THEN p.updated_at END AS "flagged_at?: i64",
                  CASE WHEN (SELECT pv.verdict FROM proposal_verdicts pv WHERE pv.proposal_id = p.id) = 'request_changes'
                       THEN (SELECT MAX(r.created_at) FROM reviews r
                             WHERE r.verdict = 'request_changes'
                               AND r.version_id = (SELECT v.id FROM proposal_versions v
                                                   WHERE v.proposal_id = p.id
                                                   ORDER BY v.version DESC LIMIT 1))
                  END AS "changes_requested_at?: i64"
           FROM proposals p
           JOIN tasks t ON t.id = p.task_id
           WHERE p.state = 'open'
           ORDER BY t.number"#
    )
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(OpenTurn {
                task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
                proposal: ProposalId::try_from_db(row.proposal)
                    .ok_or(StoreError::Corrupt("proposals.number"))?,
                changes_requested_at: row
                    .changes_requested_at
                    .map(|at| time(at, "reviews.created_at"))
                    .transpose()?,
                flagged_at: row
                    .flagged_at
                    .map(|at| time(at, "proposals.updated_at"))
                    .transpose()?,
            })
        })
        .collect()
}

/// The [`ProposalSummary`] of `task`, or of every task in number order when it is `None`.
pub async fn proposal_summaries(
    conn: &mut SqliteConnection,
    task: Option<TaskId>,
) -> Result<Vec<ProposalSummary>, StoreError> {
    let task = task.map(TaskId::to_db);
    let rows = sqlx::query!(
        r#"SELECT p.number AS "number!: i64", t.number AS "task!: i64", p.state, p.title,
                  (SELECT k.name FROM claims c JOIN workers k ON k.id = c.worker_id
                   WHERE c.proposal_id = p.id AND c.kind = 'review' AND c.released_at IS NULL) AS "review_claim?: String",
                  (SELECT cs.stats FROM proposal_versions v JOIN changesets cs ON cs.id = v.head_changeset_id
                   WHERE v.proposal_id = p.id ORDER BY v.version DESC LIMIT 1) AS "stats?: String",
                  p.conflicted AS "conflicted: bool",
                  p.checks_failed AS "checks_failed: bool",
                  (SELECT MAX(v.version) FROM proposal_versions v WHERE v.proposal_id = p.id) AS "version?: i64",
                  (SELECT pv.verdict FROM proposal_verdicts pv WHERE pv.proposal_id = p.id) AS "verdict?: String",
                  rev.number AS "revision?: i64",
                  i.actor AS "integrated_by?: String",
                  i.created_at AS "integrated_at?: i64",
                  rc."commit" AS "git_commit?: String"
           FROM proposals p
           JOIN tasks t ON t.id = p.task_id
           LEFT JOIN integrations i ON i.proposal_id = p.id AND i.result = 'succeeded'
           LEFT JOIN revisions rev ON rev.id = i.revision_id
           LEFT JOIN revision_commits rc ON rc.revision_id = rev.id
           WHERE p.state IN ('open', 'integrated')
             AND (?1 IS NULL OR t.number = ?1)
             AND p.number = (
                     SELECT MAX(q.number) FROM proposals q
                     WHERE q.task_id = p.task_id AND q.state IN ('open', 'integrated')
                 )
           ORDER BY t.number"#,
        task
    )
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            let integration = match (row.revision, row.integrated_by, row.integrated_at) {
                (Some(revision), Some(by), Some(at)) => Some(IntegrationRecord {
                    revision: RevisionId::try_from_db(revision)
                        .ok_or(StoreError::Corrupt("revisions.number"))?,
                    actor: by,
                    at: time(at, "integrations.created_at")?,
                    commit: row.git_commit,
                }),
                _ => None,
            };
            Ok(ProposalSummary {
                id: ProposalId::try_from_db(row.number)
                    .ok_or(StoreError::Corrupt("proposals.number"))?,
                task: TaskId::try_from_db(row.task).ok_or(StoreError::Corrupt("tasks.number"))?,
                title: row.title,
                review_claim: row.review_claim,
                files: row
                    .stats
                    .as_deref()
                    .and_then(parse_stats)
                    .map(|stats| stats.files_added + stats.files_modified + stats.files_deleted)
                    .ok_or(StoreError::Corrupt("changesets.stats"))?,
                integrated: row.state == "integrated",
                conflicted: row.conflicted,
                checks_failed: row.checks_failed,
                version: row
                    .version
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or(StoreError::Corrupt("proposal_versions.version"))?,
                verdict: row.verdict.as_deref().map(parse_verdict).transpose()?,
                integration,
            })
        })
        .collect()
}

/// Every review of the proposal, newest first.
pub async fn review_entries(
    conn: &mut SqliteConnection,
    proposal: ProposalId,
) -> Result<Vec<ReviewEntry>, StoreError> {
    let number = proposal.to_db();
    let rows = sqlx::query!(
        r#"SELECT r.verdict, w.name AS "worker?: String", r.reviewer_human,
                  v.version, r.created_at
           FROM reviews r
           JOIN proposal_versions v ON v.id = r.version_id
           JOIN proposals p ON p.id = v.proposal_id
           LEFT JOIN workers w ON w.id = r.reviewer_worker_id
           WHERE p.number = ?
             AND (r.kept_from_review_id IS NULL
                 OR NOT EXISTS (SELECT 1 FROM review_again_requests q WHERE q.version_id = r.version_id))
           ORDER BY r.created_at DESC, r.id DESC"#,
        number
    )
    .fetch_all(conn)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ReviewEntry {
                reviewer: actor(
                    row.worker.as_deref(),
                    row.reviewer_human.as_deref(),
                    "reviews",
                )?,
                verdict: parse_verdict(&row.verdict)?,
                version: u32::try_from(row.version)
                    .map_err(|_| StoreError::Corrupt("proposal_versions.version"))?,
                at: time(row.created_at, "reviews.created_at")?,
            })
        })
        .collect()
}
