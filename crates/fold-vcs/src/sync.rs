use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use fold_core::merge::{
    Conflict, ConflictKind, TreeMerge, contains_marker_line, merge_trees, merge3, render_markers,
};
use fold_core::object::{ObjectHash, ObjectKind, hash_object};
use fold_core::tree::{EntryKind, FlatEntry, FlatTree, flatten};
use fold_core::{FoldError, RevisionId, codes};

use crate::index::StatIndex;
use crate::materialise::{Batch, MaterialiseOptions};
use crate::objects::ObjectStore;
use crate::paths::{RepoPath, WorkspaceError, os_bytes, os_path, resolve};
use crate::platform::is_executable;
use crate::restore::remove_empty_dirs;
use crate::scan::{Reading, hash_stable, open_regular};
use crate::snapshot::load_tree;

/// Main's side of a conflict is written beside the conflicted path under this suffix.
pub const SIDECAR_SUFFIX: &[u8] = b".fold-main";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncInput {
    /// The tree of the workspace's base revision.
    pub base_tree: ObjectHash,
    /// The automatic checkpoint the calling command takes immediately before the sync, so the merge reads recorded objects and never half-written files.
    pub ours_tree: ObjectHash,
    pub main_tree: ObjectHash,
    pub base_number: RevisionId,
    pub main_number: RevisionId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    pub new_base: RevisionId,
    /// In path order.
    pub conflicted: Vec<ConflictedPath>,
    /// Files written or removed in the workspace, sidecars included.
    pub written: usize,
    /// True when the base already was main; nothing was read or written.
    pub already_current: bool,
}

/// What the service stores for each conflict until the author resolves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictedPath {
    pub path: RepoPath,
    pub kind: ConflictKind,
}

/// The three labels of the conflict markers. [`contains_marker_line`] recognises the lines `<<<<<<< workspace…`, `||||||| base r<n>` and `>>>>>>> main r<n>`, so [`unresolved`] sees a marker only when at least one label keeps that form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerLabels {
    pub ours: String,
    pub base: String,
    pub theirs: String,
}

impl MarkerLabels {
    pub fn sync(base: RevisionId, main: RevisionId) -> Self {
        Self {
            ours: "workspace".into(),
            base: format!("base {base}"),
            theirs: format!("main {main}"),
        }
    }
}

/// `<path>.fold-main`, where main's side of a conflict at `path` is written.
pub fn sidecar(path: &RepoPath) -> Result<RepoPath, WorkspaceError> {
    let mut bytes = path.as_bytes().to_vec();
    bytes.extend_from_slice(SIDECAR_SUFFIX);
    RepoPath::parse(&bytes)
}

/// Brings main into the workspace at `root` with a three-way merge of the base, the checkpoint and main. Blocking: call it through `spawn_blocking`. The stat index is updated in memory for every written or removed file; the caller saves it after this returns, when every write is on disk.
pub fn sync(
    objects: &ObjectStore,
    root: &Path,
    index: &mut StatIndex,
    input: SyncInput,
) -> Result<SyncOutcome, FoldError> {
    if input.base_number == input.main_number {
        return Ok(SyncOutcome {
            new_base: input.main_number,
            conflicted: Vec::new(),
            written: 0,
            already_current: true,
        });
    }
    let load = |hash: &ObjectHash| load_tree(objects, hash);
    let base = flatten(&input.base_tree, &load)?;
    let ours = flatten(&input.ours_tree, &load)?;
    let main = flatten(&input.main_tree, &load)?;
    let blob = |hash: &ObjectHash| read_blob(objects, hash);
    let merge = merge_trees(&base, &ours, &main, &blob)?;
    let labels = MarkerLabels::sync(input.base_number, input.main_number);
    let applied = apply(objects, root, index, &merge, &ours, &labels)?;
    Ok(SyncOutcome {
        new_base: input.main_number,
        conflicted: applied.conflicted,
        written: applied.written,
        already_current: false,
    })
}

/// Writes a finished merge into the workspace at `root`, where `ours_tree` is the recorded state the merge took as its workspace side. Clean results are written, a text conflict gets markers with `labels`, and every other conflict keeps the workspace's version and gets main's side as `<path>.fold-main`, a file or a directory. Before anything is written, each path that will be overwritten or removed must still match `ours_tree`; otherwise nothing is written and the call fails with `workspace_changed_during_sync`. Every other path, recorded or not, is left alone.
pub fn write_merge(
    objects: &ObjectStore,
    root: &Path,
    index: &mut StatIndex,
    merge: &TreeMerge,
    ours_tree: &ObjectHash,
    labels: &MarkerLabels,
) -> Result<Vec<ConflictedPath>, FoldError> {
    let ours = flatten(ours_tree, &|hash| load_tree(objects, hash))?;
    Ok(apply(objects, root, index, merge, &ours, labels)?.conflicted)
}

/// The conflicted paths that still hold a marker line or whose `<path>.fold-main` still exists, in the order given.
pub fn unresolved(
    root: &Path,
    conflicted: &[ConflictedPath],
) -> Result<Vec<RepoPath>, WorkspaceError> {
    let mut open = Vec::new();
    for conflict in conflicted {
        if exists(root, &sidecar(&conflict.path)?)? || holds_marker(root, &conflict.path)? {
            open.push(conflict.path.clone());
        }
    }
    Ok(open)
}

struct Applied {
    conflicted: Vec<ConflictedPath>,
    written: usize,
}

#[derive(Default)]
struct Plan {
    removals: BTreeSet<RepoPath>,
    /// Writes over or into a path of the workspace side, each checked against `ours`.
    writes: Vec<(RepoPath, FlatEntry)>,
    /// Sidecar roots, cleared of whatever an earlier sync left before main's side is written there.
    sidecars: Vec<RepoPath>,
    sidecar_writes: Vec<(RepoPath, FlatEntry)>,
    conflicted: Vec<ConflictedPath>,
}

fn apply(
    objects: &ObjectStore,
    root: &Path,
    index: &mut StatIndex,
    merge: &TreeMerge,
    ours: &FlatTree,
    labels: &MarkerLabels,
) -> Result<Applied, FoldError> {
    for (_, bytes) in &merge.new_blobs {
        objects.write(ObjectKind::Blob, bytes)?;
    }
    let plan = plan(objects, merge, ours, labels)?;
    check(root, index, ours, &plan)?;

    let opts = MaterialiseOptions {
        read_only: false,
        caps: *index.capabilities(),
    };
    let mut batch = Batch::open(root)?;
    let mut written = 0;
    for path in &plan.removals {
        batch.remove_entry(path)?;
        index.remove(path);
        written += 1;
    }
    for path in &plan.sidecars {
        batch.remove_tree(path)?;
        let stale: Vec<RepoPath> = index
            .iter()
            .map(|(found, _)| found)
            .filter(|found| at_or_below(found.as_bytes(), path.as_bytes()))
            .cloned()
            .collect();
        for found in stale {
            index.remove(&found);
        }
    }
    for (path, entry) in plan.writes.iter().chain(&plan.sidecar_writes) {
        let dest = resolve(root, path)?;
        if metadata(&dest)
            .map_err(WorkspaceError::from)?
            .is_some_and(|meta| meta.is_dir())
        {
            remove_empty_dirs(&dest).map_err(WorkspaceError::from)?;
        }
        let done = batch.write_entry(objects, path, entry, &opts)?;
        index.upsert(path.clone(), done.stat);
        written += 1;
    }
    batch.finish()?;
    Ok(Applied {
        conflicted: plan.conflicted,
        written,
    })
}

fn plan(
    objects: &ObjectStore,
    merge: &TreeMerge,
    ours: &FlatTree,
    labels: &MarkerLabels,
) -> Result<Plan, FoldError> {
    let conflict_paths: BTreeSet<&[u8]> =
        merge.conflicts.iter().map(|c| c.path.as_slice()).collect();
    let in_conflict = |path: &[u8]| {
        conflict_paths.contains(path)
            || path
                .iter()
                .enumerate()
                .any(|(at, byte)| *byte == b'/' && conflict_paths.contains(&path[..at]))
    };
    let mut plan = Plan::default();
    for path in ours.keys() {
        if !merge.merged.contains_key(path) && !in_conflict(path) {
            plan.removals.insert(RepoPath::parse(path)?);
        }
    }
    for (path, entry) in &merge.merged {
        if ours.get(path) != Some(entry) {
            plan.writes.push((RepoPath::parse(path)?, *entry));
        }
    }
    for conflict in &merge.conflicts {
        let path = RepoPath::parse(&conflict.path)?;
        plan.conflicted.push(ConflictedPath {
            path: path.clone(),
            kind: conflict.kind,
        });
        if conflict.kind == ConflictKind::BothModifiedText {
            let entry = render(objects, conflict, labels)?;
            plan.writes.push((path, entry));
            continue;
        }
        let side = sidecar(&path)?;
        if let Some(theirs) = conflict.theirs {
            plan.sidecars.push(side.clone());
            plan.sidecar_writes.push((side, theirs));
        } else if !conflict.theirs_below.is_empty() {
            for (relative, entry) in &conflict.theirs_below {
                let mut bytes = side.as_bytes().to_vec();
                bytes.push(b'/');
                bytes.extend_from_slice(relative);
                plan.sidecar_writes.push((RepoPath::parse(&bytes)?, *entry));
            }
            plan.sidecars.push(side);
        }
    }
    // Main may track a path where a sidecar goes; the sidecar takes it, so the batch never writes a file and a directory at one path.
    let sidecars = &plan.sidecars;
    plan.writes.retain(|(path, _)| {
        !sidecars
            .iter()
            .any(|side| at_or_below(path.as_bytes(), side.as_bytes()))
    });
    Ok(plan)
}

/// The workspace's file with fold's markers around each hunk, stored as a blob so it can be written like any entry. It keeps the workspace's mode.
fn render(
    objects: &ObjectStore,
    conflict: &Conflict,
    labels: &MarkerLabels,
) -> Result<FlatEntry, FoldError> {
    let invalid = || FoldError::new(codes::TREE_INVALID, "a text conflict lacks a text side");
    let text = |entry: Option<FlatEntry>| -> Result<String, FoldError> {
        let entry = entry.ok_or_else(invalid)?;
        String::from_utf8(read_blob(objects, &entry.hash)?).map_err(|_| invalid())
    };
    let merged = merge3(
        &text(conflict.base)?,
        &text(conflict.ours)?,
        &text(conflict.theirs)?,
    );
    let rendered = render_markers(&merged, &labels.ours, &labels.base, &labels.theirs);
    let hash = objects.write(ObjectKind::Blob, rendered.as_bytes())?;
    Ok(FlatEntry {
        kind: merged_mode(conflict),
        hash,
    })
}

/// The tree merge's mode rule: the side that moved off the base mode wins.
fn merged_mode(conflict: &Conflict) -> EntryKind {
    let kind = |entry: Option<FlatEntry>| entry.map(|entry| entry.kind);
    match (
        kind(conflict.base),
        kind(conflict.ours),
        kind(conflict.theirs),
    ) {
        (base, Some(ours), Some(theirs)) if base == Some(ours) => theirs,
        (_, Some(ours), _) => ours,
        _ => EntryKind::File,
    }
}

/// Collects every planned path whose disk state no longer matches what the merge assumed, and refuses the whole sync when there is one.
fn check(
    root: &Path,
    index: &StatIndex,
    ours: &FlatTree,
    plan: &Plan,
) -> Result<(), WorkspaceError> {
    let mut changed = Vec::new();
    for path in &plan.removals {
        if let Some(expected) = ours.get(path.as_bytes())
            && !matches(root, index, path, expected)?
        {
            changed.push(path.to_string());
        }
    }
    for (path, _) in &plan.writes {
        let fits = match ours.get(path.as_bytes()) {
            Some(expected) => matches(root, index, path, expected)?,
            None => clear_to_write(root, path, &plan.removals)?,
        };
        if !fits {
            changed.push(path.to_string());
        }
    }
    for path in &plan.sidecars {
        if !parents_clear(root, path, &plan.removals)?.unwrap_or(true) {
            changed.push(path.to_string());
        }
        unrecorded_at(root, index, ours, path, &mut changed)?;
    }
    changed.sort();
    changed.dedup();
    if changed.is_empty() {
        Ok(())
    } else {
        Err(WorkspaceError::ChangedDuringSync(changed))
    }
}

/// True when the file or symlink at `path` has the recorded kind and content: from the stat index while it is fresh, otherwise by hashing it again.
fn matches(
    root: &Path,
    index: &StatIndex,
    path: &RepoPath,
    expected: &FlatEntry,
) -> Result<bool, WorkspaceError> {
    let Some(located) = locate(root, path)? else {
        return Ok(false);
    };
    let Some(meta) = metadata(&located)? else {
        return Ok(false);
    };
    let caps = index.capabilities();
    let link = meta.file_type().is_symlink();
    let kind = if link {
        EntryKind::Symlink
    } else if !meta.is_file() {
        return Ok(false);
    } else if expected.kind == EntryKind::Symlink && !caps.symlinks {
        EntryKind::Symlink
    } else if is_executable(&meta, expected.kind, caps) {
        EntryKind::Executable
    } else {
        EntryKind::File
    };
    if kind != expected.kind {
        return Ok(false);
    }
    if let Some(hash) = index.is_fresh(path, &meta) {
        return Ok(hash == expected.hash);
    }
    let hash = if link {
        let target = fs::read_link(&located)?;
        let Some(bytes) = os_bytes(target.as_os_str()) else {
            return Ok(false);
        };
        hash_object(ObjectKind::Blob, &bytes)
    } else {
        match hash_stable(&located)? {
            Reading::Hashed(hash, _) => hash,
            Reading::Unstable | Reading::Gone | Reading::Denied => return Ok(false),
        }
    };
    Ok(hash == expected.hash)
}

/// Adds to `changed` everything at or below a sidecar root that `ours` does not record with the same content. A sidecar from an earlier sync is in the checkpoint, so replacing it loses nothing; anything else there, a file written after the checkpoint or an ignored one, would be lost. Empty directories hold nothing to lose.
fn unrecorded_at(
    root: &Path,
    index: &StatIndex,
    ours: &FlatTree,
    path: &RepoPath,
    changed: &mut Vec<String>,
) -> Result<(), WorkspaceError> {
    let Some(located) = locate(root, path)? else {
        return Ok(());
    };
    let Some(meta) = metadata(&located)? else {
        return Ok(());
    };
    if !meta.is_dir() {
        let recorded = match ours.get(path.as_bytes()) {
            Some(expected) => matches(root, index, path, expected)?,
            None => false,
        };
        if !recorded {
            changed.push(path.to_string());
        }
        return Ok(());
    }
    for entry in fs::read_dir(&located)? {
        let entry = entry?;
        match os_bytes(&entry.file_name()).and_then(|name| RepoPath::child(Some(path), &name).ok())
        {
            Some(child) => unrecorded_at(root, index, ours, &child, changed)?,
            None => changed.push(path.to_string()),
        }
    }
    Ok(())
}

/// True when writing a path the workspace side does not have destroys nothing: nothing is there, or only a directory that holds nothing but planned removals and empty directories, and no parent is a file that stays.
fn clear_to_write(
    root: &Path,
    path: &RepoPath,
    removals: &BTreeSet<RepoPath>,
) -> Result<bool, WorkspaceError> {
    match parents_clear(root, path, removals)? {
        Some(clear) => Ok(clear),
        None => {
            let located = root.join(os_path(path)?);
            match metadata(&located)? {
                None => Ok(true),
                Some(meta) if meta.is_dir() => Ok(only_removals_below(&located, path, removals)?),
                Some(_) => Ok(false),
            }
        }
    }
}

/// Walks the parents of `path` from the root. `Some` settles the answer: `Some(true)` when a parent is missing or is a file the plan removes, so nothing exists at `path`; `Some(false)` when a parent is a file that stays. `None` when every parent is a real directory.
fn parents_clear(
    root: &Path,
    path: &RepoPath,
    removals: &BTreeSet<RepoPath>,
) -> Result<Option<bool>, WorkspaceError> {
    let bytes = path.as_bytes();
    for (at, _) in bytes.iter().enumerate().filter(|(_, byte)| **byte == b'/') {
        let parent = RepoPath::parse(&bytes[..at])?;
        match metadata(&root.join(os_path(&parent)?))? {
            None => return Ok(Some(true)),
            Some(meta) if meta.is_dir() => {}
            Some(_) => return Ok(Some(removals.contains(&parent))),
        }
    }
    Ok(None)
}

/// True when every file below the directory is a planned removal, so only empty directories stay there once they are gone.
fn only_removals_below(
    dir: &Path,
    prefix: &RepoPath,
    removals: &BTreeSet<RepoPath>,
) -> Result<bool, WorkspaceError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let Some(name) = os_bytes(&entry.file_name()) else {
            return Ok(false);
        };
        let Ok(child) = RepoPath::child(Some(prefix), &name) else {
            return Ok(false);
        };
        let removed = if entry.file_type()?.is_dir() {
            only_removals_below(&entry.path(), &child, removals)?
        } else {
            removals.contains(&child)
        };
        if !removed {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The path under `root`, or `None` when a parent is not a real directory, which means nothing recorded can be there.
fn locate(root: &Path, path: &RepoPath) -> Result<Option<PathBuf>, WorkspaceError> {
    match resolve(root, path) {
        Ok(located) => Ok(Some(located)),
        Err(WorkspaceError::InvalidPath(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

fn metadata(located: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(located) {
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn exists(root: &Path, path: &RepoPath) -> Result<bool, WorkspaceError> {
    match locate(root, path)? {
        Some(located) => Ok(metadata(&located)?.is_some()),
        None => Ok(false),
    }
}

fn holds_marker(root: &Path, path: &RepoPath) -> Result<bool, WorkspaceError> {
    let Some(located) = locate(root, path)? else {
        return Ok(false);
    };
    let opened = match open_regular(&located) {
        Ok(opened) => opened,
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let Some((mut file, _)) = opened else {
        return Ok(false);
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(contains_marker_line(&String::from_utf8_lossy(&bytes)))
}

fn read_blob(objects: &ObjectStore, hash: &ObjectHash) -> Result<Vec<u8>, FoldError> {
    Ok(objects.read_expecting(hash, ObjectKind::Blob)?.payload)
}

fn at_or_below(path: &[u8], prefix: &[u8]) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(b"/"))
}
