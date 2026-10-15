use std::path::PathBuf;
use std::sync::Arc;

use fold_core::NewEvent;
use fold_core::events::types;
use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::tree::{FlatTree, Tree, flatten};
use fold_core::{Actor, FoldError, RevisionId, Timestamp, WorkerName, WorkspaceId, codes, policy};
use fold_store::WriteTx;
use fold_store::workspaces::{
    self, Checkpoint, CheckpointReason, ConflictRow, WorkspaceKind, WorkspaceRow, WorkspaceState,
};
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::index::StatIndex;
use fold_vcs::materialise::MaterialiseOptions;
use fold_vcs::objects::ObjectStore;
use fold_vcs::paths::RepoPath;
use fold_vcs::platform::Capabilities;
use fold_vcs::restore::{RestoreOptions, record_checkpoint_tree, restore_tree};
use fold_vcs::scan::ScanOptions;
use fold_vcs::snapshot::Snapshot;
use fold_vcs::sync::{ConflictedPath, unresolved};
use serde_json::json;

use super::{WorkspaceError, WorkspaceService, blocking, capabilities, open_objects};
use crate::services::events::EventBuffer;
use crate::services::projects::load_config;

/// What a command needs to read and write the files of one workspace. Every method blocks: call it inside `blocking`.
pub(super) struct Files {
    objects: ObjectStore,
    root: PathBuf,
    index_path: PathBuf,
    caps: Capabilities,
    max_file_bytes: u64,
    /// How many automatic checkpoints of the workspace stay unpruned (`auto_checkpoints_kept`).
    pub(super) auto_kept: u32,
}

impl Files {
    pub(super) fn objects(&self) -> &ObjectStore {
        &self.objects
    }

    pub(super) fn flat_tree(&self, tree: &ObjectHash) -> Result<FlatTree, FoldError> {
        flatten(tree, &|hash| {
            let object = self
                .objects
                .read_expecting(hash, ObjectKind::Tree)
                .map_err(FoldError::from)?;
            Ok(Tree::decode(&object.payload)?)
        })
    }

    pub(super) fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub(super) fn index(&self) -> StatIndex {
        StatIndex::load(&self.index_path, self.caps)
    }

    pub(super) fn save_index(&self, index: &mut StatIndex) {
        if index.save(&self.index_path).is_err() {
            tracing::warn!("a stat index was not saved; the next scan rebuilds it");
        }
    }

    /// Records the workspace as a tree against `base_tree`. A snapshot with nothing new returns a tree that already exists without writing it, so the root is refreshed here: the row that names it commits within the grace period of garbage collection.
    pub(super) fn snapshot(&self, base_tree: ObjectHash) -> Result<Snapshot, FoldError> {
        let rules = IgnoreRules::new(&self.root);
        let mut index = self.index();
        let opts = ScanOptions {
            rules: &rules,
            max_file_bytes: self.max_file_bytes,
            caps: self.caps,
            recorded: None,
        };
        let snapshot = record_checkpoint_tree(
            &self.objects,
            &self.root,
            &mut index,
            &self.index_path,
            Some(base_tree),
            &opts,
        )?;
        self.objects
            .refresh(&snapshot.tree)
            .map_err(FoldError::from)?;
        Ok(snapshot)
    }

    /// Makes the tracked files equal `target`. The caller has recorded the current state first.
    pub(super) fn restore(&self, target: &ObjectHash) -> Result<(), FoldError> {
        let rules = IgnoreRules::new(&self.root);
        let mut index = self.index();
        let opts = RestoreOptions {
            scan: ScanOptions {
                rules: &rules,
                max_file_bytes: self.max_file_bytes,
                caps: self.caps,
                recorded: None,
            },
            write: MaterialiseOptions {
                read_only: false,
                caps: self.caps,
            },
        };
        let restored = restore_tree(
            &self.objects,
            &self.root,
            &mut index,
            &self.index_path,
            target,
            &opts,
        )?;
        if !restored.degraded.is_empty() {
            tracing::warn!(
                count = restored.degraded.len(),
                "symlinks were restored as regular files"
            );
        }
        Ok(())
    }

    /// The listed conflicts whose file still holds a marker line or whose `<path>.fold-main` still exists.
    pub(super) fn unresolved(
        &self,
        conflicts: &[ConflictRow],
    ) -> Result<Vec<ConflictRow>, FoldError> {
        let mut listed = Vec::with_capacity(conflicts.len());
        for row in conflicts {
            listed.push(ConflictedPath {
                path: RepoPath::parse(&row.path)?,
                kind: row.kind,
            });
        }
        let open = unresolved(&self.root, &listed)?;
        Ok(conflicts
            .iter()
            .filter(|row| {
                open.iter()
                    .any(|path| path.as_bytes() == row.path.as_slice())
            })
            .cloned()
            .collect())
    }
}

impl WorkspaceService {
    /// The files of `row`, with the project's configured largest file.
    pub(super) async fn files(&self, row: &WorkspaceRow) -> Result<Arc<Files>, WorkspaceError> {
        let config = load_config(&self.paths).await?;
        let paths = self.paths.clone();
        let root = self.paths.root().join(&row.path);
        let index_path = self.paths.workspace_index(row.id);
        let probed = Arc::clone(&self.caps);
        blocking(move || {
            let caps = capabilities(&probed, &paths)?;
            let objects = open_objects(&paths)?;
            Ok(Arc::new(Files {
                objects,
                root,
                index_path,
                caps,
                max_file_bytes: config.max_file_bytes,
                auto_kept: u32::try_from(config.auto_checkpoints_kept).unwrap_or(u32::MAX),
            }))
        })
        .await
    }

    /// The workspace, refused unless the actor's worker owns it and it is `active`.
    pub(super) async fn owned(
        &self,
        actor: &Actor,
        workspace: WorkspaceId,
    ) -> Result<WorkspaceRow, WorkspaceError> {
        let row = workspaces::get_workspace(self.store.read(), workspace)
            .await?
            .ok_or_else(|| not_found(workspace))?;
        check_change(actor, &row)?;
        Ok(row)
    }

    pub(super) async fn tree_of(&self, revision: RevisionId) -> Result<ObjectHash, WorkspaceError> {
        Ok(workspaces::get_by_number(self.store.read(), revision)
            .await?
            .tree_hash)
    }
}

/// The ownership and state checks again, inside the transaction that records the change, so a workspace abandoned meanwhile is refused.
pub(super) async fn owned_in_tx(
    tx: &mut WriteTx,
    actor: &Actor,
    workspace: WorkspaceId,
) -> Result<WorkspaceRow, WorkspaceError> {
    let row = workspaces::get_workspace(tx.conn(), workspace)
        .await?
        .ok_or_else(|| not_found(workspace))?;
    check_change(actor, &row)?;
    Ok(row)
}

fn check_change(actor: &Actor, row: &WorkspaceRow) -> Result<(), FoldError> {
    check_active(
        policy::can_change_workspace(actor, row.worker.as_ref()),
        row,
    )
}

/// The checks of an automatic checkpoint: `check_change`, except that the daemon itself may take one.
pub(super) fn check_auto(actor: &Actor, row: &WorkspaceRow) -> Result<(), FoldError> {
    check_active(
        policy::can_take_auto_checkpoint(actor, row.worker.as_ref()),
        row,
    )
}

/// A review checkout is refused before the owner check, for the daemon too: it is read, built and tested, and never recorded, synced, restored or checkpointed.
fn check_active(
    allowed: Result<(), policy::PolicyDenied>,
    row: &WorkspaceRow,
) -> Result<(), FoldError> {
    if row.kind == WorkspaceKind::Review {
        return Err(FoldError::new(
            codes::INVALID_REQUEST,
            format!("{} is a review checkout", row.id),
        )
        .with_details([
            ("reason", json!("review_checkout")),
            ("workspace", json!(row.id.to_string())),
        ])
        .with_hint(
            "record work in a task workspace; a review checkout is only read, built and tested",
        ));
    }
    if let Err(denied) = allowed {
        return Err(FoldError::from(denied).with_details([
            ("workspace", json!(row.id.to_string())),
            ("owner", json!(row.worker.as_ref().map(WorkerName::as_str))),
        ]));
    }
    if row.state != WorkspaceState::Active {
        return Err(FoldError::new(
            codes::WORKSPACE_NOT_ACTIVE,
            format!("{} is {}", row.id, row.state.as_str()),
        )
        .with_details([
            ("workspace", json!(row.id.to_string())),
            ("state", json!(row.state.as_str())),
        ])
        .with_hint("call get_workspace to see its state"));
    }
    Ok(())
}

pub(super) fn not_found(workspace: WorkspaceId) -> FoldError {
    FoldError::new(
        codes::WORKSPACE_NOT_FOUND,
        format!("{workspace} does not exist"),
    )
    .with_hint("call get_workspace with the task to find its workspace")
}

/// Inserts a checkpoint of `row` with the workspace's current base and appends `checkpoint.recorded`.
#[expect(
    clippy::too_many_arguments,
    reason = "the step of a command, with the inputs its earlier steps produced"
)]
pub(super) async fn checkpoint_in_tx(
    tx: &mut WriteTx,
    buffer: &mut EventBuffer,
    actor: &Actor,
    row: &WorkspaceRow,
    tree: ObjectHash,
    reason: CheckpointReason,
    label: Option<&str>,
    now: Timestamp,
) -> Result<Checkpoint, WorkspaceError> {
    let checkpoint =
        workspaces::insert_checkpoint(tx, row.id, tree, reason.kind(), reason, label, now).await?;
    let event = NewEvent::new(types::CHECKPOINT_RECORDED, actor.event_actor())?
        .task(row.task)
        .workspace(row.id)
        .revision(row.base)
        .data(json!({
            "checkpoint": checkpoint.id.to_string(),
            "kind": checkpoint.kind.as_str(),
            "reason": checkpoint.reason.as_str(),
        }))?;
    buffer.append(tx, &event, now).await?;
    Ok(checkpoint)
}

/// Moves the workspace's base. The base is a revision a row already names, so a move that finds no revision means a damaged database.
pub(super) async fn set_base(
    tx: &mut WriteTx,
    workspace: WorkspaceId,
    base: RevisionId,
) -> Result<(), WorkspaceError> {
    if workspaces::set_base(tx, workspace, base).await? {
        Ok(())
    } else {
        Err(fold_store::StoreError::Corrupt("workspaces.base_revision_id").into())
    }
}
