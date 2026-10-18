use fold_core::checkpoint_policy::AutoReason;
use fold_core::claims::stale_cutoff;
use fold_core::events::types;
use fold_core::merge::ConflictReport;
use fold_core::object::ObjectHash;
use fold_core::reviews::MAX_REASON_CHARS;
use fold_core::reviews::checks::Purpose;
use fold_core::reviews::{HumanReviewMark, ReviewFact, Verdict, human_approval_standing};
use fold_core::status::ProposalFact;
use fold_core::tree::ChangeCounts;
use fold_core::{
    Actor, AgentActor, ChangesetId, CheckRunId, EventActor, FoldError, IdKind, NewEvent,
    ProposalId, RevisionId, TaskId, TaskStatus, Timestamp, WorkerName, WorkspaceId, codes, policy,
};
use fold_store::reviews::{
    self, NewProposal, NewVersion, ProposalRow, ProposalState, RetiredState, VersionRow,
};
use fold_store::tasks::{TaskRow, claims};
use fold_store::workspaces::{self, WorkspaceRow, WorkspaceState};
use fold_store::{SqliteConnection, StoreError, WriteTx, agents, counters, tasks};
use fold_vcs::restore::differs_from_latest;
use serde_json::json;

use super::batches::close_batch_of;
use super::checks::{GateDecision, Waiting};
use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;
use crate::services::page_limit;
use crate::services::tasks::{
    closed_refusal, proposal_kind_refusal, recompute, task_not_found, worker_row,
};
use crate::services::workspaces::{
    ArchiveReason, ProposalSnapshot, archive_in_tx, auto_checkpoint_in_tx,
};

/// The longest proposal title, in characters.
const MAX_TITLE_CHARS: usize = 200;

/// Whose move it is on an open proposal. It follows the status rule, so a proposal `WithReviewer` is a task in Review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    WithReviewer,
    WithAuthor,
}

impl Turn {
    pub fn as_str(self) -> &'static str {
        match self {
            Turn::WithReviewer => "with_reviewer",
            Turn::WithAuthor => "with_author",
        }
    }

    /// `None` for a closed or integrated proposal.
    fn of(row: &ProposalRow) -> Option<Turn> {
        if row.state != ProposalState::Open {
            return None;
        }
        Some(
            match ProposalFact::open(row.changes_requested, row.conflicted) {
                ProposalFact::WithAuthor => Turn::WithAuthor,
                ProposalFact::WithReviewer | ProposalFact::None => Turn::WithReviewer,
            },
        )
    }
}

/// A proposal with public ids only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalSummary {
    pub id: ProposalId,
    pub task: TaskId,
    pub title: String,
    pub author: WorkerName,
    pub state: ProposalState,
    pub conflicted: bool,
    pub current_version: u32,
    pub review_claim: Option<WorkerName>,
    /// The task's human review mark as it stands now.
    pub human_review: Option<HumanReviewMark>,
    pub turn: Option<Turn>,
    /// The latest verdict on the current version; comments are not verdicts.
    pub latest_verdict: Option<Verdict>,
    /// The human review mark is met on the current version, as the integration gate reads it for an agent; true without a mark. It tells an agent's approval that still waits for a human from a full one.
    pub human_approved: bool,
    pub close_reason: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub closed_at: Option<Timestamp>,
}

impl ProposalSummary {
    /// `facts` are the reviews of the current version; a row without a human review mark needs none.
    fn new(row: ProposalRow, facts: &[ReviewFact]) -> Self {
        let human_approved = human_approval_standing(row.human_review.as_ref(), facts, None);
        Self {
            turn: Turn::of(&row),
            latest_verdict: row.latest_verdict,
            human_approved,
            id: row.id,
            task: row.task,
            title: row.title,
            author: row.author,
            state: row.state,
            conflicted: row.conflicted,
            current_version: row.current_version,
            review_claim: row.review_claim,
            human_review: row.human_review,
            close_reason: row.close_reason,
            created_at: row.created_at,
            updated_at: row.updated_at,
            closed_at: row.closed_at,
        }
    }
}

/// One version: what it was based on, the tree it proposes with the file counts of its head changeset, and the author's evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionView {
    pub version: u32,
    pub base: RevisionId,
    pub tree: ObjectHash,
    pub changeset: ChangesetId,
    pub stats: ChangeCounts,
    pub evidence: Option<String>,
    pub created_by: String,
    pub created_at: Timestamp,
    /// A human made this version by editing the one before it in a human review, so it keeps that version's agent approvals until it is sent for review again.
    pub human_edit: bool,
    /// A human sent this version for review again, so the approvals it kept from the version before no longer count.
    pub review_again: bool,
    /// The review checkout in which the reviewer that holds the review claim fixed the version before; `created_by` is that reviewer. `None` for the author's versions and human edits.
    pub fixed_in_checkout: Option<WorkspaceId>,
}

impl From<VersionRow> for VersionView {
    fn from(row: VersionRow) -> Self {
        Self {
            version: row.version,
            base: row.base,
            tree: row.tree,
            changeset: row.head_changeset,
            human_edit: is_human_edit(&row.created_by),
            stats: row.stats,
            evidence: row.evidence,
            created_by: row.created_by,
            created_at: row.created_at,
            review_again: row.review_again,
            fixed_in_checkout: row.fixed_in_checkout,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalDetail {
    pub summary: ProposalSummary,
    /// Oldest first.
    pub versions: Vec<VersionView>,
    /// The version that was asked for, else the current one.
    pub selected: VersionView,
    /// The report of the latest conflicted integration of the selected version. A new version has none until an integration of it conflicts.
    pub conflict: Option<ConflictReport>,
    /// The status of the proposal's task as the board shows it.
    pub task_status: TaskStatus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProposalFilter {
    pub state: Option<ProposalState>,
    /// Only proposals that no reviewer holds: without an unreleased review claim, or with one whose lease has run out.
    pub unclaimed: bool,
    /// Only the newest this many, lowered to [`MAX_PROPOSAL_LIST`]; 0 is `invalid_request` and `None` lists all.
    pub limit: Option<u32>,
}

/// The most proposals one listing returns.
pub const MAX_PROPOSAL_LIST: u32 = 200;

/// The proposals an agent's listing returns when it names no limit.
pub const DEFAULT_PROPOSAL_LIST: u32 = 50;

/// How an open proposal ends. A closed or rejected proposal names why; an integrated one needs no reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retirement<'a> {
    Closed { reason: &'a str },
    Rejected { reason: &'a str },
    Integrated,
}

impl<'a> Retirement<'a> {
    /// The text stored as the review claim's `release_reason`.
    pub fn as_str(self) -> &'static str {
        match self {
            Retirement::Closed { .. } => "closed",
            Retirement::Rejected { .. } => "rejected",
            Retirement::Integrated => "integrated",
        }
    }

    fn state(self) -> RetiredState {
        match self {
            Retirement::Closed { .. } | Retirement::Rejected { .. } => RetiredState::Closed,
            Retirement::Integrated => RetiredState::Integrated,
        }
    }

    fn close_reason(self) -> Option<&'a str> {
        match self {
            Retirement::Closed { reason } | Retirement::Rejected { reason } => Some(reason),
            Retirement::Integrated => None,
        }
    }

    fn archive_reason(self) -> ArchiveReason {
        match self {
            Retirement::Closed { .. } => ArchiveReason::ProposalClosed,
            Retirement::Rejected { .. } => ArchiveReason::ProposalRejected,
            Retirement::Integrated => ArchiveReason::Integrated,
        }
    }
}

/// What `create_proposal` and `update_proposal` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proposed {
    /// The version exists: the tree had no proposal check to run, or a passed run of the same files on the same base stands.
    Made(Box<ProposalDetail>),
    /// fold runs the proposal checks in the background and makes the version only when they pass; the result comes as a message from fold.
    Checking {
        run: CheckRunId,
        task: TaskId,
        workspace: WorkspaceId,
    },
}

impl Proposed {
    /// The proposal, when the version was made at once.
    pub fn detail(self) -> Option<ProposalDetail> {
        match self {
            Proposed::Made(detail) => Some(*detail),
            Proposed::Checking { .. } => None,
        }
    }
}

/// The inputs of the version a proposal command makes once its checks allow it.
pub(super) struct VersionInput<'a> {
    pub actor: &'a Actor,
    pub snapped: &'a WorkspaceRow,
    pub recorded: &'a ProposalSnapshot,
    pub evidence: Option<&'a str>,
    /// The version adds, changes or removes the checks file, so it needs a human.
    pub checks_changed: bool,
    pub now: Timestamp,
}

impl ReviewService {
    /// Opens a proposal for `task` with version 1, by the worker that holds the task's implementation claim and owns its active task workspace. The workspace is snapshotted first, outside the transaction, so every object exists before a row names it; the snapshot follows the rules of `record_changes`, so a workspace with an unresolved conflict or a sync or restore that stopped part-way is refused, and a resolved conflict list is cleared with the proposal. When the snapshot's `fold-checks.toml` has a proposal check that runs for the change and no passed run of the same files on the same base stands, nothing more is written now: fold runs the checks in the background (`Proposed::Checking`) and makes the proposal only when they pass. Otherwise the first step of the transaction is an automatic `before_proposal` checkpoint of the snapshot, unless the latest checkpoint already holds it on the same base, so a checkpoint that cannot be taken refuses the proposal. When the snapshot differs from the workspace's latest changeset, a changeset with the title as its message is recorded with it. The task moves to Review. A version that adds, changes or removes the checks file gives the task the human review mark when it has none.
    pub async fn create_proposal(
        &self,
        actor: &Actor,
        task: TaskId,
        title: &str,
        evidence: Option<&str>,
    ) -> Result<Proposed, ReviewError> {
        let title = check_title(title)?;
        let evidence = check_evidence(evidence);
        let read = self.store.read();
        let facts = OpenFacts {
            task: tasks::get_by_number(read, task).await?,
            holder: claims::unreleased_for_task(read, task).await?,
            open: reviews::open_for_task(read, task).await?,
            live: workspaces::live_task_workspace(read, task).await?,
        };
        let workspace = open_checks(actor, task, facts)?.1;
        let recorded = self.workspaces.snapshot_for_proposal(&workspace).await?;
        let gate = self.proposal_gate(&workspace, recorded.tree).await?;
        let checks_changed = match gate {
            GateDecision::Ready { checks_changed } => checks_changed,
            GateDecision::Run(job) => {
                let tree = recorded.tree;
                drop(recorded);
                let waiting = Waiting {
                    proposal: None,
                    title: Some(title),
                    evidence: evidence.as_deref(),
                };
                return self
                    .start_gate(
                        actor,
                        &workspace,
                        tree,
                        Purpose::CreateProposal,
                        waiting,
                        job,
                    )
                    .await;
            }
        };
        let input = VersionInput {
            actor,
            snapped: &workspace,
            recorded: &recorded,
            evidence: evidence.as_deref(),
            checks_changed,
            now: recorded.taken_at,
        };
        let mut buffer = EventBuffer::new();
        let id = self
            .store
            .write(async |tx| open_in_tx(tx, &mut buffer, task, title, &input).await)
            .await?;
        drop(recorded);
        self.bus.publish(buffer);
        self.workspaces
            .proposal_changed(workspace.id, Some(id))
            .await;
        Ok(Proposed::Made(Box::new(self.get_proposal(id, None).await?)))
    }

    /// Adds the next version to an open proposal, by the author's worker: the task workspace is snapshotted as `create_proposal` does, and the version takes the workspace's current base; its automatic checkpoint has the reason `before_proposal_update`. `evidence` replaces the current version's evidence; `None` keeps it. A version whose base, tree and evidence all equal the current one is refused with `version_unchanged`. The proposal checks gate the version as they gate `create_proposal`. The conflict flag is cleared and the task returns to Review; earlier versions, their reviews and their approvals stay as they are.
    pub async fn update_proposal(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        evidence: Option<&str>,
    ) -> Result<Proposed, ReviewError> {
        let evidence = check_evidence(evidence);
        let read = self.store.read();
        let row = update_checks(
            actor,
            proposal,
            reviews::get_proposal(read, proposal).await?,
        )?;
        let live = workspaces::live_task_workspace(read, row.task).await?;
        let workspace = usable_workspace(live, row.task, &row.author)?;
        let recorded = self.workspaces.snapshot_for_proposal(&workspace).await?;
        let current = reviews::list_versions(read, proposal)
            .await?
            .pop()
            .ok_or(StoreError::Corrupt("proposal_versions.version"))?;
        unchanged_refusal(
            proposal,
            &current,
            workspace.base,
            recorded.tree,
            evidence.clone().or(current.evidence.clone()).as_deref(),
        )?;
        let gate = self.proposal_gate(&workspace, recorded.tree).await?;
        let checks_changed = match gate {
            GateDecision::Ready { checks_changed } => checks_changed,
            GateDecision::Run(job) => {
                let tree = recorded.tree;
                drop(recorded);
                let waiting = Waiting {
                    proposal: Some(proposal),
                    title: None,
                    evidence: evidence.as_deref(),
                };
                return self
                    .start_gate(
                        actor,
                        &workspace,
                        tree,
                        Purpose::UpdateProposal,
                        waiting,
                        job,
                    )
                    .await;
            }
        };
        let input = VersionInput {
            actor,
            snapped: &workspace,
            recorded: &recorded,
            evidence: evidence.as_deref(),
            checks_changed,
            now: recorded.taken_at,
        };
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| add_version_in_tx(tx, &mut buffer, proposal, &input).await)
            .await?;
        drop(recorded);
        self.bus.publish(buffer);
        self.workspaces
            .proposal_changed(workspace.id, Some(proposal))
            .await;
        Ok(Proposed::Made(Box::new(
            self.get_proposal(proposal, None).await?,
        )))
    }

    /// Closes an open proposal, by the author's agent, an agent that decides reviews (the main agent, the review lead or a batch reviewer) or a human. Its review claim is released and its review checkouts are archived; the task returns to In Progress because its workspace stays active. Closing is what allows `abandon_workspace` afterwards. `reason` is the closer's short explanation, which `proposal.closed` carries.
    pub async fn close_proposal(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        reason: &str,
    ) -> Result<ProposalDetail, ReviewError> {
        let reason = reason.trim();
        if reason.is_empty() || reason.chars().count() > MAX_REASON_CHARS {
            return Err(FoldError::new(
                codes::INVALID_REQUEST,
                format!("the reason must be 1 to {MAX_REASON_CHARS} characters"),
            )
            .with_details([("field", json!("reason"))])
            .with_hint("call close_proposal again with a short reason")
            .into());
        }
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let task = self
            .store
            .write(async |tx| {
                let row =
                    require_open(reviews::get_proposal(tx.conn(), proposal).await?, proposal)?;
                let standing = agents::standing_of(tx.conn(), actor).await?;
                if let Err(denied) = policy::can_close_proposal(actor, standing, &row.author) {
                    return Err(FoldError::from(denied)
                        .with_details([
                            ("proposal", json!(proposal.to_string())),
                            ("author", json!(row.author.as_str())),
                        ])
                        .into());
                }
                let closer = actor.event_actor();
                let task = retire_proposal(
                    tx,
                    &mut buffer,
                    &closer,
                    proposal,
                    Retirement::Closed { reason },
                    now,
                )
                .await?;
                let event = NewEvent::new(types::PROPOSAL_CLOSED, closer)?
                    .task(task)
                    .proposal(proposal)
                    .data(json!({"reason": reason}))?;
                buffer.append(tx, &event, now).await?;
                buffer.extend(recompute(tx, &[task], now).await?.events);
                Ok::<_, ReviewError>(task)
            })
            .await?;
        self.bus.publish(buffer);
        if let Some(live) = workspaces::live_task_workspace(self.store.read(), task).await? {
            self.workspaces.proposal_changed(live.id, None).await;
        }
        self.get_proposal(proposal, None).await
    }

    /// Proposals in number order, filtered by state and by whether a reviewer holds them. A review claim whose lease has run out counts as unclaimed, since `claim_review` takes it over.
    pub async fn list_proposals(
        &self,
        filter: ProposalFilter,
    ) -> Result<Vec<ProposalSummary>, ReviewError> {
        let limit = page_limit(filter.limit, MAX_PROPOSAL_LIST)?;
        let unclaimed_at = if filter.unclaimed {
            Some(stale_cutoff(self.clock.now(), self.lease_minutes().await?))
        } else {
            None
        };
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        let mut rows = reviews::list_proposals(&mut *tx, filter.state, unclaimed_at).await?;
        if let Some(limit) = limit {
            let keep = usize::try_from(limit).unwrap_or(usize::MAX);
            rows.drain(..rows.len().saturating_sub(keep));
        }
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let facts = match row.human_review {
                Some(_) => reviews::review_facts(&mut *tx, row.id, row.current_version).await?,
                None => Vec::new(),
            };
            out.push(ProposalSummary::new(row, &facts));
        }
        tx.commit().await.map_err(StoreError::from)?;
        Ok(out)
    }

    /// A proposal with every version and the conflict report of the selected one. `version` selects one of them, else the current one is selected.
    pub async fn get_proposal(
        &self,
        proposal: ProposalId,
        version: Option<u32>,
    ) -> Result<ProposalDetail, ReviewError> {
        let mut read = self.store.read().begin().await.map_err(StoreError::from)?;
        let detail = proposal_detail(&mut read, proposal, version).await?;
        read.commit().await.map_err(StoreError::from)?;
        Ok(detail)
    }
}

/// The writes of a new proposal in the caller's transaction: the refusals of `create_proposal` again, the automatic checkpoint and changeset, the proposal with version 1, `proposal.opened`, the mark of a changed checks file and the recalculated status.
pub(super) async fn open_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    task: TaskId,
    title: &str,
    input: &VersionInput<'_>,
) -> Result<ProposalId, ReviewError> {
    let (actor, recorded, now) = (input.actor, input.recorded, input.now);
    let facts = OpenFacts {
        task: tasks::fetch(tx.conn(), task).await?,
        holder: claims::unreleased_for_task(tx.conn(), task).await?,
        open: reviews::open_for_task(tx.conn(), task).await?,
        live: workspaces::live_task_workspace(tx.conn(), task).await?,
    };
    let (author, live) = open_checks(actor, task, facts)?;
    same_workspace(input.snapped, &live)?;
    let worker = worker_row(tx, author).await?;
    let head = record_in_tx(
        tx,
        buffer,
        actor,
        &live,
        recorded,
        AutoReason::BeforeProposal,
        title,
        now,
    )
    .await?;
    let number = counters::allocate(tx, IdKind::Proposal).await?;
    let id = ProposalId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
    let new = NewProposal {
        id,
        task,
        author_worker_id: worker.id,
        title,
    };
    let first = NewVersion {
        base: live.base,
        tree: recorded.tree,
        head_changeset: head,
        evidence: input.evidence,
        created_by: worker.name.as_str(),
        fixed_in_checkout: None,
    };
    match reviews::open_proposal(tx, &new, &first, now).await {
        Ok(()) => {}
        Err(error) if error.unique_violation() => {
            return match reviews::open_for_task(tx.conn(), task).await? {
                Some(open) => Err(proposal_exists(task, open)),
                None => Err(error.into()),
            };
        }
        Err(error) => return Err(error.into()),
    }
    let event = NewEvent::new(types::PROPOSAL_OPENED, EventActor::worker(&worker.name))?
        .task(task)
        .proposal(id)
        .workspace(live.id)
        .revision(live.base)
        .data(json!({
            "version": 1,
            "changeset": head.to_string(),
            "author": worker.name.as_str(),
        }))?;
    buffer.append(tx, &event, now).await?;
    mark_checks_change(tx, buffer, task, input.checks_changed, now).await?;
    buffer.extend(recompute(tx, &[task], now).await?.events);
    Ok(id)
}

/// The writes of a new version in the caller's transaction: the refusals of `update_proposal` again, `version_unchanged` against the current version, the automatic checkpoint and changeset, the version, `proposal.version_added`, the mark of a changed checks file and the recalculated status. Returns the new version's number.
pub(super) async fn add_version_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    proposal: ProposalId,
    input: &VersionInput<'_>,
) -> Result<u32, ReviewError> {
    let (actor, recorded, now) = (input.actor, input.recorded, input.now);
    let found = reviews::get_proposal(tx.conn(), proposal).await?;
    let row = update_checks(actor, proposal, found)?;
    let live = workspaces::live_task_workspace(tx.conn(), row.task).await?;
    let live = usable_workspace(live, row.task, &row.author)?;
    let current = reviews::list_versions(tx.conn(), proposal)
        .await?
        .pop()
        .ok_or(StoreError::Corrupt("proposal_versions.version"))?;
    same_workspace(input.snapped, &live)?;
    let evidence = input
        .evidence
        .map(str::to_owned)
        .or(current.evidence.clone());
    unchanged_refusal(
        proposal,
        &current,
        live.base,
        recorded.tree,
        evidence.as_deref(),
    )?;
    let author = row.author.clone();
    let head = record_in_tx(
        tx,
        buffer,
        actor,
        &live,
        recorded,
        AutoReason::BeforeProposalUpdate,
        &row.title,
        now,
    )
    .await?;
    let version = current.version.saturating_add(1);
    let new = NewVersion {
        base: live.base,
        tree: recorded.tree,
        head_changeset: head,
        evidence: evidence.as_deref(),
        created_by: author.as_str(),
        fixed_in_checkout: None,
    };
    reviews::add_version(tx, proposal, version, &new, now).await?;
    let event = NewEvent::new(types::PROPOSAL_VERSION_ADDED, EventActor::worker(&author))?
        .task(row.task)
        .proposal(proposal)
        .workspace(live.id)
        .revision(live.base)
        .data(json!({"version": version, "changeset": head.to_string()}))?;
    buffer.append(tx, &event, now).await?;
    mark_checks_change(tx, buffer, row.task, input.checks_changed, now).await?;
    buffer.extend(recompute(tx, &[row.task], now).await?.events);
    Ok(version)
}

/// `version_unchanged` when a new version would hold the current one's base, tree and evidence.
fn unchanged_refusal(
    proposal: ProposalId,
    current: &VersionRow,
    base: RevisionId,
    tree: ObjectHash,
    evidence: Option<&str>,
) -> Result<(), ReviewError> {
    if current.base != base || current.tree != tree || current.evidence.as_deref() != evidence {
        return Ok(());
    }
    Err(FoldError::new(
        codes::VERSION_UNCHANGED,
        format!("v{} of {proposal} already holds this work", current.version),
    )
    .with_details([
        ("proposal", json!(proposal.to_string())),
        ("version", json!(current.version)),
    ])
    .with_hint("change the workspace or the evidence, then call update_proposal")
    .into())
}

/// Gives the task the human review mark fold sets for a change of the checks file, with `task.human_review_set`, when the version changes the file and the task has no mark.
pub(super) async fn mark_checks_change(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    task: TaskId,
    checks_changed: bool,
    now: Timestamp,
) -> Result<(), ReviewError> {
    let current = tasks::fetch(tx.conn(), task)
        .await?
        .ok_or_else(|| task_not_found(task))?
        .human_review;
    let Some(mark) = policy::checks_file_mark(current.as_ref(), checks_changed) else {
        return Ok(());
    };
    tasks::set_human_review(tx, task, &mark, now).await?;
    let event = NewEvent::new(types::TASK_HUMAN_REVIEW_SET, mark.set_by.clone())?
        .task(task)
        .data(json!({
            "reviewer": null,
            "reason": mark.reason,
            "set_by": mark.set_by.as_str(),
        }))?;
    buffer.append(tx, &event, now).await?;
    Ok(())
}

/// A version a human wrote is a human edit: only a human edit names a human as its author.
pub(super) fn is_human_edit(created_by: &str) -> bool {
    EventActor::parse(created_by).is_ok_and(|author| author.is_human())
}

/// [`ReviewService::get_proposal`] on `conn`, so a caller reads it in the same snapshot as the rest of its view.
pub(super) async fn proposal_detail(
    conn: &mut SqliteConnection,
    proposal: ProposalId,
    version: Option<u32>,
) -> Result<ProposalDetail, ReviewError> {
    let row = reviews::get_proposal(&mut *conn, proposal)
        .await?
        .ok_or_else(|| proposal_not_found(proposal))?;
    let versions: Vec<VersionView> = reviews::list_versions(&mut *conn, proposal)
        .await?
        .into_iter()
        .map(VersionView::from)
        .collect();
    let wanted = version.unwrap_or(row.current_version);
    let selected = versions
        .iter()
        .find(|view| view.version == wanted)
        .cloned()
        .ok_or_else(|| version_not_found(proposal, wanted, row.current_version))?;
    let conflict = reviews::conflict_report(&mut *conn, proposal, selected.version).await?;
    let task_status = tasks::fetch(&mut *conn, row.task)
        .await?
        .ok_or(StoreError::Corrupt("proposals.task_id"))?
        .status;
    let facts = match row.human_review {
        Some(_) => reviews::review_facts(&mut *conn, row.id, row.current_version).await?,
        None => Vec::new(),
    };
    let summary = ProposalSummary::new(row, &facts);
    Ok(ProposalDetail {
        summary,
        versions,
        selected,
        conflict,
        task_status,
    })
}

/// Ends an open proposal inside the caller's transaction: sets its state, close reason and `closed_at`, releases its review claim with the outcome as `release_reason` (`claim.released`), and archives every live review checkout of the proposal (`workspace.archived`), whose directories stay until garbage collection. Close, reject and integration share it; the caller appends its own `proposal.*` event and runs `recompute` for the returned task.
pub async fn retire_proposal(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &EventActor,
    proposal: ProposalId,
    outcome: Retirement<'_>,
    now: Timestamp,
) -> Result<TaskId, ReviewError> {
    let row = require_open(reviews::get_proposal(tx.conn(), proposal).await?, proposal)?;
    if !reviews::retire(tx, proposal, outcome.state(), outcome.close_reason(), now).await? {
        return Err(not_open(&row));
    }
    if let Some(holder) = reviews::release_review_claim(tx, proposal, outcome.as_str(), now).await?
    {
        let event = NewEvent::new(types::CLAIM_RELEASED, actor.clone())?
            .task(row.task)
            .proposal(proposal)
            .data(json!({
                "kind": "review",
                "proposal": proposal.to_string(),
                "worker": holder.as_str(),
                "reason": outcome.as_str(),
            }))?;
        buffer.append(tx, &event, now).await?;
    }
    for workspace in reviews::live_review_workspaces(tx.conn(), proposal).await? {
        archive_in_tx::<ReviewError>(tx, buffer, actor, workspace, outcome.archive_reason(), now)
            .await?;
    }
    close_batch_of(tx, buffer, actor, proposal, now).await?;
    Ok(row.task)
}

/// What decides whether a worker may open a proposal for a task, read on the read pool before the snapshot and again in the transaction.
struct OpenFacts {
    task: Option<TaskRow>,
    holder: Option<claims::ClaimRow>,
    open: Option<ProposalId>,
    live: Option<WorkspaceRow>,
}

/// The refusals of `create_proposal` that need no files: the task is open, the actor's worker holds the implementation claim, the task has no open proposal, and the worker owns the task's active workspace. The conflict and unfinished-write refusals come with the snapshot.
fn open_checks(
    actor: &Actor,
    task: TaskId,
    facts: OpenFacts,
) -> Result<(&AgentActor, WorkspaceRow), ReviewError> {
    let row = facts.task.ok_or_else(|| task_not_found(task))?;
    if let Some(refusal) = closed_refusal(&row).or_else(|| proposal_kind_refusal(&row)) {
        return Err(refusal.into());
    }
    let holder = facts.holder.map(|claim| claim.worker);
    let author = policy::can_open_proposal(actor, holder.as_ref()).map_err(|denied| {
        FoldError::from(denied).with_details([
            ("task", json!(task.to_string())),
            ("holder", json!(holder.as_ref().map(WorkerName::as_str))),
        ])
    })?;
    if let Some(open) = facts.open {
        return Err(proposal_exists(task, open));
    }
    let workspace = usable_workspace(facts.live, task, &author.worker)?;
    Ok((author, workspace))
}

/// The refusals of `update_proposal` about the proposal itself: it exists, it is open, and the actor is its author's worker.
fn update_checks(
    actor: &Actor,
    proposal: ProposalId,
    found: Option<ProposalRow>,
) -> Result<ProposalRow, ReviewError> {
    let row = require_open(found, proposal)?;
    if let Err(denied) = policy::can_update_proposal(actor, &row.author) {
        return Err(FoldError::from(denied)
            .with_details([
                ("proposal", json!(proposal.to_string())),
                ("author", json!(row.author.as_str())),
            ])
            .into());
    }
    Ok(row)
}

pub(super) fn usable_workspace(
    live: Option<WorkspaceRow>,
    task: TaskId,
    worker: &WorkerName,
) -> Result<WorkspaceRow, ReviewError> {
    let Some(live) = live else {
        return Err(no_task_workspace(task));
    };
    if live.state != WorkspaceState::Active {
        return Err(FoldError::new(
            codes::WORKSPACE_NOT_ACTIVE,
            format!("{} is {}", live.id, live.state.as_str()),
        )
        .with_details([
            ("workspace", json!(live.id.to_string())),
            ("state", json!(live.state.as_str())),
        ])
        .with_hint("the workspace directory is missing; call get_workspace to see its state")
        .into());
    }
    if live.worker.as_ref() != Some(worker) {
        return Err(FoldError::new(
            codes::NOT_WORKSPACE_OWNER,
            format!("{} belongs to another worker", live.id),
        )
        .with_details([
            ("workspace", json!(live.id.to_string())),
            ("owner", json!(live.worker.as_ref().map(WorkerName::as_str))),
        ])
        .with_hint("call get_workspace to see the owner")
        .into());
    }
    Ok(live)
}

/// The refusal of a task without an active task workspace.
pub(super) fn no_task_workspace(task: TaskId) -> ReviewError {
    FoldError::new(
        codes::WORKSPACE_NOT_ACTIVE,
        format!("{task} has no active task workspace"),
    )
    .with_details([("task", json!(task.to_string()))])
    .with_hint("call create_workspace, do the work there, then try again")
    .into()
}

/// The snapshot was taken of `snapped`; the transaction refuses to record it when the task's workspace was replaced or synced to another base meanwhile.
fn same_workspace(snapped: &WorkspaceRow, live: &WorkspaceRow) -> Result<(), ReviewError> {
    if snapped.id == live.id && snapped.base == live.base {
        return Ok(());
    }
    Err(FoldError::new(
        codes::INVALID_REQUEST,
        format!("{} changed while it was recorded", snapped.id),
    )
    .with_details([
        ("reason", json!("workspace_changed")),
        ("workspace", json!(live.id.to_string())),
    ])
    .with_hint("call the command again")
    .into())
}

/// Takes the automatic checkpoint of the snapshot with `reason`, then records the snapshot as the workspace's newest changeset when it differs from the latest one, and returns the head changeset the version names.
#[expect(
    clippy::too_many_arguments,
    reason = "the step of a command, with the inputs its earlier steps produced"
)]
async fn record_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    workspace: &WorkspaceRow,
    recorded: &ProposalSnapshot,
    reason: AutoReason,
    message: &str,
    now: Timestamp,
) -> Result<ChangesetId, ReviewError> {
    auto_checkpoint_in_tx(
        tx,
        buffer,
        actor,
        workspace,
        recorded.tree,
        reason,
        recorded.auto_kept,
        now,
    )
    .await?;
    workspaces::set_conflicts(tx, workspace.id, &[]).await?;
    let latest = workspaces::latest_changeset(tx.conn(), workspace.id).await?;
    if let Some(latest) = &latest
        && !differs_from_latest(Some(&latest.tree), &recorded.tree)
    {
        return Ok(latest.id);
    }
    let created_by = actor.event_actor();
    let changeset = workspaces::insert_changeset(
        tx,
        workspace.id,
        workspace.task,
        workspace.base,
        recorded.tree,
        message,
        recorded.stats,
        created_by.as_str(),
        now,
    )
    .await?;
    let stats = recorded.stats;
    let files = stats
        .files_added
        .saturating_add(stats.files_modified)
        .saturating_add(stats.files_deleted);
    let event = NewEvent::new(types::CHANGESET_RECORDED, created_by)?
        .task(workspace.task)
        .workspace(workspace.id)
        .revision(workspace.base)
        .data(json!({"changeset": changeset.id.to_string(), "files": files}))?;
    buffer.append(tx, &event, now).await?;
    Ok(changeset.id)
}

fn check_title(title: &str) -> Result<&str, ReviewError> {
    fold_core::text::one_line(title, MAX_TITLE_CHARS).map_err(|_| {
        FoldError::new(
            codes::INVALID_REQUEST,
            format!("the title must be one line of 1 to {MAX_TITLE_CHARS} characters"),
        )
        .with_details([("field", json!("title"))])
        .with_hint("call create_proposal again with a short one-line title")
        .into()
    })
}

/// Evidence with no visible text is no evidence.
fn check_evidence(evidence: Option<&str>) -> Option<String> {
    evidence
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

pub(super) fn require_open(
    row: Option<ProposalRow>,
    proposal: ProposalId,
) -> Result<ProposalRow, ReviewError> {
    let row = row.ok_or_else(|| proposal_not_found(proposal))?;
    if row.state == ProposalState::Open {
        Ok(row)
    } else {
        Err(not_open(&row))
    }
}

pub(super) fn version_not_found(proposal: ProposalId, wanted: u32, current: u32) -> ReviewError {
    FoldError::new(
        codes::PROPOSAL_NOT_FOUND,
        format!("{proposal} has no v{wanted}"),
    )
    .with_details([
        ("proposal", json!(proposal.to_string())),
        ("version", json!(wanted)),
        ("current_version", json!(current)),
    ])
    .with_hint("call get_proposal without a version to see every version")
    .into()
}

pub(super) fn proposal_not_found(proposal: ProposalId) -> ReviewError {
    proposal_missing(proposal).into()
}

/// The refusal of an unknown proposal, the same in every domain.
pub(crate) fn proposal_missing(proposal: ProposalId) -> FoldError {
    FoldError::new(
        codes::PROPOSAL_NOT_FOUND,
        format!("{proposal} does not exist"),
    )
    .with_details([("proposal", json!(proposal.to_string()))])
    .with_hint("call list_proposals to see the proposals")
}

fn not_open(row: &ProposalRow) -> ReviewError {
    FoldError::new(
        codes::PROPOSAL_NOT_OPEN,
        format!("{} is {}", row.id, row.state.as_str()),
    )
    .with_details([
        ("proposal", json!(row.id.to_string())),
        ("state", json!(row.state.as_str())),
    ])
    .with_hint("call create_proposal to open a new proposal for the task")
    .into()
}

fn proposal_exists(task: TaskId, open: ProposalId) -> ReviewError {
    FoldError::new(
        codes::PROPOSAL_EXISTS,
        format!("{task} already has the open proposal {open}"),
    )
    .with_details([
        ("task", json!(task.to_string())),
        ("proposal", json!(open.to_string())),
    ])
    .with_hint("call update_proposal to add a version to the open proposal")
    .into()
}
