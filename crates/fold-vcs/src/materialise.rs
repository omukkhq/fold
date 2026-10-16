use std::collections::BTreeSet;
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::tree::{EntryKind, FlatEntry, Tree, TreeError};

use crate::index::{StatEntry, StatIndex};
use crate::objects::ObjectStore;
use crate::paths::{RepoPath, WorkspaceError};
use crate::platform::{Capabilities, DirHandle, SymlinkOutcome, set_file_mode, sync_dir};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Past this many written files the open handles are flushed early, so a large tree never exhausts file descriptors.
const SYNC_AHEAD: usize = 256;

#[derive(Debug, Clone, Copy)]
pub struct MaterialiseOptions {
    /// Clears the write bits of every file, for review checkouts.
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

/// Writes the whole tree into `root`, which must not exist or must be empty. Everything written is on disk when this returns, so the caller can save the returned index. Its freshness boundary is taken after the last write, so a file edited in the same clock tick is hashed again. A failure part-way leaves the directory in place and returns the error.
pub fn materialise(
    objects: &ObjectStore,
    tree: &ObjectHash,
    root: &Path,
    opts: &MaterialiseOptions,
) -> Result<Materialised, WorkspaceError> {
    prepare_target(root)?;
    let mut batch = Batch::open(root)?;
    batch.dirs.insert(Vec::new());
    let mut done = Materialised {
        index: StatIndex::empty(opts.caps),
        degraded: Vec::new(),
        synced: Synced::default(),
    };
    let root_dir = batch.root_handle()?.try_clone()?;
    write_tree(objects, tree, None, &root_dir, opts, &mut batch, &mut done)?;
    let staging = batch.staging()?.try_clone()?;
    done.synced = batch.finish()?;
    done.index.mark_boundary_in(&staging)?;
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

/// Each file is written through the open handle of its directory, so no directory is opened again from the root.
fn write_tree(
    objects: &ObjectStore,
    hash: &ObjectHash,
    prefix: Option<&RepoPath>,
    dir: &DirHandle,
    opts: &MaterialiseOptions,
    batch: &mut Batch,
    done: &mut Materialised,
) -> Result<(), WorkspaceError> {
    let tree = Tree::decode(&objects.read_expecting(hash, ObjectKind::Tree)?.payload)?;
    let key = prefix.map_or_else(Vec::new, |prefix| prefix.as_bytes().to_vec());
    for entry in tree.entries {
        let path = RepoPath::child(prefix, &entry.name)?;
        if entry.kind == EntryKind::Directory {
            let Some((child, created)) = dir.child_or_create(&entry.name)? else {
                return Err(WorkspaceError::invalid(path.as_bytes()));
            };
            if created {
                batch.dirs.insert(key.clone());
            }
            write_tree(objects, &entry.hash, Some(&path), &child, opts, batch, done)?;
            continue;
        }
        let flat = FlatEntry {
            kind: entry.kind,
            hash: entry.hash,
        };
        batch.dirs.insert(key.clone());
        let written = batch.write_into(objects, dir, &entry.name, &flat, opts)?;
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

/// The writes and removals of one create, sync or restore in one workspace, flushed together by [`Batch::finish`]: the written files first, then every directory whose entries changed. The caller saves the stat index only after `finish`. One batch runs per workspace at a time, because opening a batch removes whatever an interrupted one left in the staging directory `<root>/.fold/tmp/`. Below the root every directory is reached through handles opened without following a symlink: a parent that is a symlink when it is reached is refused, and one swapped for a symlink after it was opened still takes the write in the directory that was opened.
#[derive(Debug)]
pub struct Batch {
    root: PathBuf,
    /// `None` until the root exists.
    handle: Option<DirHandle>,
    staging: Option<DirHandle>,
    unsynced: Vec<File>,
    /// Directories below the root that gained or lost an entry, as slash-joined names; the empty name is the root.
    dirs: BTreeSet<Vec<u8>>,
    /// The parent of the root and of each missing ancestor, when the batch created the root.
    outer: BTreeSet<PathBuf>,
    synced: Synced,
}

impl Batch {
    pub fn open(root: &Path) -> Result<Self, WorkspaceError> {
        let handle = match DirHandle::open(root) {
            Ok(handle) => Some(handle),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let staging = match &handle {
            Some(handle) => open_staging(handle, false)?,
            None => None,
        };
        if let Some(staging) = &staging {
            sweep(staging)?;
        }
        Ok(Self {
            root: root.to_path_buf(),
            handle,
            staging,
            unsynced: Vec::new(),
            dirs: BTreeSet::new(),
            outer: BTreeSet::new(),
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
        let (parents, name) = path.parents_and_name();
        let parent = self.create_dirs(&parents, path)?;
        self.write_into(objects, &parent, name, entry, opts)
    }

    /// [`Batch::write_entry`] into the open directory `parent`, which the caller notes for the flush.
    fn write_into(
        &mut self,
        objects: &ObjectStore,
        parent: &DirHandle,
        name: &[u8],
        entry: &FlatEntry,
        opts: &MaterialiseOptions,
    ) -> Result<Written, WorkspaceError> {
        let staging = self.staging()?;
        let tmp = format!(
            "{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        )
        .into_bytes();
        // The stat comes from the staged entry, which no one else can reach: an edit that lands at the destination right after the rename then differs from the index, so the next scan reads it.
        let staged = stage(objects, staging, &tmp, entry, opts).and_then(|staged| {
            staging.rename(&tmp, parent, name)?;
            Ok(staged)
        });
        let (outcome, file, meta) = match staged {
            Ok(staged) => staged,
            Err(error) => {
                let _ = staging.remove_file(&tmp);
                return Err(error);
            }
        };
        if let Some(file) = file {
            self.hold(file)?;
        }
        Ok(Written {
            stat: StatEntry::from_metadata(&meta, entry.kind, entry.hash, &opts.caps),
            degraded: outcome == SymlinkOutcome::Emulated,
        })
    }

    /// Removes the file or symlink at `path`, never following a link, then any parent directory it leaves empty. The workspace root itself is never removed. A path that is already absent is not an error.
    pub fn remove_entry(&mut self, path: &RepoPath) -> Result<(), WorkspaceError> {
        let (mut parents, name) = path.parents_and_name();
        let Some(mut held) = self.open_dirs(&parents, path)? else {
            return Ok(());
        };
        let Some(parent) = held.last() else {
            return Ok(());
        };
        match parent.metadata(name) {
            Ok(meta) if meta.is_dir() => return Err(WorkspaceError::invalid(path.as_bytes())),
            Ok(_) => parent.remove_file(name)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        loop {
            self.dirs.insert(parents.join(&b'/'));
            let Some(emptied) = parents.pop() else {
                break;
            };
            held.pop();
            match held.last() {
                Some(holder) if holder.remove_dir(emptied).is_ok() => {}
                _ => break,
            }
        }
        Ok(())
    }

    /// Removes whatever is at `path`, a directory with everything below it included, never following a link. Unlike [`Batch::remove_entry`] it leaves the parent directory in place. A path that is already absent is not an error.
    pub fn remove_tree(&mut self, path: &RepoPath) -> Result<(), WorkspaceError> {
        let (parents, name) = path.parents_and_name();
        let Some(held) = self.open_dirs(&parents, path)? else {
            return Ok(());
        };
        let Some(parent) = held.last() else {
            return Ok(());
        };
        match parent.metadata(name) {
            Ok(meta) if meta.is_dir() => parent.remove_all(name)?,
            Ok(_) => parent.remove_file(name)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        self.dirs.insert(parents.join(&b'/'));
        Ok(())
    }

    /// Clears the way for a file at `path` when a directory stands there: the directory and the empty directories below it are removed, never following a link, and anything else below makes it an error. Nothing at `path`, or something that is not a directory, is left alone.
    pub fn remove_empty_dirs(&mut self, path: &RepoPath) -> Result<(), WorkspaceError> {
        let (parents, name) = path.parents_and_name();
        let Some(held) = self.open_dirs(&parents, path)? else {
            return Ok(());
        };
        let Some(parent) = held.last() else {
            return Ok(());
        };
        match parent.child(name) {
            Ok(Some(_)) => parent.remove_empty_dirs(name)?,
            Ok(None) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        self.dirs.insert(parents.join(&b'/'));
        Ok(())
    }

    /// A changed directory that a later removal pruned is skipped, also when a file written later took the place of it or of a parent.
    pub fn finish(mut self) -> Result<Synced, WorkspaceError> {
        self.sync_files()?;
        for dir in &self.outer {
            match sync_dir(dir) {
                Ok(()) => self.synced.dirs += 1,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let Some(root) = &self.handle else {
            return Ok(self.synced);
        };
        'dirs: for dir in &self.dirs {
            let mut current = root.try_clone()?;
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
            current.sync()?;
            self.synced.dirs += 1;
        }
        Ok(self.synced)
    }

    /// Opened when the batch first needs it, and created then with its missing ancestors when it does not exist.
    fn root_handle(&mut self) -> io::Result<&DirHandle> {
        let handle = match self.handle.take() {
            Some(handle) => handle,
            None => {
                let mut current = self.root.as_path();
                while fs::symlink_metadata(current).is_err() {
                    let Some(parent) = current.parent() else {
                        break;
                    };
                    self.outer.insert(parent.to_path_buf());
                    current = parent;
                }
                fs::create_dir_all(&self.root)?;
                DirHandle::open(&self.root)?
            }
        };
        Ok(self.handle.insert(handle))
    }

    fn staging(&mut self) -> Result<&DirHandle, WorkspaceError> {
        let staging = match self.staging.take() {
            Some(staging) => staging,
            None => open_staging(self.root_handle()?, true)?
                .ok_or_else(|| WorkspaceError::invalid(b".fold/tmp"))?,
        };
        Ok(self.staging.insert(staging))
    }

    /// Opens the directories `names` below the root, the root included first, without creating any. `None` when one is missing; one that is a symlink or not a directory refuses `path`.
    fn open_dirs(
        &self,
        names: &[&[u8]],
        path: &RepoPath,
    ) -> Result<Option<Vec<DirHandle>>, WorkspaceError> {
        let Some(root) = &self.handle else {
            return Ok(None);
        };
        let mut held = vec![root.try_clone()?];
        for name in names {
            let Some(current) = held.last() else {
                break;
            };
            match current.child(name) {
                Ok(Some(child)) => held.push(child),
                Ok(None) => return Err(WorkspaceError::invalid(path.as_bytes())),
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            }
        }
        Ok(Some(held))
    }

    /// Opens the directories `names` below the root, creating the missing ones. Each directory that gains an entry is noted for the flush: the one holding each created directory, and the last one, which `path` is written into. One that is a symlink or not a directory refuses `path`.
    fn create_dirs(
        &mut self,
        names: &[&[u8]],
        path: &RepoPath,
    ) -> Result<DirHandle, WorkspaceError> {
        let mut current = self.root_handle()?.try_clone()?;
        let mut below: Vec<u8> = Vec::new();
        for name in names {
            let Some((child, created)) = current.child_or_create(name)? else {
                return Err(WorkspaceError::invalid(path.as_bytes()));
            };
            if created {
                self.dirs.insert(below.clone());
            }
            if !below.is_empty() {
                below.push(b'/');
            }
            below.extend_from_slice(name);
            current = child;
        }
        self.dirs.insert(below);
        Ok(current)
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

/// `<root>/.fold/tmp`, created when `create`. A level that is anything but a real directory is refused, so a planted link never redirects staging or the sweep.
fn open_staging(root: &DirHandle, create: bool) -> Result<Option<DirHandle>, WorkspaceError> {
    let Some(fold) = staging_level(root, b".fold", create)? else {
        return Ok(None);
    };
    staging_level(&fold, b"tmp", create)
}

fn staging_level(
    parent: &DirHandle,
    name: &[u8],
    create: bool,
) -> Result<Option<DirHandle>, WorkspaceError> {
    let opened = if create {
        parent
            .child_or_create(name)
            .map(|found| found.map(|(dir, _)| dir))
    } else {
        parent.child(name)
    };
    match opened {
        Ok(Some(dir)) => Ok(Some(dir)),
        Ok(None) => Err(WorkspaceError::invalid(b".fold/tmp")),
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn sweep(staging: &DirHandle) -> io::Result<()> {
    for name in staging.names()? {
        staging.remove_all(&name)?;
    }
    Ok(())
}

/// Returns the handle to sync at the end of the batch (the staged file, or none for a real symlink, whose directory entry is all there is to flush) and the staged entry's metadata.
fn stage(
    objects: &ObjectStore,
    staging: &DirHandle,
    tmp: &[u8],
    entry: &FlatEntry,
    opts: &MaterialiseOptions,
) -> Result<(SymlinkOutcome, Option<File>, Metadata), WorkspaceError> {
    let mut blob = objects.open_blob(&entry.hash)?;
    if entry.kind == EntryKind::Symlink {
        let mut target = Vec::new();
        blob.read_to_end(&mut target)?;
        if opts.caps.symlinks {
            staging.symlink(&target, tmp)?;
            return Ok((SymlinkOutcome::Created, None, staging.metadata(tmp)?));
        }
        let mut file = staging.create_file(tmp)?;
        file.write_all(&target)?;
        set_file_mode(&file, false, opts.read_only, &opts.caps)?;
        let meta = file.metadata()?;
        return Ok((SymlinkOutcome::Emulated, Some(file), meta));
    }
    let mut file = staging.create_file(tmp)?;
    io::copy(&mut blob, &mut file)?;
    set_file_mode(
        &file,
        entry.kind == EntryKind::Executable,
        opts.read_only,
        &opts.caps,
    )?;
    let meta = file.metadata()?;
    Ok((SymlinkOutcome::Created, Some(file), meta))
}
