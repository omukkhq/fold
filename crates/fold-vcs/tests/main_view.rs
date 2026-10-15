#![cfg(test)]

pub mod common;

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use fold_core::RevisionId;
use fold_core::object::ObjectHash;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_vcs::main_view::{MARKER, read_marker, rewrite, update, write_marker};
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::Capabilities;
use fold_vcs::revision_store::write_initial_tree;

use common::*;

/// A project directory with its object store and an absent `main/`. Dropping it gives every directory back its write bit, so the temporary directory can be removed.
struct Project {
    dir: tempfile::TempDir,
    objects: ObjectStore,
}

impl Project {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let objects = ObjectStore::open(&dir.path().join(".fold")).unwrap();
        Self { dir, objects }
    }

    fn main(&self) -> PathBuf {
        self.dir.path().join("main")
    }

    fn fold_dir(&self) -> PathBuf {
        self.dir.path().join(".fold")
    }

    fn flat(&self, entries: &[(&str, EntryKind, &[u8])]) -> FlatTree {
        entries
            .iter()
            .map(|(path, kind, content)| {
                let entry = FlatEntry {
                    kind: *kind,
                    hash: blob(&self.objects, content),
                };
                (path.as_bytes().to_vec(), entry)
            })
            .collect()
    }

    fn tree(&self, entries: &[(&str, EntryKind, &[u8])]) -> ObjectHash {
        write_initial_tree(&self.objects, self.flat(entries)).unwrap()
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.main().join(path)).unwrap()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        unseal_all(self.dir.path());
    }
}

fn unseal_all(dir: &Path) {
    let Ok(meta) = fs::symlink_metadata(dir) else {
        return;
    };
    if !meta.is_dir() {
        return;
    }
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
    for entry in fs::read_dir(dir).unwrap() {
        unseal_all(&entry.unwrap().path());
    }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

fn inode(path: &Path) -> u64 {
    fs::symlink_metadata(path).unwrap().ino()
}

fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

fn strings(paths: &[fold_vcs::paths::RepoPath]) -> Vec<String> {
    paths.iter().map(ToString::to_string).collect()
}

const FILE: EntryKind = EntryKind::File;

#[test]
fn new_revision_rewrites_only_changed_paths() {
    let project = Project::new();
    let first = project.tree(&[
        ("a.txt", FILE, b"one\n"),
        ("src/keep.rs", FILE, b"keep\n"),
        ("src/lib.rs", FILE, b"lib\n"),
    ]);
    let all = update(
        &project.objects,
        &project.main(),
        &project.fold_dir(),
        None,
        &first,
        r(1),
        HOST,
    )
    .unwrap();
    assert_eq!(
        strings(&all.written),
        ["a.txt", "src/keep.rs", "src/lib.rs"]
    );
    let kept = inode(&project.main().join("src/keep.rs"));
    let changed = inode(&project.main().join("a.txt"));

    let second = project.tree(&[
        ("a.txt", FILE, b"two\n"),
        ("src/keep.rs", FILE, b"keep\n"),
        ("src/new.rs", FILE, b"new\n"),
    ]);
    let done = update(
        &project.objects,
        &project.main(),
        &project.fold_dir(),
        Some(&first),
        &second,
        r(2),
        HOST,
    )
    .unwrap();

    assert_eq!(strings(&done.written), ["a.txt", "src/new.rs"]);
    assert_eq!(strings(&done.removed), ["src/lib.rs"]);
    assert_eq!(inode(&project.main().join("src/keep.rs")), kept);
    assert_ne!(inode(&project.main().join("a.txt")), changed);
    assert_eq!(project.read("a.txt"), "two\n");
    assert_eq!(project.read("src/new.rs"), "new\n");
    assert!(!project.main().join("src/lib.rs").exists());
    assert_eq!(read_marker(&project.fold_dir()).unwrap(), Some(r(2)));

    let same = update(
        &project.objects,
        &project.main(),
        &project.fold_dir(),
        Some(&second),
        &second,
        r(3),
        HOST,
    )
    .unwrap();
    assert!(same.written.is_empty() && same.removed.is_empty());
    assert_eq!(read_marker(&project.fold_dir()).unwrap(), Some(r(3)));
}

#[test]
fn files_are_read_only_and_executables_keep_the_bit() {
    let project = Project::new();
    let from = project.flat(&[
        ("README.md", FILE, b"# demo\n"),
        ("bin/run.sh", EntryKind::Executable, b"#!/bin/sh\n"),
    ]);
    rewrite(
        &project.objects,
        &project.main(),
        &FlatTree::new(),
        &from,
        HOST,
    )
    .unwrap();

    assert_eq!(mode(&project.main().join("README.md")), 0o444);
    assert_eq!(mode(&project.main().join("bin/run.sh")), 0o555);
    assert_eq!(mode(&project.main().join("bin")), 0o555);
    assert_eq!(mode(&project.main()), 0o555);

    // A sealed directory is opened for the change and sealed again.
    let to = project.flat(&[
        ("README.md", FILE, b"# demo\n"),
        ("bin/run.sh", EntryKind::Executable, b"#!/bin/sh\nexit 0\n"),
        ("bin/tool", FILE, b"tool\n"),
    ]);
    rewrite(&project.objects, &project.main(), &from, &to, HOST).unwrap();
    assert_eq!(project.read("bin/run.sh"), "#!/bin/sh\nexit 0\n");
    assert_eq!(mode(&project.main().join("bin/run.sh")), 0o555);
    assert_eq!(mode(&project.main().join("bin/tool")), 0o444);
    assert_eq!(mode(&project.main().join("bin")), 0o555);
    assert_eq!(mode(&project.main()), 0o555);
}

#[test]
fn deleted_paths_and_symlinks_are_handled() {
    let project = Project::new();
    let outside = project.dir.path().join("outside.txt");
    fs::write(&outside, "secret\n").unwrap();
    let from = project.flat(&[
        ("README.md", FILE, b"# demo\n"),
        ("docs/a.md", FILE, b"a\n"),
        ("docs/deep/b.md", FILE, b"b\n"),
        ("link", EntryKind::Symlink, b"README.md"),
        ("plain", EntryKind::Symlink, b"../outside.txt"),
        ("swap", FILE, b"file\n"),
    ]);
    rewrite(
        &project.objects,
        &project.main(),
        &FlatTree::new(),
        &from,
        HOST,
    )
    .unwrap();
    assert_eq!(
        fs::read_link(project.main().join("plain")).unwrap(),
        Path::new("../outside.txt")
    );

    let to = project.flat(&[
        ("README.md", FILE, b"# demo\n"),
        ("away", EntryKind::Symlink, b"/nonexistent/target"),
        ("link", EntryKind::Symlink, b"docs-gone"),
        ("plain", FILE, b"plain\n"),
        ("swap/inner.txt", FILE, b"inner\n"),
    ]);
    let done = rewrite(&project.objects, &project.main(), &from, &to, HOST).unwrap();

    assert_eq!(
        strings(&done.removed),
        ["docs/a.md", "docs/deep/b.md", "swap"]
    );
    assert!(!project.main().join("docs").exists());
    assert_eq!(
        fs::read_link(project.main().join("link")).unwrap(),
        Path::new("docs-gone")
    );
    assert_eq!(
        fs::read_link(project.main().join("away")).unwrap(),
        Path::new("/nonexistent/target")
    );
    // The link was replaced, never written through.
    assert!(
        !fs::symlink_metadata(project.main().join("plain"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(project.read("plain"), "plain\n");
    assert_eq!(fs::read_to_string(&outside).unwrap(), "secret\n");
    assert_eq!(project.read("swap/inner.txt"), "inner\n");
    assert_eq!(mode(&project.main().join("swap")), 0o555);
    assert!(done.degraded.is_empty());

    let back = rewrite(&project.objects, &project.main(), &to, &from, HOST).unwrap();
    assert!(
        back.removed
            .iter()
            .any(|path| path.to_string() == "swap/inner.txt")
    );
    assert_eq!(project.read("swap"), "file\n");
    assert_eq!(project.read("docs/deep/b.md"), "b\n");
}

#[test]
fn a_path_escape_in_a_tree_is_refused() {
    let project = Project::new();
    let escape = project.flat(&[("../escape.txt", FILE, b"x\n")]);
    let error = rewrite(
        &project.objects,
        &project.main(),
        &FlatTree::new(),
        &escape,
        HOST,
    )
    .unwrap_err();
    assert_eq!(error.code(), "invalid_path");
    assert!(!project.dir.path().join("escape.txt").exists());
    assert!(!project.main().exists());

    let reserved = project.flat(&[("src/.git/config", FILE, b"x\n")]);
    let error = rewrite(
        &project.objects,
        &project.main(),
        &FlatTree::new(),
        &reserved,
        HOST,
    )
    .unwrap_err();
    assert_eq!(error.code(), "invalid_path");

    // A directory someone replaced with a link is never written through.
    let elsewhere = project.dir.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::create_dir_all(project.main()).unwrap();
    std::os::unix::fs::symlink(&elsewhere, project.main().join("src")).unwrap();
    let from = project.flat(&[("src/lib.rs", FILE, b"old\n")]);
    let to = project.flat(&[("src/lib.rs", FILE, b"new\n")]);
    let error = rewrite(&project.objects, &project.main(), &from, &to, HOST).unwrap_err();
    assert_eq!(error.code(), "invalid_path");
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn marker_is_written_after_the_files() {
    let project = Project::new();
    assert_eq!(read_marker(&project.fold_dir()).unwrap(), None);
    let first = project.tree(&[("a.txt", FILE, b"one\n")]);
    update(
        &project.objects,
        &project.main(),
        &project.fold_dir(),
        None,
        &first,
        r(1),
        HOST,
    )
    .unwrap();
    assert_eq!(read_marker(&project.fold_dir()).unwrap(), Some(r(1)));

    // A blob the object store lacks fails the rewrite before any file changes, and the marker stays.
    let mut broken = project.flat(&[("a.txt", FILE, b"two\n")]);
    broken.insert(
        b"b.txt".to_vec(),
        FlatEntry {
            kind: FILE,
            hash: ObjectHash::from_bytes([9; 32]),
        },
    );
    let broken = write_initial_tree(&project.objects, broken).unwrap();
    let error = update(
        &project.objects,
        &project.main(),
        &project.fold_dir(),
        Some(&first),
        &broken,
        r(2),
        HOST,
    )
    .unwrap_err();
    assert_eq!(error.code(), "object_missing");
    assert_eq!(read_marker(&project.fold_dir()).unwrap(), Some(r(1)));
    assert_eq!(project.read("a.txt"), "one\n");

    let third = project.tree(&[("a.txt", FILE, b"three\n")]);
    update(
        &project.objects,
        &project.main(),
        &project.fold_dir(),
        Some(&first),
        &third,
        r(3),
        HOST,
    )
    .unwrap();
    assert_eq!(project.read("a.txt"), "three\n");
    assert_eq!(read_marker(&project.fold_dir()).unwrap(), Some(r(3)));
    assert_eq!(
        fs::read_to_string(project.fold_dir().join(MARKER)).unwrap(),
        "r3\n"
    );

    write_marker(&project.fold_dir(), r(4)).unwrap();
    assert_eq!(read_marker(&project.fold_dir()).unwrap(), Some(r(4)));
    fs::write(project.fold_dir().join(MARKER), "not a revision").unwrap();
    assert_eq!(read_marker(&project.fold_dir()).unwrap(), None);
}

#[test]
fn rewrite_succeeds_with_the_portable_fallbacks() {
    let project = Project::new();
    let caps = Capabilities {
        symlinks: false,
        exec_bit: false,
        ..HOST
    };
    for caps in [caps, BARE] {
        let main = project.main();
        let from = project.flat(&[
            ("bin/run.sh", EntryKind::Executable, b"#!/bin/sh\n"),
            ("link", EntryKind::Symlink, b"bin/run.sh"),
        ]);
        let done = rewrite(&project.objects, &main, &FlatTree::new(), &from, caps).unwrap();
        assert_eq!(strings(&done.degraded), ["link"]);
        let link = fs::symlink_metadata(main.join("link")).unwrap();
        assert!(link.file_type().is_file());
        assert_eq!(project.read("link"), "bin/run.sh");
        assert_eq!(project.read("bin/run.sh"), "#!/bin/sh\n");
        for path in ["link", "bin/run.sh"] {
            assert_eq!(mode(&main.join(path)) & 0o222, 0, "{path} is writable");
        }

        let to = project.flat(&[
            ("bin/run.sh", EntryKind::Executable, b"#!/bin/sh\nexit 0\n"),
            ("link", EntryKind::Symlink, b"elsewhere"),
        ]);
        rewrite(&project.objects, &main, &from, &to, caps).unwrap();
        assert_eq!(project.read("link"), "elsewhere");
        assert_eq!(project.read("bin/run.sh"), "#!/bin/sh\nexit 0\n");

        rewrite(&project.objects, &main, &to, &FlatTree::new(), caps).unwrap();
        assert!(!main.join("bin").exists());
        assert!(!main.join("link").exists());
    }
}
