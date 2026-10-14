use std::fs;
use std::io::{Cursor, Read};
use std::time::{Duration, SystemTime};

use fold_core::object::{ObjectHash, ObjectKind, hash_object};
use fold_vcs::objects::{ObjectError, ObjectStore};

#[cfg(test)]
mod support {
    use super::*;

    pub fn store() -> (tempfile::TempDir, ObjectStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        (dir, store)
    }

    pub fn compressible(len: usize) -> Vec<u8> {
        b"abcdefgh".iter().copied().cycle().take(len).collect()
    }

    /// Deterministic bytes that zstd cannot shrink.
    pub fn incompressible(len: usize) -> Vec<u8> {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    }

    pub fn flag(store: &ObjectStore, hash: &ObjectHash) -> u8 {
        fs::read(store.path(hash)).unwrap()[2]
    }
}
use support::*;

#[test]
fn open_creates_the_directories() {
    let (dir, _store) = store();
    assert!(dir.path().join("objects/tmp").is_dir());
}

#[test]
fn every_kind_round_trips() {
    let (_dir, store) = store();
    for kind in [ObjectKind::Blob, ObjectKind::Tree, ObjectKind::Revision] {
        let hash = store.write(kind, b"payload").unwrap();
        assert_eq!(hash, hash_object(kind, b"payload"));
        assert!(store.contains(&hash).unwrap());
        let object = store.read(&hash).unwrap();
        assert_eq!(object.kind, kind);
        assert_eq!(object.payload, b"payload");
        assert_eq!(store.read_expecting(&hash, kind).unwrap(), object);
    }
    let empty = store.write(ObjectKind::Blob, b"").unwrap();
    assert_eq!(store.read(&empty).unwrap().payload, b"");
}

#[test]
fn same_content_is_one_file() {
    let (_dir, store) = store();
    let first = store.write(ObjectKind::Blob, b"same").unwrap();
    let second = store.write(ObjectKind::Blob, b"same").unwrap();
    assert_eq!(first, second);
    assert_eq!(store.entries().unwrap().len(), 1);
}

#[test]
fn compression_rules() {
    let (_dir, store) = store();
    let at_limit = store.write(ObjectKind::Blob, &compressible(512)).unwrap();
    assert_eq!(flag(&store, &at_limit), 0);
    let over = compressible(513);
    let packed = store.write(ObjectKind::Blob, &over).unwrap();
    assert_eq!(flag(&store, &packed), 1);
    assert_eq!(packed, hash_object(ObjectKind::Blob, &over));
    assert_eq!(store.read(&packed).unwrap().payload, over);
    let noise = incompressible(4096);
    let plain = store.write(ObjectKind::Blob, &noise).unwrap();
    assert_eq!(flag(&store, &plain), 0);
    assert_eq!(plain, hash_object(ObjectKind::Blob, &noise));
    let tree = store.write(ObjectKind::Tree, &compressible(4096)).unwrap();
    assert_eq!(flag(&store, &tree), 0);
}

#[test]
fn streamed_write_matches_buffered_write() {
    let (_dir, store) = store();
    let big = [compressible(3 << 20), incompressible(2 << 20)].concat();
    let streamed = store.write_blob_from(&mut Cursor::new(&big)).unwrap();
    assert_eq!(streamed.size, big.len() as u64);
    assert_eq!(flag(&store, &streamed.hash), 1);
    let buffered = store.write(ObjectKind::Blob, &big).unwrap();
    assert_eq!(streamed.hash, buffered);
    let mut back = Vec::new();
    store
        .open_blob(&streamed.hash)
        .unwrap()
        .read_to_end(&mut back)
        .unwrap();
    assert_eq!(back, big);
    assert_eq!(store.entries().unwrap().len(), 1);
    assert_eq!(
        fs::read_dir(store.path(&buffered).parent().unwrap())
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn streamed_write_stores_each_shape() {
    let (dir, store) = store();
    for data in [
        Vec::new(),
        b"tiny".to_vec(),
        compressible(600),
        incompressible(5000),
    ] {
        let written = store.write_blob_from(&mut Cursor::new(&data)).unwrap();
        assert_eq!(written.hash, hash_object(ObjectKind::Blob, &data));
        assert_eq!(store.read(&written.hash).unwrap().payload, data);
        let mut back = Vec::new();
        store
            .open_blob(&written.hash)
            .unwrap()
            .read_to_end(&mut back)
            .unwrap();
        assert_eq!(back, data);
    }
    assert_eq!(
        fs::read_dir(dir.path().join("objects/tmp"))
            .unwrap()
            .count(),
        0
    );
    let again = store.write_blob_from(&mut Cursor::new(b"tiny")).unwrap();
    assert_eq!(again.size, 4);
}

#[test]
fn scratch_files_are_not_objects() {
    let (dir, store) = store();
    let hash = hash_object(ObjectKind::Blob, b"x");
    let text = hash.to_string();
    fs::write(dir.path().join("objects/tmp").join(&text), b"junk").unwrap();
    fs::write(dir.path().join("objects/tmp/other"), b"junk").unwrap();
    assert!(!store.contains(&hash).unwrap());
    assert!(matches!(store.read(&hash), Err(ObjectError::Missing(_))));
    assert!(store.entries().unwrap().is_empty());
}

#[test]
fn entries_report_size_and_remove_deletes() {
    let (_dir, store) = store();
    let hash = store.write(ObjectKind::Blob, b"hello").unwrap();
    let entries = store.entries().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].hash, hash);
    assert_eq!(
        entries[0].size,
        fs::metadata(store.path(&hash)).unwrap().len()
    );
    assert!(store.remove(&hash).unwrap());
    assert!(!store.remove(&hash).unwrap());
    assert!(!store.contains(&hash).unwrap());
}

#[test]
fn corruption_is_object_corrupt() {
    let (_dir, store) = store();
    let data = compressible(2000);
    let packed = store.write(ObjectKind::Blob, &data).unwrap();
    let plain = store.write(ObjectKind::Blob, b"plain text").unwrap();

    type Mutation = Box<dyn Fn(Vec<u8>) -> Vec<u8>>;
    let cases: Vec<(ObjectHash, Mutation)> = vec![
        (plain, Box::new(|b| b[..b.len() - 3].to_vec())),
        (plain, Box::new(|b| b[..2].to_vec())),
        (plain, Box::new(|b| b[..3].to_vec())),
        (
            plain,
            Box::new(|mut b| {
                b[0] = 9;
                b
            }),
        ),
        (
            plain,
            Box::new(|mut b| {
                b[1] = 7;
                b
            }),
        ),
        (
            plain,
            Box::new(|mut b| {
                b[2] = 5;
                b
            }),
        ),
        (
            plain,
            Box::new(|mut b| {
                b[3] += 1;
                b
            }),
        ),
        (
            plain,
            Box::new(|mut b| {
                b.push(0);
                b
            }),
        ),
        (
            plain,
            Box::new(|mut b| {
                b.splice(3..4, [0x80u8; 10]);
                b
            }),
        ),
        (
            plain,
            Box::new(|mut b| {
                b.splice(3..4, [0xffu8; 11]);
                b
            }),
        ),
        (packed, Box::new(|b| b[..b.len() / 2].to_vec())),
        (
            packed,
            Box::new(|mut b| {
                b[3] = 0x10;
                b.truncate(4);
                b.push(0);
                b
            }),
        ),
    ];
    for (index, (hash, mutate)) in cases.iter().enumerate() {
        let path = store.path(hash);
        let original = fs::read(&path).unwrap();
        fs::write(&path, mutate(original.clone())).unwrap();
        let error = store.read(hash).unwrap_err();
        assert_eq!(error.code(), "object_corrupt", "case {index}");
        let streamed = store.open_blob(hash).and_then(|mut r| {
            let mut sink = Vec::new();
            r.read_to_end(&mut sink).map_err(ObjectError::from)
        });
        assert!(streamed.is_err(), "case {index}");
        fs::write(&path, original).unwrap();
    }
    assert_eq!(store.read(&packed).unwrap().payload, data);
}

#[test]
fn any_changed_byte_is_object_corrupt() {
    let (_dir, store) = store();
    let packed_data = compressible(2000);
    let packed = store.write(ObjectKind::Blob, &packed_data).unwrap();
    let plain = store.write(ObjectKind::Blob, b"hello world").unwrap();
    assert_eq!(flag(&store, &packed), 1);
    assert_eq!(flag(&store, &plain), 0);
    for hash in [packed, plain] {
        let path = store.path(&hash);
        let original = fs::read(&path).unwrap();
        for index in 0..original.len() {
            let mut damaged = original.clone();
            damaged[index] ^= 0x01;
            fs::write(&path, damaged).unwrap();
            let read = store.read(&hash).unwrap_err();
            assert_eq!(read.code(), "object_corrupt", "read, byte {index}");
            let streamed = store
                .open_blob(&hash)
                .and_then(|mut reader| {
                    let mut sink = Vec::new();
                    reader.read_to_end(&mut sink).map_err(ObjectError::from)
                })
                .unwrap_err();
            assert_eq!(streamed.code(), "object_corrupt", "stream, byte {index}");
        }
        fs::write(&path, original).unwrap();
        assert!(store.read(&hash).is_ok());
    }
}

#[test]
fn a_reader_that_stops_at_the_last_byte_still_checks_the_hash() {
    let (_dir, store) = store();
    let packed_data = compressible(2000);
    let packed = store.write(ObjectKind::Blob, &packed_data).unwrap();
    let plain_data = b"hello world".to_vec();
    let plain = store.write(ObjectKind::Blob, &plain_data).unwrap();
    for (hash, data) in [(packed, packed_data), (plain, plain_data)] {
        let mut whole = vec![0u8; data.len()];
        store
            .open_blob(&hash)
            .unwrap()
            .read_exact(&mut whole)
            .unwrap();
        assert_eq!(whole, data);
        let path = store.path(&hash);
        let original = fs::read(&path).unwrap();
        let mut damaged = original.clone();
        *damaged.last_mut().unwrap() ^= 0x01;
        fs::write(&path, damaged).unwrap();
        let mut reader = store.open_blob(&hash).unwrap();
        let error = reader.read_exact(&mut whole).unwrap_err();
        assert_eq!(ObjectError::from(error).code(), "object_corrupt");
        for _ in 0..2 {
            let again = reader.read(&mut whole).unwrap_err();
            assert_eq!(ObjectError::from(again).code(), "object_corrupt");
        }
        fs::write(&path, original).unwrap();
    }
}

#[test]
fn an_empty_blob_is_checked_on_its_first_read() {
    let (_dir, store) = store();
    let empty = store.write(ObjectKind::Blob, b"").unwrap();
    let mut sink = Vec::new();
    store
        .open_blob(&empty)
        .unwrap()
        .read_to_end(&mut sink)
        .unwrap();
    assert!(sink.is_empty());
    let other = hash_object(ObjectKind::Blob, b"not empty");
    let path = store.path(&other);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::copy(store.path(&empty), &path).unwrap();
    let error = store
        .open_blob(&other)
        .unwrap()
        .read(&mut [0u8; 8])
        .unwrap_err();
    assert_eq!(ObjectError::from(error).code(), "object_corrupt");
    assert_eq!(store.read(&other).unwrap_err().code(), "object_corrupt");
}

#[test]
fn io_errors_other_than_corruption_stay_io() {
    let error = ObjectError::from(std::io::Error::other("disk"));
    assert_eq!(error.code(), "object_io");
    let permission = ObjectError::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    assert_eq!(permission.code(), "object_io");
}

#[test]
fn wrong_kind_is_corrupt() {
    let (_dir, store) = store();
    let hash = store.write(ObjectKind::Tree, b"t").unwrap();
    let error = store.read_expecting(&hash, ObjectKind::Blob).unwrap_err();
    assert_eq!(error.code(), "object_corrupt");
    assert!(matches!(
        store.open_blob(&hash),
        Err(ObjectError::Corrupt { .. })
    ));
}

#[test]
fn missing_hash_is_object_missing() {
    let (_dir, store) = store();
    let hash = hash_object(ObjectKind::Blob, b"never written");
    assert_eq!(store.read(&hash).unwrap_err().code(), "object_missing");
    assert!(matches!(
        store.open_blob(&hash),
        Err(ObjectError::Missing(_))
    ));
    assert!(!store.contains(&hash).unwrap());
}

#[test]
fn rewriting_keeps_bytes_and_moves_the_modification_time_forward() {
    let (_dir, store) = store();
    let payload = compressible(2000);
    let hash = store.write(ObjectKind::Blob, &payload).unwrap();
    let path = store.path(&hash);
    let before = fs::read(&path).unwrap();
    let old = SystemTime::now() - Duration::from_secs(7200);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(old)
        .unwrap();

    store.write(ObjectKind::Blob, &payload).unwrap();
    let after_write = fs::metadata(&path).unwrap().modified().unwrap();
    assert!(after_write > old + Duration::from_secs(3600));

    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(old)
        .unwrap();
    store.write_blob_from(&mut Cursor::new(&payload)).unwrap();
    assert!(fs::metadata(&path).unwrap().modified().unwrap() > old + Duration::from_secs(3600));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(
        fs::read_dir(
            store
                .path(&hash)
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("tmp")
        )
        .unwrap()
        .count(),
        0
    );
}

#[test]
fn concurrent_writers_of_one_hash_both_succeed() {
    let (_dir, store) = store();
    let data = compressible(100_000);
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let store = store.clone();
            let data = data.clone();
            std::thread::spawn(move || store.write(ObjectKind::Blob, &data).unwrap())
        })
        .collect();
    let hashes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(hashes.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(store.read(&hashes[0]).unwrap().payload, data);
    assert_eq!(store.entries().unwrap().len(), 1);
}
