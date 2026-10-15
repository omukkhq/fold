mod common;

use std::path::PathBuf;
use std::sync::Arc;

use fold_core::{AgentName, EventFilter, ManualClock};
use fold_daemon::projects::Slot;
use fold_daemon::services::agents::AgentService;
use fold_daemon::{Daemon, DaemonConfig, RunningDaemon};
use fold_store::events;
use serde_json::json;

use common::{DaemonGuard, Host, adapter, daemons, list_agents};

#[cfg(test)]
async fn start(host: &Host) -> (RunningDaemon, PathBuf) {
    let socket = host.dir.path().join("elsewhere").join("test.sock");
    let daemon = Daemon::start(
        DaemonConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            socket: socket.clone(),
            data_dir: host.data_home().join("fold"),
            runtime_dir: host.dir.path().join("elsewhere"),
            web_dir: None,
            session_lifetime: fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME,
        },
        Arc::new(ManualClock::new()),
    )
    .await
    .unwrap();
    (daemon, socket)
}

#[cfg(test)]
fn agents(daemon: &RunningDaemon) -> (AgentService, fold_store::Store) {
    let Some(Slot::Open(handle)) = daemon.state().projects.slot("app") else {
        panic!("the project app is not open");
    };
    let service = AgentService::new(
        handle.store.clone(),
        handle.bus.clone(),
        handle.clock.clone(),
    );
    (service, handle.store.clone())
}

#[tokio::test]
async fn fold_agent_tag_grants_integrator_as_the_human_from_fold_human() {
    let host = Host::new().await;
    let (daemon, socket) = start(&host).await;
    let (service, store) = agents(&daemon);
    let mut ctx = service.start_session(1).await.unwrap();
    service
        .register_agent(&mut ctx, &AgentName::parse("review").unwrap(), &[])
        .await
        .unwrap();
    let socket_flag = socket.to_string_lossy().into_owned();

    let output = host
        .fold(
            &[
                "agent",
                "tag",
                "review",
                "integrator",
                "--socket",
                &socket_flag,
            ],
            &host.root,
        )
        .env("FOLD_HUMAN", "tanim")
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout} {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(stdout, "integrator granted to review; tags: integrator\n");
    // Agents see the grant in the directory.
    let (mcp, mut child) = adapter(host.fold(&["mcp", "--socket", &socket_flag], &host.root)).await;
    assert_eq!(
        list_agents(&mcp).await["agents"][0]["tags"],
        json!(["integrator"])
    );
    mcp.cancel().await.unwrap();
    assert!(child.wait().await.unwrap().success());

    let subdir = host.root.join("src");
    std::fs::create_dir_all(&subdir).unwrap();
    let output = host
        .fold(
            &[
                "agent",
                "untag",
                "review",
                "integrator",
                "--socket",
                &socket_flag,
            ],
            &subdir,
        )
        .env("FOLD_HUMAN", "tanim")
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "integrator removed from review; tags: none\n"
    );

    let tag_events: Vec<_> = events::read_after(store.read(), 0, &EventFilter::default(), 100)
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type.as_str().starts_with("agent.tag"))
        .map(|event| {
            (
                event.event_type.to_string(),
                event.actor.to_string(),
                event.data,
            )
        })
        .collect();
    assert_eq!(
        tag_events,
        vec![
            (
                "agent.tag_added".to_owned(),
                "human:tanim".to_owned(),
                json!({"agent": "review", "tag": "integrator"})
            ),
            (
                "agent.tag_removed".to_owned(),
                "human:tanim".to_owned(),
                json!({"agent": "review", "tag": "integrator"})
            ),
        ]
    );
    let started = daemons(&host.default_socket());
    assert!(started.is_empty(), "the command started a second daemon");
    DaemonGuard(started).stop().await;
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_refusal_from_the_daemon_exits_with_its_code() {
    let host = Host::new().await;
    let (daemon, socket) = start(&host).await;
    let socket_flag = socket.to_string_lossy().into_owned();
    let output = host
        .fold(
            &[
                "agent",
                "tag",
                "ghost",
                "integrator",
                "--socket",
                &socket_flag,
            ],
            &host.root,
        )
        .env("FOLD_HUMAN", "tanim")
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("agent_not_found"), "{stderr}");

    let outside = host.dir.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let output = host
        .fold(
            &[
                "agent",
                "tag",
                "review",
                "integrator",
                "--socket",
                &socket_flag,
            ],
            &outside,
        )
        .env("FOLD_HUMAN", "tanim")
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("project_not_found"), "{stderr}");

    let output = host
        .fold(
            &[
                "agent",
                "tag",
                "Review",
                "integrator",
                "--socket",
                &socket_flag,
            ],
            &host.root,
        )
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid_name"));
    daemon.shutdown().await;
}

#[tokio::test]
async fn any_tag_other_than_integrator_is_a_usage_error() {
    let host = Host::new().await;
    for tag in ["ui", "review"] {
        let output = host
            .fold(&["agent", "tag", "review", tag], &host.root)
            .output()
            .await
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{tag}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("invalid value"), "{stderr}");
    }
    assert!(daemons(&host.default_socket()).is_empty());
}
