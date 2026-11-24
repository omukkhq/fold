use fold_core::object::ObjectHash;
use fold_core::tree::ChangeCounts;
use fold_core::{FoldError, RevisionId, Timestamp, WorkspaceId};
use fold_store::workspaces::WorkspaceRow;
use fold_vcs::integrate::tree_merge;
use fold_vcs::scan::{changes, count_changes};
use fold_vcs::sync::{ConflictedPath, MarkerLabels, SIDECAR_SUFFIX, merge_target, unresolved};
use tokio::sync::{OwnedMutexGuard, OwnedRwLockReadGuard};

use super::{WorkspaceError, WorkspaceService, blocking, open_objects};

/// The files a review checkout of a conflicted version holds when it is made.
#[derive(Debug, Clone)]
pub(crate) struct ConflictTree {
    /// Every object of it is in the object store.
    pub tree: ObjectHash,
    /// The paths that conflict, with a marker or a `.fold-main` side, in the order the merge wrote them.
    pub conflicted: Vec<ConflictedPath>,
}

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

impl WorkspaceService {
    /// The tree of a review checkout of a conflicted version: `version_tree` merged onto the tree of main's revision `main` against the tree of the version's `base`, with markers labelled as a sync labels them around each conflicted text hunk and main's side of each other conflict as `<path>.fold-main`, stored with every object it needs. The same inputs always give the same tree, so a fix in the checkout is compared against it without a row that names it. It holds the shared object lock while it writes.
    pub(crate) async fn conflict_checkout_tree(
        &self,
        base: RevisionId,
        version_tree: ObjectHash,
        main: RevisionId,
    ) -> Result<ConflictTree, WorkspaceError> {
        let base_tree = self.tree_of(base).await?;
        let main_tree = self.tree_of(main).await?;
        let paths = self.paths.clone();
        let objects = self.objects.clone().read_owned().await;
        blocking(move || {
            let _held = objects;
            let store = open_objects(&paths)?;
            let merge = tree_merge(&store, &base_tree, &version_tree, &main_tree)?;
            let labels = MarkerLabels::sync(base, main);
            let (tree, conflicted) =
                merge_target(&store, &merge, &version_tree, &labels, SIDECAR_SUFFIX)?;
            Ok(ConflictTree { tree, conflicted })
        })
        .await
    }

    /// The paths of `conflicted` that the review checkout `checkout` still holds a marker line in, or whose `<path>.fold-main` side is still there, as text.
    pub(crate) async fn unresolved_in_checkout(
        &self,
        checkout: &WorkspaceRow,
        conflicted: Vec<ConflictedPath>,
    ) -> Result<Vec<String>, WorkspaceError> {
        let dir = self.workspace_dir(checkout);
        blocking(move || {
            Ok(unresolved(&dir, &conflicted)
                .map_err(FoldError::from)?
                .into_iter()
                .map(|path| String::from_utf8_lossy(path.as_bytes()).into_owned())
                .collect())
        })
        .await
    }
}
