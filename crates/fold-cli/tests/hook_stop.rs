use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Arc;

use fold_core::{Address, AgentName, ManualClock, MessageBody};
use fold_daemon::projects::{ProjectHandle, Slot};
use fold_daemon::services::messages::{MessageService, SendMessage};
use fold_daemon::{Daemon, DaemonConfig, RunningDaemon};
use serde_json::json;

use crate::common::{Host, daemons, feed};

/// A daemon of the host on a socket of its own, with agents `frontend` and `backend` in project `app`.
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
    for name in ["frontend", "backend"] {
        let mut ctx = agents.start_session(1).await.unwrap();
        agents
            .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
            .await
            .unwrap();
    }
    (daemon, socket, handle)
}

#[cfg(test)]
async fn send_to_frontend(daemon: &RunningDaemon, handle: &ProjectHandle) {
    let messages = MessageService::new(
        handle.store.clone(),
        daemon.state().registry.clone(),
        handle.bus.clone(),
        handle.clock.clone(),
    );
    let backend = fold_core::WorkerName::parse("backend").unwrap();
    let actor = fold_core::Actor::Agent(fold_core::AgentActor {
        agent: backend.agent(),
        worker: backend,
        tags: Vec::new(),
    });
    messages
        .send(
            &actor,
            SendMessage {
                to: Address::parse_all(&["frontend"]).unwrap(),
                body: MessageBody::parse("text").unwrap(),
                task: None,
                proposal: None,
                reply_to: None,
            },
        )
        .await
        .unwrap();
}

/// `fld hook stop` with `stdin`, run in the project root; `socket` is passed as `--socket` when given.
#[cfg(test)]
async fn hook(host: &Host, socket: Option<&Path>, agent: Option<&str>, stdin: &str) -> Output {
    let mut args = vec!["hook", "stop"];
    if let Some(socket) = socket {
        args.extend(["--socket", socket.to_str().unwrap()]);
    }
    let mut command = host.fold(&args, &host.root);
    if let Some(agent) = agent {
        command.env("FOLD_AGENT", agent);
    }
    let mut child = command.spawn().unwrap();
    feed(&mut child, stdin).await;
    child.wait_with_output().await.unwrap()
}

#[cfg(test)]
fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[cfg(test)]
fn stop_input(cwd: &Path, active: bool) -> String {
    json!({"session_id": "s", "hook_event_name": "Stop", "stop_hook_active": active, "cwd": cwd})
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn unread_messages_keep_the_turn_going_once() {
    let host = Host::new().await;
    let (daemon, socket, handle) = start(&host).await;
    let input = stop_input(&host.root, false);
    let output = hook(&host, Some(&socket), Some("frontend"), &input).await;
    assert_eq!(output.status.code(), Some(0));
    assert!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "no unread messages, no output"
    );

    send_to_frontend(&daemon, &handle).await;
    send_to_frontend(&daemon, &handle).await;
    let output = hook(&host, Some(&socket), Some("frontend"), &input).await;
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(text(&output.stderr), "");
    assert_eq!(
        text(&output.stdout),
        "{\"decision\":\"block\",\"reason\":\"2 unread fold messages. Call get_inbox.\"}\n"
    );

    let continued = stop_input(&host.root, true);
    let output = hook(&host, Some(&socket), Some("frontend"), &continued).await;
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty() && output.stderr.is_empty());

    let output = hook(&host, Some(&socket), None, &input).await;
    assert_eq!(output.status.code(), Some(0));
    assert!(
        output.stdout.is_empty() && output.stderr.is_empty(),
        "without FOLD_AGENT the hook does nothing"
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_input_and_unknown_agents_exit_0_with_one_line() {
    let host = Host::new().await;
    let (daemon, socket, _handle) = start(&host).await;
    for (agent, stdin, code) in [
        ("frontend", "not json".to_owned(), "invalid_request"),
        (
            "frontend",
            "{\"stop_hook_active\": false}".to_owned(),
            "invalid_request",
        ),
        (
            "ghost",
            stop_input(&host.root, false),
            "agent_not_registered",
        ),
    ] {
        let output = hook(&host, Some(&socket), Some(agent), &stdin).await;
        assert_eq!(output.status.code(), Some(0), "{stdin}");
        assert!(output.stdout.is_empty(), "{stdin}");
        assert_eq!(text(&output.stderr), format!("fld hook stop: {code}\n"));
    }
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_running_daemon_the_hook_starts_none() {
    let host = Host::new().await;
    let output = hook(
        &host,
        None,
        Some("frontend"),
        &stop_input(&host.root, false),
    )
    .await;
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert_eq!(text(&output.stderr), "fld hook stop: daemon_unreachable\n");
    assert!(!host.default_socket().exists());
    assert!(daemons(&host.default_socket()).is_empty());
}
