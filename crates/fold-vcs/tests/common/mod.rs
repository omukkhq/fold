#![cfg(test)]

use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::tree::{EntryKind, Tree, TreeEntry};
use fold_vcs::materialise::MaterialiseOptions;
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::Capabilities;

pub const HOST: Capabilities = Capabilities {
    symlinks: true,
    exec_bit: true,
    inodes: true,
    read_only_bit: true,
};

pub const BARE: Capabilities = Capabilities {
    symlinks: false,
    exec_bit: false,
    inodes: false,
    read_only_bit: false,
};

pub fn store() -> (tempfile::TempDir, ObjectStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = ObjectStore::open(&dir.path().join(".fold")).unwrap();
    (dir, store)
}

pub fn blob(store: &ObjectStore, bytes: &[u8]) -> ObjectHash {
    store.write(ObjectKind::Blob, bytes).unwrap()
}

pub fn tree(store: &ObjectStore, entries: &[(&str, EntryKind, ObjectHash)]) -> ObjectHash {
    let tree = Tree {
        entries: entries
            .iter()
            .map(|(name, kind, hash)| TreeEntry {
                name: name.as_bytes().to_vec(),
                kind: *kind,
                hash: *hash,
            })
            .collect(),
    };
    store
        .write(ObjectKind::Tree, &tree.encode().unwrap())
        .unwrap()
}

pub fn opts(caps: Capabilities, read_only: bool) -> MaterialiseOptions {
    MaterialiseOptions { read_only, caps }
}

/// src/lib.rs, src/deep/run.sh (executable), link -> src/lib.rs, empty.
pub fn sample(store: &ObjectStore) -> ObjectHash {
    let lib = blob(store, b"pub fn x() {}\n");
    let script = blob(store, b"#!/bin/sh\n");
    let target = blob(store, b"src/lib.rs");
    let empty = blob(store, b"");
    let deep = tree(store, &[("run.sh", EntryKind::Executable, script)]);
    let src = tree(
        store,
        &[
            ("deep", EntryKind::Directory, deep),
            ("lib.rs", EntryKind::File, lib),
        ],
    );
    tree(
        store,
        &[
            ("empty", EntryKind::File, empty),
            ("link", EntryKind::Symlink, target),
            ("src", EntryKind::Directory, src),
        ],
    )
}
