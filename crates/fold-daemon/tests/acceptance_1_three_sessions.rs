#![cfg(test)]

pub mod common;

use common::acceptance::{AGENTS, Harness, body};
use serde_json::{Value, json};

/// Acceptance 1: three sessions registered in one project see one project. Each view tool returns the same body to every caller; only the footer, which counts the caller's own unread messages and new events, may differ.
#[tokio::test]
async fn acceptance_1_three_sessions_share_one_view_of_agents_tasks_and_repository() {
    let harness = Harness::start().await;
    for agent in AGENTS {
        let registered = harness
            .call(agent, "register_agent", json!({"name": agent}))
            .await;
        assert_eq!(registered["agent"], agent);
        assert_eq!(registered["main_worker"], agent);
        assert_eq!(
            registered["created"], false,
            "the handshake registered {agent}"
        );
    }

    let api = harness
        .call(
            "frontend",
            "create_task",
            json!({"title": "Add the login endpoint", "description": "POST /login"}),
        )
        .await;
    let screen = harness
        .call(
            "frontend",
            "create_task",
            json!({"title": "Build the login screen"}),
        )
        .await;
    let (api, screen) = (api["id"].as_str().unwrap(), screen["id"].as_str().unwrap());
    harness
        .call(
            "frontend",
            "add_dependency",
            json!({"task": screen, "needs": api}),
        )
        .await;

    // `list_agents` shows each worker's last tool, so every session first makes the same call, and each view tool then goes round all three sessions before the next one.
    for agent in AGENTS {
        harness.call(agent, "list_agents", json!({})).await;
    }
    let mut views: Vec<Value> = Vec::new();
    for tool in ["list_agents", "get_task_graph", "get_repository_state"] {
        let mut seen = Vec::new();
        for agent in AGENTS {
            let result = harness.call_raw(agent, tool, json!({})).await;
            assert_eq!(result.is_error, Some(false), "{agent} {tool}: {result:?}");
            seen.push(body(&result));
        }
        assert_eq!(seen[1], seen[0], "backend sees the {tool} of frontend");
        assert_eq!(seen[2], seen[0], "review sees the {tool} of frontend");
        views.push(seen.swap_remove(0));
    }

    let agents = &views[0]["agents"];
    let names: Vec<&str> = agents
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, AGENTS);
    for agent in agents.as_array().unwrap() {
        assert_eq!(agent["presence"], "active", "{agent}");
    }
    assert_eq!(agents[2]["tags"], json!(["integrator"]));

    let tasks = views[1]["tasks"].as_array().unwrap();
    let graph: Vec<(&str, &str, &Value)> = tasks
        .iter()
        .map(|task| {
            (
                task["id"].as_str().unwrap(),
                task["title"].as_str().unwrap(),
                &task["needs"],
            )
        })
        .collect();
    assert_eq!(
        graph,
        vec![
            (api, "Add the login endpoint", &json!([])),
            (screen, "Build the login screen", &json!([api])),
        ]
    );

    let repository = &views[2];
    assert_eq!(repository["head"], "r1");
    assert_eq!(repository["recent"].as_array().unwrap().len(), 1);
}
