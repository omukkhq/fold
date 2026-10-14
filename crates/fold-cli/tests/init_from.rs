use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use fold_core::ProjectPaths;
use fold_store::Store;
use fold_store::workspaces::{RevisionRow, head};
use tempfile::TempDir;

#[cfg(test)]
struct Sandbox {
    dir: TempDir,
}

#[cfg(test)]
impl Sandbox {
    fn new() -> Self {
        Self {
            dir: TempDir::new().unwrap(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// The environment holds no real home and no runtime directory, so no run touches the user's registry or a real daemon.
    fn fold(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fold"));
        // Coverage runs name the profile file through this variable.
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command
            .args(args)
            .current_dir(self.dir.path())
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_DATA_HOME", self.path("data"))
            .output()
            .unwrap()
    }
}

#[cfg(test)]
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[cfg(test)]
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[cfg(test)]
fn write(root: &Path, path: &str, bytes: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

#[cfg(test)]
fn r1(project: &Path) -> RevisionRow {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = Store::open_project(&ProjectPaths::new(project))
            .await
            .unwrap();
        let row = head(store.read()).await.unwrap().unwrap();
        store.close().await.unwrap();
        row
    })
}

/// Every entry below `root` with its kind, mode and content or link target, so a test can tell whether anything changed.
#[cfg(test)]
fn snapshot(root: &Path) -> BTreeMap<PathBuf, (u32, Vec<u8>)> {
    let mut found = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let mode = meta.permissions().mode();
            let content = if meta.file_type().is_symlink() {
                fs::read_link(&path)
                    .unwrap()
                    .into_os_string()
                    .into_encoded_bytes()
            } else if meta.is_dir() {
                pending.push(path.clone());
                Vec::new()
            } else {
                fs::read(&path).unwrap()
            };
            found.insert(path.strip_prefix(root).unwrap().to_owned(), (mode, content));
        }
    }
    found
}

#[cfg(test)]
fn source(sandbox: &Sandbox) -> PathBuf {
    let root = sandbox.path("source");
    write(&root, "README.md", b"# app\n");
    write(&root, "src/lib.rs", b"pub fn x() {}\n");
    write(&root, "run.sh", b"#!/bin/sh\n");
    fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("src/lib.rs", root.join("link")).unwrap();
    write(&root, ".git/HEAD", b"ref: refs/heads/main\n");
    write(&root, ".gitignore", b"*.log\n");
    write(&root, "debug.log", b"noise\n");
    root
}

#[test]
fn init_from_creates_r1_as_main_and_fills_main_read_only() {
    let sandbox = Sandbox::new();
    let source = source(&sandbox);
    let before = snapshot(&source);
    let project = sandbox.path("app");
    // A path through `..` checks that the stored source is the canonical one.
    let given = sandbox.path("home/../source");
    fs::create_dir_all(sandbox.path("home")).unwrap();
    let output = sandbox.fold(&[
        "init",
        project.to_str().unwrap(),
        "--from",
        given.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{output:?}");
    let canonical = project.canonicalize().unwrap();
    assert_eq!(
        stdout(&output),
        format!(
            "initialised project app at {}\nr1: 5 files, 46 bytes\n",
            canonical.display()
        )
    );

    let row = r1(&project);
    assert_eq!(row.number.to_string(), "r1");
    assert_eq!(
        row.import_source.as_deref(),
        Some(source.canonicalize().unwrap().to_str().unwrap())
    );
    let main = project.join("main");
    assert_eq!(
        fs::read(main.join("src/lib.rs")).unwrap(),
        b"pub fn x() {}\n"
    );
    assert_eq!(fs::read(main.join("README.md")).unwrap(), b"# app\n");
    assert_eq!(
        fs::read_link(main.join("link")).unwrap(),
        Path::new("src/lib.rs")
    );
    let mode = |path: &str| fs::metadata(main.join(path)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode("src/lib.rs"), 0o444);
    assert_eq!(mode("run.sh"), 0o555);
    assert!(!main.join(".git").exists());
    assert!(!main.join("debug.log").exists());
    assert_eq!(fs::read(main.join(".gitignore")).unwrap(), b"*.log\n");
    assert_eq!(snapshot(&source), before, "the source is only read");
}

#[test]
fn plain_init_records_an_empty_r1_and_writes_no_ignore_file() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("app");
    let output = sandbox.fold(&["init", project.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).ends_with("\nr1: 0 files, 0 bytes\n"));
    let row = r1(&project);
    assert_eq!(row.number.to_string(), "r1");
    assert_eq!(row.import_source, None);
    let main = project.join("main");
    assert!(main.is_dir());
    let tracked: Vec<_> = snapshot(&main)
        .into_keys()
        .filter(|path| !path.starts_with(".fold"))
        .collect();
    assert!(tracked.is_empty(), "{tracked:?}");
    for dir in [&project, &main] {
        assert!(!dir.join(".gitignore").exists());
        assert!(!dir.join(".foldignore").exists());
    }
}

#[test]
fn a_file_over_the_size_limit_is_printed_as_skipped() {
    let sandbox = Sandbox::new();
    let source = sandbox.path("source");
    write(&source, "small.txt", b"ok");
    let big = fs::File::create(source.join("big.bin")).unwrap();
    big.set_len(52_428_801).unwrap();
    let project = sandbox.path("app");
    let output = sandbox.fold(&[
        "init",
        project.to_str().unwrap(),
        "--from",
        source.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        stdout(&output).ends_with("\nr1: 1 file, 2 bytes\nskipped big.bin (too_large)\n"),
        "{}",
        stdout(&output)
    );
    assert!(!project.join("main/big.bin").exists());
}

#[test]
fn a_missing_source_fails_with_invalid_request_before_anything_is_created() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("app");
    let missing = sandbox.path("absent");
    let output = sandbox.fold(&[
        "init",
        project.to_str().unwrap(),
        "--from",
        missing.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("invalid_request"), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(!project.exists());
    assert!(
        !sandbox
            .path("data")
            .join("fold")
            .join("registry.db")
            .exists()
    );

    write(&sandbox.path("."), "file.txt", b"x");
    let output = sandbox.fold(&[
        "init",
        project.to_str().unwrap(),
        "--from",
        sandbox.path("file.txt").to_str().unwrap(),
    ]);
    assert!(stderr(&output).contains("invalid_request"), "{output:?}");
    assert!(!project.join(".fold").exists());
}

#[test]
fn a_failing_import_removes_its_fold_dir_and_leaves_the_source_unchanged() {
    let sandbox = Sandbox::new();
    let source = source(&sandbox);
    let before = snapshot(&source);
    fs::set_permissions(&source, fs::Permissions::from_mode(0o000)).unwrap();
    // A process that reads any directory, such as root, cannot make the import fail this way.
    let readable = fs::read_dir(&source).is_ok();
    let project = sandbox.path("app");
    let output = sandbox.fold(&[
        "init",
        project.to_str().unwrap(),
        "--from",
        source.to_str().unwrap(),
    ]);
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return;
    }
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr(&output).contains("workspace_io"), "{output:?}");
    assert!(!project.join(".fold").exists());
    assert!(!project.join("main").exists());
    assert_eq!(snapshot(&source), before);

    let retry = sandbox.fold(&[
        "init",
        project.to_str().unwrap(),
        "--from",
        source.to_str().unwrap(),
    ]);
    assert!(retry.status.success(), "{retry:?}");
}

#[test]
fn init_on_an_initialised_project_is_refused_before_any_import() {
    let sandbox = Sandbox::new();
    let source = source(&sandbox);
    let project = sandbox.path("app");
    let args = [
        "init",
        project.to_str().unwrap(),
        "--from",
        source.to_str().unwrap(),
    ];
    assert!(sandbox.fold(&args).status.success());
    let first = r1(&project);
    write(&source, "new.txt", b"new");

    let again = sandbox.fold(&args);
    assert_eq!(again.status.code(), Some(1));
    assert!(stderr(&again).contains("project_exists"), "{again:?}");
    assert_eq!(r1(&project), first);
    assert!(!project.join("main/new.txt").exists());
}
