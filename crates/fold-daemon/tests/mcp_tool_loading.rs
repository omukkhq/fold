use std::collections::BTreeSet;

use fold_core::roles::Role;
use fold_daemon::mcp::handshake::{self, Answer, PROTOCOL_VERSION};
use fold_daemon::mcp::loading::{Seat, always_loaded, is_always_loaded};
use rmcp::ServiceExt;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::common::acceptance::{Client, Harness, WAIT};
use crate::common::roles::give_role;

/// The names of the listed tools that carry the always-load mark, and the number of listed tools.
#[cfg(test)]
async fn marked(client: &Client) -> (BTreeSet<String>, usize) {
    let tools = client.list_all_tools().await.unwrap();
    let marked = tools
        .iter()
        .filter(|tool| is_always_loaded(tool))
        .map(|tool| tool.name.clone().into_owned())
        .collect();
    (marked, tools.len())
}

#[cfg(test)]
fn names(seat: Seat) -> BTreeSet<String> {
    always_loaded(seat)
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

#[tokio::test]
async fn each_role_loads_its_frequent_tools_at_once_and_defers_the_rare_ones() {
    let mut harness = Harness::start_without_leads().await;
    harness.make_main("frontend").await;
    let store = harness.handle.store.clone();
    give_role(&store, "backend", Role::AreaLead, None).await;
    give_role(&store, "builder", Role::TaskWorker, None).await;
    give_role(&store, "triage", Role::ReviewLead, None).await;
    for agent in ["builder", "triage", "visitor"] {
        harness.connect(agent).await;
    }
    let rare = [
        "define_area",
        "set_worker_limits",
        "restart_worker",
        "add_exclude_patterns",
        "list_review_batches",
        "release_task",
    ];
    for (agent, seat) in [
        ("frontend", Seat::Role(Role::Main)),
        ("backend", Seat::Role(Role::AreaLead)),
        ("triage", Seat::Role(Role::ReviewLead)),
        ("builder", Seat::Role(Role::TaskWorker)),
        ("review", Seat::Role(Role::Reviewer)),
        ("visitor", Seat::NoRole),
    ] {
        let (marked, listed) = marked(harness.client(agent)).await;
        assert_eq!(marked, names(seat), "{agent}");
        assert!(
            marked.len() * 4 < listed,
            "{agent}: most tools stay deferred"
        );
        for tool in rare {
            assert!(!marked.contains(tool), "{agent}: {tool}");
        }
    }
    let (worker, _) = marked(harness.client("builder")).await;
    for tool in [
        "get_task",
        "create_proposal",
        "run_checks",
        "sync_workspace",
    ] {
        assert!(worker.contains(tool), "{tool}");
    }
    let (reviewer, _) = marked(harness.client("review")).await;
    for tool in [
        "create_review_checkout",
        "submit_review",
        "submit_review_fix",
    ] {
        assert!(reviewer.contains(tool), "{tool}");
    }
    assert!(!reviewer.contains("create_proposal"));
}

#[tokio::test]
async fn a_session_without_an_agent_loads_only_the_tool_that_binds_it() {
    let harness = Harness::start_without_leads().await;
    let (client_end, server_end) = tokio::io::duplex(256 * 1024);
    let _server = tokio::spawn(handshake::accept(
        harness.state.clone(),
        server_end,
        Default::default(),
    ));
    let (read, mut write) = tokio::io::split(client_end);
    let hello = json!({"fold": PROTOCOL_VERSION, "cwd": harness.root(), "pid": 4242});
    write
        .write_all(format!("{hello}\n").as_bytes())
        .await
        .unwrap();
    let mut read = BufReader::new(read);
    let mut line = String::new();
    tokio::time::timeout(WAIT, read.read_line(&mut line))
        .await
        .expect("the handshake answer within the wait")
        .unwrap();
    let answer: Answer = serde_json::from_str(&line).unwrap();
    assert!(answer.ok, "{line}");
    let client = ().serve((read, write)).await.unwrap();
    let (marked, _) = marked(&client).await;
    assert_eq!(marked, names(Seat::Unbound));
    assert_eq!(marked, BTreeSet::from(["register_agent".to_owned()]));
}
