use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use fold_core::object::{ObjectHash, ObjectHasher, ObjectKind, hash_object, varint};
use fold_core::{FoldError, codes};
use thiserror::Error;

use crate::lock;
use crate::platform::sync_dir;

const FORMAT_VERSION: u8 = 1;
const COMPRESSION_NONE: u8 = 0;
const COMPRESSION_ZSTD: u8 = 1;
const ZSTD_LEVEL: i32 = 3;
/// Blobs of this size or smaller are never worth a compression attempt.
const COMPRESS_THRESHOLD: u64 = 512;
const BUFFER_SIZE: usize = 64 * 1024;
const MAX_VARINT_LEN: usize = 10;
/// Caps the up-front allocation for a payload whose header length is untrusted.
const MAX_PREALLOC: u64 = 64 * 1024 * 1024;
/// A streamed blob up to this size is held in memory and stored with one write; a larger one goes through a scratch file first, because its hash, and so its name, is known only at its end. A larger blob is compressed only when this much of its start compresses.
const IN_MEMORY_BLOB: u64 = 1024 * 1024;

static SCRATCH_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum ObjectError {
    #[error("object {0} is missing")]
    Missing(ObjectHash),
    #[error("object {hash} is corrupt: {reason}")]
    Corrupt {
        hash: ObjectHash,
        reason: &'static str,
    },
    #[error("object store I/O failed: {0}")]
    Io(#[source] io::Error),
}

/// A [`BlobReader`] returns its `ObjectError` through `std::io::Read`, so the conversion recovers it and `?` keeps its code; any other I/O error becomes `Io`.
impl From<io::Error> for ObjectError {
    fn from(error: io::Error) -> Self {
        error.downcast::<Self>().unwrap_or_else(Self::Io)
    }
}

impl ObjectError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Missing(_) => codes::OBJECT_MISSING,
            Self::Corrupt { .. } => codes::OBJECT_CORRUPT,
            Self::Io(_) => codes::OBJECT_IO,
        }
    }
}

impl From<ObjectError> for FoldError {
    fn from(error: ObjectError) -> Self {
        FoldError::new(error.code(), error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub kind: ObjectKind,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobWrite {
    pub hash: ObjectHash,
    pub size: u64,
}

/// One stored object as garbage collection sees it. `size` is the file size on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectEntry {
    pub hash: ObjectHash,
    pub size: u64,
    pub modified: SystemTime,
}

#[derive(Debug)]
struct Header {
    kind: ObjectKind,
    compression: u8,
    raw_len: u64,
}

/// Content-addressed object files under `<dir>/objects`. The API is synchronous; async callers go through `spawn_blocking`.
#[derive(Debug, Clone)]
pub struct ObjectStore {
    objects: PathBuf,
    scratch: PathBuf,
    /// The fan directories, by device and inode, whose entry in `objects/` this store and its clones flushed. A writer that finds its fan here does not flush `objects/` again. Any other writer flushes it itself, also when another writer created the fan a moment before and has not flushed it yet, so no object is published into a fan whose own entry could still be lost. The set lives with the store and not with the process because a file system can give a new directory the inode of a removed one: a project removed and made again starts with an empty set.
    flushed_fans: Arc<Mutex<BTreeSet<(u64, u64)>>>,
}

impl ObjectStore {
    /// `dir` is the project's `.fold` directory.
    pub fn open(dir: &Path) -> Result<Self, ObjectError> {
        let objects = dir.join("objects");
        let scratch = objects.join("tmp");
        fs::create_dir_all(&scratch)?;
        Ok(Self {
            objects,
            scratch,
            flushed_fans: Arc::default(),
        })
    }

    /// `objects/tmp/`, where writes are staged before their rename into place.
    pub fn scratch_dir(&self) -> &Path {
        &self.scratch
    }

    pub fn path(&self, hash: &ObjectHash) -> PathBuf {
        let text = hash.to_string();
        self.objects.join(&text[..2]).join(&text[2..])
    }

    pub fn contains(&self, hash: &ObjectHash) -> Result<bool, ObjectError> {
        match fs::metadata(self.path(hash)) {
            Ok(meta) => Ok(meta.is_file()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub fn write(&self, kind: ObjectKind, bytes: &[u8]) -> Result<ObjectHash, ObjectError> {
        let hash = hash_object(kind, bytes);
        if self.touch_existing(&hash)? {
            return Ok(hash);
        }
        let (compression, payload) = encode(kind, bytes)?;
        let (file, tmp) = self.create_scratch()?;
        let stored = (|| {
            let mut out = BufWriter::with_capacity(BUFFER_SIZE, file);
            write_header(&mut out, kind, compression, bytes.len() as u64)?;
            out.write_all(&payload)?;
            out.into_inner()
                .map_err(io::IntoInnerError::into_error)?
                .sync_all()
        })();
        self.finish(stored, &tmp, &hash)?;
        Ok(hash)
    }

    /// Stores the blob `reader` gives. A small one is read into memory and written once; a large one streams through the hasher into a scratch file, so it never has to fit in memory.
    pub fn write_blob_from(&self, reader: &mut impl Read) -> Result<BlobWrite, ObjectError> {
        let mut head = Vec::new();
        reader
            .by_ref()
            .take(IN_MEMORY_BLOB + 1)
            .read_to_end(&mut head)?;
        if head.len() as u64 <= IN_MEMORY_BLOB {
            let hash = self.write(ObjectKind::Blob, &head)?;
            return Ok(BlobWrite {
                hash,
                size: head.len() as u64,
            });
        }
        let (mut raw, raw_path) = self.create_scratch()?;
        let result = self.write_blob_streamed(&mut head.as_slice().chain(reader), &mut raw);
        drop(raw);
        let _ = fs::remove_file(&raw_path);
        result
    }

    fn write_blob_streamed(
        &self,
        reader: &mut impl Read,
        raw: &mut File,
    ) -> Result<BlobWrite, ObjectError> {
        let mut hasher = ObjectHasher::new(ObjectKind::Blob);
        let mut buffer = vec![0u8; BUFFER_SIZE];
        let mut size = 0u64;
        loop {
            let read = match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            };
            hasher.update(&buffer[..read]);
            raw.write_all(&buffer[..read])?;
            size += read as u64;
        }
        let hash = hasher.finish();
        if self.touch_existing(&hash)? {
            return Ok(BlobWrite { hash, size });
        }
        raw.rewind()?;
        let (file, tmp) = self.create_scratch()?;
        let stored = store_blob_file(raw, file, size);
        self.finish(stored, &tmp, &hash)?;
        Ok(BlobWrite { hash, size })
    }

    pub fn read(&self, hash: &ObjectHash) -> Result<Object, ObjectError> {
        self.read_checked(hash, None)
    }

    /// Like [`read`](Self::read), and refuses an object whose stored kind differs from `kind`.
    pub fn read_expecting(
        &self,
        hash: &ObjectHash,
        kind: ObjectKind,
    ) -> Result<Object, ObjectError> {
        self.read_checked(hash, Some(kind))
    }

    fn read_checked(
        &self,
        hash: &ObjectHash,
        expected: Option<ObjectKind>,
    ) -> Result<Object, ObjectError> {
        let (header, mut stream) = self.open_stream(hash, expected)?;
        let capacity = usize::try_from(header.raw_len.min(MAX_PREALLOC)).unwrap_or(0);
        let mut payload = Vec::with_capacity(capacity);
        stream
            .by_ref()
            .take(header.raw_len.saturating_add(1))
            .read_to_end(&mut payload)
            .map_err(|error| payload_error(hash, error))?;
        if payload.len() as u64 != header.raw_len {
            return Err(corrupt(hash, "payload length differs from the header"));
        }
        if hash_object(header.kind, &payload) != *hash {
            return Err(corrupt(hash, "content does not match its hash"));
        }
        Ok(Object {
            kind: header.kind,
            payload,
        })
    }

    /// The length of a blob's content, from its header alone, so a directory listing never reads whole files.
    pub fn blob_size(&self, hash: &ObjectHash) -> Result<u64, ObjectError> {
        let file = match File::open(self.path(hash)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ObjectError::Missing(*hash));
            }
            Err(error) => return Err(error.into()),
        };
        let header = read_header(hash, &mut BufReader::new(file))?;
        if header.kind != ObjectKind::Blob {
            return Err(corrupt(hash, "kind differs from the one asked for"));
        }
        Ok(header.raw_len)
    }

    /// Streams a blob, decompressing on the fly. A hash mismatch surfaces on the read that delivers the last byte, and a length mismatch at the end of the stream, each as an `InvalidData` error that carries an [`ObjectError`]; `ObjectError::from` recovers it.
    pub fn open_blob(&self, hash: &ObjectHash) -> Result<BlobReader, ObjectError> {
        let (header, inner) = self.open_stream(hash, Some(ObjectKind::Blob))?;
        Ok(BlobReader {
            inner,
            hash: *hash,
            expected: header.raw_len,
            seen: 0,
            hasher: Some(ObjectHasher::new(ObjectKind::Blob)),
            failure: None,
        })
    }

    fn open_stream(
        &self,
        hash: &ObjectHash,
        expected: Option<ObjectKind>,
    ) -> Result<(Header, Box<dyn Read + Send>), ObjectError> {
        let file = match File::open(self.path(hash)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(ObjectError::Missing(*hash));
            }
            Err(error) => return Err(error.into()),
        };
        let mut reader = BufReader::with_capacity(BUFFER_SIZE, file);
        let header = read_header(hash, &mut reader)?;
        if expected.is_some_and(|kind| kind != header.kind) {
            return Err(corrupt(hash, "kind differs from the one asked for"));
        }
        let stream: Box<dyn Read + Send> = if header.compression == COMPRESSION_ZSTD {
            Box::new(zstd::stream::read::Decoder::with_buffer(reader)?)
        } else {
            Box::new(reader)
        };
        Ok((header, stream))
    }

    /// Every stored object with its size and modification time. Scratch files are not objects and are never listed.
    pub fn entries(&self) -> Result<Vec<ObjectEntry>, ObjectError> {
        let mut found = Vec::new();
        for fan in fs::read_dir(&self.objects)? {
            let fan = fan?;
            let Some(prefix) = fan.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if prefix.len() != 2 || !fan.file_type()?.is_dir() {
                continue;
            }
            for item in fs::read_dir(fan.path())? {
                let item = item?;
                let Some(rest) = item.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let Ok(hash) = format!("{prefix}{rest}").parse::<ObjectHash>() else {
                    continue;
                };
                let meta = item.metadata()?;
                if meta.is_file() {
                    found.push(ObjectEntry {
                        hash,
                        size: meta.len(),
                        modified: meta.modified()?,
                    });
                }
            }
        }
        Ok(found)
    }

    /// Returns whether the object existed.
    pub fn remove(&self, hash: &ObjectHash) -> Result<bool, ObjectError> {
        match fs::remove_file(self.path(hash)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// Returns whether the object exists. An existing object is being reused, so its modification time is refreshed as a rewrite would refresh it.
    pub fn refresh(&self, hash: &ObjectHash) -> Result<bool, ObjectError> {
        self.touch_existing(hash)
    }

    /// Refreshes the modification time of an existing object so garbage collection's grace period sees it as recent. The file's bytes are never rewritten.
    fn touch_existing(&self, hash: &ObjectHash) -> Result<bool, ObjectError> {
        match OpenOptions::new().write(true).open(self.path(hash)) {
            Ok(file) => {
                // File times live in the filesystem's clock domain, not in fold's Clock.
                file.set_modified(SystemTime::now())?;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn create_scratch(&self) -> Result<(File, PathBuf), ObjectError> {
        loop {
            let number = SCRATCH_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = self
                .scratch
                .join(format!("{}-{number}", std::process::id()));
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => return Ok((file, path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Renames a complete, synced scratch file into place. The scratch file is removed on every path that does not publish it.
    fn finish(
        &self,
        stored: io::Result<()>,
        tmp: &Path,
        hash: &ObjectHash,
    ) -> Result<(), ObjectError> {
        let outcome = stored
            .map_err(ObjectError::from)
            .and_then(|()| self.publish(tmp, hash));
        if outcome.is_err() {
            let _ = fs::remove_file(tmp);
        }
        outcome
    }

    fn publish(&self, tmp: &Path, hash: &ObjectHash) -> Result<(), ObjectError> {
        let dest = self.path(hash);
        let fan = dest.parent().unwrap_or(&self.objects);
        fs::create_dir_all(fan)?;
        if let Err(error) = fs::rename(tmp, &dest) {
            // Where rename refuses an existing target, a concurrent writer of the same hash got there first with identical content.
            if self.touch_existing(hash)? {
                let _ = fs::remove_file(tmp);
                return Ok(());
            }
            return Err(error.into());
        }
        sync_dir(fan)?;
        let meta = fs::metadata(fan)?;
        let identity = (meta.dev(), meta.ino());
        let mut flushed = lock(&self.flushed_fans);
        if !flushed.contains(&identity) {
            sync_dir(&self.objects)?;
            flushed.insert(identity);
        }
        Ok(())
    }
}

/// Reads a blob's decompressed bytes and checks their hash once the last byte is read and their count at the end of the stream, so a damaged object never reads back as valid content, even to a caller that stops at the header's length. A corruption error repeats on every later read, so a retry never sees a clean end.
pub struct BlobReader {
    inner: Box<dyn Read + Send>,
    hash: ObjectHash,
    expected: u64,
    seen: u64,
    hasher: Option<ObjectHasher>,
    failure: Option<&'static str>,
}

impl BlobReader {
    /// The content length the object header records, known before any byte is read.
    pub fn size(&self) -> u64 {
        self.expected
    }

    fn fail(&mut self, reason: &'static str) -> io::Error {
        self.failure = Some(reason);
        io::Error::new(io::ErrorKind::InvalidData, corrupt(&self.hash, reason))
    }
}

impl Read for BlobReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(reason) = self.failure {
            return Err(self.fail(reason));
        }
        let read = match self.inner.read(buf) {
            Ok(read) => read,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::InvalidData
                        | io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::Other
                ) =>
            {
                return Err(self.fail("payload cannot be decoded"));
            }
            Err(error) => return Err(error),
        };
        self.seen += read as u64;
        if self.seen > self.expected || (read == 0 && !buf.is_empty() && self.seen != self.expected)
        {
            return Err(self.fail("blob length differs from the header"));
        }
        if let Some(hasher) = self.hasher.as_mut() {
            hasher.update(&buf[..read]);
            if self.seen == self.expected
                && self.hasher.take().map(ObjectHasher::finish) != Some(self.hash)
            {
                return Err(self.fail("content does not match its hash"));
            }
        }
        Ok(read)
    }
}

fn corrupt(hash: &ObjectHash, reason: &'static str) -> ObjectError {
    ObjectError::Corrupt {
        hash: *hash,
        reason,
    }
}

fn payload_error(hash: &ObjectHash, error: io::Error) -> ObjectError {
    match error.kind() {
        io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof | io::ErrorKind::Other => {
            corrupt(hash, "payload cannot be decoded")
        }
        _ => error.into(),
    }
}

fn encode(kind: ObjectKind, bytes: &[u8]) -> io::Result<(u8, Cow<'_, [u8]>)> {
    if kind == ObjectKind::Blob && bytes.len() as u64 > COMPRESS_THRESHOLD {
        let packed = zstd::bulk::compress(bytes, ZSTD_LEVEL)?;
        if saves_enough(packed.len() as u64, bytes.len() as u64) {
            return Ok((COMPRESSION_ZSTD, Cow::Owned(packed)));
        }
    }
    Ok((COMPRESSION_NONE, Cow::Borrowed(bytes)))
}

/// Compression is kept only when it saves at least 10%.
fn saves_enough(packed: u64, raw: u64) -> bool {
    packed.saturating_mul(10) <= raw.saturating_mul(9)
}

fn write_header(
    out: &mut impl Write,
    kind: ObjectKind,
    compression: u8,
    raw_len: u64,
) -> io::Result<()> {
    let mut header = vec![FORMAT_VERSION, kind.as_u8(), compression];
    varint::encode(raw_len, &mut header);
    out.write_all(&header)
}

/// Writes header and payload of a blob held in `raw` (already rewound) into `file`, compressing only when it pays. A blob whose start does not compress is written uncompressed at once, so only a blob that compresses at its start and not as a whole is written twice.
fn store_blob_file(raw: &mut File, file: File, size: u64) -> io::Result<()> {
    let mut input = BufReader::with_capacity(BUFFER_SIZE, raw);
    let mut out = BufWriter::with_capacity(BUFFER_SIZE, file);
    if size > COMPRESS_THRESHOLD && start_compresses(&mut input)? {
        write_header(&mut out, ObjectKind::Blob, COMPRESSION_ZSTD, size)?;
        out.flush()?;
        let header_len = out.get_mut().stream_position()?;
        zstd::stream::copy_encode(&mut input, &mut out, ZSTD_LEVEL)?;
        out.flush()?;
        let end = out.get_mut().stream_position()?;
        if saves_enough(end - header_len, size) {
            return out
                .into_inner()
                .map_err(io::IntoInnerError::into_error)?
                .sync_all();
        }
        let file = out.get_mut();
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        input.rewind()?;
    }
    write_header(&mut out, ObjectKind::Blob, COMPRESSION_NONE, size)?;
    io::copy(&mut input, &mut out)?;
    out.into_inner()
        .map_err(io::IntoInnerError::into_error)?
        .sync_all()
}

/// Compresses the first [`IN_MEMORY_BLOB`] bytes in memory, then rewinds `input`.
fn start_compresses(input: &mut BufReader<&mut File>) -> io::Result<bool> {
    let mut sample = Vec::new();
    input
        .by_ref()
        .take(IN_MEMORY_BLOB)
        .read_to_end(&mut sample)?;
    input.rewind()?;
    let packed = zstd::bulk::compress(&sample, ZSTD_LEVEL)?;
    Ok(saves_enough(packed.len() as u64, sample.len() as u64))
}

fn read_header(hash: &ObjectHash, reader: &mut impl Read) -> Result<Header, ObjectError> {
    let mut fixed = [0u8; 3];
    read_exact_or_corrupt(hash, reader, &mut fixed)?;
    if fixed[0] != FORMAT_VERSION {
        return Err(corrupt(hash, "unknown format version"));
    }
    let kind = ObjectKind::from_u8(fixed[1]).ok_or_else(|| corrupt(hash, "unknown kind"))?;
    if fixed[2] != COMPRESSION_NONE && fixed[2] != COMPRESSION_ZSTD {
        return Err(corrupt(hash, "unknown compression"));
    }
    let mut digits = Vec::with_capacity(MAX_VARINT_LEN);
    loop {
        let mut byte = [0u8; 1];
        read_exact_or_corrupt(hash, reader, &mut byte)?;
        digits.push(byte[0]);
        if byte[0] & 0x80 == 0 {
            break;
        }
        if digits.len() == MAX_VARINT_LEN {
            return Err(corrupt(hash, "length is not a valid varint"));
        }
    }
    let (raw_len, _) =
        varint::decode(&digits).map_err(|_| corrupt(hash, "length is not a valid varint"))?;
    Ok(Header {
        kind,
        compression: fixed[2],
        raw_len,
    })
}

fn read_exact_or_corrupt(
    hash: &ObjectHash,
    reader: &mut impl Read,
    buf: &mut [u8],
) -> Result<(), ObjectError> {
    reader.read_exact(buf).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            corrupt(hash, "header is truncated")
        } else {
            error.into()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compression_threshold_is_ten_percent() {
        assert!(saves_enough(90, 100));
        assert!(!saves_enough(91, 100));
    }

    #[test]
    fn error_codes() {
        let hash = hash_object(ObjectKind::Blob, b"x");
        assert_eq!(ObjectError::Missing(hash).code(), "object_missing");
        assert_eq!(corrupt(&hash, "r").code(), "object_corrupt");
        let io = ObjectError::from(io::Error::other("x"));
        assert_eq!(io.code(), "object_io");
        assert_eq!(FoldError::from(io).code(), "object_io");
    }

    /// Fails once with `Interrupted`, then yields its bytes.
    struct InterruptedOnce {
        interrupted: bool,
        bytes: io::Cursor<Vec<u8>>,
    }

    impl Read for InterruptedOnce {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.bytes.read(buf)
        }
    }

    #[test]
    fn a_passing_stream_error_is_not_corruption_and_does_not_stick() {
        let bytes = b"retry".to_vec();
        let mut reader = BlobReader {
            inner: Box::new(InterruptedOnce {
                interrupted: false,
                bytes: io::Cursor::new(bytes.clone()),
            }),
            hash: hash_object(ObjectKind::Blob, &bytes),
            expected: bytes.len() as u64,
            seen: 0,
            hasher: Some(ObjectHasher::new(ObjectKind::Blob)),
            failure: None,
        };
        let mut buf = [0u8; 16];
        let error = reader.read(&mut buf).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        let mut sink = Vec::new();
        reader.read_to_end(&mut sink).unwrap();
        assert_eq!(sink, bytes);
    }
}
