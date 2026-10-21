#![cfg(test)]

use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;

use crate::common::{DaemonGuard, Host, daemons};

#[tokio::test]
async fn fld_notify_adds_lists_tests_and_removes_targets_and_keeps_secrets_private() {
    let host = Host::new().await;
    let root = host.dir.path().join("site");
    std::fs::create_dir_all(&root).unwrap();
    let init = host
        .fold(&["init"], &root)
        .env("FOLD_HUMAN", "ana")
        .output()
        .await
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    let run = |args: &'static [&'static str]| {
        let mut command = host.fold(args, &root);
        command.env("FOLD_HUMAN", "ana");
        command
    };

    let empty = run(&["notify"]).output().await.unwrap();
    let guard = DaemonGuard(daemons(&host.default_socket()));
    assert!(empty.status.success(), "{empty:?}");
    assert_eq!(
        String::from_utf8(empty.stdout).unwrap(),
        "no targets: add one with fld notify add webhook <url>, or fld notify add email\n"
    );

    let added = run(&[
        "notify",
        "add",
        "webhook",
        "http://127.0.0.1:9/hooks/tok",
        "--interval",
        "5",
    ])
    .output()
    .await
    .unwrap();
    assert!(added.status.success(), "{added:?}");
    let text = String::from_utf8(added.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        "added nt-1 webhook http://127.0.0.1:9/… every 5 minutes"
    );
    let secret = lines[1].strip_prefix("signing secret: ").unwrap();
    assert_eq!(secret.len(), 48);

    let mut piped = run(&[
        "notify",
        "add",
        "webhook",
        "https://hooks.example.com/x",
        "--host",
        "--secret-stdin",
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    piped
        .stdin
        .take()
        .unwrap()
        .write_all(b"given-secret-0123456789\n")
        .await
        .unwrap();
    let piped = piped.wait_with_output().await.unwrap();
    assert!(piped.status.success(), "{piped:?}");
    assert_eq!(
        String::from_utf8(piped.stdout).unwrap(),
        "added nt-2 webhook https://hooks.example.com/… every 10 minutes\n"
    );

    let listed = run(&["notify", "list"]).output().await.unwrap();
    assert_eq!(
        String::from_utf8(listed.stdout).unwrap(),
        "nt-1 webhook http://127.0.0.1:9/… every 5 minutes\n"
    );
    let listed = run(&["notify", "list", "--host"]).output().await.unwrap();
    assert_eq!(
        String::from_utf8(listed.stdout).unwrap(),
        "nt-2 webhook https://hooks.example.com/… every 10 minutes\n"
    );

    let web = run(&["notify", "web-url", "https://fold.example.com"])
        .output()
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8(web.stdout).unwrap(),
        "links start with https://fold.example.com\n"
    );

    let test = run(&["notify", "test", "nt-1"]).output().await.unwrap();
    assert!(!test.status.success());
    let error = String::from_utf8(test.stderr).unwrap();
    assert!(error.contains("notify_send_failed"), "{error}");
    assert!(!error.contains("tok"), "{error}");

    let removed = run(&["notify", "remove", "nt-1"]).output().await.unwrap();
    assert_eq!(String::from_utf8(removed.stdout).unwrap(), "removed nt-1\n");
    let listed = run(&["notify"]).output().await.unwrap();
    assert_eq!(
        String::from_utf8(listed.stdout).unwrap(),
        "the project has no targets of its own and uses the host's:\nnt-2 webhook https://hooks.example.com/… every 10 minutes\n"
    );
    let gone = run(&["notify", "remove", "nt-1"]).output().await.unwrap();
    assert!(
        String::from_utf8(gone.stderr)
            .unwrap()
            .contains("notify_target_not_found")
    );

    let dir = host.data_home().join("fold/notify");
    let mode =
        |path: std::path::PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(dir.join("host/host.json")), 0o600);
    assert_eq!(mode(dir.join("host")), 0o700);
    assert_eq!(mode(dir.clone()), 0o700);
    let kept = std::fs::read_to_string(dir.join("host/host.json")).unwrap();
    assert!(kept.contains("given-secret-0123456789"));
    guard.stop().await;
}
