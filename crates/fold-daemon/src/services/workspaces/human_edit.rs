use fold_core::object::ObjectHash;
use fold_core::tree::ChangeCounts;
use fold_core::{RevisionId, Timestamp, WorkspaceId};
use fold_vcs::edit::replace_file;
use fold_vcs::paths::RepoPath;
use tokio::sync::{OwnedMutexGuard, OwnedRwLockReadGuard};

use super::{WorkspaceError, WorkspaceService, blocking, open_objects};
use crate::services::projects::load_config;

/// The tree of a human's edit of a proposal version, with the workspace lock and the shared object lock that built it. The caller keeps it until the transaction that names `tree` commits, so no record, sync or restore of the task workspace runs in between and garbage collection keeps the new objects.
pub(crate) struct HumanEditTree {
    /// Every object of it is in the object store.
    pub tree: ObjectHash,
    /// Against the version's base revision.
    pub stats: ChangeCounts,
    pub taken_at: Timestamp,
    _held: (OwnedMutexGuard<()>, OwnedRwLockReadGuard<()>),
}

impl WorkspaceService {
    /// Builds the tree of `version_tree` with the file at `path` set to `content`, under the lock of the task workspace `workspace`, without reading or writing its files: the task worker's next sync or restore writes the edit there. The project's largest file applies to the new content.
    pub(crate) async fn build_human_edit(
        &self,
        workspace: WorkspaceId,
        version_tree: ObjectHash,
        base: RevisionId,
        path: RepoPath,
        content: Vec<u8>,
    ) -> Result<HumanEditTree, WorkspaceError> {
        let op = self.ops.lock(workspace).await;
        let config = load_config(&self.paths).await?;
        let base_tree = self.tree_of(base).await?;
        let objects = self.objects.clone().read_owned().await;
        let taken_at = self.clock.now();
        let paths = self.paths.clone();
        blocking(move || {
            let held = (op, objects);
            let edited = replace_file(
                &open_objects(&paths)?,
                &version_tree,
                &base_tree,
                &path,
                &content,
                config.max_file_bytes,
            )?;
            Ok(HumanEditTree {
                tree: edited.tree,
                stats: edited.stats,
                taken_at,
                _held: held,
            })
        })
        .await
    }
}
