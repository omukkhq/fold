#![cfg(test)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use fold_store::testing::eventually;

use crate::common::Host;

const WAIT: Duration = Duration::from_secs(20);

fn script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A login shell in `dir` whose startup prints noise to both outputs, adds `bin` to `PATH`, exports a secret, and then runs the command it was given, as `sh` would.
fn fake_shell(dir: &Path, bin: &Path) -> PathBuf {
    let shell = dir.join("fake-shell");
    script(
        &shell,
        &format!(
            "[ \"$1 $2 $3\" = '-l -i -c' ] || exit 9\necho 'Last login: today'\necho 'motd' >&2\nexport PATH='{}':\"$PATH\" FOLD_TEST_SECRET=hunter2-secret\nexec /bin/sh -c \"$4\"",
            bin.display()
        ),
    );
    shell
}

#[tokio::test]
async fn the_shell_env_command_prints_the_environment_between_its_markers() {
    let host = Host::new().await;
    let output = host
        .fold(&["shell-env"], &host.root)
        .env("FOLD_TEST_VALUE", "a b=c")
        .stdin(Stdio::null())
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let out = output.stdout;
    assert!(out.starts_with(b"\0fold-shell-env-begin\0"));
    assert!(out.ends_with(b"\0fold-shell-env-end\0"));
    let entry: &[u8] = b"\0FOLD_TEST_VALUE=a b=c\0";
    assert!(
        out.windows(entry.len()).any(|window| window == entry),
        "{}",
        String::from_utf8_lossy(&out)
    );
}

/// A daemon reads the login shell's environment once at its start: it logs that workers take it, naming only the source, never a value.
#[tokio::test]
async fn a_daemon_takes_the_login_shells_environment_and_logs_no_value() {
    let host = Host::new().await;
    let bin = host.dir.path().join("shell-bin");
    script(&bin.join("claude"), "exit 0");
    let shell = fake_shell(host.dir.path(), &bin);
    let socket = host.dir.path().join("shell.sock");
    let log = host.dir.path().join("serve.log");
    let mut daemon = host
        .fold(&["serve", "--socket", socket.to_str().unwrap()], &host.root)
        .env("SHELL", &shell)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    eventually(WAIT, async || {
        tokio::net::UnixStream::connect(&socket)
            .await
            .map(drop)
            .map_err(|error| error.kind())
    })
    .await;
    daemon.start_kill().unwrap();
    daemon.wait().await.unwrap();
    let text = std::fs::read_to_string(&log).unwrap();
    let line = text
        .lines()
        .find(|line| line.contains("workers run with the login shell's environment"))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(line.contains(r#""source":"shell""#), "{line}");
    assert!(!text.contains("hunter2-secret"), "{text}");
    assert!(!text.contains(&bin.display().to_string()), "{text}");
    assert!(
        !text.contains("motd") && !text.contains("Last login"),
        "{text}"
    );
}
