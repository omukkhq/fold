use fold_core::merge::ConflictReport;
use fold_core::reviews::{HumanReviewMark, ReviewFact};
use fold_core::revision::StoredRevision;
use fold_core::{
    IntegrationId, ProposalId, RevisionId, TaskId, TaskStatus, Timestamp, WorkspaceId,
};
use sqlx::{SqliteConnection, SqliteExecutor};

use super::{ProposalRow, ReviewTarget, get_proposal, review_facts, review_target};
use crate::error::StoreError;
use crate::store::WriteTx;
use crate::tasks;
use crate::workspaces::{RevisionRow, insert_revision, live_task_workspace};

/// What the integration gates of a proposal read, and the task workspace an integration archives, as one connection sees them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationFacts {
    pub proposal: ProposalRow,
    /// The current version.
    pub target: ReviewTarget,
    /// The reviews of the current version only.
    pub reviews: Vec<ReviewFact>,
    pub human_review: Option<HumanReviewMark>,
    pub prerequisites: Vec<(TaskId, TaskStatus)>,
    pub live_workspace: Option<WorkspaceId>,
}

/// The facts of `proposal`, or `None` when it does not exist. Run it in one read transaction, or on the writer's connection inside the write that acts on them.
pub async fn integration_facts(
    conn: &mut SqliteConnection,
    proposal: ProposalId,
) -> Result<Option<IntegrationFacts>, StoreError> {
    let Some(row) = get_proposal(&mut *conn, proposal).await? else {
        return Ok(None);
    };
    let target = review_target(&mut *conn, proposal)
        .await?
        .ok_or(StoreError::Corrupt("proposal_versions.version"))?;
    let reviews = review_facts(&mut *conn, proposal, target.version).await?;
    let human_review = tasks::fetch(conn, row.task)
        .await?
        .ok_or(StoreError::Corrupt("proposals.task_id"))?
        .human_review;
    let prerequisites = tasks::prerequisite_statuses(&mut *conn, row.task).await?;
    let live_workspace = live_task_workspace(&mut *conn, row.task)
        .await?
        .map(|live| live.id);
    Ok(Some(IntegrationFacts {
        proposal: row,
        target,
        reviews,
        human_review,
        prerequisites,
        live_workspace,
    }))
}

/// What an integration brings onto main: a version of a proposal, or the newest commit of the Git remote. A proposal of a sync task carries the remote commit its task resolves, so the Git writers see it as merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationSubject<'a> {
    Proposal {
        proposal: ProposalId,
        version: u32,
        remote_commit: Option<&'a str>,
    },
    Pull {
        remote_commit: &'a str,
    },
}

/// One integration attempt as the `integrations` row records it. `number` comes from the `integration` counter; `actor` is a worker name or `human:<name>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewIntegration<'a> {
    pub number: IntegrationId,
    pub subject: IntegrationSubject<'a>,
    pub actor: &'a str,
    pub created_at: Timestamp,
}

/// Records a succeeded integration and inserts the revision it made, in the caller's write. `stored` names `new.number` as its integration. The two rows name each other and `integrations` refuses `UPDATE`, so the integration row is inserted first with the row id the revision is about to take (the writer is the only one, so it is the next row id), and both foreign keys are checked at commit. The revision must be the next revision; the caller moves `main` to it afterwards.
pub async fn insert_succeeded(
    tx: &mut WriteTx,
    new: &NewIntegration<'_>,
    stored: &StoredRevision,
) -> Result<RevisionRow, StoreError> {
    if stored.revision().integration != Some(new.number) {
        return Err(StoreError::Invalid(
            "the revision does not name the integration",
        ));
    }
    insert(tx, new, "succeeded", "{}").await?;
    let row = insert_revision(tx, stored).await?;
    let (number, revision) = (new.number.to_db(), row.number.to_db());
    let linked = sqlx::query_scalar!(
        r#"SELECT COUNT(*) AS "count!: i64"
           FROM integrations i JOIN revisions r ON r.id = i.revision_id
           WHERE i.number = ?1 AND r.number = ?2"#,
        number,
        revision
    )
    .fetch_one(tx.conn())
    .await?;
    if linked == 1 {
        Ok(row)
    } else {
        Err(StoreError::Corrupt("integrations.revision_id"))
    }
}

/// Records an integration the gates refused, with `report` (JSON text holding the refusal code and ids). No revision is named.
pub async fn insert_refused(
    tx: &mut WriteTx,
    new: &NewIntegration<'_>,
    report: &str,
) -> Result<(), StoreError> {
    insert(tx, new, "refused", report).await
}

/// Records an integration whose merge conflicted, with its report. No revision is named and main does not move.
pub async fn insert_conflicted(
    tx: &mut WriteTx,
    new: &NewIntegration<'_>,
    report: &ConflictReport,
) -> Result<(), StoreError> {
    let report =
        serde_json::to_string(report).map_err(|_| StoreError::Invalid("the conflict report"))?;
    insert(tx, new, "conflicted", &report).await
}

/// Sets the conflict flag of an open proposal, which puts the task back with its author until a new version clears it. False, with nothing changed, when the proposal is not open.
pub async fn mark_conflicted(
    tx: &mut WriteTx,
    proposal: ProposalId,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (number, now) = (proposal.to_db(), now.as_millis());
    let updated = sqlx::query!(
        "UPDATE proposals SET conflicted = 1, updated_at = ?2 WHERE number = ?1 AND state = 'open'",
        number,
        now
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(updated == 1)
}

/// The report of the latest `conflicted` integration of `version` of `proposal`, as it was stored; `None` when that version never conflicted.
pub async fn conflict_report(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
    version: u32,
) -> Result<Option<ConflictReport>, StoreError> {
    let (number, version) = (proposal.to_db(), i64::from(version));
    let report = sqlx::query_scalar!(
        r#"SELECT i.report
           FROM integrations i
           JOIN proposal_versions v ON v.id = i.version_id
           JOIN proposals p ON p.id = v.proposal_id
           WHERE p.number = ?1 AND v.version = ?2 AND i.result = 'conflicted'
           ORDER BY i.id DESC
           LIMIT 1"#,
        number,
        version
    )
    .fetch_optional(executor)
    .await?;
    report
        .map(|text| {
            serde_json::from_str(&text).map_err(|_| StoreError::Corrupt("integrations.report"))
        })
        .transpose()
}

async fn insert(
    tx: &mut WriteTx,
    new: &NewIntegration<'_>,
    result: &str,
    report: &str,
) -> Result<(), StoreError> {
    let number = new.number.to_db();
    let (kind, proposal, version, remote_commit) = match new.subject {
        IntegrationSubject::Proposal {
            proposal,
            version,
            remote_commit,
        } => (
            "proposal",
            Some(proposal.to_db()),
            Some(i64::from(version)),
            remote_commit,
        ),
        IntegrationSubject::Pull { remote_commit } => ("pull", None, None, Some(remote_commit)),
    };
    let succeeded = result == "succeeded";
    let created_at = new.created_at.as_millis();
    sqlx::query!(
        "INSERT INTO integrations
             (number, kind, proposal_id, version_id, remote_commit, result, revision_id, report, actor, created_at)
         VALUES (
             ?1, ?2,
             (SELECT id FROM proposals WHERE number = ?3),
             (SELECT v.id FROM proposal_versions v JOIN proposals p ON p.id = v.proposal_id
              WHERE p.number = ?3 AND v.version = ?4),
             ?5, ?6,
             CASE WHEN ?7 THEN (SELECT COALESCE(MAX(id), 0) + 1 FROM revisions) END,
             ?8, ?9, ?10
         )",
        number,
        kind,
        proposal,
        version,
        remote_commit,
        result,
        succeeded,
        report,
        new.actor,
        created_at
    )
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// The newest main revision whose succeeded integration brought a remote commit in, a pull or the proposal of a sync task, with that commit.
pub async fn latest_remote_commit(
    executor: impl SqliteExecutor<'_>,
) -> Result<Option<(RevisionId, String)>, StoreError> {
    let row = sqlx::query!(
        r#"SELECT r.number AS "number!: i64", i.remote_commit AS "remote_commit!: String"
           FROM integrations i JOIN revisions r ON r.id = i.revision_id
           WHERE i.result = 'succeeded' AND i.remote_commit IS NOT NULL
           ORDER BY r.number DESC
           LIMIT 1"#
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        RevisionId::try_from_db(row.number)
            .map(|number| (number, row.remote_commit))
            .ok_or(StoreError::Corrupt("revisions.number"))
    })
    .transpose()
}
