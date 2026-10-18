use fold_core::object::ObjectHash;
use fold_core::tree::ChangeCounts;
use fold_core::{RevisionId, Timestamp, WorkspaceId};
use fold_store::workspaces::WorkspaceRow;
use fold_vcs::scan::{changes, count_changes};
use tokio::sync::{OwnedMutexGuard, OwnedRwLockReadGuard};

use super::{WorkspaceError, WorkspaceService, blocking};

/// The tree of a reviewer's fix: the files of its review checkout recorded against the version's base, with the locks that took it. The caller keeps it until the transaction that names `tree` commits, so no record, sync or restore of the task workspace runs in between and garbage collection keeps the new objects.
pub(crate) struct ReviewFixTree {
    /// Every object of it is in the object store.
    pub tree: ObjectHash,
    /// Against the version's base revision.
    pub stats: ChangeCounts,
    /// The paths whose entry differs from the fixed version's tree, in path order.
    pub changed: Vec<String>,
    pub taken_at: Timestamp,
    _held: (
        OwnedMutexGuard<()>,
        OwnedMutexGuard<()>,
        OwnedRwLockReadGuard<()>,
    ),
}

impl WorkspaceService {
    /// Records the files of the review checkout `checkout` as a tree against the tree of `base`, by the ignore rules and the largest file of a record, and lists the paths that differ from `version_tree`. It holds the lock of the task workspace `task_workspace` first, then the checkout's, then the shared object lock, the order every command that names a task workspace takes them in. It reads the checkout and writes only objects and the checkout's stat index; neither workspace's files change.
    pub(crate) async fn snapshot_review_fix(
        &self,
        task_workspace: WorkspaceId,
        checkout: &WorkspaceRow,
        base: RevisionId,
        version_tree: ObjectHash,
    ) -> Result<ReviewFixTree, WorkspaceError> {
        let task_op = self.ops.lock(task_workspace).await;
        let checkout_op = self.ops.lock(checkout.id).await;
        let files = self.files(checkout).await?;
        let base_tree = self.tree_of(base).await?;
        let objects = self.objects.clone().read_owned().await;
        let taken_at = self.clock.now();
        blocking(move || {
            let held = (task_op, checkout_op, objects);
            let snapshot = files.snapshot(base_tree)?;
            let base = files.flat_tree(&base_tree)?;
            let version = files.flat_tree(&version_tree)?;
            let changed = changes(&version, &snapshot.entries)
                .into_iter()
                .map(|change| String::from_utf8_lossy(&change.path).into_owned())
                .collect();
            Ok(ReviewFixTree {
                tree: snapshot.tree,
                stats: count_changes(&changes(&base, &snapshot.entries)),
                changed,
                taken_at,
                _held: held,
            })
        })
        .await
    }
}
