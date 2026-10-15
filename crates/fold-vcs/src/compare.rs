use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::tree::{FlatEntry, FlatTree, flatten};
use fold_core::{FoldError, codes};
use serde_json::json;

use crate::diff::{
    DEFAULT_CONTEXT, Hunk, MAX_INLINE_BYTES, count_lines, diff_text, is_binary, line_count,
};
use crate::ignore::IgnoreRules;
use crate::index::StatIndex;
use crate::objects::ObjectStore;
use crate::paths::{RepoPath, WorkspaceError, os_bytes, resolve};
use crate::platform::Capabilities;
use crate::scan::{self, DEFAULT_MAX_FILE_BYTES, ScanOptions, StableRead};
use crate::snapshot::{keep_base, load_tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    /// A deleted path and an added path with the same blob. The pairing is for display only; merging never uses it.
    Renamed,
    /// The same blob with another entry kind, such as a file that became executable.
    ModeChanged,
}

impl ChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::ModeChanged => "mode_changed",
        }
    }
}

/// One changed path. A hash is `None` on the side where the path does not exist. Line counts are zero for a binary file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: RepoPath,
    /// The path before a rename; `None` for every other change.
    pub old_path: Option<RepoPath>,
    pub change: ChangeKind,
    pub binary: bool,
    pub old_hash: Option<ObjectHash>,
    pub new_hash: Option<ObjectHash>,
    pub added_lines: u32,
    pub removed_lines: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub files: u32,
    pub added_lines: u32,
    pub removed_lines: u32,
}

/// The changed files in path order and their sums.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Comparison {
    pub files: Vec<FileChange>,
    pub totals: Totals,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: RepoPath,
    pub old_path: Option<RepoPath>,
    pub change: ChangeKind,
    pub binary: bool,
    /// A side is larger than [`MAX_INLINE_BYTES`], so no hunks are built. Its content is not checked, so it is not marked binary.
    pub too_large: bool,
    /// Empty for a binary or too-large file and for a rename or mode change, whose content is the same on both sides.
    pub hunks: Vec<Hunk>,
}

/// One side of a comparison: the entries of a recorded tree, or the scanned working files of a workspace. A working side reads a file from the object store when the store holds its blob, and otherwise from the workspace with the scanner's stable read, so a file that changed after the scan is compared as it is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    entries: FlatTree,
    working: Option<PathBuf>,
}

impl State {
    pub fn recorded(entries: FlatTree) -> Self {
        Self {
            entries,
            working: None,
        }
    }

    /// `root` is the workspace directory the entries were scanned from.
    pub fn working(root: PathBuf, entries: FlatTree) -> Self {
        Self {
            entries,
            working: Some(root),
        }
    }

    pub fn entries(&self) -> &FlatTree {
        &self.entries
    }

    pub fn is_working(&self) -> bool {
        self.working.is_some()
    }
}

/// Every file and symlink of a stored tree.
pub fn load_flat(objects: &ObjectStore, tree: &ObjectHash) -> Result<FlatTree, FoldError> {
    flatten(tree, &|hash| load_tree(objects, hash))
}

/// The working files of the workspace at `root` as a comparison side. It writes nothing: the stat index at `index_path` is only read, to skip hashing files whose metadata did not move, and no object is stored. A path the scan skips or finds unstable keeps what `base_tree` has there, as a snapshot would record it.
pub fn working_state(
    objects: &ObjectStore,
    root: &Path,
    index_path: &Path,
    base_tree: &ObjectHash,
    caps: Capabilities,
) -> Result<State, FoldError> {
    let base = load_flat(objects, base_tree)?;
    let rules = IgnoreRules::new(root);
    let mut index = StatIndex::load(index_path, caps);
    let opts = ScanOptions {
        rules: &rules,
        max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        caps,
        recorded: Some(&base),
    };
    let scanned = scan::scan(root, &mut index, &opts)?;
    let mut entries = scanned.entries;
    let unrecorded = scanned
        .skipped
        .iter()
        .map(|skipped| &skipped.path)
        .chain(&scanned.unstable);
    for path in unrecorded {
        keep_base(&mut entries, &base, path);
    }
    Ok(State::working(root.to_path_buf(), entries))
}

/// The files that differ between `from` and `to`, with line counts read from both sides.
pub fn compare(objects: &ObjectStore, from: &State, to: &State) -> Result<Comparison, FoldError> {
    let mut comparison = Comparison::default();
    for found in classify(from.entries(), to.entries()) {
        let file = measure(objects, from, to, &found)?;
        let totals = &mut comparison.totals;
        totals.files = totals.files.saturating_add(1);
        totals.added_lines = totals.added_lines.saturating_add(file.added_lines);
        totals.removed_lines = totals.removed_lines.saturating_add(file.removed_lines);
        comparison.files.push(file);
    }
    Ok(comparison)
}

/// The hunks of one changed file, named by its path on the `to` side (its new path for a rename, its old path for a deletion). A path that the comparison does not list is refused with `path_not_in_comparison`.
pub fn file_diff(
    objects: &ObjectStore,
    from: &State,
    to: &State,
    path: &RepoPath,
) -> Result<FileDiff, FoldError> {
    let found = classify(from.entries(), to.entries())
        .into_iter()
        .find(|found| &found.path == path)
        .ok_or_else(|| {
            FoldError::new(
                codes::PATH_NOT_IN_COMPARISON,
                "the path is not a changed file of this comparison",
            )
            .with_details([("path", json!(path.to_string()))])
        })?;
    let old = side_content(objects, from, found.old_path.as_ref(), found.old)?;
    let new = side_content(objects, to, Some(&found.path), found.new)?;
    let too_large = [&old, &new]
        .into_iter()
        .flatten()
        .any(|bytes| bytes.len() > MAX_INLINE_BYTES);
    let text = (!too_large)
        .then(|| texts(old.as_deref(), new.as_deref()))
        .flatten();
    let hunks = match text {
        Some((old, new)) => diff_text(old, new, DEFAULT_CONTEXT),
        None => Vec::new(),
    };
    Ok(FileDiff {
        path: found.path,
        old_path: found.renamed_from,
        change: found.change,
        binary: !too_large && text.is_none(),
        too_large,
        hunks,
    })
}

/// A changed path before any content is read.
struct Found {
    path: RepoPath,
    /// Where the old side's content lives: the same path, or the old path of a rename.
    old_path: Option<RepoPath>,
    renamed_from: Option<RepoPath>,
    change: ChangeKind,
    old: Option<FlatEntry>,
    new: Option<FlatEntry>,
}

fn classify(from: &FlatTree, to: &FlatTree) -> Vec<Found> {
    let mut found = Vec::new();
    let mut deleted: BTreeMap<ObjectHash, Vec<(&Vec<u8>, FlatEntry)>> = BTreeMap::new();
    let mut added: BTreeMap<ObjectHash, Vec<(&Vec<u8>, FlatEntry)>> = BTreeMap::new();
    for (path, new) in to {
        match from.get(path) {
            None => added.entry(new.hash).or_default().push((path, *new)),
            Some(old) if old == new => {}
            Some(old) => {
                let change = if old.hash == new.hash {
                    ChangeKind::ModeChanged
                } else {
                    ChangeKind::Modified
                };
                push(
                    &mut found,
                    path,
                    Some(path),
                    None,
                    change,
                    Some(*old),
                    Some(*new),
                );
            }
        }
    }
    for (path, old) in from.iter().filter(|(path, _)| !to.contains_key(*path)) {
        deleted.entry(old.hash).or_default().push((path, *old));
    }
    for (hash, mut news) in added {
        let mut olds = deleted.remove(&hash).unwrap_or_default();
        let paired = news.len().min(olds.len());
        // Both lists are in path order, so the pairing is the same every time.
        for ((old_path, old), (new_path, new)) in olds.drain(..paired).zip(news.drain(..paired)) {
            push(
                &mut found,
                new_path,
                Some(old_path),
                Some(old_path),
                ChangeKind::Renamed,
                Some(old),
                Some(new),
            );
        }
        for (path, new) in news {
            push(
                &mut found,
                path,
                None,
                None,
                ChangeKind::Added,
                None,
                Some(new),
            );
        }
        if !olds.is_empty() {
            deleted.insert(hash, olds);
        }
    }
    for (path, old) in deleted.into_values().flatten() {
        push(
            &mut found,
            path,
            Some(path),
            None,
            ChangeKind::Deleted,
            Some(old),
            None,
        );
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

fn push(
    found: &mut Vec<Found>,
    path: &[u8],
    old_path: Option<&Vec<u8>>,
    renamed_from: Option<&Vec<u8>>,
    change: ChangeKind,
    old: Option<FlatEntry>,
    new: Option<FlatEntry>,
) {
    // A tree never holds a path that `RepoPath` refuses, so a refused one is a corrupt tree and is left out.
    let Ok(path) = RepoPath::parse(path) else {
        return;
    };
    let parse = |path: Option<&Vec<u8>>| path.and_then(|path| RepoPath::parse(path).ok());
    found.push(Found {
        path,
        old_path: parse(old_path),
        renamed_from: parse(renamed_from),
        change,
        old,
        new,
    });
}

fn measure(
    objects: &ObjectStore,
    from: &State,
    to: &State,
    found: &Found,
) -> Result<FileChange, FoldError> {
    let old = side_content(objects, from, found.old_path.as_ref(), found.old)?;
    let new = side_content(objects, to, Some(&found.path), found.new)?;
    let text = texts(old.as_deref(), new.as_deref());
    let (added_lines, removed_lines) = match (found.change, text) {
        (ChangeKind::Renamed | ChangeKind::ModeChanged, _) | (_, None) => (0, 0),
        (ChangeKind::Added, Some((_, new))) => (line_count(new), 0),
        (ChangeKind::Deleted, Some((old, _))) => (0, line_count(old)),
        (ChangeKind::Modified, Some((old, new))) => {
            let counts = count_lines(old, new);
            (counts.added, counts.removed)
        }
    };
    Ok(FileChange {
        path: found.path.clone(),
        old_path: found.renamed_from.clone(),
        change: found.change,
        binary: text.is_none(),
        old_hash: found.old.map(|entry| entry.hash),
        new_hash: found.new.map(|entry| entry.hash),
        added_lines,
        removed_lines,
    })
}

/// Both sides as text, or `None` when either side is binary. A missing side is empty text.
fn texts<'a>(old: Option<&'a [u8]>, new: Option<&'a [u8]>) -> Option<(&'a str, &'a str)> {
    let text = |bytes: Option<&'a [u8]>| match bytes {
        None => Some(""),
        Some(bytes) if is_binary(bytes) => None,
        Some(bytes) => std::str::from_utf8(bytes).ok(),
    };
    Some((text(old)?, text(new)?))
}

fn side_content(
    objects: &ObjectStore,
    state: &State,
    path: Option<&RepoPath>,
    entry: Option<FlatEntry>,
) -> Result<Option<Vec<u8>>, FoldError> {
    match (path, entry) {
        (Some(path), Some(entry)) => content(objects, state, path, entry).map(Some),
        _ => Ok(None),
    }
}

fn content(
    objects: &ObjectStore,
    state: &State,
    path: &RepoPath,
    entry: FlatEntry,
) -> Result<Vec<u8>, FoldError> {
    if let Some(root) = &state.working
        && !objects.contains(&entry.hash).map_err(FoldError::from)?
    {
        return read_working(root, path);
    }
    Ok(objects
        .read_expecting(&entry.hash, ObjectKind::Blob)
        .map_err(FoldError::from)?
        .payload)
}

/// A symlink gives its target text and is never followed.
fn read_working(root: &Path, path: &RepoPath) -> Result<Vec<u8>, FoldError> {
    let located = resolve(root, path)?;
    let changed = || {
        FoldError::new(
            codes::WORKSPACE_IO,
            "a working file changed while it was compared; compare again",
        )
        .with_details([("path", json!(path.to_string()))])
    };
    let meta = fs::symlink_metadata(&located).map_err(|_| changed())?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(&located).map_err(|_| changed())?;
        return os_bytes(target.as_os_str()).ok_or_else(changed);
    }
    match scan::read_stable(&located).map_err(WorkspaceError::from)? {
        StableRead::Read(bytes, _) => Ok(bytes),
        StableRead::Unstable | StableRead::Gone | StableRead::Denied => Err(changed()),
    }
}
