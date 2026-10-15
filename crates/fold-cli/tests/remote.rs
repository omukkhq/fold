use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use fold_core::ProjectConfig;
use tempfile::TempDir;

#[cfg(test)]
struct Sandbox {
    dir: TempDir,
}

#[cfg(test)]
impl Sandbox {
    fn new() -> Self {
        let sandbox = Self {
            dir: TempDir::new().unwrap(),
        };
        fs::create_dir_all(sandbox.path("config/fold")).unwrap();
        fs::write(
            sandbox.path("config/fold/daemon.toml"),
            "listen = \"127.0.0.1:0\"\n",
        )
        .unwrap();
        fs::create_dir_all(sandbox.path("empty-bin")).unwrap();
        sandbox
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn socket(&self) -> PathBuf {
        self.path("fold.sock")
    }

    /// `fld <args>` in the sandbox with no real home, no runtime directory and `path` as `PATH`.
    fn command(&self, path: &str, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
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
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("FOLD_HUMAN", "ana")
            .env("PATH", path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        command
    }

    fn fold_with_path(&self, path: &str, args: &[&str]) -> Output {
        self.command(path, args).output().unwrap()
    }

    fn fold(&self, args: &[&str]) -> Output {
        self.fold_with_path(&system_path(), args)
    }

    fn empty_path(&self) -> String {
        self.path("empty-bin").to_str().unwrap().to_owned()
    }

    /// A project `app` with its `r1`, registered in the sandbox.
    fn project(&self) -> PathBuf {
        let init = self.fold(&["init", "app"]);
        assert!(init.status.success(), "{init:?}");
        self.path("app")
    }

    /// An empty bare repository standing in for the remote.
    fn remote(&self, name: &str) -> PathBuf {
        let remote = self.path(name);
        fs::create_dir(&remote).unwrap();
        git(
            &remote,
            &["init", "--quiet", "--bare", "--initial-branch=main"],
        );
        remote
    }

    /// `fld serve` on the sandbox socket with `path` as `PATH`, stopped when the guard drops.
    fn serve(&self, path: &str) -> Daemon {
        let _ = fs::remove_file(self.socket());
        let socket = self.socket();
        let child = self
            .command(path, &["serve", "--socket", socket.to_str().unwrap()])
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

    fn push(&self, project: &Path) -> Output {
        let socket = self.socket();
        self.fold(&[
            "push",
            "--socket",
            socket.to_str().unwrap(),
            "--project",
            project.to_str().unwrap(),
        ])
    }
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
fn system_path() -> String {
    std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_owned())
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
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn remote_set_writes_a_named_section_keeps_the_rest_and_configures_the_mirror() {
    let sandbox = Sandbox::new();
    let project = sandbox.project();
    let config_path = project.join(".fold/config.toml");
    let template = fs::read_to_string(&config_path).unwrap();
    let first = sandbox.remote("first.git");

    // Run from inside the project with a relative path, which is stored absolute.
    let output = sandbox
        .command(
            &system_path(),
            &["remote", "set", "origin", "../../first.git", "main"],
        )
        .current_dir(project.join("main"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        "remote origin set: fold syncs main with its branch main\n"
    );
    let text = fs::read_to_string(&config_path).unwrap();
    assert!(text.starts_with(&template), "{text}");
    let canonical = first.canonicalize().unwrap();
    let config = ProjectConfig::from_toml(&text).unwrap();
    assert_eq!(config.remotes["origin"].url, canonical.to_str().unwrap());
    assert_eq!(config.remotes["origin"].branch, "main");
    assert!(!config.remotes["origin"].push_on_integrate);
    let mirror = project.join(".fold/git");
    assert_eq!(
        git(&mirror, &["rev-parse", "--is-bare-repository"]),
        "true\n"
    );
    assert_eq!(
        git(&mirror, &["remote", "get-url", "origin"]),
        format!("{}\n", canonical.display())
    );

    let second = sandbox.remote("second.git");
    let project_arg = project.to_str().unwrap();
    let output = sandbox.fold(&[
        "remote",
        "set",
        "backup",
        second.to_str().unwrap(),
        "trunk",
        "--project",
        project_arg,
    ]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        git(&mirror, &["remote", "get-url", "backup"]),
        format!("{}\n", second.display())
    );
    let config = ProjectConfig::from_toml(&fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(config.remotes.len(), 2);
    assert_eq!(config.remotes["backup"].branch, "trunk");
    let listed = sandbox.fold(&["remote", "--project", project_arg]);
    assert_eq!(
        stdout(&listed),
        format!(
            "backup\ttrunk\t{}\norigin\tmain\t{}\n",
            second.display(),
            canonical.display()
        )
    );
    let removed = sandbox.fold(&["remote", "remove", "backup", "--project", project_arg]);
    assert_eq!(stdout(&removed), "remote backup removed\n");
    assert_eq!(git(&mirror, &["remote"]), "origin\n");
    let text = fs::read_to_string(&config_path).unwrap();

    let missing = sandbox.fold_with_path(
        &sandbox.empty_path(),
        &[
            "remote",
            "set",
            "x",
            "/srv/x.git",
            "main",
            "--project",
            project_arg,
        ],
    );
    assert!(!missing.status.success());
    assert!(stderr(&missing).contains("git_not_found"), "{missing:?}");
    assert_eq!(fs::read_to_string(&config_path).unwrap(), text);

    let bad = sandbox.fold(&[
        "remote",
        "set",
        "x",
        "/srv/x.git",
        "a..b",
        "--project",
        project_arg,
    ]);
    assert!(stderr(&bad).contains("invalid_request"), "{bad:?}");
    let outside = sandbox.fold(&["remote", "set", "x", "/srv/x.git", "main"]);
    assert!(
        stderr(&outside).contains("project_not_found"),
        "{outside:?}"
    );
}

#[test]
fn push_goes_through_the_daemon_and_needs_git_on_its_path() {
    let sandbox = Sandbox::new();
    let project = sandbox.project();
    let remote = sandbox.remote("remote.git");
    let unset = {
        let _daemon = sandbox.serve(&system_path());
        sandbox.push(&project)
    };
    assert!(stderr(&unset).contains("remote_not_set"), "{unset:?}");
    assert!(stderr(&unset).contains("fld remote set"), "{unset:?}");
    let set = sandbox.fold(&[
        "remote",
        "set",
        "origin",
        remote.to_str().unwrap(),
        "main",
        "--project",
        project.to_str().unwrap(),
    ]);
    assert!(set.status.success(), "{set:?}");

    {
        let _daemon = sandbox.serve(&system_path());
        let output = sandbox.push(&project);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(stdout(&output), "pushed r1 to origin/main: 1 commit\n");
        let message = git(&remote, &["log", "-1", "--format=%B", "main"]);
        assert_eq!(message, "fold r1\n\nFold-Revision: r1\n\n");
        let again = sandbox.push(&project);
        assert_eq!(stdout(&again), "origin/main is up to date: r1 is pushed\n");
        fs::create_dir(sandbox.path("elsewhere")).unwrap();
        let outside = sandbox.push(&sandbox.path("elsewhere"));
        assert!(
            stderr(&outside).contains("project_not_found"),
            "{outside:?}"
        );
    }

    fs::remove_file(project.join(".fold/git-sync.map")).unwrap();
    let _daemon = sandbox.serve(&sandbox.empty_path());
    let output = sandbox.push(&project);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("git_not_found"), "{output:?}");
}
