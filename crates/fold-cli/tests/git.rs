use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use fold_cli::claude_files;
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

    /// The environment holds no real home and no runtime directory, so no run touches the user's registry or a real daemon. `path` is the `PATH` the binary sees.
    fn fold_in(&self, cwd: &Path, path: &str, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("PATH", path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        // Coverage runs name the profile file through this variable. `env_clear` drops every variable set before it, so it comes after.
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command.output().unwrap()
    }

    fn fold(&self, args: &[&str]) -> Output {
        self.fold_in(self.dir.path(), &system_path(), args)
    }
}

#[cfg(test)]
fn system_path() -> String {
    std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_owned())
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

#[cfg(test)]
fn repo(sandbox: &Sandbox) -> (PathBuf, String) {
    let repo = sandbox.path("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    fs::write(repo.join("README.md"), "# app\n").unwrap();
    fs::write(repo.join("big.bin"), vec![0u8; 64]).unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "start"]);
    let head = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    (repo, head)
}

#[test]
fn init_from_git_prints_r1_and_its_commit() {
    let sandbox = Sandbox::new();
    let (repo, head) = repo(&sandbox);
    let output = sandbox.fold_in(&repo, &system_path(), &["init", "--from-git"]);
    assert!(output.status.success(), "{output:?}");
    let canonical = repo.canonicalize().unwrap();
    assert_eq!(
        stdout(&output),
        format!(
            "initialised project repo at {}\nr1: 2 files, 70 bytes\nr1 is Git commit {head}\n{}",
            canonical.display(),
            machine_warnings(&repo)
        )
    );
    let map = fs::read_to_string(repo.join(".fold/git-sync.map")).unwrap();
    assert_eq!(map, format!("r1 {head}\n"));
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]).trim(), head);
    let both = sandbox.fold(&["init", "x", "--from-git", "b"]);
    assert!(!both.status.success());
}

#[test]
fn init_from_git_without_git_on_path_is_git_not_found() {
    let sandbox = Sandbox::new();
    let (repo, _) = repo(&sandbox);
    fs::create_dir_all(sandbox.path("empty-bin")).unwrap();
    let empty_path = sandbox.path("empty-bin");
    let output = sandbox.fold_in(&repo, empty_path.to_str().unwrap(), &["init", "--from-git"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("git_not_found"), "{output:?}");
    assert!(!repo.join(".fold").exists());
}
