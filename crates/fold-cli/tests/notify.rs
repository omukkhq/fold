#![cfg(test)]

use std::os::unix::fs::PermissionsExt;
use std::process::Output;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::Command;

use crate::common::{DaemonGuard, Host, daemons, feed};

/// A receiver that answers every post with 200 and counts them.
async fn receiver() -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hooks/cli-token", listener.local_addr().unwrap());
    let (sender, posts) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let sender = sender.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                let mut length = 0;
                loop {
                    line.clear();
                    reader.read_line(&mut line).await.unwrap();
                    if line.trim_end().is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).await.unwrap();
                let _ = sender.send(String::from_utf8(body).unwrap());
                reader
                    .get_mut()
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .await
                    .unwrap();
            });
        }
    });
    (url, posts)
}

fn command(host: &Host, args: &[&str]) -> Command {
    let mut command = host.fold(args, &host.root);
    command
        .env("FOLD_HUMAN", "ana")
        .env_remove("CLAUDE_CODE_MESSAGING_SOCKET")
        .env_remove("CLAUDE_CODE_MESSAGING_TOKEN")
        .env_remove("CLAUDE_CODE_SESSION_ID");
    command
}

async fn run(host: &Host, args: &[&str]) -> Output {
    command(host, args).output().await.unwrap()
}

async fn ok(host: &Host, args: &[&str]) -> String {
    let output = run(host, args).await;
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test]
async fn fld_notify_adds_lists_tests_logs_and_removes_targets_and_keeps_secrets_private() {
    let host = Host::new().await;
    let empty = ok(&host, &["notify"]).await;
    let guard = DaemonGuard(daemons(&host.default_socket()));
    assert!(empty.starts_with("No notice targets\n"), "{empty}");
    assert!(empty.contains("Host targets: on"), "{empty}");
    assert!(empty.contains("Notices carry no links"), "{empty}");

    let (url, mut posts) = receiver().await;
    let mut piped = command(
        &host,
        &[
            "notify",
            "add",
            "slack",
            "Team Slack",
            "--events",
            "recommended",
            "--moves-to",
            "review,done",
            "--window",
            "0",
        ],
    )
    .spawn()
    .unwrap();
    feed(&mut piped, &format!("{url}\n")).await;
    let piped = piped.wait_with_output().await.unwrap();
    assert!(piped.status.success(), "{piped:?}");
    let added = String::from_utf8(piped.stdout).unwrap();
    assert!(
        added.starts_with("Added nt-1\nnt-1 Team Slack · Slack · on\n"),
        "{added}"
    );
    assert!(
        !added.contains("cli-token") && !added.contains("secret"),
        "{added}"
    );

    let generic = ok(
        &host,
        &[
            "notify",
            "add",
            "webhook",
            "Release bot",
            "https://ci.example.org/fold-hook",
            "--host",
            "--events",
            "all",
        ],
    )
    .await;
    assert!(generic.starts_with("Added nt-2\n"), "{generic}");
    let secret = generic
        .lines()
        .find_map(|line| line.strip_prefix("  ↳ signing secret: "))
        .unwrap()
        .to_owned();
    assert!(secret.starts_with("whsec_"), "{generic}");

    let refused = run(
        &host,
        &[
            "notify",
            "add",
            "webhook",
            "Plain",
            "http://hooks.example.com/x",
        ],
    )
    .await;
    assert!(!refused.status.success());
    let error = String::from_utf8(refused.stderr).unwrap();
    assert!(
        error.contains("plaintext_notify_target") && !error.contains("hooks.example.com/x"),
        "{error}"
    );

    let web = ok(&host, &["notify", "web-url", "https://fold.example.org"]).await;
    assert_eq!(web, "Links start with https://fold.example.org\n");

    let listed = ok(&host, &["notify", "list"]).await;
    assert!(
        listed.contains("nt-1 Team Slack · Slack · on\n  ↳ http://127.0.0.1:"),
        "{listed}"
    );
    assert!(
        listed.contains(
            "nt-2 Release bot · Generic JSON · on\n  ↳ https://ci.example.org/… · All events"
        ),
        "{listed}"
    );
    assert!(
        listed.contains("  ↳ card moves to Review, Done · each notice alone"),
        "{listed}"
    );
    assert!(
        !listed.contains(&secret) && !listed.contains("cli-token"),
        "{listed}"
    );

    let tested = ok(&host, &["notify", "test", "Team Slack"]).await;
    assert_eq!(tested, "nt-1 took the test notice\n");
    let body = posts.recv().await.unwrap();
    assert!(body.contains("Test notice for Team Slack"), "{body}");
    let log = ok(&host, &["notify", "log", "nt-1"]).await;
    assert!(log.starts_with("ns-1 delivered · 200 · try 1"), "{log}");
    assert!(log.contains("webhook.test nt-1"), "{log}");

    let edited = ok(
        &host,
        &[
            "notify",
            "edit",
            "nt-1",
            "--name",
            "Design channel",
            "--switch",
            "off",
        ],
    )
    .await;
    assert!(
        edited.starts_with("nt-1 Design channel · Slack · off\n"),
        "{edited}"
    );
    let rotated = ok(&host, &["notify", "rotate", "Release bot"]).await;
    assert!(rotated.starts_with("New secret for nt-2\n"), "{rotated}");
    assert!(!rotated.contains(&secret), "{rotated}");
    let slack_rotate = run(&host, &["notify", "rotate", "nt-1"]).await;
    assert!(
        String::from_utf8(slack_rotate.stderr)
            .unwrap()
            .contains("invalid_notify_target")
    );

    let off = ok(&host, &["notify", "host-targets", "off"]).await;
    assert_eq!(
        off,
        "Host targets: off, only the project's own targets get its notices\n"
    );
    assert!(
        ok(&host, &["notify", "host-targets"])
            .await
            .starts_with("Host targets: off")
    );
    assert!(
        ok(&host, &["notify", "host-targets", "on"])
            .await
            .starts_with("Host targets: on")
    );

    assert_eq!(
        ok(&host, &["notify", "remove", "Design channel"]).await,
        "Removed nt-1\n"
    );
    assert_eq!(
        ok(&host, &["notify", "remove", "nt-2"]).await,
        "Removed nt-2\n"
    );
    let gone = run(&host, &["notify", "remove", "nt-1"]).await;
    assert!(
        String::from_utf8(gone.stderr)
            .unwrap()
            .contains("notify_target_not_found")
    );

    let dir = host.data_home().join("fold/notices");
    let mode =
        |path: std::path::PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(dir.join("host/host.json")), 0o600);
    assert_eq!(mode(dir.join("host")), 0o700);
    assert_eq!(mode(dir.clone()), 0o700);
    assert_eq!(mode(dir.join("projects/app.json")), 0o600);
    guard.stop().await;
}
