#![cfg(test)]

use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use fold_daemon::config::LockOwner;
use fold_store::testing::eventually;
use tokio::process::Child;

use crate::common::{DaemonGuard, Host, daemons};

const WAIT: Duration = Duration::from_secs(20);
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn lock(host: &Host) -> std::path::PathBuf {
    host.runtime_base().join("fold").join("daemon.lock")
}

/// The record of the daemon that holds the lock now.
fn owner(host: &Host) -> LockOwner {
    LockOwner::parse(&std::fs::read_to_string(lock(host)).unwrap()).unwrap()
}

/// `fld serve --socket <socket>` as a child of the test, once it answers.
async fn serve(host: &Host, socket: &Path) -> Child {
    let mut command = host.fold(&["serve", "--socket", socket.to_str().unwrap()], &host.root);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command.spawn().unwrap();
    eventually(WAIT, async || {
        tokio::net::UnixStream::connect(socket)
            .await
            .map(drop)
            .map_err(|error| error.kind())
    })
    .await;
    child
}

async fn restart(host: &Host, path: Option<&std::ffi::OsStr>) -> Output {
    let socket = host.default_socket();
    let mut command = host.fold(
        &["restart", "--socket", socket.to_str().unwrap()],
        &host.root,
    );
    command.stdin(Stdio::null());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let output = tokio::time::timeout(WAIT * 4, command.output())
        .await
        .expect("fld restart ends")
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[tokio::test]
async fn restart_with_no_daemon_starts_one_and_says_so() {
    let host = Host::new().await;
    let socket = host.default_socket();
    let output = restart(&host, None).await;
    let guard = DaemonGuard(daemons(&socket));
    assert_eq!(
        stdout(&output),
        format!(
            "no fold daemon was running; started {VERSION}, socket {}\n",
            socket.display()
        )
    );
    assert_eq!(guard.0.len(), 1, "one daemon on the socket");
    assert_eq!(i64::from(owner(&host).pid), i64::from(guard.0[0]));
    guard.stop().await;
}

/// The old daemon ends through its own shutdown, which exits 0 and removes its socket, and a new daemon from the installed binary serves the same socket.
#[tokio::test]
async fn restart_stops_the_running_daemon_cleanly_and_starts_a_new_one() {
    let host = Host::new().await;
    let socket = host.default_socket();
    let mut old = serve(&host, &socket).await;
    let old_pid = old.id().unwrap();
    assert_eq!(owner(&host).pid, old_pid);
    assert_eq!(owner(&host).version, VERSION);

    let output = restart(&host, None).await;
    let status = tokio::time::timeout(WAIT, old.wait())
        .await
        .expect("the old daemon exited")
        .unwrap();
    assert!(status.success(), "a SIGTERM shutdown exits 0: {status:?}");
    let guard = DaemonGuard(daemons(&socket));
    assert_eq!(
        stdout(&output),
        format!(
            "restarted the fold daemon: {VERSION} -> {VERSION}, socket {}\n",
            socket.display()
        )
    );
    assert_eq!(guard.0.len(), 1, "{:?}", guard.0);
    assert_ne!(guard.0[0], i32::try_from(old_pid).unwrap());
    assert_eq!(i64::from(owner(&host).pid), i64::from(guard.0[0]));
    tokio::net::UnixStream::connect(&socket).await.unwrap();
    guard.stop().await;
}

/// A `systemctl` stand-in whose `restart` starts the daemon the way the unit's `ExecStart` would.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn restart_with_an_installed_unit_stops_the_daemon_then_restarts_the_unit() {
    use std::os::unix::fs::PermissionsExt;

    let host = Host::new().await;
    let socket = host.default_socket();
    let unit = host.config_home().join("systemd/user/fold.service");
    std::fs::create_dir_all(unit.parent().unwrap()).unwrap();
    std::fs::write(&unit, "").unwrap();
    let bin = host.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = host.dir.path().join("systemctl.log");
    std::fs::write(
        bin.join("systemctl"),
        format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\n[ \"$2\" = restart ] && '{}' serve --socket '{}' </dev/null >/dev/null 2>&1 &\nexit 0\n",
            log.display(),
            env!("CARGO_BIN_EXE_fld"),
            socket.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(
        bin.join("systemctl"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let mut old = serve(&host, &socket).await;

    let output = restart(&host, Some(&path)).await;
    assert!(old.wait().await.unwrap().success());
    let guard = DaemonGuard(daemons(&socket));
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "--user cat fold.service\n--user restart fold.service\n"
    );
    assert_eq!(
        stdout(&output),
        format!(
            "restarted the fold daemon through fold.service: {VERSION} -> {VERSION}, socket {}\n",
            socket.display()
        )
    );
    assert_eq!(guard.0.len(), 1, "{:?}", guard.0);
    guard.stop().await;
}

/// The daemon writes its record right after it takes the lock and binds the socket later, so a SIGTERM sent once the record names the child arrives during the start. The start then finishes and the clean shutdown follows: exit 0, the socket removed and the lock free.
#[tokio::test]
async fn a_sigterm_during_the_start_ends_the_daemon_through_the_clean_shutdown() {
    let host = Host::new().await;
    let socket = host.default_socket();
    let mut command = host.fold(&["serve", "--socket", socket.to_str().unwrap()], &host.root);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().unwrap();
    let pid = child.id().unwrap();
    let deadline = std::time::Instant::now() + WAIT;
    while std::fs::read_to_string(lock(&host))
        .ok()
        .and_then(|text| LockOwner::parse(&text))
        .is_none_or(|owner| owner.pid != pid)
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the daemon recorded itself"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let answered = std::os::unix::net::UnixStream::connect(&socket).is_ok();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()).unwrap(),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    let status = tokio::time::timeout(WAIT, child.wait())
        .await
        .expect("the daemon exited")
        .unwrap();
    assert!(!answered, "the signal came before the socket answered");
    assert!(
        status.success(),
        "a SIGTERM during the start exits 0: {status:?}"
    );
    assert!(!socket.exists(), "the clean shutdown removed the socket");
    let free = std::fs::File::open(lock(&host)).unwrap();
    free.try_lock().unwrap();
}
