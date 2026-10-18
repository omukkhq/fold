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
impl Sandbox {
    fn new() -> Self {
        Self {
            dir: TempDir::new().unwrap(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// The Git config files and the identity variables are the sandbox's own, so the machine's Git identity never decides a test.
    fn fold(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        self.command(args, env).output().unwrap()
    }

    fn command(&self, args: &[&str], env: &[(&str, &str)]) -> Command {
        let global = self.path("gitconfig");
        let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
        command
            .args(args)
            .current_dir(self.dir.path())
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_RUNTIME_DIR", self.path("run"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &global)
            .envs(env.iter().copied());
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command
    }

    fn socket(&self) -> PathBuf {
        self.path("fold.sock")
    }

    /// `fld serve` on the sandbox socket, with no Git identity variable, stopped when the guard drops.
    fn serve(&self) -> Served {
        let config = self.path("home/.config/fold");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("daemon.toml"), "listen = \"127.0.0.1:0\"\n").unwrap();
        let socket = self.socket();
        let child = self
            .command(&["serve", "--socket", socket.to_str().unwrap()], &[])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let served = Served(child);
        let deadline = Instant::now() + Duration::from_secs(20);
        while std::os::unix::net::UnixStream::connect(&socket).is_err() {
            assert!(
                Instant::now() < deadline,
                "the daemon did not open its socket"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        served
    }

    /// A repository with one commit, made by a `git` that names its identity on the command line, so no config file holds one.
    fn repository(&self, name: &str) -> PathBuf {
        let repo = self.path(name);
        fs::create_dir(&repo).unwrap();
        fs::write(repo.join("README.md"), "# app\n").unwrap();
        for args in [
            &["init", "--quiet", "--initial-branch=main"][..],
            &["add", "README.md"],
            &[
                "-c",
                "user.name=Setup",
                "-c",
                "user.email=setup@example.com",
                "commit",
                "--quiet",
                "-m",
                "one",
            ],
        ] {
            let status = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", self.path("gitconfig"))
                .status()
                .unwrap();
            assert!(status.success());
        }
        repo
    }

    fn write_global(&self, text: &str) {
        fs::write(self.path("gitconfig"), text).unwrap();
    }
}

#[cfg(test)]
struct Served(Child);

#[cfg(test)]
impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(test)]
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The value in the project's own `.git/config`, which a global file never shows.
#[cfg(test)]
fn local(project: &Path, key: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["config", "--local", "--get", key])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
}

#[test]
fn without_a_terminal_init_warns_and_succeeds() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("p");
    let output = sandbox.fold(&["init", "--with-git", project.to_str().unwrap()], &[]);
    assert!(output.status.success(), "{output:?}");
    let text = stderr(&output);
    assert!(text.contains("git config user.name"), "{text}");
    assert!(text.contains("git config user.email"), "{text}");
    assert_eq!(local(&project, "user.name"), None);
    assert_eq!(local(&project, "user.email"), None);
}

#[test]
fn the_flags_write_the_project_config_only() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("p");
    let output = sandbox.fold(
        &[
            "init",
            "--with-git",
            project.to_str().unwrap(),
            "--git-name",
            "Ana Lima",
            "--git-email",
            "ana@example.com",
        ],
        &[],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stderr(&output), "");
    assert_eq!(local(&project, "user.name").as_deref(), Some("Ana Lima"));
    assert_eq!(
        local(&project, "user.email").as_deref(),
        Some("ana@example.com")
    );
    assert!(!sandbox.path("gitconfig").exists());
}

#[test]
fn a_flag_value_that_starts_with_a_dash_is_a_value() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("p");
    let output = sandbox.fold(
        &[
            "init",
            "--with-git",
            project.to_str().unwrap(),
            "--git-name=-dash",
        ],
        &[],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(local(&project, "user.name").as_deref(), Some("-dash"));
    let text = stderr(&output);
    assert!(text.contains("git config user.email"), "{text}");
    assert!(!text.contains("git config user.name"), "{text}");
}

#[test]
fn a_global_identity_is_left_alone() {
    let sandbox = Sandbox::new();
    sandbox.write_global("[user]\n\tname = Global\n\temail = global@example.com\n");
    let project = sandbox.path("p");
    let output = sandbox.fold(&["init", "--with-git", project.to_str().unwrap()], &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stderr(&output), "");
    assert_eq!(local(&project, "user.name"), None);
    assert_eq!(local(&project, "user.email"), None);
}

#[test]
fn only_the_missing_value_is_named() {
    let sandbox = Sandbox::new();
    sandbox.write_global("[user]\n\tname = Global\n");
    let project = sandbox.path("p");
    let output = sandbox.fold(&["init", "--with-git", project.to_str().unwrap()], &[]);
    assert!(output.status.success(), "{output:?}");
    let text = stderr(&output);
    assert!(text.contains("git config user.email"), "{text}");
    assert!(!text.contains("git config user.name"), "{text}");
}

/// Git's identity variables in the environment of `fld init`, which the daemon that writes the later commits never sees.
#[cfg(test)]
const IDENTITY_VARIABLES: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "Env Name"),
    ("GIT_COMMITTER_NAME", "Other"),
    ("GIT_AUTHOR_EMAIL", "env@example.com"),
    ("EMAIL", "other@example.com"),
];

#[test]
fn identity_variables_alone_go_into_the_project_config_and_every_fold_commit() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("p");
    let output = sandbox.fold(
        &["init", "--with-git", project.to_str().unwrap()],
        &IDENTITY_VARIABLES,
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stderr(&output), "");
    assert_eq!(local(&project, "user.name").as_deref(), Some("Env Name"));
    assert_eq!(
        local(&project, "user.email").as_deref(),
        Some("env@example.com")
    );
    let env = "Env Name <env@example.com>|Env Name <env@example.com>";
    assert_eq!(first_commit(&project), env);

    let _served = sandbox.serve();
    fs::write(project.join("notes.txt"), "mine\n").unwrap();
    let socket = sandbox.socket();
    let recorded = sandbox.fold(
        &[
            "record",
            "--socket",
            socket.to_str().unwrap(),
            "--project",
            project.to_str().unwrap(),
        ],
        &[("FOLD_HUMAN", "ana")],
    );
    assert!(recorded.status.success(), "{recorded:?}");
    let output = Command::new("git")
        .arg("-C")
        .arg(&project)
        .args(["log", "--format=%an <%ae>|%cn <%ce>", "HEAD"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    let people = String::from_utf8(output.stdout).unwrap();
    assert!(people.lines().count() >= 2, "{people}");
    assert!(people.lines().all(|line| line == env), "{people}");
}

#[test]
fn a_config_identity_wins_over_identity_variables_and_is_not_copied() {
    let sandbox = Sandbox::new();
    sandbox.write_global("[user]\n\tname = Global\n\temail = global@example.com\n");
    let project = sandbox.path("p");
    let output = sandbox.fold(
        &["init", "--with-git", project.to_str().unwrap()],
        &IDENTITY_VARIABLES,
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stderr(&output), "");
    assert_eq!(local(&project, "user.name"), None);
    assert_eq!(local(&project, "user.email"), None);
    assert_eq!(
        first_commit(&project),
        "Global <global@example.com>|Global <global@example.com>"
    );
}

#[test]
fn invalid_identity_variables_count_as_unset() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("p");
    let output = sandbox.fold(
        &["init", "--with-git", project.to_str().unwrap()],
        &[("GIT_AUTHOR_NAME", "Bad <name>"), ("EMAIL", "nobody")],
    );
    assert!(output.status.success(), "{output:?}");
    let text = stderr(&output);
    assert!(text.contains("git config user.name"), "{text}");
    assert!(text.contains("git config user.email"), "{text}");
    assert_eq!(local(&project, "user.name"), None);
}

#[test]
fn a_bad_flag_value_is_refused_before_anything_is_created() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("p");
    let output = sandbox.fold(
        &[
            "init",
            "--with-git",
            project.to_str().unwrap(),
            "--git-email",
            "nobody",
        ],
        &[],
    );
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--git-email"), "{output:?}");
    assert!(!project.exists());
}

#[test]
fn from_git_asks_nothing_and_warns_about_nothing() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repository("app");
    let output = sandbox.fold(&["init", "--from-git", repo.to_str().unwrap()], &[]);
    assert!(output.status.success(), "{output:?}");
    assert!(!stderr(&output).contains("git config"), "{output:?}");
    assert_eq!(local(&repo, "user.name"), None);
}

#[test]
fn a_folder_inside_a_repository_is_refused_without_the_question() {
    let sandbox = Sandbox::new();
    let repo = sandbox.repository("app");
    let output = sandbox.fold(&["init", repo.join("sub").to_str().unwrap()], &[]);
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("inside the Git repository"), "{text}");
    assert!(!text.contains("git config"), "{text}");
}

#[test]
fn from_git_takes_no_identity_flags() {
    let sandbox = Sandbox::new();
    let output = sandbox.fold(&["init", "--from-git", "--git-name", "Ana"], &[]);
    assert!(!output.status.success());
}

#[test]
fn a_plain_init_asks_nothing_and_makes_no_repository() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("p");
    let output = sandbox.fold(&["init", project.to_str().unwrap()], &[]);
    assert!(output.status.success(), "{output:?}");
    assert!(!stderr(&output).contains("git config"), "{output:?}");
    assert!(!project.join(".git").exists());
    let output = sandbox.fold(
        &[
            "init",
            sandbox.path("q").to_str().unwrap(),
            "--git-name",
            "Ana",
        ],
        &[],
    );
    assert!(
        !output.status.success(),
        "the identity is only for --with-git"
    );
}

/// The author and committer of the commit of `r1`, as Git shows them.
#[cfg(test)]
fn first_commit(project: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["log", "-1", "--format=%an <%ae>|%cn <%ce>"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn the_first_commit_takes_the_flag_identity() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("p");
    let output = sandbox.fold(
        &[
            "init",
            "--with-git",
            project.to_str().unwrap(),
            "--git-name",
            "Ana Lima",
            "--git-email",
            "ana@example.com",
        ],
        &[],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        first_commit(&project),
        "Ana Lima <ana@example.com>|Ana Lima <ana@example.com>"
    );
}

#[test]
fn the_first_commit_takes_the_global_identity() {
    let sandbox = Sandbox::new();
    sandbox.write_global("[user]\n\tname = Global\n\temail = global@example.com\n");
    let project = sandbox.path("p");
    let output = sandbox.fold(&["init", "--with-git", project.to_str().unwrap()], &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        first_commit(&project),
        "Global <global@example.com>|Global <global@example.com>"
    );
}

#[test]
fn the_first_commit_has_the_fold_signature_without_an_identity() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("p");
    let output = sandbox.fold(&["init", "--with-git", project.to_str().unwrap()], &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        first_commit(&project),
        "fold <fold@localhost>|fold <fold@localhost>"
    );
}
