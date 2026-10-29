use std::collections::BTreeMap;
use std::ffi::OsStr;
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
        self.fold_with(cwd, args, &[])
    }

    /// [`Self::fold_in`] with `vars` set last, so they replace a value of the same name.
    fn fold_with(&self, cwd: &Path, args: &[&str], vars: &[(&str, &OsStr)]) -> Output {
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
        command.envs(vars.iter().copied());
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
fn a_plain_init_makes_the_folder_files_r1_on_the_empty_genesis_and_no_git_data() {
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
            crate::common::r1_line(&project, 4, 40),
            machine_warnings(&project)
        )
    );
    let row = r1(&project);
    assert_eq!(row.number.to_string(), "r1");
    assert_eq!(
        row.parent.map(|parent| parent.to_string()).as_deref(),
        Some("r0")
    );
    // The folder's files stay as they were, writable; init adds only the empty `.foldignore` and the Claude Code files.
    let after = snapshot(&project);
    for (path, entry) in &before {
        assert_eq!(after.get(path), Some(entry), "{path:?}");
    }
    assert_eq!(
        fs::read_to_string(project.join(".gitignore")).unwrap(),
        "*.log\n"
    );
    assert_eq!(fs::metadata(project.join(".foldignore")).unwrap().len(), 0);
    assert!(!project.join(".git").exists());
}

#[test]
fn init_with_git_records_the_gitignore_change_as_r2_and_commits_from_r1() {
    let sandbox = Sandbox::new();
    let project = folder(&sandbox);
    let output = sandbox.fold_in(&project, &["init", "--with-git"]);
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(
        text.contains("\nr2 records the .gitignore that keeps .fold/ out of Git\nGit sync is on: every revision gets its Git commit in .git\n"),
        "{text}"
    );
    assert_eq!(r1(&project).number.to_string(), "r2");
    assert_eq!(
        fs::read_to_string(project.join(".gitignore")).unwrap(),
        "*.log\n.fold/\n"
    );
    assert_eq!(
        git(&project, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".foldignore\n.gitignore\nAGENTS.md\nREADME.md\nlink\nrun.sh\nsrc/lib.rs\n"
    );
    // fold's skills stay in the folder, and Git leaves them out as fold does.
    for path in fold_cli::claude_files::SKILL_FILES {
        assert!(project.join(path).is_file(), "{path}");
    }
    // The empty genesis gets no commit, so the history starts at r1.
    assert_eq!(git(&project, &["rev-list", "--count", "HEAD"]).trim(), "2");
    assert_eq!(git(&project, &["status", "--porcelain"]), "");
    assert!(
        fs::read_to_string(project.join(".git/info/exclude"))
            .unwrap()
            .lines()
            .any(|line| line == "/.fold/")
    );
}

#[test]
fn init_with_git_in_a_new_dir_writes_a_gitignore_that_copies_the_foldignore() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("app");
    let output = sandbox.fold(&["init", "--with-git", project.to_str().unwrap()]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::metadata(project.join(".foldignore")).unwrap().len(), 0);
    assert!(!project.join(".gitignore").is_symlink());
    assert_eq!(
        fs::read_to_string(project.join(".gitignore")).unwrap(),
        ".fold/\n"
    );
    assert_eq!(
        git(&project, &["ls-tree", "--name-only", "HEAD"]),
        ".foldignore\n.gitignore\nAGENTS.md\n"
    );
    assert_eq!(git(&project, &["status", "--porcelain"]), "");
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
}

#[test]
fn a_plain_init_refuses_a_git_repository_and_both_refuse_a_folder_inside_one() {
    let sandbox = Sandbox::new();
    let repo = sandbox.path("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    let output = sandbox.fold_in(&repo, &["init"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("invalid_request"), "{output:?}");
    assert!(
        stderr(&output).contains("hint: run `fld init --with-git`"),
        "{output:?}"
    );
    assert!(!repo.join(".fold").exists());

    // A folder inside the repository would get a repository nested in that one.
    let nested = repo.join("sub");
    fs::create_dir_all(&nested).unwrap();
    let top = repo.canonicalize().unwrap();
    for args in [&["init"][..], &["init", "--with-git"][..]] {
        let output = sandbox.fold_in(&nested, args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&output).contains(&format!(
                "hint: run `fld init --with-git` in {}",
                top.display()
            )),
            "{output:?}"
        );
        assert!(!nested.join(".fold").exists() && !nested.join(".git").exists());
    }

    assert!(
        !sandbox
            .path("data")
            .join("fold")
            .join("registry.db")
            .exists()
    );
    // --from-git is gone: --with-git takes a Git repository, a fold project or a new folder.
    assert!(
        !sandbox
            .fold_in(&repo, &["init", "--from-git"])
            .status
            .success()
    );
}

#[test]
fn a_plain_init_finds_the_repository_around_it_without_git_and_never_above_a_ceiling() {
    let sandbox = Sandbox::new();
    let repo = sandbox.path("repo");
    let nested = repo.join("sub");
    fs::create_dir_all(&nested).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    let top = repo.canonicalize().unwrap();
    let no_git = sandbox.path("no-git");
    fs::create_dir(&no_git).unwrap();
    let output = sandbox.fold_with(&nested, &["init"], &[("PATH", no_git.as_os_str())]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        stderr(&output).contains(&format!(
            "hint: run `fld init --with-git` in {}",
            top.display()
        )),
        "{output:?}"
    );
    assert!(!nested.join(".fold").exists());

    // Git refuses to read a repository that another user owns, and that repository still holds the folder.
    for args in [&["init"][..], &["init", "--with-git"][..]] {
        let output = sandbox.fold_with(
            &nested,
            args,
            &[("GIT_TEST_ASSUME_DIFFERENT_OWNER", OsStr::new("1"))],
        );
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(
            stderr(&output).contains(&format!(
                "hint: run `fld init --with-git` in {}",
                top.display()
            )),
            "{output:?}"
        );
        assert!(!nested.join(".fold").exists() && !nested.join(".git").exists());
    }

    // Git never goes up into a ceiling folder, so the folder below one is inside no repository.
    let output = sandbox.fold_with(
        &nested,
        &["init"],
        &[("GIT_CEILING_DIRECTORIES", repo.as_os_str())],
    );
    assert!(output.status.success(), "{output:?}");
    assert!(nested.join(".fold").is_dir() && !nested.join(".git").exists());
}

#[test]
fn both_inits_refuse_a_folder_in_a_git_directory_with_or_without_git() {
    let sandbox = Sandbox::new();
    let repo = sandbox.path("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    git(
        sandbox.dir.path(),
        &["init", "--quiet", "--bare", "bare.git"],
    );
    let (dot_git, bare) = (repo.join(".git"), sandbox.path("bare.git"));
    fs::create_dir(dot_git.join("notes")).unwrap();
    fs::create_dir(bare.join("notes")).unwrap();
    let no_git = sandbox.path("no-git");
    fs::create_dir(&no_git).unwrap();
    // The folder above the sandbox is a ceiling, so a repository around it never changes an answer.
    let above = sandbox.dir.path().canonicalize().unwrap();
    let ceiling = above.parent().unwrap().as_os_str();
    let with_git = [("GIT_CEILING_DIRECTORIES", ceiling)];
    let without_git = [
        ("GIT_CEILING_DIRECTORIES", ceiling),
        ("PATH", no_git.as_os_str()),
    ];
    for (folder, git_dir) in [
        (dot_git.join("notes"), &dot_git),
        (dot_git.clone(), &dot_git),
        (bare.join("notes"), &bare),
        (bare.clone(), &bare),
    ] {
        let hint = format!(
            "hint: choose a folder outside {}",
            git_dir.canonicalize().unwrap().display()
        );
        for (args, vars) in [
            (&["init"][..], &with_git[..]),
            (&["init", "--with-git"][..], &with_git[..]),
            (&["init"][..], &without_git[..]),
        ] {
            let output = sandbox.fold_with(&folder, args, vars);
            assert_eq!(output.status.code(), Some(1), "{args:?} {output:?}");
            assert!(stderr(&output).contains(&hint), "{args:?} {output:?}");
            assert!(!folder.join(".fold").exists());
        }
    }
    // `fld init DIR` makes DIR inside the Git directory, and takes it away again with the refusal.
    let new = dot_git.join("new");
    for args in [&["init"][..], &["init", "--with-git"][..]] {
        let mut args = args.to_vec();
        args.push(new.to_str().unwrap());
        let output = sandbox.fold_with(sandbox.dir.path(), &args, &with_git);
        assert_eq!(output.status.code(), Some(1), "{args:?} {output:?}");
        assert!(
            stderr(&output).contains("inside the Git directory"),
            "{output:?}"
        );
        assert!(!new.exists());
    }
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
    for made in [".claude", "AGENTS.md"] {
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
