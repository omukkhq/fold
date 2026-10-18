use std::os::unix::net::UnixListener;

use fold_core::events::types::WORKSPACE_WATCH_DEGRADED;
use fold_core::{EventActor, NewEvent};
use fold_store::{Store, StoreError, events};
use tempfile::TempDir;

use super::*;
use crate::platform::Probe;
use fold_cli::claude_files::{
    AGENTS_BEGIN, AGENTS_END, AGENTS_FILE, ALLOW_FOLD_TOOLS, CLAUDE_FILE, MCP_FILE,
    MCP_SERVER_NAME, MCP_SERVERS_KEY, SETTINGS_FILE, SKILL_FILE,
};
use fold_cli::claude_user::{UserFiles, allows_fold_tools, runs_hook};
use fold_core::layout::HOOKS;

const HOUR_MS: i64 = 60 * 60 * 1000;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

fn mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn executable(path: &Path) {
    std::fs::write(path, "#!/bin/sh\n").unwrap();
    mode(path, 0o755);
}

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: TempDir::new().unwrap(),
        }
    }

    /// Below the canonical folder, the form fold records and prints: macOS reaches its temporary folders through the `/var` symlink.
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().canonicalize().unwrap().join(name)
    }

    /// A host whose probe root holds nothing: no `/proc`, no systemd.
    fn bare_host(&self) -> Host {
        std::fs::create_dir_all(self.path("run/fold")).unwrap();
        std::fs::create_dir_all(self.path("bin")).unwrap();
        Host {
            dirs: Dirs {
                home: Some(self.path("home")),
                config_home: Some(self.path("config")),
                data_home: Some(self.path("data")),
                runtime_dir: Some(self.path("run")),
                library_runtime: false,
            },
            user: Some("ana".to_owned()),
            uid: 501,
            path: Some(std::env::join_paths([self.path("bin")]).unwrap()),
            platform: Platform {
                linux: true,
                macos: false,
                file_modes: true,
            },
            probe: Probe::new(self.path("root")),
            names: crate::fold_name::Environment {
                user: Some("ana".into()),
                ..crate::fold_name::Environment::default()
            },
            claude: Some(UserFiles {
                config: self.path("home/.claude.json"),
                settings: self.path("home/.claude/settings.json"),
            }),
        }
    }

    /// A host with inotify, a systemd directory and a fake `systemctl` and `git` on `PATH`.
    fn linux_host(&self, watches: u64) -> Host {
        let host = self.bare_host();
        let inotify = self.path("root/proc/sys/fs/inotify");
        std::fs::create_dir_all(&inotify).unwrap();
        std::fs::write(inotify.join("max_user_watches"), format!("{watches}\n")).unwrap();
        std::fs::create_dir_all(self.path("root/run/systemd/system")).unwrap();
        std::fs::create_dir_all(self.path("root/var/lib/systemd/linger")).unwrap();
        executable(&self.path("bin/systemctl"));
        executable(&self.path("bin/git"));
        host
    }

    fn install_unit(&self) {
        let unit = self.path("config/systemd/user/fold.service");
        std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
        std::fs::write(unit, "").unwrap();
    }

    /// A project directory with a migrated database, the objects directory and every Claude Code file.
    /// A project with Git sync on and its own repository.
    fn project(&self, name: &str) -> PathBuf {
        let root = self.plain_project(name);
        rt().block_on(async {
            let store = Store::open_project(&ProjectPaths::new(&root))
                .await
                .unwrap();
            store
                .write::<_, StoreError>(async |tx| {
                    fold_store::workspaces::enable_git_sync(tx, "human:ana", at(0)).await?;
                    Ok(())
                })
                .await
                .unwrap();
            store.close().await.unwrap();
        });
        git(&root, &["init", "--quiet"]);
        fold_vcs::git_remote::exclude_fold_dir(&root.join(".git")).unwrap();
        root
    }

    /// A project with Git sync off and no repository, as a plain `fld init` leaves it.
    fn plain_project(&self, name: &str) -> PathBuf {
        let root = self.path(name);
        std::fs::create_dir_all(root.join(".fold/objects")).unwrap();
        rt().block_on(async {
            let store = Store::open_project(&ProjectPaths::new(&root))
                .await
                .unwrap();
            store.close().await.unwrap();
        });
        std::fs::create_dir_all(root.join(".claude/skills/fold")).unwrap();
        std::fs::write(root.join(SKILL_FILE), "# fold\n").unwrap();
        std::fs::write(root.join(AGENTS_FILE), good_agents()).unwrap();
        root
    }
}

/// `git` in `dir` without the user's configuration.
fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "a")
        .env("GIT_AUTHOR_EMAIL", "a@b")
        .env("GIT_COMMITTER_NAME", "a")
        .env("GIT_COMMITTER_EMAIL", "a@b")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}");
}

fn good_mcp() -> String {
    serde_json::json!({ MCP_SERVERS_KEY: { MCP_SERVER_NAME: { "command": "fld", "args": ["mcp"] } } })
        .to_string()
}

fn settings(allow: &str, hooks: &[(&str, &str)]) -> String {
    let hooks: serde_json::Map<_, _> = hooks
        .iter()
        .map(|(event, command)| {
            let group =
                serde_json::json!([{ "hooks": [{ "type": "command", "command": command }] }]);
            ((*event).to_owned(), group)
        })
        .collect();
    serde_json::json!({ "permissions": { "allow": [allow] }, "hooks": hooks }).to_string()
}

fn good_settings() -> String {
    settings(ALLOW_FOLD_TOOLS, &HOOKS)
}

fn good_agents() -> String {
    format!("{AGENTS_BEGIN}\nfold\n{AGENTS_END}\n")
}

fn seed_event(root: &Path, reason: &str, created: Timestamp) {
    rt().block_on(async {
        let store = Store::open_project(&ProjectPaths::new(root)).await.unwrap();
        let mut event =
            NewEvent::new(WORKSPACE_WATCH_DEGRADED, EventActor::parse("fold").unwrap()).unwrap();
        event = event.reason(reason);
        event.data = serde_json::json!({ "reason": reason });
        store
            .write::<_, StoreError>(async |tx| {
                events::append(tx, &event, created).await?;
                Ok(())
            })
            .await
            .unwrap();
        store.close().await.unwrap();
    });
}

fn find<'a>(checks: &'a [Check], name: &str) -> &'a Check {
    checks.iter().find(|check| check.name == name).unwrap()
}

fn project_report(host: &Host, root: &Path, now: Timestamp) -> Vec<Check> {
    project_checks(root, host, now, &rt())
}

fn snapshot(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(next).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                found.push((entry.path(), meta.len()));
            }
        }
    }
    found.sort();
    found
}

// Socket

fn socket_in(fixture: &Fixture) -> PathBuf {
    fixture.path("run/fold/fold.sock")
}

fn bind(path: &Path) -> UnixListener {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    UnixListener::bind(path).unwrap()
}

#[test]
fn a_daemon_that_answers_gives_ok() {
    let fixture = Fixture::new();
    let path = socket_in(&fixture);
    let listener = bind(&path);
    mode(&path, 0o600);
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 64];
        let _ = stream.read(&mut request).unwrap();
        stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
    });
    let state = inspect_socket(&path, Platform::current());
    server.join().unwrap();
    assert_eq!(state, SocketState::Answers);
    let check = socket_check(Some(&path), Some(state));
    assert_eq!(check.status, Status::Ok);
    assert_eq!(check.fix, None);
}

#[test]
fn a_socket_with_mode_0644_fails_with_the_chmod_fix() {
    let fixture = Fixture::new();
    let path = socket_in(&fixture);
    drop(bind(&path));
    mode(&path, 0o644);
    let state = inspect_socket(&path, Platform::current());
    assert_eq!(
        state,
        SocketState::WrongMode {
            mode: 0o644,
            answers: false
        }
    );
    let check = socket_check(Some(&path), Some(state));
    assert_eq!(check.status, Status::Fail);
    assert!(check.message.contains("0644"));
    assert_eq!(check.fix, Some(format!("chmod 600 {}", path.display())));
    let no_modes = Platform {
        linux: false,
        macos: false,
        file_modes: false,
    };
    assert_eq!(inspect_socket(&path, no_modes), SocketState::Dead);
}

#[test]
fn a_daemon_on_a_loose_socket_still_holds_its_lock_rightfully() {
    let fixture = Fixture::new();
    let host = fixture.bare_host();
    let path = socket_in(&fixture);
    let listener = bind(&path);
    mode(&path, 0o644);
    let holder = File::create(fixture.path("run/fold/daemon.lock")).unwrap();
    holder.try_lock().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 64];
        let _ = stream.read(&mut request).unwrap();
        stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
    });
    let checks = host_checks(None, &FileConfig::default(), &host);
    server.join().unwrap();
    assert_eq!(find(&checks, "socket").status, Status::Fail);
    assert_eq!(find(&checks, "locks").status, Status::Ok);
}

#[test]
fn a_missing_or_dead_socket_warns_with_fold_serve() {
    let fixture = Fixture::new();
    let path = socket_in(&fixture);
    assert_eq!(
        inspect_socket(&path, Platform::current()),
        SocketState::Missing
    );
    let missing = socket_check(Some(&path), Some(SocketState::Missing));
    assert_eq!(
        (missing.status, missing.fix.as_deref()),
        (Status::Warn, Some("fld serve"))
    );
    drop(bind(&path));
    mode(&path, 0o600);
    let state = inspect_socket(&path, Platform::current());
    assert_eq!(state, SocketState::Dead);
    let dead = socket_check(Some(&path), Some(state));
    assert_eq!(
        (dead.status, dead.fix.as_deref()),
        (Status::Warn, Some("fld serve"))
    );
}

#[test]
fn a_listener_that_does_not_speak_http_is_not_a_daemon() {
    let fixture = Fixture::new();
    let path = socket_in(&fixture);
    let listener = bind(&path);
    mode(&path, 0o600);
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        drop(stream);
    });
    assert_eq!(
        inspect_socket(&path, Platform::current()),
        SocketState::Dead
    );
    server.join().unwrap();
}

#[test]
fn no_socket_path_is_a_warning() {
    let check = socket_check(None, None);
    assert_eq!(check.status, Status::Warn);
    let fix = check.fix.unwrap();
    assert!(fix.starts_with("export XDG_RUNTIME_DIR=") && fix.contains("--socket"));
}

#[test]
fn only_mode_0600_is_private() {
    assert!(mode_is_private(0o600));
    assert!(mode_is_private(0o140_600));
    for bad in [0o644, 0o666, 0o700, 0o400, 0o660] {
        assert!(!mode_is_private(bad), "{bad:o}");
    }
}

// Locks

#[test]
fn a_held_lock_with_a_dead_socket_fails_naming_the_file() {
    let fixture = Fixture::new();
    let path = fixture.path("run/fold/daemon.lock");
    assert_eq!(inspect_lock(&path), LockState::Absent);
    assert_eq!(lock_check(Some(&path), false).status, Status::Ok);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let holder = File::create(&path).unwrap();
    assert_eq!(inspect_lock(&path), LockState::Free);
    holder.try_lock().unwrap();
    assert_eq!(inspect_lock(&path), LockState::Held);
    let check = lock_check(Some(&path), false);
    assert_eq!(check.status, Status::Fail);
    let fix = check.fix.unwrap();
    assert!(fix.contains(&format!("rm {}", path.display())), "{fix}");
    assert_eq!(lock_check(Some(&path), true).status, Status::Ok);
    drop(holder);
    assert_eq!(lock_check(Some(&path), false).status, Status::Ok);
    assert_eq!(lock_check(None, false).status, Status::Ok);
}

// HTTP, git

#[test]
fn http_is_ok_with_a_listener_and_a_warning_without() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    assert_eq!(http_check(addr).status, Status::Ok);
    drop(listener);
    let check = http_check(addr);
    assert_eq!(
        (check.status, check.fix.as_deref()),
        (Status::Warn, Some("fld serve"))
    );
}

/// A `claude` on the fixture's `PATH` that prints `version` for `--version` and `auth` for `auth status`, exiting with `code` there.
fn fake_claude(fixture: &Fixture, version: &str, auth: &str, code: u8) {
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in\n  --version) printf '%s\\n' '{version}' ;;\n  auth) printf '%s\\n' '{auth}'; exit {code} ;;\nesac\n"
    );
    let path = fixture.path("bin/claude");
    std::fs::write(&path, script).unwrap();
    mode(&path, 0o755);
}

#[test]
fn the_claude_verdict_names_a_fix_for_each_problem() {
    let current = Some(MIN_CLAUDE_VERSION);
    let missing = claude_verdict(ClaudeFacts::Missing);
    assert_eq!(
        (missing.status, missing.fix.as_deref()),
        (Status::Warn, Some(CLAUDE_INSTALL))
    );
    let unreadable = claude_verdict(ClaudeFacts::Found {
        version: None,
        signed_in: Some(true),
    });
    assert_eq!(
        (unreadable.status, unreadable.fix.as_deref()),
        (Status::Warn, Some("claude update"))
    );
    let old = claude_verdict(ClaudeFacts::Found {
        version: Some(ClaudeVersion::new(2, 1, 242)),
        signed_in: Some(true),
    });
    assert_eq!(
        (old.status, old.fix.as_deref()),
        (Status::Warn, Some("claude update"))
    );
    assert!(
        old.message.contains("2.1.242 is older than"),
        "{}",
        old.message
    );
    let signed_out = claude_verdict(ClaudeFacts::Found {
        version: current,
        signed_in: Some(false),
    });
    assert_eq!(
        (signed_out.status, signed_out.fix.as_deref()),
        (Status::Warn, Some("claude auth login"))
    );
    let unknown = claude_verdict(ClaudeFacts::Found {
        version: current,
        signed_in: None,
    });
    assert_eq!(
        (unknown.status, unknown.fix.as_deref()),
        (Status::Warn, Some("claude auth login"))
    );
    let good = claude_verdict(ClaudeFacts::Found {
        version: Some(ClaudeVersion::new(3, 0, 0)),
        signed_in: Some(true),
    });
    assert_eq!(
        (good.status, good.message.as_str()),
        (Status::Ok, "claude 3.0.0 is signed in")
    );
}

#[test]
fn the_claude_row_asks_the_claude_on_path() {
    let fixture = Fixture::new();
    let host = fixture.bare_host();
    assert_eq!(claude_check(&host).fix.as_deref(), Some(CLAUDE_INSTALL));
    let current = format!("{MIN_CLAUDE_VERSION} (Claude Code)");
    fake_claude(
        &fixture,
        &current,
        r#"{"loggedIn": true, "authMethod": "claude.ai", "email": "ana@example.com"}"#,
        0,
    );
    let check = find(&host_checks(None, &FileConfig::default(), &host), "claude").clone();
    assert_eq!(check.status, Status::Ok);
    assert!(!check.message.contains("ana@example.com"));
    fake_claude(&fixture, &current, r#"{"loggedIn": false}"#, 1);
    assert_eq!(
        claude_check(&host).fix.as_deref(),
        Some("claude auth login")
    );
    fake_claude(&fixture, "2.0.0 (Claude Code)", r#"{"loggedIn": true}"#, 0);
    let old = claude_check(&host);
    assert_eq!(old.fix.as_deref(), Some("claude update"));
    assert!(old.message.contains("2.0.0"));
}

#[test]
fn a_command_that_runs_too_long_is_stopped() {
    let fixture = Fixture::new();
    let slow = fixture.path("slow");
    std::fs::write(&slow, "#!/bin/sh\necho early\nexec sleep 30\n").unwrap();
    mode(&slow, 0o755);
    let started = std::time::Instant::now();
    assert_eq!(run_briefly(&slow, &[], Duration::from_millis(200)), None);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        run_briefly(&fixture.path("absent"), &[], CLAUDE_TIMEOUT),
        None
    );
}

#[test]
fn git_is_a_warning_when_missing() {
    let fixture = Fixture::new();
    let mut host = fixture.bare_host();
    let check = git_check(&host);
    assert_eq!(
        (check.status, check.fix.as_deref()),
        (Status::Warn, Some("install git"))
    );
    assert!(check.message.contains("fld pull"));
    executable(&fixture.path("bin/git"));
    host.path = Some(std::env::join_paths([fixture.path("bin")]).unwrap());
    assert_eq!(git_check(&host).status, Status::Ok);
}

// Service manager, linger, inotify

#[test]
fn a_probe_with_no_proc_and_no_systemd_fails_nothing() {
    let fixture = Fixture::new();
    let host = fixture.bare_host();
    let service = service_manager_check(&host);
    assert_eq!(service.status, Status::Warn);
    assert_eq!(service.message, "no service manager support");
    assert_eq!(
        service.fix.as_deref(),
        Some("fld serve, started by the host's own supervisor")
    );
    for check in [linger_check(&host), inotify_limit_check(&host)] {
        assert_eq!(
            (check.status, check.message.as_str()),
            (Status::Ok, NOT_APPLICABLE)
        );
    }
    let other = Host {
        platform: Platform {
            linux: false,
            macos: false,
            file_modes: false,
        },
        ..fixture.linux_host(100)
    };
    assert_eq!(inotify_limit_check(&other).message, NOT_APPLICABLE);
    assert_eq!(
        service_manager_check(&other).message,
        "no service manager support"
    );
    assert_eq!(linger_check(&other).message, NOT_APPLICABLE);
}

#[test]
fn on_macos_launchd_is_the_service_manager_and_linux_checks_do_not_apply() {
    let fixture = Fixture::new();
    // A probe root with systemd and inotify files must not count on macOS.
    let mut host = fixture.linux_host(100);
    host.platform = Platform {
        linux: false,
        macos: true,
        file_modes: true,
    };
    let missing = service_manager_check(&host);
    assert_eq!(missing.status, Status::Warn);
    assert_eq!(missing.message, "no service manager support");

    executable(&fixture.path("bin/launchctl"));
    let available = service_manager_check(&host);
    assert_eq!(available.status, Status::Ok);
    assert_eq!(
        available.message,
        "launchd user agents are available; fold's agent is not installed"
    );
    assert_eq!(available.fix, None);
    let agent = fixture.path("home/Library/LaunchAgents/dev.omukk.fold.plist");
    std::fs::create_dir_all(agent.parent().unwrap()).unwrap();
    std::fs::write(&agent, "").unwrap();
    assert_eq!(
        service_manager_check(&host).message,
        format!(
            "launchd user agents are available; fold's agent is {}",
            agent.display()
        )
    );
    for check in [linger_check(&host), inotify_limit_check(&host)] {
        assert_eq!(
            (check.status, check.message.as_str()),
            (Status::Ok, NOT_APPLICABLE)
        );
    }
}

#[test]
fn the_watch_limit_warns_below_65536_with_the_sysctl_fix() {
    let fixture = Fixture::new();
    let low = inotify_limit_check(&fixture.linux_host(8192));
    assert_eq!(low.status, Status::Warn);
    let fix = low.fix.unwrap();
    assert!(fix.contains("sudo sysctl fs.inotify.max_user_watches=524288"));
    assert!(fix.contains("/etc/sysctl.d/99-fold.conf"));
    assert_eq!(
        inotify_limit_check(&fixture.linux_host(65_535)).status,
        Status::Warn
    );
    assert_eq!(
        inotify_limit_check(&fixture.linux_host(65_536)).status,
        Status::Ok
    );
    assert_eq!(watch_limit_verdict(true, Some(1)), WatchLimit::Low(1));
    assert_eq!(
        watch_limit_verdict(false, Some(1)),
        WatchLimit::NotApplicable
    );
    assert_eq!(watch_limit_verdict(true, None), WatchLimit::NotApplicable);
    assert_eq!(watch_limit_verdict(true, Some(524_288)), WatchLimit::Enough);
}

#[test]
fn linger_warns_only_for_an_installed_service() {
    let fixture = Fixture::new();
    let host = fixture.linux_host(65_536);
    assert_eq!(service_manager_check(&host).status, Status::Ok);
    let not_installed = linger_check(&host);
    assert_eq!(not_installed.status, Status::Ok);
    assert!(not_installed.message.contains("not installed"));
    fixture.install_unit();
    let off = linger_check(&host);
    assert_eq!(off.status, Status::Warn);
    assert_eq!(off.fix.as_deref(), Some("loginctl enable-linger ana"));
    std::fs::write(fixture.path("root/var/lib/systemd/linger/ana"), "").unwrap();
    assert_eq!(linger_check(&host).status, Status::Ok);
    let mut nobody = host;
    nobody.user = None;
    assert_eq!(linger_check(&nobody).message, NOT_APPLICABLE);
}

// Project checks

#[test]
fn a_healthy_project_is_all_ok_and_nothing_changes_on_disk() {
    let fixture = Fixture::new();
    let host = fixture.linux_host(524_288);
    let root = fixture.project("demo");
    let before = snapshot(&root);
    let checks = project_report(&host, &root, at(1_000));
    let names: Vec<_> = checks.iter().map(|check| check.name).collect();
    assert_eq!(
        names,
        [
            "database",
            "objects",
            "git history",
            "project folder",
            "exclude file",
            "inotify watches",
            "claude files",
            "role prompts",
            "claude md"
        ]
    );
    for check in &checks {
        assert_eq!(check.status, Status::Ok, "{check:?}");
    }
    assert_eq!(snapshot(&root), before);
}

#[test]
fn a_project_without_git_sync_gets_no_git_check() {
    let fixture = Fixture::new();
    let host = fixture.linux_host(524_288);
    let root = fixture.plain_project("plain");
    let checks = project_report(&host, &root, at(1_000));
    let names: Vec<_> = checks.iter().map(|check| check.name).collect();
    assert_eq!(
        names,
        [
            "database",
            "objects",
            "git sync",
            "inotify watches",
            "claude files",
            "role prompts",
            "claude md"
        ]
    );
    let sync = find(&checks, "git sync");
    assert_eq!(sync.status, Status::Ok);
    assert!(sync.message.contains("fld init --with-git"), "{sync:?}");
}

#[test]
fn a_git_history_without_the_last_mapped_commit_names_the_repair() {
    let fixture = Fixture::new();
    let mut host = fixture.bare_host();
    host.path = std::env::var_os("PATH");
    let root = fixture.project("demo");
    let mirror = root.join(".git");
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&mirror)
            .args(args)
            .env("GIT_AUTHOR_NAME", "a")
            .env("GIT_AUTHOR_EMAIL", "a@b")
            .env("GIT_COMMITTER_NAME", "a")
            .env("GIT_COMMITTER_EMAIL", "a@b")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    std::fs::create_dir_all(&mirror).unwrap();
    run(&["init", "--quiet", "--bare"]);
    let map = root.join(".fold").join(SYNC_MAP_FILE);
    std::fs::write(&map, format!("r1 {}\n", "a".repeat(40))).unwrap();
    let checks = project_report(&host, &root, at(1_000));
    let check = find(&checks, "git history");
    assert_eq!(check.status, Status::Warn);
    assert!(check.message.contains("the commit of r1"), "{check:?}");
    assert!(check.fix.as_deref().unwrap().contains("fld pull"));

    let commit = run(&[
        "commit-tree",
        "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
        "-m",
        "x",
    ]);
    std::fs::write(&map, format!("r1 {commit}\n")).unwrap();
    let checks = project_report(&host, &root, at(1_000));
    assert_eq!(find(&checks, "git history").status, Status::Ok);

    std::fs::write(&map, "not a map\n").unwrap();
    let checks = project_report(&host, &root, at(1_000));
    assert_eq!(find(&checks, "git history").status, Status::Fail);
    host.path = None;
    std::fs::write(&map, format!("r1 {commit}\n")).unwrap();
    let checks = project_report(&host, &root, at(1_000));
    assert_eq!(find(&checks, "git history").message, NOT_APPLICABLE);
}

#[test]
fn a_missing_git_repository_is_not_reported_as_a_missing_commit() {
    let fixture = Fixture::new();
    let mut host = fixture.bare_host();
    host.path = std::env::var_os("PATH");
    let root = fixture.project("demo");
    std::fs::write(
        root.join(".fold").join(SYNC_MAP_FILE),
        format!("r1 {}\n", "a".repeat(40)),
    )
    .unwrap();
    std::fs::remove_dir_all(root.join(".git")).unwrap();
    let checks = project_report(&host, &root, at(1_000));
    let check = find(&checks, "git history");
    assert_eq!(check.status, Status::Warn);
    assert!(check.message.contains("no Git repository"), "{check:?}");
    assert!(check.fix.as_deref().unwrap().contains("fld init"));
}

#[test]
fn a_registry_path_that_is_not_utf8_is_refused_not_replaced_by_the_default() {
    use std::os::unix::ffi::OsStrExt;
    let dirs = Dirs {
        data_home: Some(PathBuf::from(std::ffi::OsStr::from_bytes(b"/data/\xff"))),
        home: Some(PathBuf::from("/home/ana")),
        ..Dirs::default()
    };
    let error = registered_projects(&dirs, &rt()).unwrap_err();
    assert_eq!(error.code(), Some("invalid_host_paths"));
}

#[test]
fn a_descendant_that_keeps_stdout_open_does_not_hold_the_command_nor_hide_its_output() {
    let fixture = Fixture::new();
    let leaky = fixture.path("leaky");
    std::fs::write(&leaky, "#!/bin/sh\nsleep 5 &\necho early\n").unwrap();
    mode(&leaky, 0o755);
    let started = std::time::Instant::now();
    assert_eq!(
        run_briefly(&leaky, &[], Duration::from_millis(300)).as_deref(),
        Some("early\n")
    );
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[test]
fn a_project_folder_with_changes_that_are_not_committed_is_a_warning() {
    let fixture = Fixture::new();
    let mut host = fixture.bare_host();
    host.path = std::env::var_os("PATH");
    let root = fixture.project("demo");
    let folder = |host: &Host| {
        let checks = project_report(host, &root, at(1_000));
        find(&checks, "project folder").clone()
    };
    assert_eq!(folder(&host).status, Status::Ok);
    std::fs::write(root.join(".git/info/exclude"), "/.fold/\n").unwrap();
    std::fs::write(root.join("app.txt"), "one\n").unwrap();
    git(&root, &["add", "app.txt"]);
    git(&root, &["commit", "--quiet", "-m", "one"]);
    let index = std::fs::read(root.join(".git/index")).unwrap();
    assert_eq!(folder(&host).status, Status::Ok);
    // An untracked file does not hold the folder back; a change to a tracked one does.
    std::fs::write(root.join("new.txt"), "x\n").unwrap();
    assert_eq!(folder(&host).status, Status::Ok);
    std::fs::write(root.join("app.txt"), "two\n").unwrap();
    let check = folder(&host);
    assert_eq!(check.status, Status::Warn);
    assert!(
        check.fix.as_deref().unwrap().contains("fld record"),
        "{check:?}"
    );
    assert!(check.fix.as_deref().unwrap().contains("fld record"));
    // The check reads the folder and never writes the index.
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
    host.path = None;
    assert_eq!(folder(&host).message, NOT_APPLICABLE);
    host.path = std::env::var_os("PATH");
    std::fs::remove_dir_all(root.join(".git")).unwrap();
    let missing = folder(&host);
    assert_eq!(missing.status, Status::Warn);
    assert!(missing.fix.as_deref().unwrap().contains("fld init"));
}

#[test]
fn a_damaged_database_copy_fails_naming_the_file() {
    let fixture = Fixture::new();
    let host = fixture.bare_host();
    let root = fixture.project("demo");
    let db = root.join(".fold/fold.db");
    let good = std::fs::read(&db).unwrap();

    std::fs::write(
        &db,
        b"this is not a database at all, it is plain text of some length",
    )
    .unwrap();
    let garbage = project_report(&host, &root, at(0));
    let check = find(&garbage, "database");
    assert_eq!(check.status, Status::Fail);
    assert!(
        check.message.contains("quick_check failed"),
        "{}",
        check.message
    );
    assert_eq!(
        check.fix,
        Some(format!("restore {} from a backup", db.display()))
    );

    // Pages the connection never reads at open time: only quick_check finds the damage.
    let mut late = good.clone();
    let from = late.len() / 2;
    late[from..from + 8192].fill(0xAB);
    std::fs::write(&db, late).unwrap();
    let checks = project_report(&host, &root, at(0));
    let check = find(&checks, "database");
    assert_eq!(check.status, Status::Fail);
    assert!(check.message.contains("quick_check"), "{}", check.message);

    let mut early = good.clone();
    early[4096..16_384].fill(0);
    std::fs::write(&db, early).unwrap();
    let checks = project_report(&host, &root, at(0));
    assert_eq!(find(&checks, "database").status, Status::Fail);

    let mut rollback = good;
    rollback[18] = 1;
    rollback[19] = 1;
    std::fs::write(&db, rollback).unwrap();
    let checks = project_report(&host, &root, at(0));
    let check = find(&checks, "database");
    assert_eq!(check.status, Status::Fail);
    assert!(check.message.contains("journal_mode is not wal"));

    std::fs::remove_file(&db).unwrap();
    let checks = project_report(&host, &root, at(0));
    assert!(find(&checks, "database").message.contains("missing"));
}

#[test]
fn the_database_facts_map_to_issues() {
    let healthy = DatabaseFacts {
        exists: true,
        wal: Some(true),
        quick_check_problems: Some(0),
        quick_check_errored: false,
        degraded: Vec::new(),
        git_sync: Some(true),
    };
    assert!(database_issues(&healthy).is_empty());
    let unknown_journal = DatabaseFacts {
        wal: None,
        ..healthy
    };
    assert!(database_issues(&unknown_journal).is_empty());
    let errored = DatabaseFacts {
        quick_check_errored: true,
        ..DatabaseFacts {
            wal: None,
            ..DatabaseFacts::default()
        }
    };
    assert_eq!(
        database_issues(&DatabaseFacts {
            exists: true,
            ..errored
        }),
        ["quick_check failed"]
    );
    let problems = DatabaseFacts {
        quick_check_problems: Some(3),
        ..unknown_journal
    };
    assert_eq!(
        database_issues(&problems),
        ["quick_check reported 3 problems"]
    );
    assert_eq!(
        database_issues(&DatabaseFacts::default()),
        ["the file is missing"]
    );
}

#[test]
fn the_header_says_whether_the_journal_is_wal() {
    let mut header = [0_u8; 100];
    header[..16].copy_from_slice(SQLITE_MAGIC);
    assert_eq!(header_is_wal(&header), Some(false));
    header[18] = 2;
    header[19] = 2;
    assert_eq!(header_is_wal(&header), Some(true));
    assert_eq!(header_is_wal(&header[..19]), None);
    assert_eq!(header_is_wal(&[0_u8; 100]), None);
}

#[test]
fn a_degraded_event_from_the_last_day_warns_on_the_inotify_check() {
    let fixture = Fixture::new();
    let host = fixture.linux_host(524_288);
    let root = fixture.project("demo");
    let now = at(100 * HOUR_MS);
    seed_event(&root, "watch_limit", at(99 * HOUR_MS));
    let checks = project_report(&host, &root, now);
    let check = find(&checks, "inotify watches");
    assert_eq!(check.status, Status::Warn);
    assert!(check.message.contains("watch_limit"));
    assert!(
        check
            .fix
            .as_deref()
            .unwrap()
            .contains("sudo sysctl fs.inotify.max_user_watches=524288")
    );
    assert!(checks.iter().all(|check| check.status != Status::Fail));

    let later = project_report(&host, &root, at(124 * HOUR_MS + 1));
    assert_eq!(find(&later, "inotify watches").status, Status::Ok);
}

#[test]
fn other_degraded_reasons_and_a_host_without_inotify_do_not_warn() {
    let fixture = Fixture::new();
    let root = fixture.project("demo");
    seed_event(&root, "unsupported_filesystem", at(99 * HOUR_MS));
    let host = fixture.linux_host(524_288);
    let checks = project_report(&host, &root, at(100 * HOUR_MS));
    assert_eq!(find(&checks, "inotify watches").status, Status::Ok);
    seed_event(&root, "watch_limit", at(99 * HOUR_MS));
    let bare = Fixture::new().bare_host();
    let checks = project_report(&bare, &root, at(100 * HOUR_MS));
    let check = find(&checks, "inotify watches");
    assert_eq!(
        (check.status, check.message.as_str()),
        (Status::Ok, NOT_APPLICABLE)
    );
}

#[test]
fn the_degraded_window_is_24_hours() {
    let now = at(48 * HOUR_MS);
    let events = [
        at(24 * HOUR_MS + 1),
        at(24 * HOUR_MS),
        at(0),
        at(48 * HOUR_MS),
        at(49 * HOUR_MS),
    ];
    assert_eq!(within_degraded_window(&events, now), 3);
    assert_eq!(within_degraded_window(&[], now), 0);
}

#[test]
fn the_degraded_query_starts_where_the_window_does() {
    let now = at(48 * HOUR_MS);
    let start = degraded_window_start(now);
    assert_eq!(start, 24 * HOUR_MS);
    assert_eq!(within_degraded_window(&[at(start)], now), 0);
    assert_eq!(within_degraded_window(&[at(start + 1)], now), 1);
    assert_eq!(degraded_window_start(at(0)), -24 * HOUR_MS);
}

#[test]
fn objects_checks_cover_missing_unwritable_and_low_space() {
    let path = Path::new("/p/.fold/objects");
    let ok = ObjectsFacts {
        exists: true,
        writable: true,
        free_bytes: Some(MIN_FREE_BYTES),
    };
    assert_eq!(objects_verdict(path, ok).status, Status::Ok);
    assert_eq!(
        objects_verdict(
            path,
            ObjectsFacts {
                free_bytes: None,
                ..ok
            }
        )
        .status,
        Status::Ok
    );
    let not_created = objects_verdict(
        path,
        ObjectsFacts {
            exists: false,
            ..ok
        },
    );
    assert_eq!(not_created.status, Status::Ok);
    let locked = objects_verdict(
        path,
        ObjectsFacts {
            writable: false,
            ..ok
        },
    );
    assert_eq!(locked.status, Status::Fail);
    assert_eq!(
        locked.fix.as_deref(),
        Some("chmod -R u+rwX /p/.fold/objects")
    );
    let low = objects_verdict(
        path,
        ObjectsFacts {
            free_bytes: Some(MIN_FREE_BYTES - 1),
            ..ok
        },
    );
    assert_eq!(low.status, Status::Warn);
    assert!(low.fix.unwrap().contains("df -h /p/.fold/objects"));
}

#[test]
fn the_objects_directory_is_inspected_on_disk() {
    let fixture = Fixture::new();
    let objects = fixture.path("objects");
    let absent = inspect_objects(&objects);
    assert!(!absent.exists && absent.writable);
    assert!(!inspect_objects(&fixture.path("none/objects")).writable);
    std::fs::create_dir(&objects).unwrap();
    let facts = inspect_objects(&objects);
    assert!(facts.exists && facts.writable);
    assert!(facts.free_bytes.is_some());
    mode(&objects, 0o500);
    let locked = inspect_objects(&objects);
    // A superuser can write anywhere, so only an unprivileged run proves the check.
    if !rustix::process::geteuid().is_root() {
        assert!(!locked.writable);
        assert_eq!(objects_check(&objects).status, Status::Fail);
    }
    mode(&objects, 0o700);
}

#[test]
fn a_missing_project_directory_fails_alone() {
    let fixture = Fixture::new();
    let checks = project_report(&fixture.bare_host(), &fixture.path("gone"), at(0));
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].status, Status::Fail);
    assert!(checks[0].fix.as_deref().unwrap().contains("gone"));
}

#[test]
fn a_project_folder_without_its_fold_data_fails_alone_with_the_init_that_replaces_it() {
    let fixture = Fixture::new();
    let host = fixture.linux_host(RECOMMENDED_WATCHES);
    let root = fixture.project("my app");
    std::fs::remove_dir_all(root.join(".fold")).unwrap();
    let checks = project_report(&host, &root, at(0));
    assert_eq!(checks.len(), 1);
    let check = &checks[0];
    assert_eq!((check.name, check.status), ("fold data", Status::Fail));
    assert_eq!(
        check.message,
        format!(
            "{} is missing: the fold data of the project was deleted",
            root.join(".fold").display()
        )
    );
    let quoted = output::shell_quote(&root);
    assert_eq!(
        check.fix.as_deref(),
        Some(format!("cd {quoted} && fld init --from-git").as_str())
    );
    std::fs::remove_dir_all(root.join(".git")).unwrap();
    let checks = project_report(&host, &root, at(0));
    assert_eq!(
        checks[0].fix.as_deref(),
        Some(format!("cd {quoted} && fld init").as_str())
    );
}

// Claude Code files

#[test]
fn the_claude_files_warn_for_each_missing_fold_entry() {
    let agents = &good_agents();
    let named = (AGENTS_FILE, Some(agents.as_str()));
    assert!(claude_file_problems(named, true, &[]).is_empty());
    assert_eq!(
        claude_file_problems((AGENTS_FILE, None), false, &[]),
        [
            "AGENTS.md is missing",
            ".claude/skills/fold/SKILL.md is missing"
        ]
    );
    assert_eq!(
        claude_file_problems((AGENTS_FILE, Some("no markers")), true, &[]),
        ["AGENTS.md lacks the fold section"]
    );
    assert_eq!(
        claude_file_problems((AGENTS_FILE, Some(AGENTS_BEGIN)), true, &[]),
        ["AGENTS.md lacks the fold section"]
    );
    assert_eq!(
        claude_file_problems((CLAUDE_FILE, None), true, &[]),
        ["CLAUDE.md is missing"]
    );
    assert_eq!(
        claude_file_problems(named, true, &[MCP_FILE, SETTINGS_FILE]),
        [
            ".mcp.json holds fold entries, which belong in your Claude Code config",
            ".claude/settings.json holds fold entries, which belong in your Claude Code config"
        ]
    );
}

#[test]
fn fold_entries_left_in_the_project_files_name_the_refresh() {
    let fixture = Fixture::new();
    let root = fixture.project("demo");
    let report = |root: &Path| project_report(&fixture.bare_host(), root, at(0));
    std::fs::write(root.join(MCP_FILE), r#"{"mcpServers":{"pencil":{}}}"#).unwrap();
    std::fs::write(root.join(SETTINGS_FILE), r#"{"model":"opus"}"#).unwrap();
    assert_eq!(find(&report(&root), "claude files").status, Status::Ok);
    std::fs::write(root.join(MCP_FILE), good_mcp()).unwrap();
    std::fs::write(root.join(SETTINGS_FILE), good_settings()).unwrap();
    let checks = report(&root);
    let check = find(&checks, "claude files");
    assert_eq!(check.status, Status::Warn);
    assert_eq!(
        check.message,
        ".mcp.json holds fold entries, which belong in your Claude Code config; .claude/settings.json holds fold entries, which belong in your Claude Code config"
    );
    assert_eq!(check.fix.as_deref(), Some("fld init --refresh"));
}

#[test]
fn the_claude_setup_check_reads_the_user_files_and_names_the_fix() {
    let fixture = Fixture::new();
    let host = fixture.bare_host();
    let files = host.claude.clone().unwrap();
    let check = |host: &Host| {
        find(
            &host_checks(None, &FileConfig::default(), host),
            "claude setup",
        )
        .clone()
    };
    let missing = check(&host);
    assert_eq!(missing.status, Status::Warn);
    assert_eq!(
        missing.message,
        format!(
            "{} lacks the fold server; {} does not allow the fold tools and lacks the fold hooks",
            files.config.display(),
            files.settings.display()
        )
    );
    assert_eq!(missing.fix.as_deref(), Some("fld setup --claude"));
    std::fs::create_dir_all(files.settings.parent().unwrap()).unwrap();
    std::fs::write(&files.config, good_mcp()).unwrap();
    std::fs::write(&files.settings, settings(ALLOW_FOLD_TOOLS, &HOOKS[..1])).unwrap();
    assert_eq!(
        check(&host).message,
        format!("{} lacks the fold hooks", files.settings.display())
    );
    std::fs::write(&files.settings, good_settings()).unwrap();
    let complete = check(&host);
    assert_eq!(
        (complete.status, complete.message.as_str()),
        (Status::Ok, "fold is in your Claude Code config")
    );
    let declined = claude_setup_verdict(Some(UserSetup::Incomplete(vec!["x".into()])), true);
    assert_eq!(declined.status, Status::Ok);
    assert!(declined.message.ends_with("fld setup --claude adds it"));
    let mut homeless = host;
    homeless.claude = None;
    assert_eq!(check(&homeless).message, NOT_APPLICABLE);
}

#[test]
fn the_fold_tools_and_hooks_are_matched_by_entry_not_by_substring() {
    let ok = |text: &str| {
        let value = serde_json::from_str(text).unwrap();
        (
            allows_fold_tools(&value),
            HOOKS
                .iter()
                .all(|(event, command)| runs_hook(&value, event, command)),
        )
    };
    assert_eq!(ok(&settings("mcp__fold__*", &HOOKS)), (true, true));
    assert_eq!(ok(&settings("mcp__folder", &HOOKS)), (false, true));
    let full_path: Vec<_> = HOOKS
        .iter()
        .map(|(event, command)| (*event, format!("/usr/local/bin/{command}")))
        .collect();
    let full_path: Vec<_> = full_path.iter().map(|(e, c)| (*e, c.as_str())).collect();
    assert_eq!(ok(&settings(ALLOW_FOLD_TOOLS, &full_path)), (true, true));
    let swapped = [(HOOKS[0].0, HOOKS[1].1), (HOOKS[1].0, HOOKS[0].1)];
    assert_eq!(ok(&settings(ALLOW_FOLD_TOOLS, &swapped)), (true, false));
    assert_eq!(
        ok(r#"{"permissions":{"allow":"mcp__fold"}}"#),
        (false, false)
    );
}

#[test]
fn a_role_prompt_under_a_name_of_no_role_is_named_with_the_roles() {
    let fixture = Fixture::new();
    let root = fixture.project("demo");
    let roles = root.join(".fold/roles");
    std::fs::create_dir_all(&roles).unwrap();
    for name in ["task_worker.md", "main.md", "notes.txt"] {
        std::fs::write(roles.join(name), "# prompt\n").unwrap();
    }
    let checks = project_report(&fixture.bare_host(), &root, at(0));
    assert_eq!(find(&checks, "role prompts").status, Status::Ok);
    std::fs::write(roles.join("reviewer.md"), "# prompt\n").unwrap();
    std::fs::write(roles.join("area-lead.md"), "# prompt\n").unwrap();
    let checks = project_report(&fixture.bare_host(), &root, at(0));
    let check = find(&checks, "role prompts");
    assert_eq!(check.status, Status::Warn);
    assert_eq!(
        check.message,
        "fold never reads area-lead.md, reviewer.md in .fold/roles, because no role has that name"
    );
    assert_eq!(
        check.fix.as_deref(),
        Some(
            "rename each file to <role>.md, where the role is area_lead, review_lead, task_worker, batch_reviewer or main"
        )
    );
}

#[test]
fn an_agents_file_without_the_markers_gives_the_claude_files_warning() {
    let fixture = Fixture::new();
    let root = fixture.project("demo");
    std::fs::write(root.join(AGENTS_FILE), "# my rules\n").unwrap();
    let checks = project_report(&fixture.bare_host(), &root, at(0));
    let check = find(&checks, "claude files");
    assert_eq!(check.status, Status::Warn);
    assert!(check.message.contains("AGENTS.md lacks the fold section"));
    assert_eq!(check.fix.as_deref(), Some("fld init --refresh"));
    assert_eq!(
        failed(&[Section {
            title: String::new(),
            checks
        }]),
        0
    );
}

#[test]
fn claude_md_files_in_the_root_and_a_parent_warn_once_per_path() {
    let fixture = Fixture::new();
    let root = fixture.project("demo");
    std::fs::write(root.join(".claude/CLAUDE.md"), "rules").unwrap();
    std::fs::write(fixture.path("CLAUDE.local.md"), "mine").unwrap();
    let checks = project_report(&fixture.bare_host(), &root, at(0));
    let shadows: Vec<_> = checks
        .iter()
        .filter(|check| check.name == "claude md")
        .collect();
    assert_eq!(shadows.len(), 2);
    for check in &shadows {
        assert_eq!(check.status, Status::Warn);
    }
    let root_file = root.join(".claude/CLAUDE.md");
    assert_eq!(
        shadows[0].message,
        format!("{} makes Claude Code skip AGENTS.md", root_file.display())
    );
    assert_eq!(
        shadows[0].fix,
        Some(format!(
            "move the text of {0} into AGENTS.md and delete {0}",
            root_file.display()
        ))
    );
    let parent_file = fixture.path("CLAUDE.local.md");
    assert!(
        shadows[1]
            .message
            .starts_with(&parent_file.display().to_string())
    );
    assert_eq!(
        failed(&[Section {
            title: String::new(),
            checks
        }]),
        0
    );
}

/// The section belongs in a root `CLAUDE.md` file, which then hides nothing; a `CLAUDE.md` link to `AGENTS.md` hides nothing either, and a link to another file does.
#[test]
fn the_claude_files_check_reads_the_file_that_gets_the_section() {
    let fixture = Fixture::new();
    let root = fixture.project("demo");
    let report = |root: &Path| project_report(&fixture.bare_host(), root, at(0));
    let claude_md = |checks: &[Check]| {
        let check = find(checks, "claude md");
        (check.status, check.message.clone())
    };

    std::fs::write(root.join(CLAUDE_FILE), "# mine\n").unwrap();
    let checks = report(&root);
    let check = find(&checks, "claude files");
    assert_eq!(check.status, Status::Warn);
    assert_eq!(
        check.message,
        "CLAUDE.md lacks the fold section; AGENTS.md holds a second fold section, which belongs in CLAUDE.md"
    );
    assert_eq!(check.fix.as_deref(), Some("fld init --refresh"));
    assert_eq!(
        claude_md(&checks),
        (Status::Ok, "no CLAUDE.md hides AGENTS.md".to_owned())
    );

    std::fs::write(root.join(CLAUDE_FILE), good_agents()).unwrap();
    std::fs::write(root.join(AGENTS_FILE), "# shared\n").unwrap();
    assert_eq!(find(&report(&root), "claude files").status, Status::Ok);

    std::fs::remove_file(root.join(CLAUDE_FILE)).unwrap();
    std::fs::write(root.join(AGENTS_FILE), good_agents()).unwrap();
    std::os::unix::fs::symlink(AGENTS_FILE, root.join(CLAUDE_FILE)).unwrap();
    let checks = report(&root);
    assert_eq!(find(&checks, "claude files").status, Status::Ok);
    assert_eq!(claude_md(&checks).0, Status::Ok);

    std::fs::remove_file(root.join(CLAUDE_FILE)).unwrap();
    std::fs::write(root.join("rules.md"), "# rules\n").unwrap();
    std::os::unix::fs::symlink("rules.md", root.join(CLAUDE_FILE)).unwrap();
    let checks = report(&root);
    assert_eq!(find(&checks, "claude files").status, Status::Ok);
    assert_eq!(
        claude_md(&checks),
        (
            Status::Warn,
            format!(
                "{} makes Claude Code skip AGENTS.md",
                root.join(CLAUDE_FILE).display()
            )
        )
    );
}

#[test]
fn a_relative_project_directory_is_made_absolute() {
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(absolute_dir(Path::new(".")), cwd.canonicalize().unwrap());
    assert_eq!(
        absolute_dir(Path::new("no/such/dir")),
        cwd.join("no/such/dir")
    );
    assert_eq!(absolute_dir(Path::new("")), PathBuf::new());
    let fixture = Fixture::new();
    let root = fixture.project("demo");
    let args = DoctorArgs {
        project: Some(root.join("../demo")),
        socket: None,
    };
    let sections = collect(&args, &fixture.bare_host(), at(0), &rt()).unwrap();
    assert_eq!(
        sections[1].title,
        format!("project {}", root.canonicalize().unwrap().display())
    );
}

#[test]
fn the_users_own_claude_file_does_not_count() {
    let fixture = Fixture::new();
    let home = fixture.path("home");
    let root = home.join("work/demo");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(home.join(".claude/CLAUDE.md"), "mine").unwrap();
    assert!(shadowing_files(&root, Some(&home)).is_empty());
    let found = shadowing_files(&root, None);
    assert_eq!(found, [home.join(".claude/CLAUDE.md")]);
    std::fs::create_dir_all(root.join(".claude")).unwrap();
    std::fs::write(root.join(".claude/CLAUDE.md"), "x").unwrap();
    assert_eq!(
        shadowing_files(&root, Some(&home)),
        [root.join(".claude/CLAUDE.md")]
    );
    let clean = shadowing_checks(&fixture.path("home/elsewhere"), Some(&home));
    assert_eq!(clean.len(), 1);
    assert_eq!(clean[0].status, Status::Ok);
}

// Whole runs

#[test]
fn the_report_lists_every_check_with_its_fix_line() {
    let sections = vec![Section {
        title: "host".to_owned(),
        checks: vec![
            Check::ok("one", "fine"),
            Check::warn("two", "careful", "do this"),
            Check::fail("three", "broken", "do that"),
        ],
    }];
    let mut out = Vec::new();
    write_report(&sections, &mut out).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "host\n  ok   one: fine\n  warn two: careful\n       fix: do this\n  fail three: broken\n       fix: do that\n"
    );
    assert_eq!(failed(&sections), 1);
}

#[test]
fn collect_checks_one_project_without_a_daemon() {
    let fixture = Fixture::new();
    let host = fixture.bare_host();
    let root = fixture.project("demo");
    std::fs::write(root.join(".fold/fold.db"), b"damaged").unwrap();
    let args = DoctorArgs {
        project: Some(root.clone()),
        socket: None,
    };
    let sections = collect(&args, &host, at(0), &rt()).unwrap();
    assert_eq!(sections[0].title, "host");
    assert_eq!(sections[1].title, format!("project {}", root.display()));
    assert_eq!(failed(&sections), 1);
    let host_checks = &sections[0].checks;
    assert_eq!(find(host_checks, "socket").status, Status::Warn);
    assert_eq!(find(host_checks, "http").name, "http");
}

#[test]
fn collect_reads_the_projects_from_the_registry() {
    let fixture = Fixture::new();
    let host = fixture.bare_host();
    let runtime = rt();
    let args = DoctorArgs {
        project: None,
        socket: None,
    };
    let none = collect(&args, &host, at(0), &runtime).unwrap();
    assert_eq!(none.len(), 1);

    let registry = fixture.path("data/fold/registry.db");
    std::fs::create_dir_all(registry.parent().unwrap()).unwrap();
    std::fs::write(&registry, b"garbage").unwrap();
    let broken = collect(&args, &host, at(0), &runtime).unwrap();
    let check = find(&broken[0].checks, "registry");
    assert_eq!(check.status, Status::Fail);
    assert!(check.fix.as_deref().unwrap().contains("registry.db"));

    std::fs::remove_file(&registry).unwrap();
    let root = fixture.project("demo");
    runtime.block_on(async {
        let store = Store::open_registry(&registry).await.unwrap();
        store
            .write::<_, StoreError>(async |tx| {
                sqlx::query("INSERT INTO projects (slug, name, root, created_at) VALUES ('demo', 'Demo', ?, 0)")
                    .bind(root.to_str().unwrap())
                    .execute(tx.conn())
                    .await
                    .map_err(StoreError::from)?;
                Ok(())
            })
            .await
            .unwrap();
        store.close().await.unwrap();
    });
    let listed = collect(&args, &host, at(0), &runtime).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[1].title, "project demo");
    assert_eq!(failed(&listed), 0);
}

#[test]
fn a_host_without_a_home_cannot_find_the_registry() {
    let fixture = Fixture::new();
    let mut host = fixture.bare_host();
    host.dirs.home = None;
    host.dirs.data_home = None;
    let args = DoctorArgs {
        project: None,
        socket: None,
    };
    let error = collect(&args, &host, at(0), &rt()).unwrap_err();
    assert_eq!(error.code(), Some("invalid_host_paths"));
}

#[test]
fn a_bad_daemon_file_is_a_failed_check_and_the_defaults_stand_in() {
    let fixture = Fixture::new();
    let host = fixture.bare_host();
    assert!(daemon_file(&host.dirs).1.is_none());
    std::fs::create_dir_all(fixture.path("config/fold")).unwrap();
    std::fs::write(fixture.path("config/fold/daemon.toml"), "listen = 5").unwrap();
    let (file, check) = daemon_file(&host.dirs);
    assert!(file.listen.is_none());
    let check = check.unwrap();
    assert_eq!(check.status, Status::Fail);
    assert!(check.fix.unwrap().contains("daemon.toml"));
    assert!(daemon_file(&Dirs::default()).1.is_none());
}

#[test]
fn the_socket_flag_beats_the_file_and_the_runtime_directory() {
    let fixture = Fixture::new();
    let host = fixture.bare_host();
    let flag = fixture.path("flag.sock");
    let file = FileConfig::default();
    let checks = host_checks(Some(&flag), &file, &host);
    assert!(find(&checks, "socket").message.contains("flag.sock"));
    std::fs::create_dir_all(fixture.path("config/fold")).unwrap();
    let toml = format!("socket = \"{}\"\n", fixture.path("file.sock").display());
    std::fs::write(fixture.path("config/fold/daemon.toml"), toml).unwrap();
    let checks = host_checks(Some(&flag), &file, &host);
    assert!(find(&checks, "socket").message.contains("flag.sock"));
    let checks = host_checks(None, &file, &host);
    assert!(find(&checks, "socket").message.contains("file.sock"));
    std::fs::write(fixture.path("config/fold/daemon.toml"), "bogus = 1").unwrap();
    let checks = host_checks(None, &file, &host);
    assert!(
        find(&checks, "socket")
            .message
            .contains("run/fold/fold.sock")
    );
    std::fs::remove_file(fixture.path("config/fold/daemon.toml")).unwrap();
    let checks = host_checks(None, &file, &host);
    assert!(
        find(&checks, "socket")
            .message
            .contains("run/fold/fold.sock")
    );
    let mut no_runtime = host;
    no_runtime.dirs.runtime_dir = None;
    let checks = host_checks(None, &FileConfig::default(), &no_runtime);
    assert!(find(&checks, "socket").message.contains("XDG_RUNTIME_DIR"));
    assert_eq!(find(&checks, "locks").status, Status::Ok);
}

#[test]
fn errors_carry_their_codes() {
    assert_eq!(
        DoctorError::ChecksFailed { failed: 2 }.code(),
        Some("doctor_checks_failed")
    );
    assert_eq!(
        DoctorError::ChecksFailed { failed: 2 }.to_string(),
        "checks failed: 2"
    );
    assert_eq!(DoctorError::Runtime(io::Error::other("x")).code(), None);
    assert_eq!(DoctorError::from(io::Error::other("x")).code(), None);
}

#[test]
fn a_database_in_use_by_a_daemon_is_read_through_its_wal() {
    let fixture = Fixture::new();
    let root = fixture.project("demo");
    let runtime = rt();
    runtime.block_on(async {
        let store = Store::open_project(&ProjectPaths::new(&root))
            .await
            .unwrap();
        let mut event =
            NewEvent::new(WORKSPACE_WATCH_DEGRADED, EventActor::parse("fold").unwrap()).unwrap();
        event.data = serde_json::json!({ "reason": "watch_limit" });
        // A row from before the reason column: only `data` names the cause, so the check does not count it.
        let untagged = event.clone();
        event = event.reason("watch_limit");
        store
            .write::<_, StoreError>(async |tx| {
                events::append(tx, &event, at(5)).await?;
                events::append(tx, &untagged, at(5)).await?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(root.join(".fold/fold.db-wal").exists());
        let facts = inspect_database(&root.join(".fold/fold.db"), Some(at(5))).await;
        assert_eq!(facts.quick_check_problems, Some(0));
        assert_eq!(facts.degraded, [at(5)]);
        store.close().await.unwrap();
    });
}

#[test]
fn an_info_exclude_that_is_not_the_link_names_the_refresh() {
    let fixture = Fixture::new();
    let root = fixture.project("demo");
    let host = fixture.bare_host();
    let check = |root: &Path| find(&project_report(&host, root, at(1_000)), "exclude file").clone();
    assert_eq!(check(&root).status, Status::Ok);
    std::fs::remove_file(root.join(".git/info/exclude")).unwrap();
    std::fs::write(root.join(".git/info/exclude"), "*.log\n").unwrap();
    let plain = check(&root);
    assert_eq!(plain.status, Status::Warn);
    assert!(plain.fix.as_deref().unwrap().contains("fld init --refresh"));
    std::fs::remove_dir_all(root.join(".git")).unwrap();
    assert_eq!(check(&root).message, NOT_APPLICABLE);
}

#[test]
fn the_fold_name_shows_where_it_comes_from() {
    let fixture = Fixture::new();
    let mut host = fixture.bare_host();
    let check = find(
        &host_checks(None, &FileConfig::default(), &host),
        "fold name",
    )
    .clone();
    assert_eq!(
        (check.status, check.message.as_str()),
        (Status::Ok, "ana from USER")
    );
    let stored = fixture.path("config/fold/human.toml");
    std::fs::create_dir_all(stored.parent().unwrap()).unwrap();
    std::fs::write(&stored, "name = \"bo\"\n").unwrap();
    let check = find(
        &host_checks(None, &FileConfig::default(), &host),
        "fold name",
    )
    .clone();
    assert_eq!(check.message, format!("bo from {}", stored.display()));
    host.names.fold_human = Some("Lead Dev".into());
    let check = find(
        &host_checks(None, &FileConfig::default(), &host),
        "fold name",
    )
    .clone();
    assert_eq!(check.status, Status::Warn);
    assert_eq!(
        check.message,
        "Lead Dev from FOLD_HUMAN is not a name fold accepts"
    );
    assert!(check.fix.as_deref().unwrap().contains("FOLD_HUMAN"));
    let none = fold_name_check(None);
    assert_eq!(
        (none.status, none.message.as_str()),
        (Status::Warn, "fld cannot tell who you are")
    );
}
