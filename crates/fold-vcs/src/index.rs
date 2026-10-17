use std::collections::BTreeMap;
use std::fs::{self, File, Metadata};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use fold_core::object::{ObjectHash, varint};
use fold_core::tree::EntryKind;

use crate::paths::{RepoPath, entries_at_or_below};
use crate::platform::{Capabilities, DirHandle, file_identity, file_mode};

const MAGIC: &[u8; 4] = b"FIDX";
const VERSION: u8 = 1;
const CHECKSUM_LEN: usize = 32;
/// The entry kind rides above the permission bits, which never exceed `0o7777`.
const KIND_SHIFT: u32 = 16;
const MODE_MASK: u32 = 0xFFFF;

static PROBE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// What the last scan or write saw of one file, and the hash it had then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatEntry {
    pub kind: EntryKind,
    pub size: u64,
    pub mtime_ns: u64,
    /// `0` when the platform has no inode.
    pub inode: u64,
    /// Permission bits only.
    pub mode: u32,
    pub hash: ObjectHash,
}

impl StatEntry {
    /// `meta` must come from `symlink_metadata`, so a link is described and never followed.
    pub fn from_metadata(
        meta: &Metadata,
        kind: EntryKind,
        hash: ObjectHash,
        caps: &Capabilities,
    ) -> Self {
        Self {
            kind,
            size: meta.len(),
            mtime_ns: mtime_ns(meta),
            inode: file_identity(meta, caps),
            mode: file_mode(meta),
            hash,
        }
    }
}

pub fn mtime_ns(meta: &Metadata) -> u64 {
    meta.modified().map_or(0, system_time_ns)
}

fn system_time_ns(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |elapsed| {
        u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
    })
}

/// Per-workspace cache of file metadata and hashes, so a scan hashes only files whose metadata moved.
#[derive(Debug, Clone)]
pub struct StatIndex {
    entries: BTreeMap<RepoPath, StatEntry>,
    caps: Capabilities,
    rebuilt: bool,
    /// A file-system time taken after the last write the entries describe: the last save, or [`StatIndex::mark_boundary`]. `0` until either happens, so nothing is fresh.
    boundary_ns: u64,
}

impl StatIndex {
    pub fn empty(caps: Capabilities) -> Self {
        Self {
            entries: BTreeMap::new(),
            caps,
            rebuilt: false,
            boundary_ns: 0,
        }
    }

    /// A missing file, a wrong magic or version, or a failed checksum gives an empty index flagged `rebuilt`, so the next scan hashes every file. The save time is the index file's own modification time, which shares a clock with the files it describes.
    pub fn load(path: &Path, caps: Capabilities) -> Self {
        let mut index = Self::empty(caps);
        index.rebuilt = true;
        let Ok(bytes) = fs::read(path) else {
            return index;
        };
        let Some(entries) = decode(&bytes) else {
            return index;
        };
        index.entries = entries;
        index.rebuilt = false;
        index.boundary_ns = fs::metadata(path).map_or(0, |meta| mtime_ns(&meta));
        index
    }

    /// Saves of one index run one at a time: a save first removes the temp files that an interrupted save of the same index left beside it. The index's directory is created when missing, but not the directory that holds it, so a save after the project's `.fold/` was deleted fails instead of making `.fold/` again.
    pub fn save(&mut self, path: &Path) -> io::Result<()> {
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        match fs::create_dir(dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => {}
            Err(error) => return Err(error),
        }
        let prefix = format!(
            ".save-{}-",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
        sweep_saves(dir, &prefix)?;
        crate::atomic::write_atomic(path, &encode(&self.entries))?;
        self.boundary_ns = mtime_ns(&fs::metadata(path)?);
        self.rebuilt = false;
        Ok(())
    }

    /// True when the file was missing, damaged or of another version at load.
    pub fn rebuilt(&self) -> bool {
        self.rebuilt
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    /// Nanoseconds of the boundary; `0` for an index never saved or marked.
    pub fn boundary_ns(&self) -> u64 {
        self.boundary_ns
    }

    /// Takes the boundary from the file system clock: the modification time of a probe file written in `dir` and removed again. Call it after the last write that the entries describe, so an entry from the same clock tick is not fresh.
    pub fn mark_boundary(&mut self, dir: &Path) -> io::Result<()> {
        let number = PROBE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let probe = dir.join(format!("boundary-{}-{number}", std::process::id()));
        let file = File::create(&probe)?;
        let marked = file.metadata().map(|meta| mtime_ns(&meta));
        drop(file);
        fs::remove_file(&probe)?;
        self.boundary_ns = marked?;
        Ok(())
    }

    /// [`StatIndex::mark_boundary`] with the probe made in an open directory, so a directory inside a workspace that is swapped for a symlink never takes it.
    pub(crate) fn mark_boundary_in(&mut self, dir: &DirHandle) -> io::Result<()> {
        let number = PROBE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let probe = format!("boundary-{}-{number}", std::process::id()).into_bytes();
        let file = dir.create_file(&probe)?;
        let marked = file.metadata().map(|meta| mtime_ns(&meta));
        drop(file);
        dir.remove_file(&probe)?;
        self.boundary_ns = marked?;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn lookup(&self, path: &RepoPath) -> Option<&StatEntry> {
        self.entries.get(path)
    }

    pub fn upsert(&mut self, path: RepoPath, entry: StatEntry) {
        self.entries.insert(path, entry);
    }

    pub fn remove(&mut self, path: &RepoPath) -> Option<StatEntry> {
        self.entries.remove(path)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&RepoPath, &StatEntry)> {
        self.entries.iter()
    }

    /// The entries at `prefix` and below it, found by a range of the map; `None` gives every entry.
    pub(crate) fn at_or_below(
        &self,
        prefix: Option<&RepoPath>,
    ) -> impl Iterator<Item = (&RepoPath, &StatEntry)> {
        entries_at_or_below(&self.entries, prefix.map_or(&[][..], RepoPath::as_bytes))
    }

    /// The stored hash, only when the file type fits the recorded kind and size, modification time and mode match, and the inode too when both sides have one. An entry whose modification time is not older than the boundary is never fresh: an edit in the same clock tick as the last recorded write would otherwise go unseen. Without symlink support a symlink entry is a regular file on disk.
    pub fn is_fresh(&self, path: &RepoPath, meta: &Metadata) -> Option<ObjectHash> {
        let entry = self.entries.get(path)?;
        let kind_fits = match entry.kind {
            EntryKind::Symlink if self.caps.symlinks => meta.file_type().is_symlink(),
            _ => meta.file_type().is_file(),
        };
        if !kind_fits {
            return None;
        }
        let mtime = mtime_ns(meta);
        if entry.mtime_ns >= self.boundary_ns {
            return None;
        }
        let inode = file_identity(meta, &self.caps);
        let same_inode = entry.inode == 0 || inode == 0 || entry.inode == inode;
        (entry.size == meta.len()
            && entry.mtime_ns == mtime
            && entry.mode == file_mode(meta)
            && same_inode)
            .then_some(entry.hash)
    }
}

fn sweep_saves(dir: &Path, prefix: &str) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(prefix) && name.ends_with(".tmp") {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

fn encode(entries: &BTreeMap<RepoPath, StatEntry>) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + entries.len() * 80);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    varint::encode(entries.len() as u64, &mut out);
    for (path, entry) in entries {
        varint::encode(path.as_bytes().len() as u64, &mut out);
        out.extend_from_slice(path.as_bytes());
        out.extend_from_slice(&entry.size.to_le_bytes());
        out.extend_from_slice(&entry.mtime_ns.to_le_bytes());
        out.extend_from_slice(&entry.inode.to_le_bytes());
        let mode = (u32::from(entry.kind.as_u8()) << KIND_SHIFT) | (entry.mode & MODE_MASK);
        out.extend_from_slice(&mode.to_le_bytes());
        out.extend_from_slice(entry.hash.as_bytes());
    }
    let checksum = blake3::hash(&out);
    out.extend_from_slice(checksum.as_bytes());
    out
}

struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(len)?;
        self.0 = rest;
        Some(head)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }

    fn varint(&mut self) -> Option<u64> {
        let (value, used) = varint::decode(self.0).ok()?;
        self.take(used)?;
        Some(value)
    }
}

fn decode(bytes: &[u8]) -> Option<BTreeMap<RepoPath, StatEntry>> {
    let body_len = bytes.len().checked_sub(CHECKSUM_LEN)?;
    let (body, checksum) = bytes.split_at(body_len);
    if blake3::hash(body).as_bytes() != checksum {
        return None;
    }
    let mut cursor = Cursor(body);
    if cursor.take(MAGIC.len())? != MAGIC || cursor.take(1)? != [VERSION] {
        return None;
    }
    let count = cursor.varint()?;
    let mut entries = BTreeMap::new();
    for _ in 0..count {
        let path_len = usize::try_from(cursor.varint()?).ok()?;
        let path = RepoPath::parse(cursor.take(path_len)?).ok()?;
        let size = u64::from_le_bytes(cursor.array()?);
        let mtime_ns = u64::from_le_bytes(cursor.array()?);
        let inode = u64::from_le_bytes(cursor.array()?);
        let packed = u32::from_le_bytes(cursor.array()?);
        let kind = EntryKind::from_u8(u8::try_from(packed >> KIND_SHIFT).ok()?)?;
        let hash = ObjectHash::from_bytes(cursor.array()?);
        let entry = StatEntry {
            kind,
            size,
            mtime_ns,
            inode,
            mode: packed & MODE_MASK,
            hash,
        };
        entries.insert(path, entry);
    }
    cursor.0.is_empty().then_some(entries)
}
