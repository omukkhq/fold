use fold_core::events::types;
use fold_core::reviews::{missing_intent, proposal_intent};
use fold_core::{
    Actor, CheckpointId, FoldError, HumanName, NewEvent, ProposalId, WorkerState, codes, policy,
};
use fold_store::reviews::{self, NewVersion, ProposalRow, VersionRow};
use fold_store::workspaces::{self, CheckpointReason, WorkspaceRow};
use fold_store::{SqliteConnection, StoreError, agents};
use fold_vcs::browse::FileContent;
use serde_json::json;

use super::proposals::{ProposalDetail, is_human_edit, require_open, usable_workspace};
use super::verdicts::require_version;
use super::{ReviewError, ReviewService};
use crate::services::events::EventBuffer;
use crate::services::tasks::recompute;
use crate::services::workspaces::{checkpoint_in_tx, file_path};

/// What a human edit wrote: the new version, how many approvals it kept from the version it was made from, and the checkpoint the task worker's next sync writes into the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanEdit {
    pub version: u32,
    pub from_version: u32,
    pub kept_approvals: u32,
    pub checkpoint: CheckpointId,
    pub detail: ProposalDetail,
}

impl ReviewService {
    /// A human's change of one text file of the current version of an open proposal, in a human review only (`policy::can_edit_as_human`). `version` is the version the human read; another current version is `stale_version`. `content` replaces the whole file. A human made the change, so no proposal check runs on the new version; its integration checks run when it integrates.
    ///
    /// The edit is a new version authored by `human:<name>` on the same base, with the version's evidence and `intent`, checked as for `create_proposal`; `None` keeps the version's intent, and a version made before intents existed has none to keep, so its edit is `invalid_intent` with the reason `missing`. Versions never change. It keeps the agent approvals that stood on the version it was made from, the one exception to a new version starting without approvals, until the human sends it for review again. No human approval is kept, so a human approves the edit again and it never integrates on a human's earlier approval. It is recorded in the task workspace as a changeset and as a `human_edit` checkpoint marked as the workspace's pending write, without touching the files: fold writes into a workspace only on create, sync and restore. The worker's next sync writes the edit into the files first, and until then `record_changes` and `update_proposal` refuse with `workspace_write_unfinished`; the worker gets a push notice to sync.
    ///
    /// The edit is refused with `worker_busy` while the task worker may be changing the workspace: its worker process is starting, working or stopping, the workspace waits for an unfinished sync or restore other than the human edit the version came from, or it holds work the version does not (another base or a later changeset). Changes the worker made in the files without recording them stay in the safety checkpoint its sync takes.
    pub async fn edit_as_human(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        version: u32,
        path: &str,
        content: &str,
        intent: Option<&str>,
    ) -> Result<HumanEdit, ReviewError> {
        let path = file_path(path)?;
        let intent = intent
            .map(|text| proposal_intent(text, "edit_proposal_file"))
            .transpose()?;
        let mut conn = self
            .store
            .read()
            .acquire()
            .await
            .map_err(StoreError::from)?;
        let row = require_open(reviews::get_proposal(&mut *conn, proposal).await?, proposal)?;
        edit_gate(actor, &row)?;
        let current = current_version(&mut conn, proposal).await?;
        require_version(proposal, version, current.version)?;
        if intent.is_none() && current.intent.is_none() {
            return Err(missing_intent("edit_proposal_file").into());
        }
        let live = workspaces::live_task_workspace(&mut *conn, row.task).await?;
        let workspace = usable_workspace(live, row.task, &row.author)?;
        idle_checks(&mut conn, &row, &workspace, &current).await?;
        drop(conn);
        let edited = self
            .workspaces
            .build_human_edit(
                workspace.id,
                current.tree,
                current.base,
                path,
                content.as_bytes().to_vec(),
            )
            .await?;
        let now = edited.taken_at;
        let mut buffer = EventBuffer::new();
        let (made, kept, checkpoint) = self
            .store
            .write(async |tx| {
                let row =
                    require_open(reviews::get_proposal(tx.conn(), proposal).await?, proposal)?;
                edit_gate(actor, &row)?;
                let current = current_version(tx.conn(), proposal).await?;
                require_version(proposal, version, current.version)?;
                let live = workspaces::live_task_workspace(tx.conn(), row.task).await?;
                let live = usable_workspace(live, row.task, &row.author)?;
                if live.id != workspace.id {
                    return Err(busy(&live, "newer_work"));
                }
                idle_checks(tx.conn(), &row, &live, &current).await?;
                let by = actor.event_actor();
                let message = format!("Human edit of v{} by {by}", current.version);
                let changeset = workspaces::insert_changeset(
                    tx,
                    live.id,
                    live.task,
                    live.base,
                    edited.tree,
                    &message,
                    edited.stats,
                    by.as_str(),
                    now,
                )
                .await?;
                let stats = edited.stats;
                let files = stats
                    .files_added
                    .saturating_add(stats.files_modified)
                    .saturating_add(stats.files_deleted);
                let recorded = NewEvent::new(types::CHANGESET_RECORDED, by.clone())?
                    .task(live.task)
                    .workspace(live.id)
                    .revision(live.base)
                    .data(json!({"changeset": changeset.id.to_string(), "files": files}))?;
                buffer.append(tx, &recorded, now).await?;
                let next = current.version.saturating_add(1);
                let label = format!("v{next} of {proposal} by {by}");
                let checkpoint = checkpoint_in_tx(
                    tx,
                    &mut buffer,
                    actor,
                    &live,
                    edited.tree,
                    CheckpointReason::HumanEdit,
                    Some(&label),
                    now,
                )
                .await?;
                workspaces::set_sync_checkpoint(tx, live.id, Some(checkpoint.id)).await?;
                let new = NewVersion {
                    base: current.base,
                    tree: edited.tree,
                    head_changeset: changeset.id,
                    evidence: current.evidence.as_deref(),
                    intent: intent.as_deref().or(current.intent.as_deref()),
                    created_by: by.as_str(),
                    fixed_in_checkout: None,
                };
                reviews::add_version(tx, proposal, next, &new, now).await?;
                let kept =
                    reviews::keep_approvals(tx, proposal, current.version, next, now).await?;
                let event = NewEvent::new(types::PROPOSAL_HUMAN_EDITED, by)?
                    .task(row.task)
                    .proposal(proposal)
                    .workspace(live.id)
                    .revision(current.base)
                    .data(json!({
                        "version": next,
                        "from_version": current.version,
                        "changeset": changeset.id.to_string(),
                        "checkpoint": checkpoint.id.to_string(),
                        "kept_approvals": kept,
                    }))?;
                buffer.append(tx, &event, now).await?;
                buffer.extend(recompute(tx, &[row.task], now).await?.events);
                Ok::<_, ReviewError>(((current.version, next), kept, checkpoint.id))
            })
            .await?;
        drop(edited);
        self.bus.publish(buffer);
        self.workspaces
            .proposal_changed(workspace.id, Some(proposal))
            .await;
        let detail = self.get_proposal(proposal, None).await?;
        Ok(HumanEdit {
            version: made.1,
            from_version: made.0,
            kept_approvals: kept,
            checkpoint,
            detail,
        })
    }

    /// Sends the current version of an open proposal, a human edit, for review again, in a human review only (`policy::can_edit_as_human`). The approvals the edit kept stop counting, so the proposal waits for a reviewer's new verdict, and the review leads and the review claim's holder get a push notice. `version` is the version the human read; another current version is `stale_version`. A version that no human edited, or one already sent, is `invalid_request`.
    pub async fn send_for_review_again(
        &self,
        actor: &Actor,
        proposal: ProposalId,
        version: u32,
    ) -> Result<ProposalDetail, ReviewError> {
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        self.store
            .write(async |tx| {
                let row =
                    require_open(reviews::get_proposal(tx.conn(), proposal).await?, proposal)?;
                let human = edit_gate(actor, &row)?;
                let current = current_version(tx.conn(), proposal).await?;
                require_version(proposal, version, current.version)?;
                if !is_human_edit(&current.created_by) {
                    return Err(again_refused(proposal, version, "not_human_edit"));
                }
                if !reviews::request_review_again(tx, proposal, version, human, now).await? {
                    return Err(again_refused(proposal, version, "already_sent"));
                }
                let event =
                    NewEvent::new(types::PROPOSAL_REVIEW_AGAIN_REQUESTED, actor.event_actor())?
                        .task(row.task)
                        .proposal(proposal)
                        .data(json!({"version": version}))?;
                buffer.append(tx, &event, now).await?;
                buffer.extend(recompute(tx, &[row.task], now).await?.events);
                Ok::<_, ReviewError>(())
            })
            .await?;
        self.bus.publish(buffer);
        self.get_proposal(proposal, None).await
    }

    /// One file of a proposal version, the current one when `version` is `None`: what a human edit starts from. A symlink gives its target text and is never followed.
    pub async fn version_file(
        &self,
        proposal: ProposalId,
        version: Option<u32>,
        path: &str,
    ) -> Result<FileContent, ReviewError> {
        let path = file_path(path)?;
        let detail = self.get_proposal(proposal, version).await?;
        Ok(self
            .workspaces
            .read_tree_file(detail.selected.tree, path)
            .await?)
    }
}

fn edit_gate<'a>(actor: &'a Actor, row: &ProposalRow) -> Result<&'a HumanName, ReviewError> {
    policy::can_edit_as_human(actor, row.human_review.as_ref()).map_err(|denied| {
        FoldError::from(denied)
            .with_details([
                ("proposal", json!(row.id.to_string())),
                (
                    "reviewer",
                    json!(
                        row.human_review
                            .as_ref()
                            .and_then(|mark| mark.reviewer.as_ref())
                            .map(HumanName::as_str)
                    ),
                ),
            ])
            .into()
    })
}

pub(super) async fn current_version(
    conn: &mut SqliteConnection,
    proposal: ProposalId,
) -> Result<VersionRow, ReviewError> {
    Ok(reviews::list_versions(conn, proposal)
        .await?
        .pop()
        .ok_or(StoreError::Corrupt("proposal_versions.version"))?)
}

/// The refusals that keep a human edit or a reviewer's fix away from a task workspace its worker may be changing: the worker process of the author's agent is starting, working or stopping, the workspace waits for a sync or restore to finish, or it holds work that `current` does not. A pending write that is the human edit or the reviewer's fix which made `current` does not refuse: fold has not written it into the files yet, so a further version replaces it, and a task worker that fold stopped after its proposal does not hold back a second one.
pub(super) async fn idle_checks(
    conn: &mut SqliteConnection,
    row: &ProposalRow,
    workspace: &WorkspaceRow,
    current: &VersionRow,
) -> Result<(), ReviewError> {
    if let Some(process) = agents::find_worker_process(&mut *conn, &row.author.agent()).await?
        && matches!(
            process.state,
            WorkerState::Starting | WorkerState::Working | WorkerState::Stopping
        )
    {
        return Err(busy(workspace, "working"));
    }
    if let Some(mark) = workspaces::sync_checkpoint(&mut *conn, workspace.id).await? {
        let pending = workspaces::get_checkpoint(&mut *conn, mark).await?;
        let earlier_edit = pending.is_some_and(|checkpoint| {
            checkpoint.reason.is_version_write() && checkpoint.tree == current.tree
        });
        if !earlier_edit {
            return Err(busy(workspace, "unfinished_write"));
        }
    }
    let latest = workspaces::latest_changeset(&mut *conn, workspace.id).await?;
    if workspace.base != current.base
        || latest.map(|changeset| changeset.id) != Some(current.head_changeset)
    {
        return Err(busy(workspace, "newer_work"));
    }
    Ok(())
}

pub(super) fn busy(workspace: &WorkspaceRow, reason: &str) -> ReviewError {
    let message = match reason {
        "working" => "the task worker is working, so a change of its proposal waits",
        "unfinished_write" => "the task workspace waits for a sync or restore to finish",
        _ => "the task workspace holds work that the current version does not",
    };
    FoldError::new(codes::WORKER_BUSY, message)
        .with_details([
            ("reason", json!(reason)),
            ("workspace", json!(workspace.id.to_string())),
            (
                "worker",
                json!(workspace.worker.as_ref().map(|worker| worker.as_str())),
            ),
        ])
        .with_hint("try again when the task worker is idle, or comment and request changes")
        .into()
}

fn again_refused(proposal: ProposalId, version: u32, reason: &str) -> ReviewError {
    let message = match reason {
        "not_human_edit" => format!("v{version} of {proposal} is not a human edit"),
        _ => format!("v{version} of {proposal} was already sent for review again"),
    };
    FoldError::new(codes::INVALID_REQUEST, message)
        .with_details([
            ("reason", json!(reason)),
            ("proposal", json!(proposal.to_string())),
            ("version", json!(version)),
        ])
        .with_hint("call get_proposal to see the versions and their reviews")
        .into()
}
