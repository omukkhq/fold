use std::path::PathBuf;

use fold_core::events::types;
use fold_core::policy::AgentStanding;
use fold_core::{
    Actor, AgentActor, AgentName, EventActor, FoldError, IdKind, NewEvent, ProposalId, WorkspaceId,
    codes, policy,
};
use fold_store::agents::{self, WorkerRow};
use fold_store::reviews::{self, ProposalRow, ReviewClaimRow};
use fold_store::workspaces::{self, NewReviewCheckout};
use fold_store::{Store, StoreError, counters};
use serde_json::json;

pub use fold_store::workspaces::CheckoutTarget;

use super::proposals::require_open;
use super::verdicts::require_version;
use super::{ReviewError, ReviewService};
use crate::services::agents::{actor_worker, worker_not_found};
use crate::services::events::EventBuffer;
use crate::services::tasks::worker_row;
use crate::services::workspaces::{WorkspaceError, WorkspaceRef, WorkspaceView};

/// A review checkout with the proposal version its files hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewCheckout {
    pub workspace: WorkspaceView,
    pub proposal: ProposalId,
    pub version: u32,
}

/// An active review checkout as the project's list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedCheckout {
    pub workspace: WorkspaceView,
    pub target: CheckoutTarget,
}

/// What the transaction of a create found: its own new row, or the active checkout another create of the same version and worker inserted first.
enum Inserted {
    Created(WorkspaceId),
    Existing(WorkspaceId),
}

impl ReviewService {
    /// A copy of the current version of an open proposal for the actor's worker, under `.fold/ws/review-p-<n>`, so a review worker can read, build and test it without a task workspace, and fix what it finds there with `submit_review_fix`. `policy::can_create_review_checkout` admits the worker of the agent that holds the review claim, and the checkout belongs to that worker. A repeat call for the same version and worker returns the active checkout; any other checkout of the proposal takes the next directory name. A checkout is never synced or restored and never moves the task; fold only reads its files, when a fix in it becomes a version.
    ///
    /// The files are written and renamed into place before the transaction that inserts the row, so no row ever names a directory that is not complete; a failure to write them is `checkout_failed`. The transaction reads the current version again and refuses with `stale_version` when a version was added after the files were written, so a checkout never names a version that is not current when it is made. It looks for the active checkout again before it takes a workspace number, so a repeat that finished second takes none; the partial unique index on active checkouts backs that rule. When the transaction fails, or finds the checkout another call inserted first, the new directory is removed again: no row names it.
    pub async fn create_review_checkout(
        &self,
        actor: &Actor,
        proposal: ProposalId,
    ) -> Result<ReviewCheckout, ReviewError> {
        let read = self.store.read();
        let row = require_open(reviews::get_proposal(read, proposal).await?, proposal)?;
        let holder = reviews::unreleased_review_claim(read, proposal).await?;
        let standing = agents::standing_of(read, actor).await?;
        let reviewer = checkout_checks(actor, standing, &row, holder.as_ref())?;
        let version = row.current_version;
        let worker = find_worker(&self.store, reviewer).await?;
        if let Some(existing) =
            workspaces::active_review_checkout(read, proposal, version, worker.id).await?
        {
            return self.checkout(existing, proposal, version).await;
        }
        let tree = reviews::list_versions(read, proposal)
            .await?
            .into_iter()
            .find(|row| row.version == version)
            .ok_or(StoreError::Corrupt("proposal_versions.version"))?
            .tree;
        let taken = workspaces::review_checkout_paths(read, proposal).await?;
        let (path, dir) = self
            .workspaces
            .place_review_checkout(proposal, taken, tree)
            .await
            .map_err(|error| checkout_failed(proposal, error))?;
        #[cfg(feature = "failpoints")]
        fail::fail_point!("review_checkout_after_files");
        let now = self.clock.now();
        let mut buffer = EventBuffer::new();
        let written = self
            .store
            .write(async |tx| {
                let found = reviews::get_proposal(tx.conn(), proposal).await?;
                let row = require_open(found, proposal)?;
                require_version(proposal, version, row.current_version)?;
                let holder = reviews::unreleased_review_claim(tx.conn(), proposal).await?;
                let standing = agents::standing_of(tx.conn(), actor).await?;
                let reviewer = checkout_checks(actor, standing, &row, holder.as_ref())?;
                let worker = worker_row(tx, reviewer).await?;
                if let Some(existing) =
                    workspaces::active_review_checkout(tx.conn(), proposal, version, worker.id)
                        .await?
                {
                    return Ok(Inserted::Existing(existing));
                }
                let number = counters::allocate(tx, IdKind::Workspace).await?;
                let id =
                    WorkspaceId::from_number(number).ok_or(StoreError::Corrupt("counters.next"))?;
                let new = NewReviewCheckout {
                    id,
                    proposal,
                    version,
                    worker_id: worker.id,
                    path: &path,
                    now,
                };
                workspaces::insert_review_checkout(tx, &new).await?;
                let base = workspaces::get_workspace(tx.conn(), id)
                    .await?
                    .ok_or(StoreError::Corrupt("workspaces.number"))?
                    .base;
                let event =
                    NewEvent::new(types::WORKSPACE_CREATED, EventActor::worker(&worker.name))?
                        .task(row.task)
                        .proposal(proposal)
                        .workspace(id)
                        .revision(base)
                        .data(json!({
                            "kind": "review",
                            "path": path,
                            "worker": worker.name.as_str(),
                            "version": version,
                        }))?;
                buffer.append(tx, &event, now).await?;
                Ok::<_, ReviewError>(Inserted::Created(id))
            })
            .await;
        let id = match written {
            Ok(Inserted::Created(id)) => id,
            Ok(Inserted::Existing(existing)) => {
                remove_unnamed_dir(&path, dir).await;
                return self.checkout(existing, proposal, version).await;
            }
            Err(error) => {
                remove_unnamed_dir(&path, dir).await;
                return Err(error);
            }
        };
        self.bus.publish(buffer);
        self.checkout(id, proposal, version).await
    }

    /// Every active review checkout of the project with the proposal versions it holds, oldest first: the checkouts of one proposal and those of a review batch. It reads only.
    pub async fn list_review_checkouts(&self) -> Result<Vec<ListedCheckout>, ReviewError> {
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        let rows = workspaces::list_active_review_checkouts(&mut tx).await?;
        tx.commit().await.map_err(StoreError::from)?;
        Ok(rows
            .into_iter()
            .map(|row| ListedCheckout {
                workspace: WorkspaceView::from(row.workspace),
                target: row.target,
            })
            .collect())
    }

    /// Why a Claude Code session of `agent` may not edit files in the workspace directory `path`, relative to the project root, or `None` when it may: `path` is an active review checkout of one proposal that a worker of `agent` owns, and `agent` holds that proposal's review claim, so the edit is a fix `submit_review_fix` can make a version. A batch checkout mixes every member's work and is never edited. It reads only.
    pub async fn checkout_edit_refusal(
        &self,
        agent: &AgentName,
        path: &str,
    ) -> Result<Option<&'static str>, ReviewError> {
        let mut tx = self.store.read().begin().await.map_err(StoreError::from)?;
        let found = workspaces::list_active_review_checkouts(&mut tx)
            .await?
            .into_iter()
            .find(|row| row.workspace.path == path);
        let Some(row) = found else {
            return Ok(Some(
                "this is no active review checkout; edit only in the review checkout create_review_checkout gave you",
            ));
        };
        let proposal = match row.target {
            CheckoutTarget::Proposal { proposal, .. } => proposal,
            CheckoutTarget::Batch { .. } => {
                return Ok(Some(
                    "a batch checkout mixes every member and never becomes a version; call create_review_checkout for the member and fix it there",
                ));
            }
        };
        let owned = row
            .workspace
            .worker
            .as_ref()
            .is_some_and(|worker| worker.belongs_to(agent));
        if !owned {
            return Ok(Some(
                "this review checkout belongs to another reviewer; edit only in your own review checkout",
            ));
        }
        let holder = reviews::unreleased_review_claim(&mut *tx, proposal).await?;
        tx.commit().await.map_err(StoreError::from)?;
        if !holder.is_some_and(|claim| claim.worker.belongs_to(agent)) {
            return Ok(Some(
                "your agent does not hold the review claim of this proposal; call claim_review, then create_review_checkout",
            ));
        }
        Ok(None)
    }

    async fn checkout(
        &self,
        workspace: WorkspaceId,
        proposal: ProposalId,
        version: u32,
    ) -> Result<ReviewCheckout, ReviewError> {
        let workspace = self
            .workspaces
            .get(WorkspaceRef::Workspace(workspace))
            .await?;
        Ok(ReviewCheckout {
            workspace,
            proposal,
            version,
        })
    }
}

/// The refusals of a create that need no files, run on the read pool and again in the transaction.
fn checkout_checks<'a>(
    actor: &'a Actor,
    standing: AgentStanding,
    row: &ProposalRow,
    holder: Option<&ReviewClaimRow>,
) -> Result<&'a AgentActor, ReviewError> {
    let holder = holder.map(|claim| &claim.worker);
    policy::can_create_review_checkout(actor, standing, holder).map_err(|denied| {
        FoldError::from(denied)
            .with_details([
                ("proposal", json!(row.id.to_string())),
                ("holder", json!(holder.map(|name| name.as_str()))),
            ])
            .into()
    })
}

/// The actor's worker read on the read pool, for the lookup of an existing checkout before any file is written. The transaction reads it again.
async fn find_worker(store: &Store, actor: &AgentActor) -> Result<WorkerRow, ReviewError> {
    let mut conn = store.read().acquire().await.map_err(StoreError::from)?;
    Ok(actor_worker(&mut conn, actor)
        .await?
        .ok_or_else(worker_not_found)?)
}

/// `checkout_failed` with the code of what stopped the write. A refusal that is not about the files keeps its own code.
fn checkout_failed(proposal: ProposalId, error: WorkspaceError) -> ReviewError {
    match error {
        WorkspaceError::Fold(cause) => FoldError::new(
            codes::CHECKOUT_FAILED,
            format!("the checkout of {proposal} could not be written"),
        )
        .with_details([
            ("proposal", json!(proposal.to_string())),
            ("cause", json!(cause.code())),
        ])
        .with_hint("check the disk space and the permissions of .fold/ws/, then call create_review_checkout again")
        .into(),
        other => other.into(),
    }
}

/// Removes a checkout directory that no row names.
pub(super) async fn remove_unnamed_dir(path: &str, dir: PathBuf) {
    let removed = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(dir)).await;
    if !matches!(removed, Ok(Ok(()))) {
        tracing::warn!(path = %path, "a review checkout directory without a row was not removed");
    }
}
