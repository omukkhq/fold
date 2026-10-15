#[expect(
    dead_code,
    reason = "this file uses only part of the shared host helpers"
)]
mod common;

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

use fold_core::{Actor, AgentName, HumanName, ManualClock, TaskDescription, TaskTitle};
use fold_daemon::projects::{ProjectHandle, Slot};
use fold_daemon::services::tasks::CreateTask;
use fold_daemon::{Daemon, DaemonConfig, RunningDaemon};
use serde_json::json;
use tokio::io::AsyncWriteExt;

use common::Host;

const MESSAGING: [&str; 2] = [
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
];

/// A daemon of the host on a socket of its own, with agent `frontend` and task `T-1` in project `app`.
#[cfg(test)]
async fn start(host: &Host) -> (RunningDaemon, PathBuf, Arc<ProjectHandle>) {
    let dir = host.dir.path().join("elsewhere");
    let socket = dir.join("test.sock");
    let daemon = Daemon::start(
        DaemonConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            socket: socket.clone(),
            data_dir: host.data_home().join("fold"),
            runtime_dir: dir,
            web_dir: None,
            session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
        },
        Arc::new(ManualClock::new()),
    )
    .await
    .unwrap();
    let Some(Slot::Open(handle)) = daemon.state().projects.slot("app") else {
        panic!("the project app is not open");
    };
    let agents = handle.agents();
    let mut ctx = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut ctx, &AgentName::parse("frontend").unwrap(), &[])
        .await
        .unwrap();
    let ana = HumanName::parse("ana").unwrap();
    let now = handle.clock.now();
    daemon
        .state()
        .registry
        .write(async |tx| fold_store::projects::insert_human(tx, &ana, now).await)
        .await
        .unwrap();
    handle
        .tasks
        .create_task(
            &Actor::human(ana),
            CreateTask {
                title: TaskTitle::parse("Build the login form").unwrap(),
                description: TaskDescription::new(""),
                labels: Vec::new(),
            },
        )
        .await
        .unwrap();
    (daemon, socket, handle)
}

/// `fold hook session-start` with `stdin`, run in `cwd`, with none of the session variables but those in `env`.
#[cfg(test)]
async fn hook(host: &Host, socket: &Path, cwd: &Path, env: &[(&str, &str)], stdin: &str) -> Output {
    let socket = socket.to_str().unwrap();
    let mut command = host.fold(&["hook", "session-start", "--socket", socket], cwd);
    for name in MESSAGING {
        command.env_remove(name);
    }
    command.envs(env.iter().copied());
    let mut child = command.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(stdin.as_bytes()).await.unwrap();
    drop(input);
    child.wait_with_output().await.unwrap()
}

#[cfg(test)]
fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[cfg(test)]
fn stdin_for(cwd: &Path) -> String {
    json!({"session_id": "s", "hook_event_name": "SessionStart", "source": "startup", "cwd": cwd})
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_hook_prints_the_session_context_as_additional_context() {
    let host = Host::new().await;
    let (daemon, socket, _handle) = start(&host).await;
    let workspace = host.root.join("ws").join("t-1");
    std::fs::create_dir_all(&workspace).unwrap();
    let output = hook(
        &host,
        &socket,
        &host.root,
        &[("FOLD_AGENT", "frontend")],
        &stdin_for(&workspace),
    )
    .await;
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(text(&output.stderr), "");
    let context = "fold project app.\nYou are agent frontend, main worker frontend.\nUnread messages: 0.\nClaims: none.\nReady tasks:\n- T-1 Build the login form";
    assert_eq!(
        text(&output.stdout),
        format!(
            "{{\"hookSpecificOutput\":{{\"hookEventName\":\"SessionStart\",\"additionalContext\":{}}}}}\n",
            json!(context)
        )
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_inbox_socket_and_token_reach_the_daemon_only_when_both_are_set() {
    let host = Host::new().await;
    let (daemon, socket, handle) = start(&host).await;
    let frontend = AgentName::parse("frontend").unwrap();
    let stdin = stdin_for(&host.root);
    let inbox = host.dir.path().join("inbox.sock");
    let inbox = inbox.to_str().unwrap();
    for partial in [
        vec![("CLAUDE_CODE_MESSAGING_SOCKET", inbox)],
        vec![("CLAUDE_CODE_MESSAGING_TOKEN", "t0k")],
        vec![
            ("CLAUDE_CODE_MESSAGING_SOCKET", inbox),
            ("CLAUDE_CODE_MESSAGING_TOKEN", ""),
        ],
    ] {
        let mut env = partial.clone();
        env.push(("FOLD_AGENT", "frontend"));
        let output = hook(&host, &socket, &host.root, &env, &stdin).await;
        assert_eq!(output.status.code(), Some(0));
        assert!(!output.stdout.is_empty(), "{partial:?}");
        assert!(handle.push.sockets(&frontend).is_empty(), "{partial:?}");
    }
    let env = [
        ("FOLD_AGENT", "frontend"),
        ("CLAUDE_CODE_MESSAGING_SOCKET", inbox),
        ("CLAUDE_CODE_MESSAGING_TOKEN", "t0k"),
    ];
    let output = hook(&host, &socket, &host.root, &env, &stdin).await;
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(handle.push.sockets(&frontend), [PathBuf::from(inbox)]);
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn without_fold_agent_the_hook_prints_nothing() {
    let host = Host::new().await;
    let (daemon, socket, _handle) = start(&host).await;
    let output = hook(&host, &socket, &host.root, &[], &stdin_for(&host.root)).await;
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn failures_exit_0_with_one_line_naming_the_code() {
    let host = Host::new().await;
    let (daemon, socket, _handle) = start(&host).await;
    let agent = [("FOLD_AGENT", "frontend")];
    let outside = host.dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let cases = [
        ("{\"cwd\": ".to_owned(), "invalid_request"),
        ("[1, 2]".to_owned(), "invalid_request"),
        ("{\"session_id\": \"s\"}".to_owned(), "invalid_request"),
        (
            stdin_for(&host.dir.path().join("missing")),
            "invalid_request",
        ),
        (stdin_for(&outside), "project_not_found"),
    ];
    for (stdin, code) in cases {
        let output = hook(&host, &socket, &host.root, &agent, &stdin).await;
        assert_eq!(output.status.code(), Some(0), "{stdin}");
        assert!(output.stdout.is_empty(), "{stdin}");
        assert_eq!(
            text(&output.stderr),
            format!("fold hook session-start: {code}\n"),
            "{stdin}"
        );
    }
    let bad_name = [("FOLD_AGENT", "Not A Name")];
    let output = hook(
        &host,
        &socket,
        &host.root,
        &bad_name,
        &stdin_for(&host.root),
    )
    .await;
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        text(&output.stderr),
        "fold hook session-start: invalid_name\n"
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_daemon_exits_0_with_one_line() {
    let host = Host::new().await;
    let stdin = stdin_for(&host.root);
    let agent = [("FOLD_AGENT", "frontend")];

    // Without a runtime directory the hook cannot start a daemon.
    let absent = host.dir.path().join("absent.sock");
    let mut command = host.fold(
        &[
            "hook",
            "session-start",
            "--socket",
            absent.to_str().unwrap(),
        ],
        &host.root,
    );
    command.env_remove("XDG_RUNTIME_DIR").envs(agent);
    for name in MESSAGING {
        command.env_remove(name);
    }
    let mut child = command.spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(stdin.as_bytes()).await.unwrap();
    drop(input);
    let output = child.wait_with_output().await.unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert_eq!(
        text(&output.stderr),
        "fold hook session-start: runtime_dir_missing\n"
    );
    assert!(!absent.exists());

    // A socket that accepts and never answers runs into the answer timeout.
    let silent = host.dir.path().join("silent.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&silent).unwrap();
    let output = hook(&host, &silent, &host.root, &agent, &stdin).await;
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert_eq!(
        text(&output.stderr),
        "fold hook session-start: daemon_unreachable\n"
    );
}
