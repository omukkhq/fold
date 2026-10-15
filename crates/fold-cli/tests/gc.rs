use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use fold_core::ManualClock;
use fold_daemon::{Daemon, DaemonConfig, RunningDaemon};
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

    fn socket(&self) -> PathBuf {
        self.path("run").join("fold.sock")
    }

    /// Runs the binary in `cwd` with no real home, so no run touches the user's registry or daemon.
    async fn fold(&self, cwd: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fold"));
        command
            .args(args)
            .current_dir(cwd)
            .env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("FOLD_HUMAN", "ana");
        // Coverage runs name the profile file through this variable; `env_clear` drops it.
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            command.env("LLVM_PROFILE_FILE", profile);
        }
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }

    async fn start_daemon(&self) -> RunningDaemon {
        let config = DaemonConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            socket: self.socket(),
            data_dir: self.path("data").join("fold"),
            runtime_dir: self.path("run"),
            web_dir: None,
        };
        Daemon::start(config, Arc::new(ManualClock::new()))
            .await
            .unwrap()
    }
}

#[cfg(test)]
fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn fold_gc_prints_the_counts_of_the_project_it_runs_in() {
    let sandbox = Sandbox::new();
    let project = sandbox.path("demo");
    let init = sandbox
        .fold(sandbox.dir.path(), &["init", project.to_str().unwrap()])
        .await;
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let daemon = sandbox.start_daemon().await;
    let scratch = project.join(".fold").join("objects").join("tmp");
    fs::create_dir_all(&scratch).unwrap();
    let stale = scratch.join("stale");
    fs::write(&stale, b"private file content").unwrap();
    File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000))
        .unwrap();
    let inside = project.join("ws").join("t-1");
    fs::create_dir_all(&inside).unwrap();
    let socket = sandbox.socket();
    let socket = socket.to_str().unwrap();

    let dry = sandbox
        .fold(&inside, &["gc", "--dry-run", "--socket", socket])
        .await;
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    let text = stdout(&dry);
    assert!(text.lines().next().unwrap().contains("dry run"));
    assert!(text.contains("temporary files removed: 1\n"));
    assert!(stale.exists());

    let run = sandbox.fold(&inside, &["gc", "--socket", socket]).await;
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let text = stdout(&run);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "garbage collection of project demo finished");
    assert_eq!(lines.len(), 8);
    for line in &lines[1..] {
        let (_, number) = line.rsplit_once(": ").unwrap();
        assert!(number.parse::<u64>().is_ok(), "{line}");
    }
    assert!(text.contains("temporary files removed: 1\n"));
    assert!(!text.contains("private file content"));
    assert!(!stale.exists());

    let outside = sandbox
        .fold(sandbox.dir.path(), &["gc", "--socket", socket])
        .await;
    assert!(!outside.status.success());
    assert!(String::from_utf8_lossy(&outside.stderr).contains("project_not_found"));
    daemon.shutdown().await;

    let stopped = sandbox.fold(&project, &["gc", "--socket", socket]).await;
    assert!(!stopped.status.success());
    assert!(String::from_utf8_lossy(&stopped.stderr).contains("fold serve"));
}
