use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Output};

use tempfile::TempDir;

struct Sandbox {
    dir: TempDir,
}

impl Sandbox {
    fn new() -> io::Result<Self> {
        Ok(Self {
            dir: TempDir::new()?,
        })
    }

    /// Below the canonical folder, the form fold records and prints: macOS reaches its temporary folders through the `/var` symlink.
    fn path(&self, name: &str) -> PathBuf {
        let root = self.dir.path();
        root.canonicalize()
            .unwrap_or_else(|_| root.to_path_buf())
            .join(name)
    }

    /// The environment holds no real home, no runtime directory and an empty `PATH`, so a run sees only the sandbox.
    fn fold(&self, args: &[&str]) -> io::Result<Output> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
        command
            .args(args)
            .current_dir(self.dir.path())
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("PATH", "");
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        command.output()
    }

    /// Init runs with the caller's `PATH`, because it needs `git`; doctor still runs with none.
    fn init(&self, name: &str) -> io::Result<PathBuf> {
        let project = self.path(name);
        let output = Command::new(env!("CARGO_BIN_EXE_fld"))
            .args(["init", &project.to_string_lossy()])
            .current_dir(self.dir.path())
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .envs(
                std::env::var_os("LLVM_PROFILE_FILE").map(|profile| ("LLVM_PROFILE_FILE", profile)),
            )
            .output()?;
        assert!(output.status.success(), "{output:?}");
        Ok(project)
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn warnings_alone_exit_0_and_every_project_in_the_registry_is_checked() {
    let sandbox = Sandbox::new().unwrap();
    let alpha = sandbox.init("alpha").unwrap();
    sandbox.init("beta").unwrap();
    std::fs::remove_file(alpha.join(".mcp.json")).unwrap();
    let socket = sandbox.path("absent.sock");
    let output = sandbox
        .fold(&["doctor", "--socket", socket.to_str().unwrap()])
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = stdout(&output);
    assert!(text.starts_with("host\n"), "{text}");
    assert!(text.contains("project alpha\n") && text.contains("project beta\n"));
    assert!(text.contains("warn socket: no daemon socket at"));
    assert!(text.contains("fix: fld serve"));
    assert!(text.contains("warn git: git is not on PATH"));
    assert!(text.contains("fix: install git"));
    assert!(text.contains("warn claude: claude is not on PATH"));
    assert!(text.contains("fix: install Claude Code"));
    assert!(text.contains("ok   database: quick_check is ok and journal_mode is wal"));
    assert!(text.contains("warn claude files: .mcp.json is missing"));
    assert_eq!(text.matches("fix: fld init --refresh").count(), 1);
}

#[test]
fn a_socket_with_mode_0644_fails_with_exit_1() {
    let sandbox = Sandbox::new().unwrap();
    let socket = sandbox.path("loose.sock");
    drop(UnixListener::bind(&socket).unwrap());
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o644)).unwrap();
    let output = sandbox
        .fold(&["doctor", "--socket", socket.to_str().unwrap()])
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let text = stdout(&output);
    assert!(text.contains("fail socket:"), "{text}");
    assert!(text.contains(&format!("fix: chmod 600 {}", socket.display())));
    assert!(stderr(&output).contains("doctor_checks_failed: checks failed: 1"));
}

#[test]
fn a_damaged_database_is_a_failure_while_the_daemon_is_down() {
    let sandbox = Sandbox::new().unwrap();
    let project = sandbox.init("alpha").unwrap();
    let socket = sandbox.path("absent.sock");
    let db = project.join(".fold/fold.db");
    std::fs::write(
        &db,
        b"damaged beyond repair, and long enough to look like a file",
    )
    .unwrap();
    let output = sandbox
        .fold(&[
            "doctor",
            "--project",
            project.to_str().unwrap(),
            "--socket",
            socket.to_str().unwrap(),
        ])
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let text = stdout(&output);
    assert!(text.contains("fail database:"), "{text}");
    assert!(text.contains(&format!("fix: restore {} from a backup", db.display())));
    assert!(text.contains("warn socket:"));
}

#[test]
fn claude_md_files_warn_once_per_path_and_keep_exit_0() {
    let sandbox = Sandbox::new().unwrap();
    let project = sandbox.init("alpha").unwrap();
    std::fs::write(project.join(".claude/CLAUDE.md"), "rules").unwrap();
    std::fs::write(sandbox.path("CLAUDE.local.md"), "mine").unwrap();
    let output = sandbox
        .fold(&["doctor", "--project", project.to_str().unwrap()])
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = stdout(&output);
    for path in [
        project.join(".claude/CLAUDE.md"),
        sandbox.path("CLAUDE.local.md"),
    ] {
        let shown = path.display();
        assert!(
            text.contains(&format!("{shown} makes Claude Code skip AGENTS.md")),
            "{text}"
        );
        assert!(text.contains(&format!(
            "fix: move the text of {shown} into AGENTS.md and delete {shown}"
        )));
    }
}

#[test]
fn a_run_without_arguments_reports_the_platform_checks() {
    let sandbox = Sandbox::new().unwrap();
    let output = sandbox.fold(&["doctor"]).unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = stdout(&output);
    assert!(text.contains("service manager:"), "{text}");
    assert!(text.contains("linger:"));
    assert!(text.contains("inotify limit:"));
}

#[test]
fn a_host_without_a_home_stops_with_the_invalid_host_paths_code() {
    let sandbox = Sandbox::new().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_fld"));
    command
        .arg("doctor")
        .env_clear()
        .current_dir(sandbox.dir.path());
    // Coverage runs name the profile file through this variable. `env_clear` drops every variable set before it, so it comes after.
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("invalid_host_paths"));
}
