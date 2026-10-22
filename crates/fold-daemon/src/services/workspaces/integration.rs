use std::sync::Arc;

use fold_core::object::ObjectHash;
use fold_core::revision::{Revision, StoredRevision};
use fold_core::{FoldError, WorkspaceId, codes};
use fold_vcs::batch::{BatchMember, BatchMerge, MergedBatch, merge_batch};
use fold_vcs::compare::{State, load_flat};
use fold_vcs::integrate::{MergeOutcome, merge_onto_head};
use fold_vcs::revision_store::write_revision;
use serde_json::json;
use tokio::sync::{OwnedMutexGuard, OwnedRwLockReadGuard};

use super::{
    BatchMergeKey, BatchMergeView, WorkspaceError, WorkspaceService, blocking, open_objects,
};

/// The locks one integration attempt holds from its first object write until its transaction commits: the operation lock of each task workspace it archives, so no sync, restore or record writes there meanwhile, and the project's object lock shared, so garbage collection keeps the objects the new revisions name.
pub(crate) struct IntegrationLocks {
    _held: (Vec<OwnedMutexGuard<()>>, OwnedRwLockReadGuard<()>),
}

impl WorkspaceService {
    /// Takes the operation lock of `workspace`, when there is one, and then the object lock shared: the order every workspace command takes them in.
    pub(crate) async fn lock_for_integration(
        &self,
        workspace: Option<WorkspaceId>,
    ) -> IntegrationLocks {
        self.lock_for_batch_integration(workspace.into_iter().collect())
            .await
    }

    /// [`WorkspaceService::lock_for_integration`] for the task workspaces of a batch's members, taken in workspace number order so two holders never wait for each other.
    pub(crate) async fn lock_for_batch_integration(
        &self,
        mut workspaces: Vec<WorkspaceId>,
    ) -> IntegrationLocks {
        workspaces.sort();
        workspaces.dedup();
        let mut ops = Vec::with_capacity(workspaces.len());
        for workspace in workspaces {
            ops.push(self.ops.lock(workspace).await);
        }
        let objects = self.objects.clone().read_owned().await;
        IntegrationLocks {
            _held: (ops, objects),
        }
    }

    /// Merges the tree `other` onto main's tree `head` against `base` in the object store. A failure to read or write an object is `integration_failed`; nothing names what was written.
    pub(crate) async fn merge_for_integration(
        &self,
        _locks: &IntegrationLocks,
        base: ObjectHash,
        head: ObjectHash,
        other: ObjectHash,
    ) -> Result<MergeOutcome, WorkspaceError> {
        let paths = self.paths.clone();
        blocking(move || merge_onto_head(&open_objects(&paths)?, &base, &head, &other))
            .await
            .map_err(integration_failed)
    }

    /// Merges the batch members onto main's tree `main` one after another in the object store, with their line origins. A clean merge also goes into the project's [`BatchMergeCache`](super::BatchMergeCache), so a view of the batch's merged change reads it without merging again. A failure to read or write an object is the store's own error; the caller reports it as its command's.
    pub(crate) async fn merge_review_batch(
        &self,
        _locks: &IntegrationLocks,
        main: ObjectHash,
        members: Vec<BatchMember>,
    ) -> Result<BatchMerge, WorkspaceError> {
        let paths = self.paths.clone();
        let merges = Arc::clone(&self.batch_merges);
        blocking(move || {
            let objects = open_objects(&paths)?;
            let merge = merge_batch(&objects, &main, &members)?;
            if let BatchMerge::Merged { tree_hash, .. } = &merge {
                let view = BatchMergeView {
                    from: State::recorded(load_flat(&objects, &main)?),
                    merged: MergedBatch {
                        state: State::recorded(load_flat(&objects, tree_hash)?),
                        merged: members.len(),
                        conflict: None,
                        tree: *tree_hash,
                    },
                };
                merges.insert(BatchMergeKey { main, members }, view);
            }
            Ok(merge)
        })
        .await
    }

    /// Writes and flushes the revision object before any row names it. A failure is `integration_failed`.
    pub(crate) async fn write_integration_revision(
        &self,
        _locks: &IntegrationLocks,
        revision: Revision,
    ) -> Result<StoredRevision, WorkspaceError> {
        let paths = self.paths.clone();
        blocking(move || Ok(write_revision(&open_objects(&paths)?, &revision)?))
            .await
            .map_err(integration_failed)
    }

    /// Stops the live state of a workspace an integration archived, after its commit.
    pub(crate) async fn detach_live(&self, workspace: WorkspaceId) {
        if let Some(live) = &self.live {
            live.detach(workspace).await;
        }
    }
}

fn integration_failed(error: WorkspaceError) -> WorkspaceError {
    let cause = error.code().unwrap_or("internal_error");
    FoldError::new(
        codes::INTEGRATION_FAILED,
        format!("the objects of the integration could not be stored: {error}"),
    )
    .with_details([("cause", json!(cause))])
    .with_hint("main did not move; fix the storage problem, then call integrate_proposal again")
    .into()
}
