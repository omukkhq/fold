#![cfg(test)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// A host with no real home, its own daemon socket, `ana` as the human and UTC as the local time zone.
struct Sandbox {
    dir: TempDir,
}

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
        sandbox
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().canonicalize().unwrap().join(name)
    }

    fn socket(&self) -> PathBuf {
        self.path("fold.sock")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
        command
            .args(args)
            .current_dir(self.dir.path())
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("FOLD_HUMAN", "ana")
            .env("TZ", "UTC")
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command
    }

    /// `fld init` of a new folder `name` holding `files`, with Git sync on when `git` is set.
    fn project(&self, name: &str, git: bool, files: &[(&str, &str)]) -> PathBuf {
        let root = self.path(name);
        fs::create_dir(&root).unwrap();
        fs::write(root.join(".gitignore"), ".fold/\n").unwrap();
        for (path, text) in files {
            write(&root, path, text);
        }
        let mut args = vec!["init"];
        if git {
            args.push("--with-git");
        }
        let init = self.command(&args).current_dir(&root).output().unwrap();
        assert!(init.status.success(), "{init:?}");
        root
    }

    fn serve(&self) -> Daemon {
        let socket = self.socket();
        let child = self
            .command(&["serve", "--socket", socket.to_str().unwrap()])
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

    /// `fld <action> [args]` for `project` on the sandbox socket, which must succeed.
    fn run(&self, project: &Path, action: &[&str]) -> String {
        let output = self.output(project, action);
        assert!(output.status.success(), "{action:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    fn output(&self, project: &Path, action: &[&str]) -> Output {
        let socket = self.socket();
        let mut args = action.to_vec();
        args.extend([
            "--socket",
            socket.to_str().unwrap(),
            "--project",
            project.to_str().unwrap(),
        ]);
        self.command(&args).output().unwrap()
    }

    fn status(&self, project: &Path) -> Vec<String> {
        let text = self.run(project, &["status"]);
        assert!(!text.contains('\u{1b}'), "{text}");
        text.lines().map(str::to_owned).collect()
    }
}

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn write(root: &Path, path: &str, text: &str) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

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
        .env("GIT_AUTHOR_NAME", "Ana")
        .env("GIT_AUTHOR_EMAIL", "ana@example.com")
        .env("GIT_COMMITTER_NAME", "Ana")
        .env("GIT_COMMITTER_EMAIL", "ana@example.com")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

/// Waits until `fld status` of `project` holds `line`.
fn status_with(sandbox: &Sandbox, project: &Path, line: &str) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let lines = sandbox.status(project);
        if lines.iter().any(|found| found == line) {
            return lines;
        }
        assert!(Instant::now() < deadline, "no `{line}` in {lines:#?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn fld_status_shows_a_clean_folder_its_changes_the_stash_and_an_unstash_conflict() {
    let sandbox = Sandbox::new();
    let project = sandbox.project(
        "app",
        false,
        &[("README.md", "# app\n"), ("old.md", "old\n")],
    );
    let _daemon = sandbox.serve();
    assert_eq!(
        sandbox.status(&project),
        ["Nothing to record: the project folder is at main r1"]
    );

    write(&project, "README.md", "# app, mine\n");
    write(&project, "docs/new.md", "new\n");
    fs::remove_file(project.join("old.md")).unwrap();
    std::os::unix::fs::symlink("README.md", project.join("readme")).unwrap();
    assert_eq!(
        sandbox.status(&project),
        [
            "Project folder at main r1",
            "",
            "To record: 4 files",
            "  ↳ modified: README.md",
            "  ↳ added: docs/new.md",
            "  ↳ deleted: old.md",
            "  ↳ added: readme (symlink)",
            "  ↳ Next: `fld record` records them on main; `fld stash` puts them aside",
            "",
            "4 files to record",
        ]
    );
    let json: serde_json::Value =
        serde_json::from_str(&sandbox.run(&project, &["status", "--json"])).unwrap();
    assert_eq!(json["head"], "r1");
    assert_eq!(json["changes"][0]["path"], "README.md");
    assert_eq!(json["changes"][0]["kind"], "modified");
    assert_eq!(json["changes"].as_array().unwrap().len(), 4);
    // The status records nothing: the record finds every change still there.
    assert!(
        sandbox
            .run(&project, &["record"])
            .starts_with("recorded your changes in main as r2")
    );
    assert_eq!(
        sandbox.status(&project),
        ["Nothing to record: the project folder is at main r2"]
    );

    write(&project, "README.md", "# mine\n");
    sandbox.run(&project, &["stash"]);
    let lines = sandbox.status(&project);
    assert_eq!(
        lines[..3],
        ["Project folder at main r2", "", "Stash: 1 entry"]
    );
    assert!(
        lines[3].starts_with("  ↳ stash@{0}: 1 file; stashed on r2 by @ana, "),
        "{lines:#?}"
    );
    assert_eq!(
        lines[4..],
        [
            "    ↳ `fld unstash` applies it next",
            "",
            "nothing to record · 1 stash entry"
        ]
    );

    write(&project, "README.md", "# theirs\n");
    sandbox.run(&project, &["record"]);
    sandbox.run(&project, &["unstash"]);
    let lines = sandbox.status(&project);
    assert_eq!(
        lines[..9],
        [
            "Project folder at main r3",
            "",
            "To record: 1 file",
            "  ↳ modified: README.md",
            "  ↳ Next: `fld record` records them on main; `fld stash` puts them aside",
            "",
            "Unstash conflict: conflict markers of `fld unstash` wait in 1 path",
            "  ↳ README.md",
            "  ↳ Next: fix the conflict markers in these files, then run `fld record`",
        ]
    );
    assert!(
        lines[11].starts_with("  ↳ stash@{0}: 1 file, its unstash conflicts; stashed on r2"),
        "{lines:#?}"
    );
    assert_eq!(
        lines.last().unwrap(),
        "1 file to record · unstash conflict · 1 stash entry"
    );
}

#[test]
fn with_git_sync_fld_status_shows_the_branch_a_path_held_by_main_and_a_moved_head() {
    let sandbox = Sandbox::new();
    let project = sandbox.project("app", true, &[("a.txt", "one\n")]);
    let origin = sandbox.path("origin.git");
    fs::create_dir(&origin).unwrap();
    git(
        &origin,
        &["init", "--quiet", "--bare", "--initial-branch=main"],
    );
    let _daemon = sandbox.serve();
    let added = sandbox
        .command(&["remote", "add", "origin", origin.to_str().unwrap()])
        .current_dir(&project)
        .output()
        .unwrap();
    assert!(added.status.success(), "{added:?}");
    sandbox.run(&project, &["push"]);
    assert_eq!(
        sandbox.status(&project),
        [
            "Nothing to record: the project folder is at main r1",
            "  ↳ Git: main is synced with the branch main",
        ]
    );

    // Someone else changes a.txt on the remote while the human changes it in the folder.
    let other = sandbox.path("other");
    git(
        sandbox.dir.path(),
        &[
            "clone",
            "--quiet",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    write(&other, "a.txt", "theirs\n");
    git(&other, &["commit", "--quiet", "-am", "Change a.txt"]);
    git(&other, &["push", "--quiet", "origin", "main"]);
    write(&project, "a.txt", "mine\n");
    write(&project, "b.txt", "new\n");
    sandbox.run(&project, &["pull"]);
    assert_eq!(
        sandbox.status(&project),
        [
            "Project folder at r1, behind main r2",
            "  ↳ Why: paths main changed hold changes of yours that no revision records",
            "  ↳ Git: main is synced with the branch main",
            "",
            "To record: 2 files",
            "  ↳ modified: a.txt, held by main",
            "  ↳ added: b.txt",
            "  ↳ Next: `fld record` records them on main; `fld stash` puts them aside",
            "",
            "Held by main: changes of yours on 1 path that main changed too",
            "  ↳ a.txt",
            "  ↳ Next: run `fld record`, or take the changes back",
            "",
            "2 files to record · 1 held by main",
        ]
    );

    fs::remove_file(project.join("b.txt")).unwrap();
    write(&project, "a.txt", "theirs\n");
    let lines = status_with(
        &sandbox,
        &project,
        "Nothing to record: the project folder is at main r2",
    );
    assert_eq!(
        lines,
        [
            "Nothing to record: the project folder is at main r2",
            "  ↳ Git: main is synced with the branch main",
        ]
    );

    git(&project, &["switch", "--quiet", "-c", "side"]);
    assert_eq!(
        sandbox.status(&project),
        [
            "Project folder at main r2",
            "  ↳ Git: main is synced with the branch main",
            "",
            "Git branch moved: the branch or HEAD shows a change fold did not make, so fold does not update the folder",
            "  ↳ Next: run `git switch main` in the project folder",
            "",
            "nothing to record · Git branch moved",
        ]
    );
    git(&project, &["switch", "--quiet", "main"]);
    assert_eq!(
        sandbox.status(&project),
        [
            "Nothing to record: the project folder is at main r2",
            "  ↳ Git: main is synced with the branch main",
        ]
    );
}

#[test]
fn fld_untrack_stops_tracking_a_file_the_rules_match_and_leaves_it_on_disk() {
    let sandbox = Sandbox::new();
    let project = sandbox.project(
        "app",
        true,
        &[("README.md", "# app\n"), ("secret.env", "TOKEN=1\n")],
    );
    let _daemon = sandbox.serve();
    // `fld init` wrote a .foldignore already; this one replaces it.
    write(&project, ".foldignore", "*.env\n");
    // Unchanged, the file is only counted; once it changes, it is listed.
    let lines = status_with(
        &sandbox,
        &project,
        "  ↳ 1 tracked file matches ignore rules; `fld status --json` lists them",
    );
    assert!(!lines.contains(&"  ↳ secret.env".to_owned()), "{lines:#?}");
    write(&project, "secret.env", "TOKEN=2\n");
    let lines = status_with(
        &sandbox,
        &project,
        "Tracked although ignored: 1 file, which `fld record` records because main tracks them",
    );
    assert!(
        lines.contains(&"  ↳ modified: secret.env".to_owned()),
        "{lines:#?}"
    );
    assert!(lines.contains(&"  ↳ secret.env".to_owned()), "{lines:#?}");
    assert!(
        !lines.iter().any(|line| line.contains("match ignore rules")),
        "{lines:#?}"
    );

    let file = project.join("secret.env");
    let untracked = sandbox.run(&project, &["untrack", file.to_str().unwrap()]);
    let lines: Vec<&str> = untracked.lines().collect();
    assert!(
        lines[0].starts_with("Stopped tracking 1 file in main as r2, Git commit "),
        "{lines:#?}"
    );
    assert_eq!(lines[1], "  ↳ secret.env");
    assert_eq!(lines.len(), 3, "{lines:#?}");
    assert_eq!(fs::read_to_string(&file).unwrap(), "TOKEN=2\n");
    assert_eq!(git(&project, &["ls-files", "secret.env"]), "");
    assert_eq!(
        sandbox.status(&project),
        [
            "Project folder at main r2",
            "  ↳ Git: main is synced with the branch main",
            "",
            "To record: 1 file",
            "  ↳ modified: .foldignore",
            "  ↳ Next: `fld record` records them on main; `fld stash` puts them aside",
            "",
            "1 file to record",
        ]
    );

    let refused = sandbox.output(&project, &["untrack", file.to_str().unwrap()]);
    assert!(!refused.status.success());
    let text = String::from_utf8_lossy(&refused.stderr);
    assert!(text.contains("path_not_tracked"), "{text}");
}
