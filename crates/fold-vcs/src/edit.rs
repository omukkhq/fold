use std::collections::BTreeMap;

use fold_core::object::{ObjectHash, ObjectKind, hash_object};
use fold_core::tree::{ChangeCounts, EntryKind, FlatEntry, apply_changes};
use fold_core::{FoldError, codes};
use serde_json::json;

use crate::browse::read_file;
use crate::compare::load_flat;
use crate::diff::{MAX_INLINE_BYTES, is_binary};
use crate::objects::ObjectStore;
use crate::paths::RepoPath;
use crate::scan::{changes, count_changes};
use crate::snapshot::load_tree;

/// A stored tree with one file's content replaced, and its file counts against a base tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditedTree {
    pub tree: ObjectHash,
    pub stats: ChangeCounts,
}

/// Replaces the content of the text file at `path` in `tree` with `content`, keeping the file's kind, and stores the blob and every rebuilt tree before it returns the new root. Only a text file shown inline can be edited this way, and the new content must be text of at most `max_file_bytes` and the inline limit: anything else is `file_not_editable` with `details.reason`. Content equal to the stored file is `version_unchanged`. `stats` counts the changes of the new tree against `base_tree`.
pub fn replace_file(
    objects: &ObjectStore,
    tree: &ObjectHash,
    base_tree: &ObjectHash,
    path: &RepoPath,
    content: &[u8],
    max_file_bytes: u64,
) -> Result<EditedTree, FoldError> {
    let file = match read_file(objects, tree, path) {
        Ok(file) => file,
        Err(error) if error.code() == codes::NOT_FOUND => return Err(not_editable("missing")),
        Err(error) => return Err(error),
    };
    if !matches!(file.kind, EntryKind::File | EntryKind::Executable) {
        return Err(not_editable("not_a_file"));
    }
    if file.too_large {
        return Err(not_editable("too_large"));
    }
    if file.binary {
        return Err(not_editable("binary"));
    }
    let limit = max_file_bytes.min(MAX_INLINE_BYTES as u64);
    if content.len() as u64 > limit {
        return Err(not_editable("too_large"));
    }
    if is_binary(content) {
        return Err(not_editable("binary"));
    }
    if hash_object(ObjectKind::Blob, content) == file.hash {
        return Err(FoldError::new(
            codes::VERSION_UNCHANGED,
            "the new content equals the file in the version",
        )
        .with_hint("change the file, then save again"));
    }
    let blob = objects
        .write(ObjectKind::Blob, content)
        .map_err(FoldError::from)?;
    let edit = BTreeMap::from([(
        path.as_bytes().to_vec(),
        Some(FlatEntry {
            kind: file.kind,
            hash: blob,
        }),
    )]);
    let built = apply_changes(Some(*tree), &edit, &|hash| load_tree(objects, hash))?;
    // Children come before the trees that name them.
    for (hash, payload) in &built.new_trees {
        let stored = objects
            .write(ObjectKind::Tree, payload)
            .map_err(FoldError::from)?;
        if stored != *hash {
            return Err(FoldError::new(
                codes::OBJECT_CORRUPT,
                "a rebuilt tree was stored under another hash",
            ));
        }
    }
    objects.refresh(&built.root).map_err(FoldError::from)?;
    let base = load_flat(objects, base_tree)?;
    let edited = load_flat(objects, &built.root)?;
    Ok(EditedTree {
        tree: built.root,
        stats: count_changes(&changes(&base, &edited)),
    })
}

fn not_editable(reason: &str) -> FoldError {
    FoldError::new(
        codes::FILE_NOT_EDITABLE,
        "only a text file of the version can be edited, with text content",
    )
    .with_details([("reason", json!(reason))])
    .with_hint("edit another file, or comment and request changes")
}
