#![cfg(test)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

use crate::common::{DaemonGuard, Host, adapter, daemons, list_agents};

/// A stand-in for the user's `claude` at the front of `PATH`: it prints its arguments, its working directory and the fold variables, one per line.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
for arg in "$@"; do printf 'arg %s\n' "$arg"; done
printf 'cwd %s\n' "$(pwd -P)"
printf 'FOLD_AGENT=%s\n' "$FOLD_AGENT"
printf 'FOLD_AGENT_TAGS=%s\n' "$FOLD_AGENT_TAGS"
"#;

/// `fld claude` with `args` in `cwd`, with the fake `claude` first on `PATH` and the human `ana`.
async fn fld_claude(host: &Host, args: &[&str], cwd: &Path) -> Output {
    let bin = host.dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let program = bin.join("claude");
    std::fs::write(&program, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut all = vec!["claude"];
    all.extend_from_slice(args);
    host.fold(&all, cwd)
        .env("PATH", path)
        .env("FOLD_HUMAN", "ana")
        .env_remove("FOLD_AGENT_TAGS")
        .output()
        .await
        .unwrap()
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[tokio::test]
async fn fld_claude_starts_the_daemon_and_runs_claude_as_the_main_agent_in_the_project_root() {
    let host = Host::new().await;
    let below = host.root.join("src");
    std::fs::create_dir_all(&below).unwrap();
    assert!(daemons(&host.default_socket()).is_empty());

    let output = fld_claude(&host, &["--", "--model", "opus"], &below).await;
    let guard = DaemonGuard(daemons(&host.default_socket()));
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        format!(
            "arg --name\narg main\narg --model\narg opus\ncwd {}\nFOLD_AGENT=main\nFOLD_AGENT_TAGS=main\n",
            host.root.display()
        )
    );
    assert_eq!(guard.0.len(), 1, "fld claude started the daemon");
    assert!(
        !host.root.join(".claude").exists(),
        "fld claude writes no Claude Code settings"
    );

    // The session's adapter inherits both variables and registers the main agent.
    let mut command = host.fold(&["mcp"], &host.root);
    command
        .env("FOLD_AGENT", "main")
        .env("FOLD_AGENT_TAGS", "main");
    let (client, mut child) = adapter(command).await;
    let agents = list_agents(&client).await;
    assert_eq!(agents["agents"][0]["name"], "main", "{agents}");
    assert_eq!(agents["agents"][0]["tags"], serde_json::json!(["main"]));
    client.cancel().await.unwrap();
    child.wait().await.unwrap();

    let again = fld_claude(&host, &[], &host.root).await;
    assert!(again.status.success(), "{}", text(&again.stderr));
    assert!(text(&again.stdout).starts_with("arg --name\narg main\ncwd "));

    let other = fld_claude(&host, &["--name", "lead"], &host.root).await;
    assert_eq!(other.status.code(), Some(1));
    assert!(other.stdout.is_empty(), "claude did not run");
    let stderr = text(&other.stderr);
    assert!(
        stderr.contains("main_tag_denied: agent main is the main agent of this project\nhint: run fld claude --name main to continue as it"),
        "{stderr}"
    );
    guard.stop().await;
}

#[tokio::test]
async fn fld_claude_outside_a_project_fails_with_a_hint_and_runs_nothing() {
    let host = Host::new().await;
    let outside = host.dir.path().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();

    let output = fld_claude(&host, &[], &outside).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "claude did not run");
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("project_not_found: no registered project holds this directory\nhint: run fld claude inside a fold project, or run fld init there first"),
        "{stderr}"
    );
    assert!(daemons(&host.default_socket()).is_empty());

    let bad = fld_claude(&host, &["--name", "Not A Name"], &host.root).await;
    assert_eq!(bad.status.code(), Some(1));
    assert!(
        text(&bad.stderr).contains("invalid_name"),
        "{}",
        text(&bad.stderr)
    );
    assert!(daemons(&host.default_socket()).is_empty());
}
