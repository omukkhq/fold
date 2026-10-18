use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};

use fold_core::merge::carry_change;
use fold_core::object::{ObjectHash, ObjectKind, hash_object};
pub use fold_core::tree::ChangeKind;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree, flatten};
use fold_core::{FoldError, codes};
use serde_json::json;

use crate::diff::{
    DEFAULT_CONTEXT, Hunk, MAX_INLINE_BYTES, count_lines, diff_text, is_binary, line_count,
};
use crate::ignore::IgnoreRules;
use crate::index::StatIndex;
use crate::objects::ObjectStore;
use crate::paths::{RepoPath, WorkspaceError, open_parent};
use crate::platform::Capabilities;
use crate::scan::{self, ScanOptions, Skipped, StableRead};
use crate::snapshot::{Warning, WarningReason, keep_base, load_tree, remove_below};

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
    /// The content hash on each side; `None` on the side where the path does not exist.
    pub old_hash: Option<ObjectHash>,
    pub new_hash: Option<ObjectHash>,
    /// A side is larger than [`MAX_INLINE_BYTES`], so no hunks are built. Its content is not checked, so it is not marked binary.
    pub too_large: bool,
    /// Empty for a binary or too-large file and for a rename or mode change, whose content is the same on both sides.
    pub hunks: Vec<Hunk>,
}

/// One side of a comparison: the entries of a recorded tree, or the scanned working files of a workspace. A working side reads a file from the object store when the store holds its blob, and otherwise from the workspace with the scanner's stable read. Content that no longer has the hash the scan recorded is reported as changed, so the caller scans and compares again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    entries: FlatTree,
    working: Option<PathBuf>,
    /// Content that the store may not hold, by hash: the line-merged files of a carried state.
    blobs: BTreeMap<ObjectHash, Vec<u8>>,
}

impl State {
    pub fn recorded(entries: FlatTree) -> Self {
        Self {
            entries,
            working: None,
            blobs: BTreeMap::new(),
        }
    }

    /// `root` is the workspace directory the entries were scanned from.
    pub fn working(root: PathBuf, entries: FlatTree) -> Self {
        Self {
            entries,
            working: Some(root),
            blobs: BTreeMap::new(),
        }
    }

    pub fn entries(&self) -> &FlatTree {
        &self.entries
    }

    pub fn is_working(&self) -> bool {
        self.working.is_some()
    }
}

/// A recorded state with its conflicted paths: see [`carried_state`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarriedState {
    pub state: State,
    /// In path order.
    pub conflicts: Vec<RepoPath>,
}

/// The stored tree `tree` with its change against `base` carried onto `onto` by the three-way merge; a conflicted path keeps `tree`'s side. It writes nothing: the line-merged files stay in the state. Blocking: call it through `spawn_blocking`.
pub fn carried_state(
    objects: &ObjectStore,
    base: &ObjectHash,
    tree: &ObjectHash,
    onto: &ObjectHash,
) -> Result<CarriedState, FoldError> {
    let (base, ours, theirs) = (
        load_flat(objects, base)?,
        load_flat(objects, tree)?,
        load_flat(objects, onto)?,
    );
    let blob = |hash: &ObjectHash| -> Result<Vec<u8>, FoldError> {
        Ok(objects.read_expecting(hash, ObjectKind::Blob)?.payload)
    };
    let carried = carry_change(&base, &ours, &theirs, &blob)?;
    Ok(CarriedState {
        state: State {
            entries: carried.tree,
            working: None,
            blobs: carried.new_blobs.into_iter().collect(),
        },
        conflicts: carried
            .conflicts
            .iter()
            .filter_map(|path| RepoPath::parse(path).ok())
            .collect(),
    })
}

/// Every file and symlink of a stored tree.
pub fn load_flat(objects: &ObjectStore, tree: &ObjectHash) -> Result<FlatTree, FoldError> {
    flatten(tree, &|hash| load_tree(objects, hash))
}

/// The working side of a comparison and the files the scan could not read, sorted by path. Each such file keeps its base entry, so it never shows as deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingState {
    pub state: State,
    pub warnings: Vec<Warning>,
}

/// The working files of the workspace at `root` as a comparison side. It writes nothing: the stat index at `index_path` is only read, to skip hashing files whose metadata did not move, and no object is stored. A path the scan skips or finds unstable keeps what `base_tree` has there, as a snapshot would record it; `max_file_bytes` is the project's largest file, so a comparison skips exactly the files a recording would skip.
pub fn working_state(
    objects: &ObjectStore,
    root: &Path,
    index_path: &Path,
    base_tree: &ObjectHash,
    caps: Capabilities,
    max_file_bytes: u64,
) -> Result<WorkingState, FoldError> {
    let base = load_flat(objects, base_tree)?;
    let rules = IgnoreRules::new(root);
    let mut index = StatIndex::load(index_path, caps);
    let opts = ScanOptions {
        rules: &rules,
        max_file_bytes,
        caps,
        recorded: Some(&base),
    };
    let scanned = scan::scan(root, &mut index, &opts)?;
    let mut entries = scanned.entries;
    let mut warnings: Vec<Warning> = scanned
        .skipped
        .into_iter()
        .map(|skipped| Warning {
            path: skipped.path,
            reason: WarningReason::Skipped(skipped.reason),
        })
        .chain(scanned.unstable.into_iter().map(|path| Warning {
            path,
            reason: WarningReason::Unstable,
        }))
        .collect();
    warnings.sort_by(|a, b| a.path.cmp(&b.path));
    for warning in &warnings {
        keep_base(&mut entries, &base, &warning.path);
    }
    Ok(WorkingState {
        state: State::working(root.to_path_buf(), entries),
        warnings,
    })
}

/// What [`refresh_working`] found besides the entries.
#[derive(Debug, Default)]
pub struct Refreshed {
    /// The paths the scan looked at; `None` when it scanned the whole workspace.
    pub scope: Option<Vec<RepoPath>>,
    pub skipped: Vec<Skipped>,
    /// Files whose size or modification time kept changing over three reads.
    pub unstable: Vec<RepoPath>,
    /// Files whose content was read and hashed.
    pub hashed: usize,
}

/// Brings `current`, the working entries an earlier call found in the workspace at `root`, up to date: the whole workspace when `touched` is `None`, else only the touched paths as [`scan::rescan_paths`] looks at them. A path the scan skips or finds unstable keeps what `base` has there, so a file that cannot be read never shows as deleted. The stat index is updated in place and not saved, and nothing is written.
pub fn refresh_working(
    root: &Path,
    index: &mut StatIndex,
    current: &mut FlatTree,
    base: &FlatTree,
    touched: Option<&[RepoPath]>,
    opts: &ScanOptions<'_>,
) -> Result<Refreshed, FoldError> {
    let opts = ScanOptions {
        recorded: Some(base),
        ..*opts
    };
    let scope = touched.and_then(scan::rescan_scope);
    let scanned = match &scope {
        Some(targets) => {
            let scanned = scan::rescan_targets(root, index, targets, &opts)?;
            for target in targets {
                remove_below(current, target);
            }
            current.extend(
                scanned
                    .entries
                    .iter()
                    .map(|(path, entry)| (path.clone(), *entry)),
            );
            scanned
        }
        None => {
            let scanned = scan::scan(root, index, &opts)?;
            current.clone_from(&scanned.entries);
            scanned
        }
    };
    let unrecorded = scanned
        .skipped
        .iter()
        .map(|skipped| &skipped.path)
        .chain(&scanned.unstable);
    for path in unrecorded {
        keep_base(current, base, path);
    }
    Ok(Refreshed {
        scope,
        skipped: scanned.skipped,
        unstable: scanned.unstable,
        hashed: scanned.hashed,
    })
}

/// The blob hashes of both sides of a changed file; `None` on the side where it does not exist.
type Pair = (Option<ObjectHash>, Option<ObjectHash>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Counted {
    added: u32,
    removed: u32,
    binary: bool,
}

/// Line counts of changed files by the blob hashes of both sides, so comparing a state again reads only the files whose content moved since. It holds at most `capacity` pairs; the oldest leaves first.
#[derive(Debug)]
pub struct LineCache {
    capacity: usize,
    counts: HashMap<Pair, Counted>,
    order: VecDeque<Pair>,
}

impl LineCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            counts: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.counts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    fn get(&self, pair: &Pair) -> Option<Counted> {
        self.counts.get(pair).copied()
    }

    fn put(&mut self, pair: Pair, counted: Counted) {
        if self.capacity == 0 || self.counts.insert(pair, counted).is_some() {
            return;
        }
        self.order.push_back(pair);
        while self.order.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.counts.remove(&oldest);
            }
        }
    }
}

/// The files that differ between `from` and `to`, with line counts read from both sides.
pub fn compare(objects: &ObjectStore, from: &State, to: &State) -> Result<Comparison, FoldError> {
    compare_with(objects, from, to, None)
}

/// [`compare`] that takes the line counts of a file from `cache` when it holds the file's pair of hashes, and keeps the counts it measures.
pub fn compare_cached(
    objects: &ObjectStore,
    from: &State,
    to: &State,
    cache: &mut LineCache,
) -> Result<Comparison, FoldError> {
    compare_with(objects, from, to, Some(cache))
}

fn compare_with(
    objects: &ObjectStore,
    from: &State,
    to: &State,
    mut cache: Option<&mut LineCache>,
) -> Result<Comparison, FoldError> {
    let mut comparison = Comparison::default();
    for found in classify(from.entries(), to.entries()) {
        let file = measure(objects, from, to, &found, cache.as_deref_mut())?;
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
    let too_large = too_large(objects, from, to, &found)?;
    let (old, new) = if too_large {
        (None, None)
    } else {
        both_sides(objects, from, to, &found)?
    };
    let text = (!too_large)
        .then(|| texts(old.as_deref(), new.as_deref()))
        .flatten();
    let hunks = match (found.change, text) {
        (ChangeKind::Renamed | ChangeKind::ModeChanged, _) | (_, None) => Vec::new(),
        (_, Some((old, new))) => diff_text(old, new, DEFAULT_CONTEXT),
    };
    Ok(FileDiff {
        path: found.path,
        old_path: found.renamed_from,
        change: found.change,
        binary: !too_large && text.is_none(),
        old_hash: found.old.map(|entry| entry.hash),
        new_hash: found.new.map(|entry| entry.hash),
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

/// A deleted and an added entry pair as a rename when they hold the same blob as the same kind: a symlink never pairs with a file of the same text. Empty files never pair, because any two of them hold the same blob without one coming from the other.
fn classify(from: &FlatTree, to: &FlatTree) -> Vec<Found> {
    type ByBlob<'t> = BTreeMap<(ObjectHash, EntryKind), Vec<(&'t Vec<u8>, FlatEntry)>>;
    let mut found = Vec::new();
    let mut deleted: ByBlob<'_> = BTreeMap::new();
    let mut added: ByBlob<'_> = BTreeMap::new();
    let empty = hash_object(ObjectKind::Blob, b"");
    for (path, new) in to {
        match from.get(path) {
            None => added
                .entry((new.hash, new.kind))
                .or_default()
                .push((path, *new)),
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
        deleted
            .entry((old.hash, old.kind))
            .or_default()
            .push((path, *old));
    }
    for (key, mut news) in added {
        let mut olds = deleted.remove(&key).unwrap_or_default();
        let paired = if key.0 == empty {
            0
        } else {
            news.len().min(olds.len())
        };
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
            deleted.insert(key, olds);
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
    cache: Option<&mut LineCache>,
) -> Result<FileChange, FoldError> {
    let pair = (
        found.old.map(|entry| entry.hash),
        found.new.map(|entry| entry.hash),
    );
    let counted = match cache {
        Some(cache) => match cache.get(&pair) {
            Some(counted) => counted,
            None => {
                let counted = count(objects, from, to, found)?;
                cache.put(pair, counted);
                counted
            }
        },
        None => count(objects, from, to, found)?,
    };
    Ok(FileChange {
        path: found.path.clone(),
        old_path: found.renamed_from.clone(),
        change: found.change,
        binary: counted.binary,
        old_hash: pair.0,
        new_hash: pair.1,
        added_lines: counted.added,
        removed_lines: counted.removed,
    })
}

/// A rename and a mode change keep the blob, so their counts depend on the pair alone, as every other kind's do. A side larger than [`MAX_INLINE_BYTES`] is not read, so such a file counts no lines and is not marked binary, as its diff shows no hunks.
fn count(
    objects: &ObjectStore,
    from: &State,
    to: &State,
    found: &Found,
) -> Result<Counted, FoldError> {
    if too_large(objects, from, to, found)? {
        return Ok(Counted {
            added: 0,
            removed: 0,
            binary: false,
        });
    }
    let (old, new) = both_sides(objects, from, to, found)?;
    let text = texts(old.as_deref(), new.as_deref());
    let (added, removed) = match (found.change, text) {
        (ChangeKind::Renamed | ChangeKind::ModeChanged, _) | (_, None) => (0, 0),
        (ChangeKind::Added, Some((_, new))) => (line_count(new), 0),
        (ChangeKind::Deleted, Some((old, _))) => (0, line_count(old)),
        (ChangeKind::Modified, Some((old, new))) => {
            let counts = count_lines(old, new);
            (counts.added, counts.removed)
        }
    };
    Ok(Counted {
        added,
        removed,
        binary: text.is_none(),
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

/// Whether either side holds more than [`MAX_INLINE_BYTES`], known from the blob's header or the working file's metadata before any content is read.
fn too_large(
    objects: &ObjectStore,
    from: &State,
    to: &State,
    found: &Found,
) -> Result<bool, FoldError> {
    let limit = MAX_INLINE_BYTES as u64;
    Ok(
        side_len(objects, from, found.old_path.as_ref(), found.old)? > limit
            || side_len(objects, to, Some(&found.path), found.new)? > limit,
    )
}

/// One side's content; `None` where the path does not exist.
type Side = Option<Vec<u8>>;

/// The content of both sides. A rename or a mode change keeps the blob, so only the new side is read and stands for both.
fn both_sides(
    objects: &ObjectStore,
    from: &State,
    to: &State,
    found: &Found,
) -> Result<(Side, Side), FoldError> {
    let new = side_content(objects, to, Some(&found.path), found.new)?;
    if matches!(found.change, ChangeKind::Renamed | ChangeKind::ModeChanged) {
        return Ok((new.clone(), new));
    }
    let old = side_content(objects, from, found.old_path.as_ref(), found.old)?;
    Ok((old, new))
}

/// A missing side, or a working file that went since the scan, is 0: reading it later reports the change.
fn side_len(
    objects: &ObjectStore,
    state: &State,
    path: Option<&RepoPath>,
    entry: Option<FlatEntry>,
) -> Result<u64, FoldError> {
    let (Some(path), Some(entry)) = (path, entry) else {
        return Ok(0);
    };
    if let Some(bytes) = state.blobs.get(&entry.hash) {
        return Ok(bytes.len() as u64);
    }
    if let Some(root) = &state.working
        && !objects.contains(&entry.hash).map_err(FoldError::from)?
    {
        let Some((dir, name)) = open_parent(root, path)? else {
            return Ok(0);
        };
        return match dir.metadata(name) {
            Ok(meta) => Ok(meta.len()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(WorkspaceError::from(error).into()),
        };
    }
    objects.blob_size(&entry.hash).map_err(FoldError::from)
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
    if let Some(bytes) = state.blobs.get(&entry.hash) {
        return Ok(bytes.clone());
    }
    if let Some(root) = &state.working
        && !objects.contains(&entry.hash).map_err(FoldError::from)?
    {
        return read_working(root, path, &entry.hash);
    }
    Ok(objects
        .read_expecting(&entry.hash, ObjectKind::Blob)
        .map_err(FoldError::from)?
        .payload)
}

/// A symlink gives its target text and is never followed. The content counts only when it still has the hash the scan recorded, so a file changed since never gives line counts for that hash. A file that went, changed, or grew past [`MAX_INLINE_BYTES`] since its size was checked is reported as changed, so the caller compares again; any other failure, such as a file it may not read, is returned as it is.
fn read_working(root: &Path, path: &RepoPath, expected: &ObjectHash) -> Result<Vec<u8>, FoldError> {
    let changed = || {
        FoldError::new(
            codes::WORKSPACE_IO,
            "a working file changed while it was compared; compare again",
        )
        .with_details([("path", json!(path.to_string()))])
    };
    let failed = |error: io::Error| FoldError::from(WorkspaceError::from(error));
    let Some((dir, name)) = open_parent(root, path)? else {
        return Err(changed());
    };
    let meta = match dir.metadata(name) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(changed()),
        Err(error) => return Err(failed(error)),
    };
    let (bytes, hash) = if meta.file_type().is_symlink() {
        let target = match dir.read_link(name) {
            Ok(Some(target)) => target,
            Ok(None) => return Err(changed()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(changed()),
            Err(error) => return Err(failed(error)),
        };
        let hash = hash_object(ObjectKind::Blob, &target);
        (target, hash)
    } else {
        match scan::read_stable(&dir, name, MAX_INLINE_BYTES as u64).map_err(failed)? {
            StableRead::Read(bytes, hash) => (bytes, hash),
            StableRead::Unstable | StableRead::Gone | StableRead::TooLarge => {
                return Err(changed());
            }
            StableRead::Denied => return Err(failed(io::ErrorKind::PermissionDenied.into())),
        }
    };
    if hash != *expected {
        return Err(changed());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::scan::{DEFAULT_MAX_FILE_BYTES, SkipReason, with_rewrites_during_reads};

    const HOST: Capabilities = Capabilities {
        symlinks: true,
        exec_bit: true,
        inodes: true,
        read_only_bit: true,
    };

    fn path(text: &str) -> RepoPath {
        RepoPath::parse(text.as_bytes()).unwrap()
    }

    fn names(entries: &FlatTree) -> Vec<String> {
        entries
            .keys()
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect()
    }

    struct Workspace {
        _dir: tempfile::TempDir,
        root: PathBuf,
        objects: ObjectStore,
        base: FlatTree,
    }

    /// A workspace holding `a.txt`, `big.bin` and `src/lib.rs`, whose base tree holds the same files.
    fn workspace() -> Workspace {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        fs::create_dir_all(root.join("src")).unwrap();
        let objects = ObjectStore::open(&dir.path().join(".fold")).unwrap();
        let mut base = FlatTree::new();
        for (name, content) in [
            ("a.txt", &b"one\n"[..]),
            ("big.bin", &[7u8; 64][..]),
            ("src/lib.rs", &b"fn a() {}\n"[..]),
        ] {
            fs::write(root.join(name), content).unwrap();
            let hash = objects.write(ObjectKind::Blob, content).unwrap();
            base.insert(
                name.as_bytes().to_vec(),
                FlatEntry {
                    kind: EntryKind::File,
                    hash,
                },
            );
        }
        Workspace {
            _dir: dir,
            root,
            objects,
            base,
        }
    }

    fn opts(rules: &IgnoreRules, max_file_bytes: u64) -> ScanOptions<'_> {
        ScanOptions {
            rules,
            max_file_bytes,
            caps: HOST,
            recorded: None,
        }
    }

    #[test]
    fn the_working_side_skips_the_files_over_the_given_limit() {
        let ws = workspace();
        let rules = IgnoreRules::new(&ws.root);
        let index_path = ws.root.with_extension("index");
        let mut index = StatIndex::load(&index_path, HOST);
        let recorded = crate::restore::record_checkpoint_tree(
            &ws.objects,
            &ws.root,
            &mut index,
            &index_path,
            None,
            &opts(&rules, DEFAULT_MAX_FILE_BYTES),
        )
        .unwrap();
        fs::write(ws.root.join("big.bin"), [9u8; 100]).unwrap();
        let big = b"big.bin".as_slice();
        let limited =
            working_state(&ws.objects, &ws.root, &index_path, &recorded.tree, HOST, 80).unwrap();
        assert_eq!(limited.state.entries()[big], ws.base[big]);
        assert!(limited.warnings.contains(&Warning {
            path: path("big.bin"),
            reason: WarningReason::Skipped(SkipReason::TooLarge),
        }));
        let open = working_state(
            &ws.objects,
            &ws.root,
            &index_path,
            &recorded.tree,
            HOST,
            DEFAULT_MAX_FILE_BYTES,
        )
        .unwrap();
        assert_ne!(open.state.entries()[big].hash, ws.base[big].hash);
        assert!(open.warnings.is_empty());
    }

    #[test]
    fn a_refresh_of_touched_paths_changes_only_those_paths() {
        let ws = workspace();
        let rules = IgnoreRules::new(&ws.root);
        let opts = opts(&rules, DEFAULT_MAX_FILE_BYTES);
        let mut index = StatIndex::empty(HOST);
        let mut current = FlatTree::new();
        let full =
            refresh_working(&ws.root, &mut index, &mut current, &ws.base, None, &opts).unwrap();
        assert_eq!(full.scope, None);
        assert_eq!(full.hashed, 3);
        assert_eq!(current, ws.base);

        fs::remove_file(ws.root.join("src/lib.rs")).unwrap();
        fs::write(ws.root.join("src/main.rs"), b"fn main() {}\n").unwrap();
        // An edit nobody reported stays unseen until a path names it.
        fs::write(ws.root.join("a.txt"), b"two\n").unwrap();
        let touched = [path("src")];
        let partial = refresh_working(
            &ws.root,
            &mut index,
            &mut current,
            &ws.base,
            Some(&touched),
            &opts,
        )
        .unwrap();
        assert_eq!(partial.scope, Some(vec![path("src")]));
        assert_eq!(names(&current), ["a.txt", "big.bin", "src/main.rs"]);
        assert_eq!(current[&b"a.txt"[..]], ws.base[&b"a.txt"[..]]);
    }

    #[test]
    fn a_moved_directory_beside_a_file_that_begins_with_its_name_leaves_no_old_entries() {
        let ws = workspace();
        fs::create_dir(ws.root.join("utils")).unwrap();
        fs::write(ws.root.join("utils/a.ts"), b"a\n").unwrap();
        fs::write(ws.root.join("utils/b.ts"), b"b\n").unwrap();
        fs::write(ws.root.join("utils.ts"), b"index\n").unwrap();
        let rules = IgnoreRules::new(&ws.root);
        let opts = opts(&rules, DEFAULT_MAX_FILE_BYTES);
        let mut index = StatIndex::empty(HOST);
        let mut current = FlatTree::new();
        refresh_working(&ws.root, &mut index, &mut current, &ws.base, None, &opts).unwrap();

        fs::rename(ws.root.join("utils"), ws.root.join("lib")).unwrap();
        let touched = [path("utils"), path("lib")];
        refresh_working(
            &ws.root,
            &mut index,
            &mut current,
            &ws.base,
            Some(&touched),
            &opts,
        )
        .unwrap();
        assert_eq!(
            names(&current),
            [
                "a.txt",
                "big.bin",
                "lib/a.ts",
                "lib/b.ts",
                "src/lib.rs",
                "utils.ts"
            ]
        );
    }

    #[test]
    fn a_touched_ignore_file_widens_the_refresh_to_its_folder() {
        let ws = workspace();
        let rules = IgnoreRules::new(&ws.root);
        let opts = opts(&rules, DEFAULT_MAX_FILE_BYTES);
        let mut index = StatIndex::empty(HOST);
        let mut current = FlatTree::new();
        refresh_working(&ws.root, &mut index, &mut current, &ws.base, None, &opts).unwrap();
        fs::write(ws.root.join("src/.gitignore"), b"lib.rs\n").unwrap();
        let touched = [path("src/.gitignore")];
        let found = refresh_working(
            &ws.root,
            &mut index,
            &mut current,
            &ws.base,
            Some(&touched),
            &opts,
        )
        .unwrap();
        assert_eq!(found.scope, Some(vec![path("src")]));
        assert_eq!(names(&current), ["a.txt", "big.bin", "src/.gitignore"]);

        fs::write(ws.root.join(".gitignore"), b"*.bin\n").unwrap();
        let root_rules = [path(".gitignore")];
        let whole = refresh_working(
            &ws.root,
            &mut index,
            &mut current,
            &ws.base,
            Some(&root_rules),
            &opts,
        )
        .unwrap();
        assert_eq!(whole.scope, None);
        assert_eq!(names(&current), [".gitignore", "a.txt", "src/.gitignore"]);
    }

    #[test]
    fn an_unreadable_file_keeps_its_base_entry_instead_of_vanishing() {
        let ws = workspace();
        let rules = IgnoreRules::new(&ws.root);
        let small = opts(&rules, 16);
        let mut index = StatIndex::empty(HOST);
        let mut current = FlatTree::new();
        fs::write(ws.root.join("big.bin"), [9u8; 128]).unwrap();
        let found =
            refresh_working(&ws.root, &mut index, &mut current, &ws.base, None, &small).unwrap();
        assert_eq!(found.skipped.len(), 1);
        assert_eq!(found.skipped[0].path, path("big.bin"));
        assert_eq!(found.skipped[0].reason, SkipReason::TooLarge);
        assert_eq!(current[&b"big.bin"[..]], ws.base[&b"big.bin"[..]]);

        let opts = opts(&rules, DEFAULT_MAX_FILE_BYTES);
        fs::write(ws.root.join("a.txt"), b"moving\n").unwrap();
        let touched = [path("a.txt")];
        let unstable = with_rewrites_during_reads("a.txt", || {
            refresh_working(
                &ws.root,
                &mut index,
                &mut current,
                &ws.base,
                Some(&touched),
                &opts,
            )
        })
        .unwrap();
        assert_eq!(unstable.unstable, [path("a.txt")]);
        assert_eq!(current[&b"a.txt"[..]], ws.base[&b"a.txt"[..]]);
    }

    #[test]
    fn a_cached_comparison_matches_and_skips_the_files_it_already_counted() {
        let ws = workspace();
        let rules = IgnoreRules::new(&ws.root);
        let opts = opts(&rules, DEFAULT_MAX_FILE_BYTES);
        let mut index = StatIndex::empty(HOST);
        let mut current = FlatTree::new();
        fs::write(ws.root.join("a.txt"), b"one\ntwo\n").unwrap();
        fs::write(ws.root.join("new.txt"), b"x\ny\nz\n").unwrap();
        refresh_working(&ws.root, &mut index, &mut current, &ws.base, None, &opts).unwrap();
        let from = State::recorded(ws.base.clone());
        let to = State::working(ws.root.clone(), current.clone());
        let mut cache = LineCache::new(8);
        assert!(cache.is_empty());
        let plain = compare(&ws.objects, &from, &to).unwrap();
        let cached = compare_cached(&ws.objects, &from, &to, &mut cache).unwrap();
        assert_eq!(cached, plain);
        assert_eq!(cache.len(), 2);
        assert_eq!(plain.totals.added_lines, 4);

        // With the counts cached, the working files are not read again, so deleting them does not fail the comparison.
        fs::remove_file(ws.root.join("a.txt")).unwrap();
        fs::remove_file(ws.root.join("new.txt")).unwrap();
        let again = compare_cached(&ws.objects, &from, &to, &mut cache).unwrap();
        assert_eq!(again, plain);
        assert!(compare(&ws.objects, &from, &to).is_err());
    }

    #[test]
    fn a_working_file_that_changed_since_the_scan_gives_no_counts() {
        let ws = workspace();
        let rules = IgnoreRules::new(&ws.root);
        let opts = opts(&rules, DEFAULT_MAX_FILE_BYTES);
        let mut index = StatIndex::empty(HOST);
        let mut current = FlatTree::new();
        fs::write(ws.root.join("new.txt"), b"x\n").unwrap();
        refresh_working(&ws.root, &mut index, &mut current, &ws.base, None, &opts).unwrap();
        let from = State::recorded(ws.base.clone());
        let to = State::working(ws.root.clone(), current.clone());
        let mut cache = LineCache::new(8);

        fs::write(ws.root.join("new.txt"), b"y\nz\n").unwrap();
        let changed = compare_cached(&ws.objects, &from, &to, &mut cache).unwrap_err();
        assert_eq!(changed.code(), codes::WORKSPACE_IO);
        assert!(changed.message().contains("compare again"));
        assert!(cache.is_empty());

        fs::write(ws.root.join("new.txt"), b"x\n").unwrap();
        fs::set_permissions(
            ws.root.join("new.txt"),
            std::os::unix::fs::PermissionsExt::from_mode(0o000),
        )
        .unwrap();
        let readable = fs::read(ws.root.join("new.txt")).is_ok();
        let denied = compare_cached(&ws.objects, &from, &to, &mut cache);
        fs::set_permissions(
            ws.root.join("new.txt"),
            std::os::unix::fs::PermissionsExt::from_mode(0o644),
        )
        .unwrap();
        if !readable {
            let denied = denied.unwrap_err();
            assert_eq!(denied.code(), codes::WORKSPACE_IO);
            assert!(!denied.message().contains("compare again"));
        }
        let counted = compare_cached(&ws.objects, &from, &to, &mut cache).unwrap();
        assert_eq!(counted.totals.added_lines, 1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_working_file_over_the_inline_size_is_never_read() {
        let ws = workspace();
        let rules = IgnoreRules::new(&ws.root);
        let opts = opts(&rules, DEFAULT_MAX_FILE_BYTES);
        let mut index = StatIndex::empty(HOST);
        let mut current = FlatTree::new();
        let lines = MAX_INLINE_BYTES / 2 + 1;
        let large = ws.root.join("large.txt");
        fs::write(&large, "x\n".repeat(lines)).unwrap();
        refresh_working(&ws.root, &mut index, &mut current, &ws.base, None, &opts).unwrap();
        let from = State::recorded(ws.base.clone());
        let to = State::working(ws.root.clone(), current);

        // A file nobody may read still compares and diffs, so neither looked at its content.
        fs::set_permissions(&large, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
        let counted = compare(&ws.objects, &from, &to);
        let diff = file_diff(&ws.objects, &from, &to, &path("large.txt"));
        fs::set_permissions(&large, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        let counted = counted.unwrap();
        assert_eq!(counted.totals.files, 1);
        assert_eq!(counted.totals.added_lines, 0);
        assert!(!counted.files[0].binary);
        let diff = diff.unwrap();
        assert!(diff.too_large && !diff.binary && diff.hunks.is_empty());

        // Once the file is small again its content no longer has the scanned hash.
        fs::write(&large, b"small\n").unwrap();
        let shrunk = compare(&ws.objects, &from, &to).unwrap_err();
        assert!(shrunk.message().contains("compare again"));
    }

    #[test]
    fn the_line_cache_drops_its_oldest_pair_when_full() {
        let hash = |n: u8| Some(ObjectHash::from_bytes([n; 32]));
        let counted = Counted {
            added: 1,
            removed: 0,
            binary: false,
        };
        let mut cache = LineCache::new(2);
        cache.put((None, hash(1)), counted);
        cache.put((None, hash(2)), counted);
        cache.put((None, hash(1)), counted);
        cache.put((None, hash(3)), counted);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(&(None, hash(1))), None);
        assert_eq!(cache.get(&(None, hash(3))), Some(counted));
        let mut none = LineCache::new(0);
        none.put((None, hash(1)), counted);
        assert!(none.is_empty());
    }
}
