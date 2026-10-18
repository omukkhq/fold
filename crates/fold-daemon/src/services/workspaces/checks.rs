use std::path::PathBuf;

use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::reviews::checks::CHECKS_FILE;
use fold_core::tree::{EntryKind, FlatEntry};
use fold_store::workspaces::WorkspaceRow;
use fold_vcs::compare::load_flat;
use fold_vcs::scan::changes;

use super::{WorkspaceError, WorkspaceService, blocking, open_objects};

/// What a check run needs from two stored trees: the checks file of the changed tree and the paths the change touches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksInput {
    /// The text of the changed tree's `fold-checks.toml`; `None` without one. A file that is not UTF-8 text is given lossily, so the parser refuses it.
    pub file: Option<String>,
    /// The checks file entry of the base tree and of the changed tree, which tell whether the change touches the checks.
    pub base_entry: Option<FlatEntry>,
    pub tree_entry: Option<FlatEntry>,
    /// Every path added, modified or deleted, in path order.
    pub changed: Vec<String>,
}

impl WorkspaceService {
    /// The checks file of `tree` and the paths that differ between `base` and `tree`, read from the object store in one blocking task. A symlink or directory in the file's place counts as a file of its target text or as no file, as Git would show it.
    pub(crate) async fn checks_input(
        &self,
        base: ObjectHash,
        tree: ObjectHash,
    ) -> Result<ChecksInput, WorkspaceError> {
        let paths = self.paths.clone();
        blocking(move || {
            let objects = open_objects(&paths)?;
            let before = load_flat(&objects, &base)?;
            let after = load_flat(&objects, &tree)?;
            let changed = changes(&before, &after)
                .into_iter()
                .map(|change| String::from_utf8_lossy(&change.path).into_owned())
                .collect();
            let key = CHECKS_FILE.as_bytes();
            let tree_entry = after.get(key).copied();
            let file = match tree_entry {
                Some(entry) if entry.kind != EntryKind::Directory => {
                    let blob = objects
                        .read_expecting(&entry.hash, ObjectKind::Blob)
                        .map_err(fold_core::FoldError::from)?;
                    Some(String::from_utf8_lossy(&blob.payload).into_owned())
                }
                _ => None,
            };
            Ok(ChecksInput {
                file,
                base_entry: before.get(key).copied(),
                tree_entry,
                changed,
            })
        })
        .await
    }

    /// The directory of a workspace's files.
    pub(crate) fn workspace_dir(&self, row: &WorkspaceRow) -> PathBuf {
        self.paths.root().join(&row.path)
    }
}
