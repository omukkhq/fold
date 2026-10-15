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

pub fn write(root: &std::path::Path, path: &str, content: &[u8]) {
    let full = root.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, content).unwrap();
}

pub fn flat_paths(flat: &fold_core::tree::FlatTree) -> Vec<String> {
    flat.keys()
        .map(|path| String::from_utf8(path.clone()).unwrap())
        .collect()
}

/// Saves the index until the save lands in a later clock tick than every entry, so on a file system with a coarse clock each entry is still fresh afterwards.
pub fn settle(index: &mut fold_vcs::index::StatIndex, path: &std::path::Path) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        index.save(path).unwrap();
        let boundary = index.boundary_ns();
        if index.iter().all(|(_, entry)| entry.mtime_ns < boundary) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the file system clock did not move"
        );
    }
}

/// Runs the real `git` in `dir` with no system or user configuration and a fixed identity, and returns its standard output. Fails the test unless git exits zero.
pub fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let output = git_command(dir).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

pub fn git_command(dir: &std::path::Path) -> std::process::Command {
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(dir);
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_COMMON_DIR",
    ] {
        command.env_remove(name);
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com");
    command
}
