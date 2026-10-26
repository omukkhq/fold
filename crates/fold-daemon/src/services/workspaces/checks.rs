use std::path::PathBuf;
use std::sync::Arc;

use fold_core::FoldError;
use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::reviews::checks::CHECKS_FILE;
use fold_core::tree::{EntryKind, FlatEntry};
use fold_store::workspaces::WorkspaceRow;
use fold_vcs::compare::load_flat;
use fold_vcs::ignore::IgnoreRules;
use fold_vcs::index::StatIndex;
use fold_vcs::materialise::MaterialiseOptions;
use fold_vcs::restore::{RestoreOptions, reset_tree};
use fold_vcs::scan::{ScanOptions, changes, scan};

use super::{WorkspaceError, WorkspaceService, blocking, capabilities, open_objects};
use crate::services::projects::load_config;

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

    /// Brings fold's check directory to the tree `tree`: its tracked files become the tree's, and ignored files such as the dependencies a setup installed stay. The directory is created on first use. It is fold's own directory, never a workspace, so whatever an earlier run left in it is overwritten.
    pub(crate) async fn check_dir_to(&self, tree: ObjectHash) -> Result<PathBuf, WorkspaceError> {
        let config = load_config(&self.paths).await?;
        let paths = self.paths.clone();
        let probed = Arc::clone(&self.caps);
        blocking(move || {
            let caps = capabilities(&probed, &paths)?;
            let objects = open_objects(&paths)?;
            let root = paths.checks_dir();
            let index_path = paths.checks_index();
            let mut index = StatIndex::load(&index_path, caps);
            let rules = IgnoreRules::new(&root);
            let opts = RestoreOptions {
                scan: ScanOptions {
                    rules: &rules,
                    max_file_bytes: config.max_file_bytes,
                    caps,
                    recorded: None,
                },
                write: MaterialiseOptions {
                    read_only: false,
                    caps,
                },
            };
            reset_tree(&objects, &root, &mut index, &index_path, &tree, &opts)?;
            Ok(root)
        })
        .await
    }

    /// Whether the tracked files of fold's check directory equal the tree `tree`, so a check run left them as it found them.
    pub(crate) async fn check_dir_holds(&self, tree: ObjectHash) -> Result<bool, WorkspaceError> {
        let config = load_config(&self.paths).await?;
        let paths = self.paths.clone();
        let probed = Arc::clone(&self.caps);
        blocking(move || {
            let caps = capabilities(&probed, &paths)?;
            let objects = open_objects(&paths)?;
            let root = paths.checks_dir();
            let index_path = paths.checks_index();
            let mut index = StatIndex::load(&index_path, caps);
            let rules = IgnoreRules::new(&root);
            let expected = load_flat(&objects, &tree)?;
            let opts = ScanOptions {
                rules: &rules,
                max_file_bytes: config.max_file_bytes,
                caps,
                recorded: Some(&expected),
            };
            let scanned = scan(&root, &mut index, &opts).map_err(FoldError::from)?;
            let _ = index.save(&index_path);
            Ok(scanned.unstable.is_empty() && scanned.entries == expected)
        })
        .await
    }

    /// The directory of a workspace's files.
    pub(crate) fn workspace_dir(&self, row: &WorkspaceRow) -> PathBuf {
        self.paths.root().join(&row.path)
    }
}
