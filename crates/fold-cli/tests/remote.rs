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

    /// Below the canonical folder, the form fold records and prints: macOS reaches its temporary folders through the `/var` symlink.
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().canonicalize().unwrap().join(name)
    }

    fn socket(&self) -> PathBuf {
        self.path("fold.sock")
    }

    /// `fld <args>` in the sandbox with no real home, no runtime directory and `path` as `PATH`.
    fn command(&self, path: &str, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
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
        // Coverage runs name the profile file through this variable. `env_clear` drops every variable set before it, so it comes after.
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
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

    /// A project `app` with Git sync on and its `r1`, registered in the sandbox. Its `.gitignore` ignores `.fold/` already, so turning Git on records no revision of its own.
    fn project(&self) -> PathBuf {
        fs::create_dir(self.path("app")).unwrap();
        fs::write(self.path("app").join(".gitignore"), ".fold/\n").unwrap();
        let init = self
            .command(&system_path(), &["init", "--with-git"])
            .current_dir(self.path("app"))
            .output()
            .unwrap();
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
        self.sync("push", project, None)
    }

    /// `fld push` or `fld pull` of `project` on the sandbox socket, naming `remote` when given.
    fn sync(&self, action: &str, project: &Path, remote: Option<&str>) -> Output {
        let socket = self.socket();
        let mut args = vec![
            action,
            "--socket",
            socket.to_str().unwrap(),
            "--project",
            project.to_str().unwrap(),
        ];
        args.extend(remote);
        self.fold(&args)
    }

    /// `fld remote add <name> <url> [<branch>]` in `project`.
    fn add_remote(&self, project: &Path, name: &str, url: &Path, branch: Option<&str>) -> Output {
        let mut args = vec![
            "remote",
            "add",
            name,
            url.to_str().unwrap(),
            "--project",
            project.to_str().unwrap(),
        ];
        args.extend(branch);
        self.fold(&args)
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
fn remote_add_writes_a_named_section_keeps_the_rest_and_configures_the_mirror() {
    let sandbox = Sandbox::new();
    let project = sandbox.project();
    let config_path = project.join(".fold/config.toml");
    let template = fs::read_to_string(&config_path).unwrap();
    let first = sandbox.remote("first.git");

    // Run from a folder inside the project with a relative path, which is stored absolute.
    fs::create_dir(project.join("sub")).unwrap();
    let output = sandbox
        .command(
            &system_path(),
            &["remote", "add", "origin", "../../first.git", "main"],
        )
        .current_dir(project.join("sub"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        "remote origin added: fold syncs main with its branch main\n"
    );
    let text = fs::read_to_string(&config_path).unwrap();
    assert!(text.starts_with(&template), "{text}");
    let canonical = first.canonicalize().unwrap();
    let config = ProjectConfig::from_toml(&text).unwrap();
    assert_eq!(config.remotes["origin"].url, canonical.to_str().unwrap());
    assert_eq!(config.remotes["origin"].branch, "main");
    assert!(!config.remotes["origin"].push_on_integrate);
    let mirror = project.join(".git");
    assert_eq!(
        git(&mirror, &["rev-parse", "--is-bare-repository"]),
        "false\n"
    );
    assert_eq!(
        git(&mirror, &["remote", "get-url", "origin"]),
        format!("{}\n", canonical.display())
    );

    let second = sandbox.remote("second.git");
    let project_arg = project.to_str().unwrap();
    let output = sandbox.fold(&[
        "remote",
        "add",
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
            "add",
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
        "add",
        "x",
        "/srv/x.git",
        "a..b",
        "--project",
        project_arg,
    ]);
    assert!(stderr(&bad).contains("invalid_request"), "{bad:?}");
    let outside = sandbox.fold(&["remote", "add", "x", "/srv/x.git", "main"]);
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
    assert!(stderr(&unset).contains("fld remote add"), "{unset:?}");
    let added = sandbox.fold(&[
        "remote",
        "add",
        "origin",
        remote.to_str().unwrap(),
        "main",
        "--project",
        project.to_str().unwrap(),
    ]);
    assert!(added.status.success(), "{added:?}");

    {
        let _daemon = sandbox.serve(&system_path());
        let output = sandbox.push(&project);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(stdout(&output), "pushed r1 to origin/main: 1 commit\n");
        let message = git(&remote, &["log", "-1", "--format=%B", "main"]);
        assert_eq!(
            message,
            "Record the changes of the project folder\n\nFold-Revision: r1\n\n"
        );
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

#[test]
fn remote_add_without_a_branch_takes_the_remote_default_branch_or_main() {
    let sandbox = Sandbox::new();
    let project = sandbox.project();
    let config_path = project.join(".fold/config.toml");
    // A bare repository whose HEAD names `trunk`, with one commit on it.
    let trunk = sandbox.path("trunk.git");
    fs::create_dir(&trunk).unwrap();
    git(
        &trunk,
        &["init", "--quiet", "--bare", "--initial-branch=trunk"],
    );
    let scratch = sandbox.path("scratch");
    fs::create_dir(&scratch).unwrap();
    git(&scratch, &["init", "--quiet", "--initial-branch=trunk"]);
    fs::write(scratch.join("a.txt"), "a").unwrap();
    git(&scratch, &["add", "a.txt"]);
    git(
        &scratch,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--quiet",
            "-m",
            "a",
        ],
    );
    git(
        &scratch,
        &["push", "--quiet", trunk.to_str().unwrap(), "trunk"],
    );

    let output = sandbox.add_remote(&project, "origin", &trunk, None);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        stdout(&output),
        "remote origin added: fold syncs main with its branch trunk\n"
    );
    // A new empty repository and a path that does not exist yet have no default branch.
    let empty = sandbox.remote("empty.git");
    let output = sandbox.add_remote(&project, "empty", &empty, None);
    assert_eq!(
        stdout(&output),
        "remote empty added: fold syncs main with its branch main\n"
    );
    let absent = sandbox.path("absent.git");
    let output = sandbox.add_remote(&project, "absent", &absent, None);
    assert_eq!(
        stdout(&output),
        "remote absent added: fold syncs main with its branch main\n"
    );
    // Adding a name twice is refused, as Git does, and the first remote stays as it was.
    let config_before = fs::read_to_string(&config_path).unwrap();
    let output = sandbox.add_remote(&project, "origin", &empty, Some("release"));
    assert!(!output.status.success(), "{output:?}");
    let refusal = stderr(&output);
    assert!(
        refusal.contains("invalid_request: the project has a Git remote named origin already"),
        "{refusal}"
    );
    assert!(
        refusal
            .contains("hint: run fld remote remove origin, then fld remote add origin <url> again"),
        "{refusal}"
    );
    assert_eq!(fs::read_to_string(&config_path).unwrap(), config_before);
    assert_eq!(
        git(&project.join(".git"), &["remote", "get-url", "origin"]),
        format!("{}\n", trunk.canonicalize().unwrap().display())
    );
    let config = ProjectConfig::from_toml(&fs::read_to_string(&config_path).unwrap()).unwrap();
    let branches: Vec<_> = config
        .remotes
        .iter()
        .map(|(name, remote)| (name.as_str(), remote.branch.as_str()))
        .collect();
    assert_eq!(
        branches,
        [("absent", "main"), ("empty", "main"), ("origin", "trunk")]
    );

    let removed = sandbox.fold(&[
        "remote",
        "remove",
        "empty",
        "--project",
        project.to_str().unwrap(),
    ]);
    assert_eq!(stdout(&removed), "remote empty removed\n");
    let mirror = project.join(".git");
    assert_eq!(git(&mirror, &["remote"]), "absent\norigin\n");
    let again = sandbox.fold(&[
        "remote",
        "remove",
        "empty",
        "--project",
        project.to_str().unwrap(),
    ]);
    assert!(stderr(&again).contains("remote_not_set"), "{again:?}");
    let old = sandbox.fold(&[
        "remote",
        "set",
        "x",
        "/srv/x.git",
        "--project",
        project.to_str().unwrap(),
    ]);
    assert!(!old.status.success(), "{old:?}");
}

#[test]
fn push_and_pull_leave_out_the_name_of_the_only_remote_and_need_it_among_several() {
    let sandbox = Sandbox::new();
    let project = sandbox.project();
    let plain = sandbox.path("plain");
    let init = sandbox.fold(&["init", plain.to_str().unwrap()]);
    assert!(init.status.success(), "{init:?}");
    let _daemon = sandbox.serve(&system_path());
    for action in ["push", "pull"] {
        // Git sync being off comes before the missing remote.
        let off = sandbox.sync(action, &plain, None);
        assert!(stderr(&off).contains("Git sync is off"), "{off:?}");
        assert!(!stderr(&off).contains("remote_not_set"), "{off:?}");
        let none = sandbox.sync(action, &project, None);
        assert!(!none.status.success(), "{none:?}");
        assert!(stderr(&none).contains("remote_not_set"), "{none:?}");
        assert!(stderr(&none).contains("fld remote add"), "{none:?}");
        let named = sandbox.sync(action, &project, Some("origin"));
        assert!(stderr(&named).contains("remote_not_set"), "{named:?}");
    }

    let origin = sandbox.remote("origin.git");
    assert!(
        sandbox
            .add_remote(&project, "origin", &origin, None)
            .status
            .success()
    );
    let pushed = sandbox.sync("push", &project, None);
    assert_eq!(stdout(&pushed), "pushed r1 to origin/main: 1 commit\n");
    let head = git(&origin, &["rev-parse", "--short=7", "main"]);
    let pulled = sandbox.sync("pull", &project, None);
    assert_eq!(
        stdout(&pulled),
        format!("main is up to date with origin/main: it holds {head}")
    );

    let backup = sandbox.remote("backup.git");
    assert!(
        sandbox
            .add_remote(&project, "backup", &backup, None)
            .status
            .success()
    );
    for action in ["push", "pull"] {
        let several = sandbox.sync(action, &project, None);
        assert!(!several.status.success(), "{several:?}");
        assert!(
            stderr(&several)
                .contains("name one of the remotes backup, origin; fld remote list shows them"),
            "{several:?}"
        );
        let unknown = sandbox.sync(action, &project, Some("elsewhere"));
        assert!(stderr(&unknown).contains("remote_not_set"), "{unknown:?}");
    }
    let pulled = sandbox.sync("pull", &project, Some("backup"));
    assert_eq!(
        stdout(&pulled),
        "the remote has no branch backup/main; nothing to pull\n"
    );
    let pushed = sandbox.sync("push", &project, Some("backup"));
    assert_eq!(stdout(&pushed), "pushed r1 to backup/main: 1 commit\n");
    let pulled = sandbox.sync("pull", &project, Some("origin"));
    assert_eq!(
        stdout(&pulled),
        format!("main is up to date with origin/main: it holds {head}")
    );
}

#[test]
fn pause_and_resume_go_through_the_daemon() {
    let sandbox = Sandbox::new();
    let project = sandbox.project();
    let _daemon = sandbox.serve(&system_path());
    let socket = sandbox.socket();
    let run = |command: &str| {
        sandbox.fold(&[
            command,
            "--socket",
            socket.to_str().unwrap(),
            "--project",
            project.to_str().unwrap(),
        ])
    };
    let paused = run("pause");
    assert!(paused.status.success(), "{paused:?}");
    assert_eq!(
        stdout(&paused),
        "the agents are paused by @ana: every worker is stopped and keeps its session; run fld resume to start them again\n"
    );
    assert_eq!(
        stdout(&run("pause")),
        stdout(&paused),
        "a second pause changes nothing"
    );
    let resumed = run("resume");
    assert!(resumed.status.success(), "{resumed:?}");
    assert_eq!(
        stdout(&resumed),
        "the agents run: fold starts and resumes workers as usual\n"
    );
}
