use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::tree::{EntryKind, FlatEntry, Tree, TreeError};

use crate::index::{StatEntry, StatIndex};
use crate::objects::ObjectStore;
use crate::paths::{RepoPath, WorkspaceError, resolve};
use crate::platform::{Capabilities, SymlinkOutcome, create_symlink, set_mode, sync_dir};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Past this many written files the open handles are flushed early, so a large tree never exhausts file descriptors.
const SYNC_AHEAD: usize = 256;

#[derive(Debug, Clone, Copy)]
pub struct MaterialiseOptions {
    /// Clears the write bits of every file, for `main/` and review checkouts.
    pub read_only: bool,
    pub caps: Capabilities,
}

#[derive(Debug)]
pub struct Written {
    pub stat: StatEntry,
    /// True when a symlink entry was written as a regular file.
    pub degraded: bool,
}

/// What a batch flushed: files whose data was synced and directories whose entries were synced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Synced {
    pub files: usize,
    pub dirs: usize,
}

#[derive(Debug)]
pub struct Materialised {
    pub index: StatIndex,
    /// Symlink entries written as regular files, so the caller logs one warning.
    pub degraded: Vec<RepoPath>,
    pub synced: Synced,
}

/// Writes the whole tree into `root`, which must not exist or must be empty. Everything written is on disk when this returns, so the caller can save the returned index. A failure part-way leaves the directory in place and returns the error.
pub fn materialise(
    objects: &ObjectStore,
    tree: &ObjectHash,
    root: &Path,
    opts: &MaterialiseOptions,
) -> Result<Materialised, WorkspaceError> {
    prepare_target(root)?;
    let mut batch = Batch::open(root)?;
    batch.create_dirs(root)?;
    let mut done = Materialised {
        index: StatIndex::empty(opts.caps),
        degraded: Vec::new(),
        synced: Synced::default(),
    };
    write_tree(objects, tree, None, opts, &mut batch, &mut done)?;
    done.synced = batch.finish()?;
    Ok(done)
}

fn prepare_target(root: &Path) -> Result<(), WorkspaceError> {
    match fs::read_dir(root) {
        Ok(mut entries) => {
            if entries.next().transpose()?.is_some() {
                return Err(WorkspaceError::TargetNotEmpty(
                    root.to_string_lossy().into_owned(),
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn write_tree(
    objects: &ObjectStore,
    hash: &ObjectHash,
    prefix: Option<&RepoPath>,
    opts: &MaterialiseOptions,
    batch: &mut Batch,
    done: &mut Materialised,
) -> Result<(), WorkspaceError> {
    let tree = Tree::decode(&objects.read_expecting(hash, ObjectKind::Tree)?.payload)?;
    for entry in tree.entries {
        let path = RepoPath::child(prefix, &entry.name)?;
        if entry.kind == EntryKind::Directory {
            let dir = resolve(&batch.root, &path)?;
            batch.create_dirs(&dir)?;
            write_tree(objects, &entry.hash, Some(&path), opts, batch, done)?;
            continue;
        }
        let flat = FlatEntry {
            kind: entry.kind,
            hash: entry.hash,
        };
        let written = batch.write_entry(objects, &path, &flat, opts)?;
        if written.degraded {
            done.degraded.push(path.clone());
        }
        done.index.upsert(path, written.stat);
    }
    Ok(())
}

/// Writes one file or symlink entry as a batch of its own; see [`Batch::write_entry`].
pub fn write_entry(
    objects: &ObjectStore,
    root: &Path,
    path: &RepoPath,
    entry: &FlatEntry,
    opts: &MaterialiseOptions,
) -> Result<Written, WorkspaceError> {
    let mut batch = Batch::open(root)?;
    let written = batch.write_entry(objects, path, entry, opts)?;
    batch.finish()?;
    Ok(written)
}

/// Removes one entry as a batch of its own; see [`Batch::remove_entry`].
pub fn remove_entry(root: &Path, path: &RepoPath) -> Result<(), WorkspaceError> {
    let mut batch = Batch::open(root)?;
    batch.remove_entry(path)?;
    batch.finish()?;
    Ok(())
}

/// The writes and removals of one create, sync or restore in one workspace, flushed together by [`Batch::finish`]: the written files first, then every directory whose entries changed. The caller saves the stat index only after `finish`. One batch runs per workspace at a time, because opening a batch removes whatever an interrupted one left in the staging directory `<root>/.fold/tmp/`.
#[derive(Debug)]
pub struct Batch {
    root: PathBuf,
    unsynced: Vec<File>,
    dirs: BTreeSet<PathBuf>,
    synced: Synced,
}

impl Batch {
    pub fn open(root: &Path) -> Result<Self, WorkspaceError> {
        if staging_ready(root, false)? {
            sweep(&staging_dir(root))?;
        }
        Ok(Self {
            root: root.to_path_buf(),
            unsynced: Vec::new(),
            dirs: BTreeSet::new(),
            synced: Synced::default(),
        })
    }

    /// The content is staged in the staging directory and renamed into place, so no temp name appears in the tracked tree and an existing symlink at the path is replaced, never written through. Missing parent directories are created.
    pub fn write_entry(
        &mut self,
        objects: &ObjectStore,
        path: &RepoPath,
        entry: &FlatEntry,
        opts: &MaterialiseOptions,
    ) -> Result<Written, WorkspaceError> {
        if entry.kind == EntryKind::Directory {
            return Err(TreeError::DirectoryInFlatMap.into());
        }
        let dest = resolve(&self.root, path)?;
        let parent = dest.parent().unwrap_or(&self.root).to_path_buf();
        self.create_dirs(&parent)?;
        staging_ready(&self.root, true)?;
        let tmp = staging_dir(&self.root).join(format!(
            "{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let staged = stage(objects, &tmp, entry, opts).and_then(|staged| {
            fs::rename(&tmp, &dest)?;
            Ok(staged)
        });
        let (outcome, file) = match staged {
            Ok(staged) => staged,
            Err(error) => {
                let _ = fs::remove_file(&tmp);
                return Err(error);
            }
        };
        if let Some(file) = file {
            self.hold(file)?;
        }
        let meta = fs::symlink_metadata(&dest)?;
        Ok(Written {
            stat: StatEntry::from_metadata(&meta, entry.kind, entry.hash, &opts.caps),
            degraded: outcome == SymlinkOutcome::Emulated,
        })
    }

    /// Removes the file or symlink at `path`, never following a link, then any parent directory it leaves empty. The workspace root itself is never removed. A path that is already absent is not an error.
    pub fn remove_entry(&mut self, path: &RepoPath) -> Result<(), WorkspaceError> {
        let dest = resolve(&self.root, path)?;
        match fs::symlink_metadata(&dest) {
            Ok(meta) if meta.is_dir() => return Err(WorkspaceError::invalid(path.as_bytes())),
            Ok(_) => fs::remove_file(&dest)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        let mut changed = dest.parent();
        while let Some(dir) = changed {
            self.dirs.insert(dir.to_path_buf());
            if dir == self.root || fs::remove_dir(dir).is_err() {
                break;
            }
            changed = dir.parent();
        }
        Ok(())
    }

    /// A changed directory that a later removal pruned is skipped.
    pub fn finish(mut self) -> Result<Synced, WorkspaceError> {
        self.sync_files()?;
        for dir in &self.dirs {
            match sync_dir(dir) {
                Ok(()) => self.synced.dirs += 1,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(self.synced)
    }

    /// Creates `dir` and its missing ancestors. `dir` gains an entry, and so does the parent of each directory created.
    fn create_dirs(&mut self, dir: &Path) -> io::Result<()> {
        self.dirs.insert(dir.to_path_buf());
        let mut current = dir;
        while fs::symlink_metadata(current).is_err() {
            let Some(parent) = current.parent() else {
                break;
            };
            self.dirs.insert(parent.to_path_buf());
            current = parent;
        }
        fs::create_dir_all(dir)
    }

    fn hold(&mut self, file: File) -> io::Result<()> {
        self.unsynced.push(file);
        if self.unsynced.len() >= SYNC_AHEAD {
            self.sync_files()?;
        }
        Ok(())
    }

    fn sync_files(&mut self) -> io::Result<()> {
        for file in self.unsynced.drain(..) {
            file.sync_all()?;
            self.synced.files += 1;
        }
        Ok(())
    }
}

fn staging_dir(root: &Path) -> PathBuf {
    root.join(".fold").join("tmp")
}

/// Whether `<root>/.fold/tmp` exists, creating it when `create`. A level that is anything but a real directory is refused, so a planted link never redirects staging or the sweep.
fn staging_ready(root: &Path, create: bool) -> Result<bool, WorkspaceError> {
    let mut dir = root.to_path_buf();
    for name in [".fold", "tmp"] {
        dir.push(name);
        match fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(WorkspaceError::invalid(b".fold/tmp")),
            Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
                fs::create_dir(&dir)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(true)
}

fn sweep(staging: &Path) -> io::Result<()> {
    for entry in fs::read_dir(staging)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// Returns the handle to sync at the end of the batch: the staged file, or none for a real symlink, whose directory entry is all there is to flush.
fn stage(
    objects: &ObjectStore,
    tmp: &Path,
    entry: &FlatEntry,
    opts: &MaterialiseOptions,
) -> Result<(SymlinkOutcome, Option<File>), WorkspaceError> {
    let mut blob = objects.open_blob(&entry.hash)?;
    if entry.kind == EntryKind::Symlink {
        let mut target = Vec::new();
        blob.read_to_end(&mut target)?;
        let outcome = create_symlink(&target, tmp, &opts.caps)?;
        if outcome == SymlinkOutcome::Created {
            return Ok((outcome, None));
        }
        set_mode(tmp, false, opts.read_only, &opts.caps)?;
        return Ok((outcome, Some(File::open(tmp)?)));
    }
    let mut file = OpenOptions::new().write(true).create_new(true).open(tmp)?;
    io::copy(&mut blob, &mut file)?;
    set_mode(
        tmp,
        entry.kind == EntryKind::Executable,
        opts.read_only,
        &opts.caps,
    )?;
    Ok((SymlinkOutcome::Created, Some(file)))
}
