use fold_core::object::ObjectHash;
use fold_core::policy::AgentStanding;
use fold_core::reviews::{Approver, HumanReviewMark, ProposalIntent, ReviewFact, Side, Verdict};
use fold_core::roles::Role;
use fold_core::tree::ChangeCounts;
use fold_core::{
    ChangesetId, CommentId, EventActor, HumanName, ProposalId, ReviewBatchId, RevisionId, TaskId,
    Timestamp, WorkerName, WorkspaceId,
};
use sqlx::SqliteExecutor;

use crate::columns::time;
use crate::error::StoreError;
use crate::store::WriteTx;
use crate::workspaces::parse_stats;

pub mod batches;
pub mod checks;
mod claims;
mod human_edits;
mod integrations;
pub(crate) mod intents;
pub mod plans;
mod scopes;
mod summaries;

pub use claims::{
    ReviewClaimRow, insert_review_claim, integrated_review_claim_holder, unreleased_review_claim,
};
pub use human_edits::{keep_approvals, request_review_again};
pub use integrations::{
    IntegrationFacts, IntegrationSubject, NewIntegration, conflict_report, insert_conflicted,
    insert_refused, insert_succeeded, integration_facts, mark_checks_failed, mark_conflicted,
};
pub use scopes::{ScopeChoiceRow, insert_review_scope, insert_scope_choice, latest_scope_choice};
pub use summaries::{
    IntegrationRecord, OpenTurn, ProposalSummary, ReviewEntry, open_turns, proposal_summaries,
    review_entries,
};

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
    /// The integration checks of the current version failed, so the proposal is with its author until a new version.
    pub checks_failed: bool,
    /// A request to integrate the current version waits in the integration queue.
    pub integration_queued: bool,
    pub close_reason: Option<String>,
    /// The highest version number.
    pub current_version: u32,
    /// The latest verdict on the current version is `request_changes`, read with the rule `tasks::load_facts` uses for the status, so the board and the proposal never disagree.
    pub changes_requested: bool,
    /// The latest verdict on the current version, comments left out, by the same rule.
    pub latest_verdict: Option<Verdict>,
    /// The worker that holds the unreleased review claim.
    pub review_claim: Option<WorkerName>,
    /// The task's human review mark as it stands now.
    pub human_review: Option<HumanReviewMark>,
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
    /// How the change works in the code, part by part; `None` on a version made before intents existed.
    pub intent: Option<ProposalIntent>,
    pub created_by: String,
    pub created_at: Timestamp,
    /// A human sent this version for review again, so the approvals it kept from an earlier version no longer count.
    pub review_again: bool,
    /// The review checkout whose files the reviewer fixed to make this version; `None` for the author's versions and human edits.
    pub fixed_in_checkout: Option<WorkspaceId>,
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
    /// A checked intent; `None` only for a version made before intents existed.
    pub intent: Option<&'a ProposalIntent>,
    pub created_by: &'a str,
    /// The review checkout a reviewer's fix came from; it must be a review checkout of the same proposal.
    pub fixed_in_checkout: Option<WorkspaceId>,
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

/// Adds version `version` to an open proposal and clears its conflict and failed-checks flags, in the caller's write: a new version answers the conflict report or the failed integration checks it was made for. Earlier versions are never touched; the unique `(proposal, version)` index refuses a number that exists.
pub async fn add_version(
    tx: &mut WriteTx,
    proposal: ProposalId,
    version: u32,
    new: &NewVersion<'_>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let (number, now_ms) = (proposal.to_db(), now.as_millis());
    let updated = sqlx::query!(
        "UPDATE proposals SET conflicted = 0, checks_failed = 0, updated_at = ?2 WHERE number = ?1 AND state = 'open'",
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
    let checkout = new.fixed_in_checkout.map(WorkspaceId::to_db);
    let summary = new.intent.map(|intent| intent.summary.as_str());
    let inserted = sqlx::query_scalar!(
        r#"INSERT INTO proposal_versions (proposal_id, version, base_revision_id, tree_hash,
                                        head_changeset_id, evidence, created_by, created_at,
                                        fixed_in_checkout_id, intent_summary)
         SELECT p.id, ?2, r.id, ?4, c.id, ?6, ?7, ?8,
                (SELECT w.id FROM workspaces w WHERE w.number = ?9), ?10
         FROM proposals p, revisions r, changesets c
         WHERE p.number = ?1 AND r.number = ?3 AND c.number = ?5
           AND (?9 IS NULL OR EXISTS (SELECT 1 FROM workspaces w WHERE w.number = ?9))
         RETURNING id AS "id!: i64""#,
        proposal,
        version,
        base,
        tree,
        changeset,
        new.evidence,
        new.created_by,
        now,
        checkout,
        summary
    )
    .fetch_optional(tx.conn())
    .await?;
    let Some(version_id) = inserted else {
        return Err(StoreError::Invalid(
            "the proposal, base revision, changeset or checkout does not exist",
        ));
    };
    match new.intent {
        Some(intent) => intents::insert(tx, version_id, intent).await,
        None => Ok(()),
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

/// Makes the worker `worker_id` the author of the open proposal. False, with nothing changed, when the proposal is not open.
pub async fn set_author(
    tx: &mut WriteTx,
    proposal: ProposalId,
    worker_id: i64,
    now: Timestamp,
) -> Result<bool, StoreError> {
    let (number, now) = (proposal.to_db(), now.as_millis());
    let updated = sqlx::query!(
        "UPDATE proposals SET author_worker_id = ?2, updated_at = ?3
         WHERE number = ?1 AND state = 'open'",
        number,
        worker_id,
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
    Ok(select_one(executor, proposal).await?.pop())
}

/// Proposals in number order, only those in `state` when it is given. With `unclaimed_at`, only those that no reviewer holds at that cutoff: no unreleased review claim, or one last renewed at or before it. The caller passes the time a lease renewed then would have run out, so a stale claim, which `claim_review` takes over, counts as unclaimed.
pub async fn list_proposals(
    executor: impl SqliteExecutor<'_>,
    state: Option<ProposalState>,
    unclaimed_at: Option<Timestamp>,
) -> Result<Vec<ProposalRow>, StoreError> {
    select(executor, state, unclaimed_at).await
}

/// One proposal row as the reads select it, before its columns are checked.
struct RawProposal {
    number: i64,
    task: i64,
    author_worker_id: i64,
    author: String,
    title: String,
    state: String,
    conflicted: bool,
    checks_failed: bool,
    integration_queued: bool,
    close_reason: Option<String>,
    current_version: Option<i64>,
    verdict: Option<String>,
    review_claim: Option<String>,
    human_review: bool,
    human_reviewer: Option<String>,
    human_review_reason: Option<String>,
    human_review_set_by: Option<String>,
    created_at: i64,
    updated_at: i64,
    closed_at: Option<i64>,
}

impl RawProposal {
    fn into_row(self) -> Result<ProposalRow, StoreError> {
        let row = self;
        let worker =
            |name: &str| WorkerName::parse(name).map_err(|_| StoreError::Corrupt("workers.name"));
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
            checks_failed: row.checks_failed,
            integration_queued: row.integration_queued,
            close_reason: row.close_reason,
            current_version,
            changes_requested: row.verdict.as_deref() == Some("request_changes"),
            latest_verdict: row.verdict.as_deref().map(parse_verdict).transpose()?,
            review_claim: row.review_claim.as_deref().map(worker).transpose()?,
            human_review: crate::tasks::mark(
                row.human_review,
                row.human_reviewer,
                row.human_review_reason,
                row.human_review_set_by,
            )?,
            created_at: time(row.created_at, "proposals.created_at")?,
            updated_at: time(row.updated_at, "proposals.updated_at")?,
            closed_at: row
                .closed_at
                .map(|millis| time(millis, "proposals.closed_at"))
                .transpose()?,
        })
    }
}

/// The columns of [`RawProposal`] and the joins that give them, followed by the tail of each read: every proposal row maps the same way.
macro_rules! select_proposals {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        sqlx::query_as!(
            RawProposal,
            r#"SELECT p.number AS "number!: i64",
                      t.number AS "task!: i64",
                      p.author_worker_id,
                      w.name AS author,
                      p.title,
                      p.state,
                      p.conflicted AS "conflicted: bool",
                      p.checks_failed AS "checks_failed: bool",
                      EXISTS (
                          SELECT 1 FROM integration_requests q
                          WHERE q.proposal_id = p.id AND q.ended_at IS NULL
                            AND q.version = (SELECT MAX(v.version) FROM proposal_versions v WHERE v.proposal_id = p.id)
                      ) AS "integration_queued!: bool",
                      p.close_reason,
                      (SELECT MAX(v.version) FROM proposal_versions v WHERE v.proposal_id = p.id) AS "current_version?: i64",
                      (SELECT pv.verdict FROM proposal_verdicts pv WHERE pv.proposal_id = p.id) AS "verdict?: String",
                      (
                          SELECT k.name FROM claims c JOIN workers k ON k.id = c.worker_id
                          WHERE c.proposal_id = p.id AND c.kind = 'review' AND c.released_at IS NULL
                      ) AS "review_claim?: String",
                      t.human_review AS "human_review: bool",
                      t.human_reviewer,
                      t.human_review_reason,
                      t.human_review_set_by,
                      p.created_at,
                      p.updated_at,
                      p.closed_at
               FROM proposals p
               JOIN tasks t ON t.id = p.task_id
               JOIN workers w ON w.id = p.author_worker_id
               "# + $tail
            $(, $arg)*
        )
    };
}

/// The proposal with this number.
async fn select_one(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<Vec<ProposalRow>, StoreError> {
    let proposal = proposal.to_db();
    let rows = select_proposals!("WHERE p.number = ?1", proposal)
        .fetch_all(executor)
        .await?;
    rows.into_iter().map(RawProposal::into_row).collect()
}

/// Proposals in number order, filtered as [`list_proposals`] says.
async fn select(
    executor: impl SqliteExecutor<'_>,
    state: Option<ProposalState>,
    unclaimed_at: Option<Timestamp>,
) -> Result<Vec<ProposalRow>, StoreError> {
    let state = state.map(ProposalState::as_str);
    let unclaimed_at = unclaimed_at.map(Timestamp::as_millis);
    let rows = select_proposals!(
        "WHERE (?1 IS NULL OR p.state = ?1)
           AND (?2 IS NULL OR NOT EXISTS (
                   SELECT 1 FROM claims c
                   WHERE c.proposal_id = p.id AND c.kind = 'review' AND c.released_at IS NULL
                     AND c.renewed_at > ?2
               ))
         ORDER BY p.number",
        state,
        unclaimed_at
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(RawProposal::into_row).collect()
}

/// The versions of the proposal, oldest first.
pub async fn list_versions(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<Vec<VersionRow>, StoreError> {
    let number = proposal.to_db();
    let rows = sqlx::query!(
        r#"SELECT v.version, r.number AS "base!: i64", v.tree_hash,
                  c.number AS "changeset!: i64", c.stats, v.evidence, vi.intent AS "intent?: String", v.created_by, v.created_at,
                  EXISTS (SELECT 1 FROM review_again_requests q WHERE q.version_id = v.id) AS "review_again!: bool",
                  f.number AS "fixed_in?: i64"
           FROM proposal_versions v
           JOIN proposals p ON p.id = v.proposal_id
           JOIN revisions r ON r.id = v.base_revision_id
           JOIN changesets c ON c.id = v.head_changeset_id
           LEFT JOIN workspaces f ON f.id = v.fixed_in_checkout_id
           LEFT JOIN proposal_version_intents vi ON vi.version_id = v.id
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
                intent: intents::decode(row.intent.as_deref(), "proposal_version_intents.intent")?,
                created_by: row.created_by,
                created_at: time(row.created_at, "proposal_versions.created_at")?,
                review_again: row.review_again,
                fixed_in_checkout: row
                    .fixed_in
                    .map(|value| {
                        WorkspaceId::try_from_db(value)
                            .ok_or(StoreError::Corrupt("workspaces.number"))
                    })
                    .transpose()?,
            })
        })
        .collect()
}

/// The current version of a proposal with the two trees its line comments read: `base_tree` is the tree of the version's base revision (side `old`) and `tree` the version's own (side `new`). `version_id` is a row id and stays inside the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewTarget {
    pub version_id: i64,
    pub version: u32,
    pub base_tree: ObjectHash,
    pub tree: ObjectHash,
}

/// Who writes a review or a comment, as the rows hold it: a worker row, or a human by the name of `human:<name>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewerRef<'a> {
    Worker(i64),
    Human(&'a HumanName),
}

impl<'a> ReviewerRef<'a> {
    /// The worker and human columns.
    fn columns(self) -> (Option<i64>, Option<&'a str>) {
        match self {
            ReviewerRef::Worker(id) => (Some(id), None),
            ReviewerRef::Human(name) => (None, Some(name.as_str())),
        }
    }
}

/// Where a line comment points. `start_line` and `end_line` are 1-based and inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange<'a> {
    pub path: &'a str,
    pub side: Side,
    pub start_line: u32,
    pub end_line: u32,
}

/// A comment to insert under the review `review_id`; it takes that review's version and has no location when it is about the whole version.
#[derive(Debug, Clone, Copy)]
pub struct NewComment<'a> {
    pub id: CommentId,
    pub review_id: i64,
    pub author: ReviewerRef<'a>,
    pub location: Option<LineRange<'a>>,
    pub reply_to: Option<CommentId>,
    pub body: &'a str,
    /// The row id of the batch comment this row puts on its member, whose body the batch comment holds.
    pub batch_comment: Option<i64>,
}

/// One stored comment with public ids only. `verdict` is the verdict of the review it came with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentRow {
    pub id: CommentId,
    pub version: u32,
    pub verdict: Verdict,
    pub author: EventActor,
    pub path: Option<String>,
    pub side: Option<Side>,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub reply_to: Option<CommentId>,
    pub body: String,
    pub created_at: Timestamp,
    /// The batch comment this comment puts on its proposal, when it is one.
    pub batch_comment: Option<BatchCommentRef>,
}

/// A comment of a review batch about members together: `number` counts the batch's comments from 1, and `members` are the proposals it is on, in number order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchCommentRef {
    pub batch: ReviewBatchId,
    pub number: u32,
    pub members: Vec<ProposalId>,
}

/// The current version of the proposal and its two trees, or `None` for a proposal that does not exist.
pub async fn review_target(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<Option<ReviewTarget>, StoreError> {
    let number = proposal.to_db();
    let row = sqlx::query!(
        r#"SELECT v.id AS "id!: i64", v.version, r.tree_hash AS base_tree, v.tree_hash
           FROM proposal_versions v
           JOIN proposals p ON p.id = v.proposal_id
           JOIN revisions r ON r.id = v.base_revision_id
           WHERE p.number = ?
           ORDER BY v.version DESC
           LIMIT 1"#,
        number
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(ReviewTarget {
            version_id: row.id,
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
        })
    })
    .transpose()
}

/// Version `version` of the proposal and its two trees, as [`review_target`] reads the current one; `None` when the proposal has no such version.
pub async fn version_target(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
    version: u32,
) -> Result<Option<ReviewTarget>, StoreError> {
    let number = proposal.to_db();
    let version = i64::from(version);
    let row = sqlx::query!(
        r#"SELECT v.id AS "id!: i64", v.version, r.tree_hash AS base_tree, v.tree_hash
           FROM proposal_versions v
           JOIN proposals p ON p.id = v.proposal_id
           JOIN revisions r ON r.id = v.base_revision_id
           WHERE p.number = ?1 AND v.version = ?2"#,
        number,
        version
    )
    .fetch_optional(executor)
    .await?;
    row.map(|row| {
        Ok(ReviewTarget {
            version_id: row.id,
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
        })
    })
    .transpose()
}

/// Inserts a review of the version row `version_id` in the caller's write and returns its row id. The table is append-only, so a review is never changed afterwards.
pub async fn insert_review(
    tx: &mut WriteTx,
    version_id: i64,
    reviewer: ReviewerRef<'_>,
    verdict: Verdict,
    body: &str,
    now: Timestamp,
) -> Result<i64, StoreError> {
    let (worker, human) = reviewer.columns();
    let (verdict, now) = (verdict.as_str(), now.as_millis());
    let id = sqlx::query_scalar!(
        r#"INSERT INTO reviews (version_id, reviewer_worker_id, reviewer_human, verdict, body, created_at)
           VALUES (?, ?, ?, ?, ?, ?)
           RETURNING id AS "id!: i64""#,
        version_id,
        worker,
        human,
        verdict,
        body,
        now
    )
    .fetch_one(tx.conn())
    .await?;
    Ok(id)
}

/// Inserts a comment in the caller's write, on the version of its review.
pub async fn insert_comment(
    tx: &mut WriteTx,
    new: &NewComment<'_>,
    now: Timestamp,
) -> Result<(), StoreError> {
    let (worker, human) = new.author.columns();
    let number = new.id.to_db();
    let path = new.location.map(|range| range.path);
    let side = new.location.map(|range| range.side.as_str());
    let start = new.location.map(|range| i64::from(range.start_line));
    let end = new.location.map(|range| i64::from(range.end_line));
    let reply_to = new.reply_to.map(CommentId::to_db);
    let now = now.as_millis();
    let inserted = sqlx::query!(
        "INSERT INTO review_comments (number, version_id, review_id, author_worker_id, author_human,
                                      path, start_line, end_line, side, reply_to, body, created_at,
                                      batch_comment_id)
         SELECT ?1, r.version_id, r.id, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12
         FROM reviews r WHERE r.id = ?2",
        number,
        new.review_id,
        worker,
        human,
        path,
        start,
        end,
        side,
        reply_to,
        new.body,
        now,
        new.batch_comment
    )
    .execute(tx.conn())
    .await?
    .rows_affected();
    if inserted == 1 {
        Ok(())
    } else {
        Err(StoreError::Invalid("the review does not exist"))
    }
}

/// The proposal a comment belongs to, or `None` when no comment has this number.
pub async fn comment_proposal(
    executor: impl SqliteExecutor<'_>,
    comment: CommentId,
) -> Result<Option<ProposalId>, StoreError> {
    let number = comment.to_db();
    let found = sqlx::query_scalar!(
        r#"SELECT p.number AS "number!: i64"
           FROM review_comments c
           JOIN proposal_versions v ON v.id = c.version_id
           JOIN proposals p ON p.id = v.proposal_id
           WHERE c.number = ?"#,
        number
    )
    .fetch_optional(executor)
    .await?;
    found
        .map(|value| ProposalId::try_from_db(value).ok_or(StoreError::Corrupt("proposals.number")))
        .transpose()
}

/// The reviews of one version of the proposal. `seq` is each review's place in `(created_at, id)` order, counted from 1: the order the status cache reads the latest verdict in, so `fold_core::reviews::latest_verdict` over these facts and the board never disagree. Each approver's weight comes from the agent's `main` tag and worker role at the time of this read. The approvals a human edit kept count as the version's own until a human sends the version for review again.
pub async fn review_facts(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
    version: u32,
) -> Result<Vec<ReviewFact>, StoreError> {
    let (number, version) = (proposal.to_db(), i64::from(version));
    let rows = sqlx::query!(
        r#"SELECT r.verdict, w.name AS "worker?: String", r.reviewer_human,
                  EXISTS (
                      SELECT 1 FROM agent_tags a
                      WHERE a.agent_id = w.agent_id AND a.tag = 'main'
                  ) AS "holds_main!: bool",
                  (SELECT wp.role FROM worker_processes wp WHERE wp.agent_id = w.agent_id) AS "role?: String"
           FROM reviews r
           JOIN proposal_versions v ON v.id = r.version_id
           JOIN proposals p ON p.id = v.proposal_id
           LEFT JOIN workers w ON w.id = r.reviewer_worker_id
           WHERE p.number = ?1 AND v.version = ?2
             AND (r.kept_from_review_id IS NULL
                 OR NOT EXISTS (SELECT 1 FROM review_again_requests q WHERE q.version_id = r.version_id))
           ORDER BY r.created_at, r.id"#,
        number,
        version
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .zip(1_u64..)
        .map(|(row, seq)| {
            let reviewer = actor(
                row.worker.as_deref(),
                row.reviewer_human.as_deref(),
                "reviews",
            )?;
            let approver = match row.reviewer_human.as_deref() {
                Some(human) => Approver::Human(
                    HumanName::parse(human).map_err(|_| StoreError::Corrupt("reviews"))?,
                ),
                None => {
                    let role = row
                        .role
                        .as_deref()
                        .map(|role| {
                            Role::parse(role).ok_or(StoreError::Corrupt("worker_processes.role"))
                        })
                        .transpose()?;
                    let standing = AgentStanding {
                        holds_main: row.holds_main,
                        role,
                    };
                    if standing.acts_on_reviews() {
                        Approver::DecidingAgent
                    } else {
                        Approver::OtherAgent
                    }
                }
            };
            Ok(ReviewFact {
                verdict: parse_verdict(&row.verdict)?,
                reviewer,
                approver,
                seq,
            })
        })
        .collect()
}

/// The comments of the proposal in number order, only those on `version` when it is given.
pub async fn list_comments(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
    version: Option<u32>,
) -> Result<Vec<CommentRow>, StoreError> {
    let number = proposal.to_db();
    let version = version.map(i64::from);
    let rows = sqlx::query!(
        r#"SELECT c.number AS "number!: i64", v.version, r.verdict,
                  w.name AS "worker?: String", c.author_human,
                  c.path, c.side, c.start_line, c.end_line, c.reply_to,
                  COALESCE(bc.body, c.body) AS "body!: String", c.created_at,
                  b.number AS "batch?: i64", bc.number AS "batch_comment?: i64",
                  (SELECT group_concat(mp.number)
                   FROM review_comments mc
                   JOIN proposal_versions mv ON mv.id = mc.version_id
                   JOIN proposals mp ON mp.id = mv.proposal_id
                   WHERE mc.batch_comment_id = c.batch_comment_id) AS "batch_members?: String"
           FROM review_comments c
           JOIN reviews r ON r.id = c.review_id
           JOIN proposal_versions v ON v.id = c.version_id
           JOIN proposals p ON p.id = v.proposal_id
           LEFT JOIN workers w ON w.id = c.author_worker_id
           LEFT JOIN review_batch_comments bc ON bc.id = c.batch_comment_id
           LEFT JOIN review_batches b ON b.id = bc.batch_id
           WHERE p.number = ?1 AND (?2 IS NULL OR v.version = ?2)
           ORDER BY c.number"#,
        number,
        version
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            let line = |value: Option<i64>| {
                value
                    .map(|line| {
                        u32::try_from(line).map_err(|_| StoreError::Corrupt("review_comments.line"))
                    })
                    .transpose()
            };
            Ok(CommentRow {
                id: CommentId::try_from_db(row.number)
                    .ok_or(StoreError::Corrupt("review_comments.number"))?,
                version: u32::try_from(row.version)
                    .map_err(|_| StoreError::Corrupt("proposal_versions.version"))?,
                verdict: parse_verdict(&row.verdict)?,
                author: actor(
                    row.worker.as_deref(),
                    row.author_human.as_deref(),
                    "review_comments",
                )?,
                path: row.path,
                side: row
                    .side
                    .map(|side| {
                        Side::parse(&side).ok_or(StoreError::Corrupt("review_comments.side"))
                    })
                    .transpose()?,
                start_line: line(row.start_line)?,
                end_line: line(row.end_line)?,
                reply_to: row
                    .reply_to
                    .map(|reply| {
                        CommentId::try_from_db(reply)
                            .ok_or(StoreError::Corrupt("review_comments.reply_to"))
                    })
                    .transpose()?,
                body: row.body,
                created_at: time(row.created_at, "review_comments.created_at")?,
                batch_comment: batch_comment_ref(row.batch, row.batch_comment, row.batch_members)?,
            })
        })
        .collect()
}

fn batch_comment_ref(
    batch: Option<i64>,
    number: Option<i64>,
    members: Option<String>,
) -> Result<Option<BatchCommentRef>, StoreError> {
    let (Some(batch), Some(number)) = (batch, number) else {
        return Ok(None);
    };
    Ok(Some(BatchCommentRef {
        batch: ReviewBatchId::try_from_db(batch)
            .ok_or(StoreError::Corrupt("review_batches.number"))?,
        number: u32::try_from(number)
            .map_err(|_| StoreError::Corrupt("review_batch_comments.number"))?,
        members: batches::proposal_list(members.as_deref())?,
    }))
}

/// One stored review with public ids only: the version it reviewed, its writer, and how many comments it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRow {
    pub version: u32,
    pub verdict: Verdict,
    pub reviewer: EventActor,
    pub body: String,
    pub comments: u32,
    pub created_at: Timestamp,
    /// For an approval a human edit kept, the version whose approval it copies.
    pub kept_from: Option<u32>,
    /// The scope of an agent's review of a revised version; `None` for a review of a version with no earlier verdict, for every review by a human, and for an approval a human edit kept.
    pub scope: Option<fold_core::reviews::ReviewScopeRecord>,
}

/// The reviews of every version of the proposal, in the `(created_at, id)` order that `review_facts` counts in. The kept approvals of a version sent for review again are left out, as `review_facts` leaves them out.
pub async fn list_reviews(
    executor: impl SqliteExecutor<'_>,
    proposal: ProposalId,
) -> Result<Vec<ReviewRow>, StoreError> {
    let number = proposal.to_db();
    let rows = sqlx::query!(
        r#"SELECT v.version, r.verdict, w.name AS "worker?: String", r.reviewer_human, r.body,
                  r.created_at,
                  (SELECT COUNT(*) FROM review_comments c WHERE c.review_id = r.id) AS "comments!: i64",
                  (SELECT kv.version FROM reviews kept JOIN proposal_versions kv ON kv.id = kept.version_id
                   WHERE kept.id = r.kept_from_review_id) AS "kept_from?: i64",
                  s.since_version AS "scope_since?: i64", s.suggested AS "scope_suggested?: String",
                  s.rules AS "scope_rules?: String", s.scope AS "scope?: String",
                  s.reason AS "scope_reason?: String"
           FROM reviews r
           JOIN proposal_versions v ON v.id = r.version_id
           JOIN proposals p ON p.id = v.proposal_id
           LEFT JOIN workers w ON w.id = r.reviewer_worker_id
           LEFT JOIN review_scopes s ON s.review_id = r.id
           WHERE p.number = ?
             AND (r.kept_from_review_id IS NULL
                 OR NOT EXISTS (SELECT 1 FROM review_again_requests q WHERE q.version_id = r.version_id))
           ORDER BY r.created_at, r.id"#,
        number
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ReviewRow {
                version: u32::try_from(row.version)
                    .map_err(|_| StoreError::Corrupt("proposal_versions.version"))?,
                verdict: parse_verdict(&row.verdict)?,
                reviewer: actor(
                    row.worker.as_deref(),
                    row.reviewer_human.as_deref(),
                    "reviews",
                )?,
                body: row.body,
                comments: u32::try_from(row.comments)
                    .map_err(|_| StoreError::Corrupt("review_comments.review_id"))?,
                created_at: time(row.created_at, "reviews.created_at")?,
                kept_from: row
                    .kept_from
                    .map(|version| {
                        u32::try_from(version)
                            .map_err(|_| StoreError::Corrupt("proposal_versions.version"))
                    })
                    .transpose()?,
                scope: scopes::stored_scope(
                    row.scope_since,
                    row.scope_suggested.as_deref(),
                    row.scope_rules.as_deref(),
                    row.scope.as_deref(),
                    row.scope_reason,
                )?,
            })
        })
        .collect()
}

fn parse_verdict(text: &str) -> Result<Verdict, StoreError> {
    Verdict::parse(text).ok_or(StoreError::Corrupt("reviews.verdict"))
}

/// The label of a review's or comment's writer: the worker's name or `human:<name>`.
fn actor(
    worker: Option<&str>,
    human: Option<&str>,
    table: &'static str,
) -> Result<EventActor, StoreError> {
    match (worker, human) {
        (Some(worker), None) => WorkerName::parse(worker)
            .map(|name| EventActor::worker(&name))
            .map_err(|_| StoreError::Corrupt("workers.name")),
        (None, Some(human)) => HumanName::parse(human)
            .map(|name| EventActor::human(&name))
            .map_err(|_| StoreError::Corrupt(table)),
        _ => Err(StoreError::Corrupt(table)),
    }
}
