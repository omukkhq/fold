use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::tree::{EntryKind, Tree, TreeEntry};
use fold_core::{FoldError, codes};
use serde_json::json;

use crate::diff::{MAX_INLINE_BYTES, is_binary};
use crate::objects::ObjectStore;
use crate::paths::RepoPath;
use crate::snapshot::load_tree;

/// One entry of a stored directory. `size` is the content length of a file or the target length of a symlink, and `None` for a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: Vec<u8>,
    pub kind: EntryKind,
    pub hash: ObjectHash,
    pub size: Option<u64>,
}

/// The content of one stored file. A symlink's content is its target text; it is never followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileContent {
    pub kind: EntryKind,
    pub hash: ObjectHash,
    pub size: u64,
    /// Empty for a file larger than [`MAX_INLINE_BYTES`].
    pub bytes: Vec<u8>,
    /// As [`is_binary`] decides; a text file's bytes are valid UTF-8. A too-large file is not read, so it is not marked binary.
    pub binary: bool,
    /// The file is larger than [`MAX_INLINE_BYTES`], so only its size and hash are given.
    pub too_large: bool,
}

/// A path as a caller writes it: empty for the root, otherwise a normalised relative path. `..`, `.`, an empty component, a leading `/` and a `.git` or `.fold` component are refused with `invalid_path`.
pub fn parse_path(text: &str) -> Result<Option<RepoPath>, FoldError> {
    if text.is_empty() {
        return Ok(None);
    }
    Ok(Some(RepoPath::parse(text.as_bytes())?))
}

/// The entries of the directory at `path` in the tree `root_tree`, in name order; `None` lists the root. A path that is absent or is not a directory is `not_found`.
pub fn read_dir(
    objects: &ObjectStore,
    root_tree: &ObjectHash,
    path: Option<&RepoPath>,
) -> Result<Vec<DirEntry>, FoldError> {
    let tree = match path {
        None => load_tree(objects, root_tree)?,
        Some(path) => {
            let entry = locate(objects, root_tree, path)?;
            if entry.kind != EntryKind::Directory {
                return Err(not_found(path, "is not a directory"));
            }
            load_tree(objects, &entry.hash)?
        }
    };
    tree.entries
        .into_iter()
        .map(|entry| {
            let size = match entry.kind {
                EntryKind::Directory => None,
                _ => Some(objects.blob_size(&entry.hash).map_err(FoldError::from)?),
            };
            Ok(DirEntry {
                name: entry.name,
                kind: entry.kind,
                hash: entry.hash,
                size,
            })
        })
        .collect()
}

/// The file or symlink at `path` in the tree `root_tree`. A path that is absent or is a directory is `not_found`.
pub fn read_file(
    objects: &ObjectStore,
    root_tree: &ObjectHash,
    path: &RepoPath,
) -> Result<FileContent, FoldError> {
    let entry = locate(objects, root_tree, path)?;
    if entry.kind == EntryKind::Directory {
        return Err(not_found(path, "is a directory"));
    }
    let size = objects.blob_size(&entry.hash).map_err(FoldError::from)?;
    if size > MAX_INLINE_BYTES as u64 {
        return Ok(FileContent {
            kind: entry.kind,
            hash: entry.hash,
            size,
            bytes: Vec::new(),
            binary: false,
            too_large: true,
        });
    }
    let bytes = objects
        .read_expecting(&entry.hash, ObjectKind::Blob)
        .map_err(FoldError::from)?
        .payload;
    Ok(FileContent {
        kind: entry.kind,
        hash: entry.hash,
        size,
        binary: is_binary(&bytes),
        bytes,
        too_large: false,
    })
}

/// Walks the trees one component at a time, so only the directories on the path are read.
fn locate(
    objects: &ObjectStore,
    root_tree: &ObjectHash,
    path: &RepoPath,
) -> Result<TreeEntry, FoldError> {
    let mut tree: Tree = load_tree(objects, root_tree)?;
    let mut names = path.components().peekable();
    while let Some(name) = names.next() {
        let entry = tree
            .entries
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| not_found(path, "does not exist"))?;
        if names.peek().is_none() {
            return Ok(entry);
        }
        if entry.kind != EntryKind::Directory {
            return Err(not_found(path, "does not exist"));
        }
        tree = load_tree(objects, &entry.hash)?;
    }
    Err(not_found(path, "does not exist"))
}

fn not_found(path: &RepoPath, what: &str) -> FoldError {
    FoldError::new(
        codes::NOT_FOUND,
        format!("the path {what} in this revision"),
    )
    .with_details([("path", json!(path.to_string()))])
}
