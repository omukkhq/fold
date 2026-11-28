use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::mcp_handshake::{
    Host, INBOX_TOKEN, WAIT, handshake, list_agents, mcp, message_main, next_notice, refused,
    session_inbox, session_start_hook,
};
/// The hello of the interactive main session `fld claude` starts, with its Claude Code inbox at `socket`.
#[cfg(test)]
fn main_hello(host: &Host, socket: &Path) -> Value {
    let mut hello = host.hello(&host.root, Some("main"));
    hello["tags"] = json!(["main"]);
    hello["inbox"] = json!({"socket": socket, "token": INBOX_TOKEN});
    hello
}

#[cfg(test)]
fn open_handle(host: &Host) -> Arc<fold_daemon::projects::ProjectHandle> {
    match host.state().projects.slot("app") {
        Some(fold_daemon::projects::Slot::Open(handle)) => handle,
        other => panic!("project app is not open: {other:?}"),
    }
}

#[cfg(test)]
fn main_name() -> fold_core::AgentName {
    fold_core::AgentName::parse("main").unwrap()
}

#[tokio::test]
async fn acceptance_58_a_new_main_session_takes_over_the_old_one_by_default() {
    let host = Host::start().await;
    let old_socket = host.root.parent().unwrap().join("old-inbox.sock");
    let new_socket = host.root.parent().unwrap().join("new-inbox.sock");
    let mut old_inbox = session_inbox(&old_socket);
    let mut new_inbox = session_inbox(&new_socket);
    let (accepted, rest) = handshake(host.state(), &main_hello(&host, &old_socket)).await;
    assert!(accepted.ok, "{accepted:?}");
    let old = mcp(rest).await;
    list_agents(&old).await;

    host.clock.advance(Duration::from_secs(1)).unwrap();
    let (accepted, rest) = handshake(host.state(), &main_hello(&host, &new_socket)).await;
    assert!(accepted.ok, "{accepted:?}");
    let new = mcp(rest).await;

    let farewell = next_notice(&mut old_inbox).await;
    assert!(
        farewell.contains("a new `fld claude` took over this main session"),
        "{farewell}"
    );
    tokio::time::timeout(WAIT, old.waiting())
        .await
        .expect("fold closes the old connection")
        .unwrap();
    let handle = open_handle(&host);
    assert_eq!(
        handle.push.sockets(&main_name()),
        std::slice::from_ref(&new_socket)
    );
    assert_eq!(handle.main_sessions.count(&main_name()), 1);

    // `fld mcp` reconnects with the same line when fold closes its connection; fold refuses it, so it never takes the new session over in turn.
    let line = format!("{}\n", main_hello(&host, &old_socket));
    let (code, hint) = refused(host.state(), line.as_bytes()).await;
    assert_eq!(code, "main_session_taken_over");
    assert!(hint.unwrap().contains("--keep-others"));
    assert_eq!(handle.main_sessions.count(&main_name()), 1);
    assert_eq!(
        handle.push.sockets(&main_name()),
        std::slice::from_ref(&new_socket)
    );

    message_main(&host).await;
    assert!(next_notice(&mut new_inbox).await.contains("m-1 arrived"));
    assert!(
        old_inbox.try_recv().is_err(),
        "the old session gets no more notices"
    );
    assert_eq!(
        list_agents(&new).await["agents"][0]["presence"],
        json!("active")
    );
    new.cancel().await.unwrap();
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn acceptance_58_with_keep_others_both_main_sessions_stay_connected_and_get_notices() {
    let host = Host::start().await;
    let first_socket = host.root.parent().unwrap().join("first-inbox.sock");
    let second_socket = host.root.parent().unwrap().join("second-inbox.sock");
    let mut first_inbox = session_inbox(&first_socket);
    let mut second_inbox = session_inbox(&second_socket);
    let (accepted, rest) = handshake(host.state(), &main_hello(&host, &first_socket)).await;
    assert!(accepted.ok, "{accepted:?}");
    let first = mcp(rest).await;

    host.clock.advance(Duration::from_secs(1)).unwrap();
    let mut hello = main_hello(&host, &second_socket);
    hello["keep_others"] = json!(true);
    let (accepted, rest) = handshake(host.state(), &hello).await;
    assert!(accepted.ok, "{accepted:?}");
    let second = mcp(rest).await;

    let handle = open_handle(&host);
    assert_eq!(handle.main_sessions.count(&main_name()), 2);
    assert_eq!(
        handle.push.sockets(&main_name()),
        [first_socket.clone(), second_socket.clone()]
    );
    // A notice that carries a message posts it in one session only, so the two sessions never both answer it; every other notice reaches both.
    let notice = fold_core::agents::push::Notice {
        agent: main_name(),
        text: "fold: task T-1 is ready.".to_owned(),
        next_tool: "get_task",
        needs_no_action: false,
    };
    handle.push.deliver(&[notice], 1);
    assert!(next_notice(&mut first_inbox).await.contains("T-1 is ready"));
    assert!(
        next_notice(&mut second_inbox)
            .await
            .contains("T-1 is ready")
    );
    for client in [&first, &second] {
        assert_eq!(
            list_agents(client).await["agents"][0]["name"],
            json!("main")
        );
    }
    first.cancel().await.unwrap();
    second.cancel().await.unwrap();
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn acceptance_58_a_dead_main_connection_never_blocks_a_new_handshake() {
    let host = Host::start().await;
    let old_socket = host.root.parent().unwrap().join("old-inbox.sock");
    let new_socket = host.root.parent().unwrap().join("new-inbox.sock");
    let mut new_inbox = session_inbox(&new_socket);
    let (accepted, rest) = handshake(host.state(), &main_hello(&host, &old_socket)).await;
    assert!(accepted.ok, "{accepted:?}");
    let old = mcp(rest).await;
    // The old process dies: its connection and its inbox are gone at once.
    drop(old);

    host.clock.advance(Duration::from_secs(1)).unwrap();
    let (accepted, rest) = handshake(host.state(), &main_hello(&host, &new_socket)).await;
    assert!(accepted.ok, "{accepted:?}");
    let new = mcp(rest).await;
    message_main(&host).await;
    assert!(next_notice(&mut new_inbox).await.contains("m-1 arrived"));
    let handle = open_handle(&host);
    eventually(WAIT, async || {
        match handle.main_sessions.count(&main_name()) {
            1 => Ok(()),
            live => Err(live),
        }
    })
    .await;
    new.cancel().await.unwrap();
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn acceptance_58_the_same_adapter_that_reconnects_early_never_takes_itself_over() {
    let host = Host::start().await;
    let socket = host.root.parent().unwrap().join("inbox.sock");
    let mut inbox = session_inbox(&socket);
    let hello = main_hello(&host, &socket);
    let (accepted, rest) = handshake(host.state(), &hello).await;
    assert!(accepted.ok, "{accepted:?}");
    let first = mcp(rest).await;

    // `fld mcp` reconnects with the same line before fold has ended the session of its earlier connection.
    host.clock.advance(Duration::from_secs(1)).unwrap();
    let (accepted, rest) = handshake(host.state(), &hello).await;
    assert!(accepted.ok, "{accepted:?}");
    let second = mcp(rest).await;
    tokio::time::timeout(WAIT, first.waiting())
        .await
        .expect("fold closes the earlier connection")
        .unwrap();

    host.clock.advance(Duration::from_secs(1)).unwrap();
    let (accepted, rest) = handshake(host.state(), &hello).await;
    assert!(
        accepted.ok,
        "a later reconnect of the same adapter is accepted: {accepted:?}"
    );
    let third = mcp(rest).await;
    tokio::time::timeout(WAIT, second.waiting())
        .await
        .expect("fold closes the earlier connection")
        .unwrap();
    let handle = open_handle(&host);
    assert_eq!(handle.main_sessions.count(&main_name()), 1);
    message_main(&host).await;
    assert!(next_notice(&mut inbox).await.contains("m-1 arrived"));
    third.cancel().await.unwrap();
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn acceptance_58_a_replayed_handshake_after_a_restart_never_takes_over_a_newer_session() {
    let host = Host::start().await;
    let a_socket = host.root.parent().unwrap().join("a-inbox.sock");
    let b_socket = host.root.parent().unwrap().join("b-inbox.sock");
    let c_socket = host.root.parent().unwrap().join("c-inbox.sock");
    let mut a_inbox = session_inbox(&a_socket);
    let mut b_inbox = session_inbox(&b_socket);
    let mut c_inbox = session_inbox(&c_socket);
    let (accepted, rest) = handshake(host.state(), &main_hello(&host, &a_socket)).await;
    assert!(accepted.ok, "{accepted:?}");
    let a = mcp(rest).await;
    host.clock.advance(Duration::from_secs(1)).unwrap();
    let mut b_hello = main_hello(&host, &b_socket);
    b_hello["keep_others"] = json!(true);
    let (accepted, rest) = handshake(host.state(), &b_hello).await;
    assert!(accepted.ok, "{accepted:?}");
    let b = mcp(rest).await;

    let host = host.restart().await;
    drop(a);
    drop(b);
    // Each `fld mcp` replays its line with the mark; B reconnects first.
    host.clock.advance(Duration::from_secs(1)).unwrap();
    b_hello["reconnect"] = json!(true);
    let (accepted, rest) = handshake(host.state(), &b_hello).await;
    assert!(accepted.ok, "{accepted:?}");
    let b = mcp(rest).await;
    let mut a_hello = main_hello(&host, &a_socket);
    a_hello["reconnect"] = json!(true);
    let (accepted, rest) = handshake(host.state(), &a_hello).await;
    assert!(accepted.ok, "{accepted:?}");
    let a = mcp(rest).await;

    let handle = open_handle(&host);
    assert_eq!(handle.main_sessions.count(&main_name()), 2);
    let notice = fold_core::agents::push::Notice {
        agent: main_name(),
        text: "fold: task T-1 is ready.".to_owned(),
        next_tool: "get_task",
        needs_no_action: false,
    };
    handle.push.deliver(&[notice], 1);
    assert!(next_notice(&mut a_inbox).await.contains("T-1 is ready"));
    assert!(next_notice(&mut b_inbox).await.contains("T-1 is ready"));
    for client in [&a, &b] {
        assert_eq!(
            list_agents(client).await["agents"][0]["name"],
            json!("main")
        );
    }

    // A new `fld claude` without the mark still takes over both.
    host.clock.advance(Duration::from_secs(1)).unwrap();
    let (accepted, rest) = handshake(host.state(), &main_hello(&host, &c_socket)).await;
    assert!(accepted.ok, "{accepted:?}");
    let c = mcp(rest).await;
    for client in [a, b] {
        tokio::time::timeout(WAIT, client.waiting())
            .await
            .expect("fold closes the replayed connection")
            .unwrap();
    }
    assert!(next_notice(&mut a_inbox).await.contains("took over"));
    assert!(next_notice(&mut b_inbox).await.contains("took over"));
    assert_eq!(handle.main_sessions.count(&main_name()), 1);
    message_main(&host).await;
    assert!(next_notice(&mut c_inbox).await.contains("m-1 arrived"));
    c.cancel().await.unwrap();
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn acceptance_58_a_replayed_handshake_of_a_taken_over_adapter_is_still_refused() {
    let host = Host::start().await;
    let old_socket = host.root.parent().unwrap().join("old-inbox.sock");
    let new_socket = host.root.parent().unwrap().join("new-inbox.sock");
    let _old_inbox = session_inbox(&old_socket);
    let mut new_inbox = session_inbox(&new_socket);
    let (accepted, rest) = handshake(host.state(), &main_hello(&host, &old_socket)).await;
    assert!(accepted.ok, "{accepted:?}");
    let old = mcp(rest).await;
    host.clock.advance(Duration::from_secs(1)).unwrap();
    let (accepted, rest) = handshake(host.state(), &main_hello(&host, &new_socket)).await;
    assert!(accepted.ok, "{accepted:?}");
    let new = mcp(rest).await;
    tokio::time::timeout(WAIT, old.waiting())
        .await
        .expect("fold closes the old connection")
        .unwrap();

    let mut replay = main_hello(&host, &old_socket);
    replay["reconnect"] = json!(true);
    let line = format!("{replay}\n");
    let (code, _) = refused(host.state(), line.as_bytes()).await;
    assert_eq!(code, "main_session_taken_over");
    let handle = open_handle(&host);
    assert_eq!(handle.main_sessions.count(&main_name()), 1);
    message_main(&host).await;
    assert!(next_notice(&mut new_inbox).await.contains("m-1 arrived"));
    new.cancel().await.unwrap();
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn acceptance_58_a_taken_over_process_that_starts_a_new_conversation_gets_no_notices_again() {
    let host = Host::start().await;
    let old_socket = host.root.parent().unwrap().join("old-inbox.sock");
    let new_socket = host.root.parent().unwrap().join("new-inbox.sock");
    let mut old_inbox = session_inbox(&old_socket);
    let mut new_inbox = session_inbox(&new_socket);
    let (accepted, rest) = handshake(host.state(), &main_hello(&host, &old_socket)).await;
    assert!(accepted.ok, "{accepted:?}");
    let old = mcp(rest).await;
    host.clock.advance(Duration::from_secs(1)).unwrap();
    let (accepted, rest) = handshake(host.state(), &main_hello(&host, &new_socket)).await;
    assert!(accepted.ok, "{accepted:?}");
    let new = mcp(rest).await;
    assert!(next_notice(&mut old_inbox).await.contains("took over"));
    tokio::time::timeout(WAIT, old.waiting())
        .await
        .expect("fold closes the old connection")
        .unwrap();

    // `/clear` or a compaction in the old process runs its session-start hook again with the same inbox socket, which Claude Code names after its process.
    host.clock.advance(Duration::from_secs(1)).unwrap();
    let answer = session_start_hook(&host, "main", &old_socket).await;
    assert!(
        answer["context"].as_str().unwrap().contains("took over"),
        "{answer}"
    );
    let handle = open_handle(&host);
    assert_eq!(
        handle.push.sockets(&main_name()),
        std::slice::from_ref(&new_socket)
    );

    // `/clear` in the live session registers its own inbox again and keeps every notice.
    let answer = session_start_hook(&host, "main", &new_socket).await;
    assert!(
        !answer["context"].as_str().unwrap().contains("took over"),
        "{answer}"
    );
    assert_eq!(
        handle.push.sockets(&main_name()),
        std::slice::from_ref(&new_socket)
    );
    message_main(&host).await;
    assert!(next_notice(&mut new_inbox).await.contains("m-1 arrived"));
    let notice = fold_core::agents::push::Notice {
        agent: main_name(),
        text: "fold: task T-1 is ready.".to_owned(),
        next_tool: "get_task",
        needs_no_action: false,
    };
    handle.push.deliver(&[notice], 2);
    assert!(next_notice(&mut new_inbox).await.contains("T-1 is ready"));
    assert!(
        old_inbox.try_recv().is_err(),
        "the taken-over process gets no more notices"
    );
    new.cancel().await.unwrap();
    host.daemon.shutdown().await;
}

#[tokio::test]
async fn acceptance_58_a_new_process_with_the_socket_of_a_taken_over_one_gets_every_notice() {
    let host = Host::start().await;
    let old_socket = host.root.parent().unwrap().join("old-inbox.sock");
    let new_socket = host.root.parent().unwrap().join("new-inbox.sock");
    let mut old_inbox = session_inbox(&old_socket);
    let mut new_inbox = session_inbox(&new_socket);
    let (accepted, rest) = handshake(host.state(), &main_hello(&host, &old_socket)).await;
    assert!(accepted.ok, "{accepted:?}");
    let old = mcp(rest).await;
    host.clock.advance(Duration::from_secs(1)).unwrap();
    let (accepted, rest) = handshake(host.state(), &main_hello(&host, &new_socket)).await;
    assert!(accepted.ok, "{accepted:?}");
    let new = mcp(rest).await;
    assert!(next_notice(&mut old_inbox).await.contains("took over"));
    tokio::time::timeout(WAIT, old.waiting())
        .await
        .expect("fold closes the old connection")
        .unwrap();

    // The old process ended and a new `fld claude` got its pid, so Claude Code gives it the same inbox socket path; its adapter is another process.
    host.clock.advance(Duration::from_secs(1)).unwrap();
    let mut hello = main_hello(&host, &old_socket);
    hello["pid"] = json!(4343);
    let (accepted, rest) = handshake(host.state(), &hello).await;
    assert!(accepted.ok, "{accepted:?}");
    let third = mcp(rest).await;
    assert!(next_notice(&mut new_inbox).await.contains("took over"));
    let handle = open_handle(&host);
    assert_eq!(
        handle.push.sockets(&main_name()),
        std::slice::from_ref(&old_socket)
    );

    // Its session-start hook, at the start or after `/clear`, keeps its inbox and gets no takeover notice.
    let answer = session_start_hook(&host, "main", &old_socket).await;
    assert!(
        !answer["context"].as_str().unwrap().contains("took over"),
        "{answer}"
    );
    assert_eq!(
        handle.push.sockets(&main_name()),
        std::slice::from_ref(&old_socket)
    );
    message_main(&host).await;
    assert!(next_notice(&mut old_inbox).await.contains("m-1 arrived"));
    new.cancel().await.ok();
    third.cancel().await.unwrap();
    host.daemon.shutdown().await;
}
