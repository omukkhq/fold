use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use fold_cli::claude_files;
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

    /// The environment holds no real home, no runtime directory and no Git configuration, so no run touches the user's registry, a real daemon or the user's Git settings.
    fn fold(&self, args: &[&str]) -> Output {
        self.fold_in(self.dir.path(), args)
    }

    fn fold_in(&self, cwd: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env(
                "PATH",
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_owned()),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        // Coverage runs name the profile file through this variable. `env_clear` drops every variable set before it, so it comes after.
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command.output().unwrap()
    }
}

#[cfg(test)]
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The warnings `fld init` prints for a `CLAUDE.md` above the temporary directory, which belongs to the machine running the test.
#[cfg(test)]
fn machine_warnings(project: &Path) -> String {
    let root = project.canonicalize().unwrap();
    claude_files::shadowing_files(&root, None)
        .iter()
        .map(|path| format!("{}\n", claude_files::shadowing_warning(path)))
        .collect()
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
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

/// A folder with a file, a nested file, an executable, a symlink, a `.gitignore` and a file it ignores.
#[cfg(test)]
fn folder(sandbox: &Sandbox) -> PathBuf {
    let root = sandbox.path("app");
    write(&root, "README.md", b"# app\n");
    write(&root, "src/lib.rs", b"pub fn x() {}\n");
    write(&root, "run.sh", b"#!/bin/sh\n");
    fs::set_permissions(root.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("src/lib.rs", root.join("link")).unwrap();
    write(&root, ".gitignore", b"*.log\n");
    write(&root, "debug.log", b"noise\n");
    root
}

#[test]
fn init_makes_the_folder_files_r1_and_its_first_commit() {
    let sandbox = Sandbox::new();
    let project = folder(&sandbox);
    let before = snapshot(&project);
    let output = sandbox.fold_in(&project, &["init"]);
    assert!(output.status.success(), "{output:?}");
    let canonical = project.canonicalize().unwrap();
    assert_eq!(
        stdout(&output),
        format!(
            "initialised project app at {}\n{}\n{}",
            canonical.display(),
            crate::common::r1_line(&project, 5, 46),
            machine_warnings(&project)
        )
    );
    let row = r1(&project);
    assert_eq!(row.number.to_string(), "r1");
    assert_eq!(row.import_source, None);
    // The folder's files stay as they were, writable, with the `.gitignore` the folder had.
    let after = snapshot(&project);
    for (path, entry) in &before {
        assert_eq!(after.get(path), Some(entry), "{path:?}");
    }
    assert!(!project.join(".foldignore").exists());
    assert_eq!(
        git(&project, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".claude/settings.json\n.claude/skills/fold/SKILL.md\n.gitignore\n.mcp.json\nAGENTS.md\nREADME.md\nlink\nrun.sh\nsrc/lib.rs\n"
    );
    assert_eq!(git(&project, &["log", "--format=%s"]), "fold r1\n");
    // The Claude Code files init writes are part of r1, so Git sees nothing new.
    assert_eq!(git(&project, &["status", "--porcelain"]), "");
}

#[test]
fn init_in_a_new_dir_writes_a_regular_gitignore_and_an_empty_foldignore() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("app");
    let output = sandbox.fold(&["init", project.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    assert!(stdout(&output).ends_with(&format!(
        "\n{}\n{}",
        crate::common::r1_line(&project, 0, 0),
        machine_warnings(&project)
    )));
    assert_eq!(r1(&project).number.to_string(), "r1");
    assert_eq!(fs::metadata(project.join(".foldignore")).unwrap().len(), 0);
    assert!(!project.join(".gitignore").is_symlink());
    assert!(
        fs::read_to_string(project.join(".gitignore"))
            .unwrap()
            .starts_with("# ")
    );
    assert_eq!(git(&project, &["log", "--format=%s"]), "fold r1\n");
    assert_eq!(
        git(&project, &["ls-tree", "--name-only", "HEAD"]),
        ".claude\n.foldignore\n.gitignore\n.mcp.json\nAGENTS.md\n"
    );
    assert_eq!(git(&project, &["status", "--porcelain"]), "");
    assert!(
        fs::read_to_string(project.join(".git/info/exclude"))
            .unwrap()
            .lines()
            .any(|line| line == "/.fold/")
    );
}

#[test]
fn a_file_over_the_size_limit_is_printed_as_skipped() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("app");
    write(&project, "small.txt", b"ok");
    let big = fs::File::create(project.join("big.bin")).unwrap();
    big.set_len(52_428_801).unwrap();
    let output = sandbox.fold_in(&project, &["init"]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        stdout(&output).ends_with(&format!(
            "\n{}\nskipped big.bin (too_large)\n{}",
            crate::common::r1_line(&project, 1, 2),
            machine_warnings(&project)
        )),
        "{}",
        stdout(&output)
    );
    assert_eq!(
        git(&project, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".claude/settings.json\n.claude/skills/fold/SKILL.md\n.foldignore\n.gitignore\n.mcp.json\nAGENTS.md\nsmall.txt\n"
    );
}

#[test]
fn init_and_from_git_each_refuse_the_other_kind_of_folder_with_a_hint() {
    let sandbox = Sandbox::new();
    let repo = sandbox.path("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    let output = sandbox.fold_in(&repo, &["init"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("invalid_request"), "{output:?}");
    assert!(
        stderr(&output).contains("hint: run fld init --from-git"),
        "{output:?}"
    );
    assert!(!repo.join(".fold").exists());

    // A folder inside the repository would get a repository nested in that one.
    let nested = repo.join("sub");
    fs::create_dir_all(&nested).unwrap();
    let top = repo.canonicalize().unwrap();
    for args in [&["init"][..], &["init", "--from-git"][..]] {
        let output = sandbox.fold_in(&nested, args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&output).contains(&format!(
                "hint: run fld init --from-git in {}",
                top.display()
            )),
            "{output:?}"
        );
        assert!(!nested.join(".fold").exists() && !nested.join(".git").exists());
    }

    let plain = sandbox.path("plain");
    fs::create_dir_all(&plain).unwrap();
    let output = sandbox.fold_in(&plain, &["init", "--from-git"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("hint: run fld init there"),
        "{output:?}"
    );
    assert!(!plain.join(".fold").exists());
    let output = sandbox.fold(&[
        "init",
        sandbox.path("absent").to_str().unwrap(),
        "--from-git",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!sandbox.path("absent").exists());
    assert!(
        !sandbox
            .path("data")
            .join("fold")
            .join("registry.db")
            .exists()
    );
}

#[test]
fn a_failing_import_removes_what_init_made_and_keeps_the_folder() {
    let sandbox = Sandbox::new();
    let project = folder(&sandbox);
    // Init can enter the folder and write in it, but not list it.
    fs::set_permissions(&project, fs::Permissions::from_mode(0o300)).unwrap();
    // A process that reads any directory, such as root, cannot make the import fail this way.
    let readable = fs::read_dir(&project).is_ok();
    let output = sandbox.fold_in(&project, &["init"]);
    fs::set_permissions(&project, fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        return;
    }
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(stderr(&output).contains("workspace_io"), "{output:?}");
    assert!(!project.join(".fold").exists());
    assert!(!project.join(".git").exists());
    // The Claude Code files init wrote for r1 go with it.
    for made in [".claude", ".mcp.json", "AGENTS.md"] {
        assert!(!project.join(made).exists(), "{made}");
    }
    assert_eq!(
        fs::read(project.join("src/lib.rs")).unwrap(),
        b"pub fn x() {}\n"
    );

    let retry = sandbox.fold_in(&project, &["init"]);
    assert!(retry.status.success(), "{retry:?}");
}

#[test]
fn init_on_an_initialised_project_is_refused_before_any_import() {
    let sandbox = Sandbox::new();
    let project = folder(&sandbox);
    assert!(sandbox.fold_in(&project, &["init"]).status.success());
    let first = r1(&project);
    write(&project, "new.txt", b"new");

    let again = sandbox.fold_in(&project, &["init"]);
    assert_eq!(again.status.code(), Some(1));
    assert!(stderr(&again).contains("project_exists"), "{again:?}");
    assert_eq!(r1(&project), first);
}
