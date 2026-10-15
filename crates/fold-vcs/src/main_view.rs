use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use fold_core::object::ObjectHash;
use fold_core::tree::{FlatEntry, FlatTree};
use fold_core::{FoldError, RevisionId};

use crate::compare::load_flat;
use crate::ignore::IgnoreRules;
use crate::index::StatIndex;
use crate::materialise::{Batch, MaterialiseOptions, Synced};
use crate::objects::{ObjectError, ObjectStore};
use crate::paths::{RepoPath, WorkspaceError, resolve};
use crate::platform::{Capabilities, DirHandle, sync_dir};
use crate::scan::{ChangeKind, ScanOptions, changes, scan};

/// The file in the project's `.fold` directory that names the revision `main/` last finished showing.
pub const MARKER: &str = "main-materialised";

#[derive(Debug, Default)]
pub struct Rewritten {
    pub written: Vec<RepoPath>,
    pub removed: Vec<RepoPath>,
    /// Symlink entries written as regular files, so the caller logs one warning.
    pub degraded: Vec<RepoPath>,
    pub synced: Synced,
}

/// Brings `main/` to `revision`, then moves the marker. `from` is the tree of the revision the marker names, `None` when it names none, in which case every file of `to` is written. Before any file changes the marker records the rewrite as `rN -> rM`, and it names `revision` alone only once the files are on disk. A rewrite that stopped part-way leaves paths of both revisions in `main/`, which no diff from one stored tree describes, so while the marker shows one, `from` is ignored, the rewrite starts from what `main/` holds, and every directory of `to` is returned to `0555` afterwards.
pub fn update(
    objects: &ObjectStore,
    root: &Path,
    fold_dir: &Path,
    from: Option<&ObjectHash>,
    to: &ObjectHash,
    revision: RevisionId,
    caps: Capabilities,
) -> Result<Rewritten, FoldError> {
    let marker = read_state(fold_dir).map_err(WorkspaceError::from)?;
    let new = load_flat(objects, to)?;
    let old = match (marker.rewriting, from) {
        (Some(_), _) => shown(root, &new, caps)?,
        (None, Some(tree)) => load_flat(objects, tree)?,
        (None, None) => FlatTree::new(),
    };
    write_rewriting(fold_dir, marker.shown, revision).map_err(WorkspaceError::from)?;
    let done = rewrite(objects, root, &old, &new, caps)?;
    if marker.rewriting.is_some() && caps.read_only_bit {
        seal(root, &new)?;
    }
    write_marker(fold_dir, revision).map_err(WorkspaceError::from)?;
    Ok(done)
}

/// Returns every directory on the way to a path of `to` to `0555`. A rewrite opens the directories of all its paths before it writes the first one, so one that stopped part-way leaves directories writable whose paths the repair finds already right and does not touch again.
fn seal(root: &Path, to: &FlatTree) -> Result<(), WorkspaceError> {
    match fs::symlink_metadata(root) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let dirs = parent_dirs(to.keys().map(Vec::as_slice));
    set_dir_modes(root, &dirs, |_| 0o555)
}

/// What `main/` holds, read as a tree: every file and symlink, hashed, with no ignore rule applied, because `main/` shows tracked files only. Where the platform has no executable bit a file keeps its kind from `to`. A path the scan cannot read has no entry, so the rewrite writes it again when `to` holds it.
fn shown(root: &Path, to: &FlatTree, caps: Capabilities) -> Result<FlatTree, WorkspaceError> {
    let rules = IgnoreRules::none();
    let opts = ScanOptions {
        rules: &rules,
        max_file_bytes: u64::MAX,
        caps,
        recorded: Some(to),
    };
    Ok(scan(root, &mut StatIndex::empty(caps), &opts)?.entries)
}

/// Rewrites only the paths whose content or kind differs between `from` and `to`: deleted paths are removed first, so a file and a directory can swap, then each changed file or symlink is staged under `main/.fold/tmp`, made read-only and renamed into place. Symlinks are created as recorded and never followed. Nothing is touched until every changed path is a valid path inside `root` and every blob it needs is stored. Each directory on the way to a changed path is made writable for the change and returned to `0555` afterwards where the platform has permission bits.
pub fn rewrite(
    objects: &ObjectStore,
    root: &Path,
    from: &FlatTree,
    to: &FlatTree,
    caps: Capabilities,
) -> Result<Rewritten, WorkspaceError> {
    let mut removals = Vec::new();
    let mut writes: Vec<(RepoPath, FlatEntry)> = Vec::new();
    for change in changes(from, to) {
        let path = RepoPath::parse(&change.path)?;
        match (change.kind, to.get(&change.path)) {
            (ChangeKind::Deleted, _) | (_, None) => removals.push(path),
            (ChangeKind::Added | ChangeKind::Modified, Some(entry)) => writes.push((path, *entry)),
        }
    }
    let removing: BTreeSet<&[u8]> = removals.iter().map(RepoPath::as_bytes).collect();
    for path in removals.iter().chain(writes.iter().map(|(path, _)| path)) {
        check_inside(root, path, &removing)?;
    }
    for (_, entry) in &writes {
        if !objects.contains(&entry.hash)? {
            return Err(ObjectError::Missing(entry.hash).into());
        }
    }
    let mut done = Rewritten::default();
    if removals.is_empty() && writes.is_empty() {
        return Ok(done);
    }
    fs::create_dir_all(root)?;
    let dirs = parent_dirs(
        removals
            .iter()
            .chain(writes.iter().map(|(path, _)| path))
            .map(RepoPath::as_bytes),
    );
    set_dir_modes(root, &dirs, |mode| mode | 0o700)?;
    let opts = MaterialiseOptions {
        read_only: true,
        caps,
    };
    let mut batch = Batch::open(root)?;
    for path in removals {
        batch.remove_entry(&path)?;
        done.removed.push(path);
    }
    for (path, entry) in writes {
        batch.remove_empty_dirs(&path)?;
        let written = batch.write_entry(objects, &path, &entry, &opts)?;
        if written.degraded {
            done.degraded.push(path.clone());
        }
        done.written.push(path);
        #[cfg(feature = "failpoints")]
        fail::fail_point!("main_view_after_write");
    }
    done.synced = batch.finish()?;
    if caps.read_only_bit {
        set_dir_modes(root, &dirs, |_| 0o555)?;
    }
    Ok(done)
}

/// Refuses `path` when one of its parents in `root` is anything but a real directory, a symlink included, unless the rewrite removes that parent first.
fn check_inside(
    root: &Path,
    path: &RepoPath,
    removing: &BTreeSet<&[u8]>,
) -> Result<(), WorkspaceError> {
    let bytes = path.as_bytes();
    let ends = bytes
        .iter()
        .enumerate()
        .filter(|&(_, &byte)| byte == b'/')
        .map(|(at, _)| at);
    for end in ends {
        let parent = &bytes[..end];
        if removing.contains(parent) {
            return Ok(());
        }
        match fs::symlink_metadata(resolve(root, &RepoPath::parse(parent)?)?) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(WorkspaceError::invalid(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// The directories on the way to each of `paths`, as slash-joined names below the root; the empty name is the root.
fn parent_dirs<'a>(paths: impl Iterator<Item = &'a [u8]>) -> BTreeSet<Vec<u8>> {
    let mut dirs = BTreeSet::from([Vec::new()]);
    for bytes in paths {
        for (at, _) in bytes.iter().enumerate().filter(|&(_, &byte)| byte == b'/') {
            dirs.insert(bytes[..at].to_vec());
        }
    }
    dirs
}

/// Applies `mode` to the permission bits of each of `dirs` that is a real directory. Each one is reached through handles opened from `root` without following a symlink, so one that a removal pruned, a file replaced or a symlink took the place of is skipped and its target never changes. Removing and replacing an entry needs write access to its directory only, so the read-only files themselves are left as they are.
fn set_dir_modes(
    root: &Path,
    dirs: &BTreeSet<Vec<u8>>,
    mode: impl Fn(u32) -> u32,
) -> Result<(), WorkspaceError> {
    let base = DirHandle::open(root)?;
    'dirs: for dir in dirs {
        let mut current = base.try_clone()?;
        for name in dir
            .split(|&byte| byte == b'/')
            .filter(|name| !name.is_empty())
        {
            match current.child(name) {
                Ok(Some(child)) => current = child,
                Ok(None) => continue 'dirs,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue 'dirs,
                Err(error) => return Err(error.into()),
            }
        }
        let old = current.mode()?;
        if mode(old) != old {
            current.set_mode(mode(old))?;
        }
    }
    Ok(())
}

/// What the marker in `fold_dir` says: the revision `main/` last finished showing, and the revision of a rewrite that began after it and has not finished.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MarkerState {
    pub shown: Option<RevisionId>,
    pub rewriting: Option<RevisionId>,
}

/// Reads the marker in `fold_dir`. A missing marker, or a part that names no revision, gives `None` for that part.
pub fn read_state(fold_dir: &Path) -> io::Result<MarkerState> {
    let text = match fs::read_to_string(fold_dir.join(MARKER)) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(MarkerState::default()),
        Err(error) => return Err(error),
    };
    Ok(match text.trim().split_once("->") {
        Some((shown, rewriting)) => MarkerState {
            shown: shown.trim().parse().ok(),
            rewriting: rewriting.trim().parse().ok(),
        },
        None => MarkerState {
            shown: text.trim().parse().ok(),
            rewriting: None,
        },
    })
}

/// The revision `main/` last finished showing, which a rewrite that stopped part-way leaves in place. A missing marker, or one that names no revision, is `None`.
pub fn read_marker(fold_dir: &Path) -> io::Result<Option<RevisionId>> {
    Ok(read_state(fold_dir)?.shown)
}

/// Records that `main/` shows `revision`.
pub fn write_marker(fold_dir: &Path, revision: RevisionId) -> io::Result<()> {
    replace_marker(fold_dir, &format!("{revision}\n"))
}

/// Records that a rewrite from `shown` to `revision` begins.
fn write_rewriting(
    fold_dir: &Path,
    shown: Option<RevisionId>,
    revision: RevisionId,
) -> io::Result<()> {
    let shown = shown.map(|shown| shown.to_string()).unwrap_or_default();
    replace_marker(fold_dir, &format!("{shown} -> {revision}\n"))
}

/// Replaces the marker in one rename, so a reader sees the old text or the new one and never a partial file.
fn replace_marker(fold_dir: &Path, text: &str) -> io::Result<()> {
    let marker = fold_dir.join(MARKER);
    let tmp = fold_dir.join(format!("{MARKER}.tmp"));
    let mut file = File::create(&tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(&tmp, &marker)?;
    sync_dir(fold_dir)
}
