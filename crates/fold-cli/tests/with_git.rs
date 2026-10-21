use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

#[cfg(test)]
struct Sandbox {
    dir: TempDir,
}

#[cfg(test)]
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
impl Sandbox {
    fn new() -> Self {
        let sandbox = Self {
            dir: TempDir::new().unwrap(),
        };
        // A daemon of the test listens on a free port, never on the one a real daemon uses.
        fs::create_dir_all(sandbox.path("config/fold")).unwrap();
        fs::write(
            sandbox.path("config/fold/daemon.toml"),
            "listen = \"127.0.0.1:0\"\n",
        )
        .unwrap();
        sandbox
    }

    /// Below the canonical folder, the form fold records and prints.
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().canonicalize().unwrap().join(name)
    }

    fn socket(&self) -> PathBuf {
        self.path("fold.sock")
    }

    /// `fld <args>` in `cwd` with no real home, as the human `ana`, with the user's Git configuration left out.
    fn command(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("FOLD_HUMAN", "ana")
            .env(
                "PATH",
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_owned()),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command
    }

    fn fold_in(&self, cwd: &Path, args: &[&str]) -> Output {
        self.command(cwd, args).output().unwrap()
    }

    fn serve(&self) -> Daemon {
        let socket = self.socket();
        let child = self
            .command(
                &self.path(""),
                &["serve", "--socket", socket.to_str().unwrap()],
            )
            .env("XDG_RUNTIME_DIR", self.path("run"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let daemon = Daemon(child);
        let deadline = Instant::now() + Duration::from_secs(20);
        while std::os::unix::net::UnixStream::connect(&socket).is_err() {
            assert!(
                Instant::now() < deadline,
                "the daemon did not open its socket"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        daemon
    }
}

#[cfg(test)]
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[cfg(test)]
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn with_git_in_a_plain_project_turns_git_on_through_the_daemon() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("app");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("README.md"), "# app\n").unwrap();
    let init = sandbox.fold_in(&project, &["init"]);
    assert!(init.status.success(), "{init:?}");
    assert!(!project.join(".git").exists());

    let _daemon = sandbox.serve();
    let socket = sandbox.socket();
    let output = sandbox.fold_in(
        &project,
        &[
            "init",
            "--with-git",
            "--socket",
            socket.to_str().unwrap(),
            "--git-name",
            "Ana Lima",
            "--git-email",
            "ana@example.com",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(
        text.starts_with("turned Git on in project app: main is r2, Git commit "),
        "{text}"
    );
    assert!(
        text.contains("\nr2 records the .gitignore that keeps .fold/ out of Git\n"),
        "{text}"
    );
    assert!(
        text.ends_with("no Git remote yet: add one with fld remote add <name> <url> [<branch>]\n"),
        "{text}"
    );
    assert_eq!(
        fs::read_to_string(project.join(".gitignore")).unwrap(),
        ".fold/\n"
    );
    // The empty genesis gets no commit, so the history starts at r1.
    assert_eq!(git(&project, &["rev-list", "--count", "main"]).trim(), "2");
    assert_eq!(
        git(&project, &["log", "-1", "--format=%an <%ae>"]).trim(),
        "Ana Lima <ana@example.com>"
    );
    assert_eq!(git(&project, &["status", "--porcelain"]), "");
    assert_eq!(
        fs::read_link(project.join(".git/info/exclude")).unwrap(),
        Path::new("../../.fold/exclude")
    );

    // Git sync stays on.
    let again = sandbox.fold_in(
        &project,
        &["init", "--with-git", "--socket", socket.to_str().unwrap()],
    );
    assert!(!again.status.success());
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("Git sync is on already"),
        "{again:?}"
    );
}

#[cfg(test)]
fn removed_at(registry_db: &Path, remove: bool) -> Option<i64> {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let registry = fold_store::Store::open_registry(registry_db).await.unwrap();
        if remove {
            let now = fold_core::Timestamp::from_millis(5).unwrap();
            registry
                .write(async |tx| fold_store::projects::set_removed(tx, "app", now).await)
                .await
                .unwrap();
        }
        let found = fold_store::projects::find_registration(registry.read(), "app")
            .await
            .unwrap()
            .unwrap();
        registry.close().await.unwrap();
        found.removed_at.map(fold_core::Timestamp::as_millis)
    })
}

#[test]
fn with_git_in_a_removed_project_brings_it_back_and_turns_git_on() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("app");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("README.md"), "# app\n").unwrap();
    let init = sandbox.fold_in(&project, &["init"]);
    assert!(init.status.success(), "{init:?}");
    let registry_db = sandbox.path("data/fold/registry.db");
    assert_eq!(removed_at(&registry_db, true), Some(5));

    let _daemon = sandbox.serve();
    let socket = sandbox.socket();
    let output = sandbox.fold_in(
        &project,
        &[
            "init",
            "--with-git",
            "--socket",
            socket.to_str().unwrap(),
            "--git-name",
            "Ana Lima",
            "--git-email",
            "ana@example.com",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let text = stdout(&output);
    assert!(
        text.starts_with(
            "project app is back in fold\nturned Git on in project app: main is r2, Git commit "
        ),
        "{text}"
    );
    assert_eq!(removed_at(&registry_db, false), None);
    assert_eq!(git(&project, &["rev-list", "--count", "main"]).trim(), "2");
}

#[test]
fn with_git_in_a_git_repository_makes_its_head_the_genesis() {
    let sandbox = Sandbox::new();
    let repo = sandbox.path("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    fs::write(repo.join("README.md"), "# app\n").unwrap();
    git(&repo, &["add", "."]);
    git(
        &repo,
        &[
            "-c",
            "user.name=A",
            "-c",
            "user.email=a@b",
            "commit",
            "--quiet",
            "-m",
            "start",
        ],
    );
    // Uncommitted changes are refused first.
    fs::write(repo.join("README.md"), "# changed\n").unwrap();
    let refused = sandbox.fold_in(&repo, &["init", "--with-git"]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("commit or stash them, then run `fld init --with-git` again"),
        "{refused:?}"
    );
    git(&repo, &["checkout", "--quiet", "--", "README.md"]);
    let output = sandbox.fold_in(&repo, &["init", "--with-git"]);
    assert!(output.status.success(), "{output:?}");
    let head = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    let text = stdout(&output);
    assert!(
        text.contains(&format!("\nr0 is Git commit {head}\n")),
        "{text}"
    );
    assert!(
        text.contains("no Git remote yet: add one with fld remote add"),
        "{text}"
    );
}
