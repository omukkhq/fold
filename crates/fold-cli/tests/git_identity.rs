use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
        let global = self.path("gitconfig");
        let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
        command
            .args(args)
            .current_dir(self.dir.path())
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", &global)
            .envs(env.iter().copied());
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command.output().unwrap()
    }

    fn write_global(&self, text: &str) {
        fs::write(self.path("gitconfig"), text).unwrap();
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
    let output = sandbox.fold(&["init", project.to_str().unwrap()], &[]);
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
        &["init", project.to_str().unwrap(), "--git-name=-dash"],
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
    let output = sandbox.fold(&["init", project.to_str().unwrap()], &[]);
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
    let output = sandbox.fold(&["init", project.to_str().unwrap()], &[]);
    assert!(output.status.success(), "{output:?}");
    let text = stderr(&output);
    assert!(text.contains("git config user.email"), "{text}");
    assert!(!text.contains("git config user.name"), "{text}");
}

#[test]
fn the_environment_counts_as_an_identity() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("p");
    let output = sandbox.fold(
        &["init", project.to_str().unwrap()],
        &[
            ("GIT_AUTHOR_NAME", "A"),
            ("GIT_COMMITTER_NAME", "A"),
            ("GIT_AUTHOR_EMAIL", "a@example.com"),
            ("GIT_COMMITTER_EMAIL", "a@example.com"),
        ],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stderr(&output), "");
}

#[test]
fn a_bad_flag_value_is_refused_before_anything_is_created() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("p");
    let output = sandbox.fold(
        &["init", project.to_str().unwrap(), "--git-email", "nobody"],
        &[],
    );
    assert!(!output.status.success());
    assert!(stderr(&output).contains("--git-email"), "{output:?}");
    assert!(!project.exists());
}

#[test]
fn from_git_takes_no_identity_flags() {
    let sandbox = Sandbox::new();
    let output = sandbox.fold(&["init", "--from-git", "--git-name", "Ana"], &[]);
    assert!(!output.status.success());
}
