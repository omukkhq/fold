#![cfg(test)]

use std::collections::BTreeSet;
use std::path::Path;

use axum::http::{Method, StatusCode};
use fold_core::{AgentName, WorkerState, codes};
use fold_store::agents::WorkerProcessRow;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, SLUG, WAIT};
use crate::workers_support::{FakeClaude, Run};

const MAIN: &str = "frontend";
const LEAD: &str = "design-lead";
const PROJECT_MCP: &str = r#"{"mcpServers":{"pencil":{"command":"pencil","args":["--stdio"]},"browser":{"command":"browser"}}}"#;

async fn worker(h: &Harness, agent: &str) -> WorkerProcessRow {
    h.handle
        .agents()
        .get_worker(&AgentName::parse(agent).unwrap())
        .await
        .unwrap()
}

async fn until_state(h: &Harness, agent: &str, state: WorkerState) -> WorkerProcessRow {
    eventually(WAIT, async || {
        let row = worker(h, agent).await;
        if row.state == state {
            Ok(row)
        } else {
            Err(row.state)
        }
    })
    .await
}

/// The newest run of `agent` once it has read its first turn.
async fn first_run(fake: &FakeClaude, agent: &str) -> Run {
    eventually(WAIT, async || {
        let runs = fake.runs_of(agent);
        match runs.last() {
            Some(last) if last.stdin.len() >= 2 => Ok(last.clone()),
            _ => Err(runs.len()),
        }
    })
    .await
}

/// The server names of the MCP configuration a run was given, in name order.
fn servers(run: &Run) -> Vec<String> {
    let text = std::fs::read_to_string(run.flag("--mcp-config").unwrap()).unwrap();
    let config: Value = serde_json::from_str(&text).unwrap();
    config["mcpServers"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn prompt(run: &Run) -> String {
    std::fs::read_to_string(run.flag("--append-system-prompt-file").unwrap()).unwrap()
}

/// Label `design` with tasks `T-1` and `T-2`, the area `design` with `profile`, and its lead, started and connected.
async fn design_area(h: &mut Harness, profile: Value) -> Value {
    std::fs::write(Path::new(h.root()).join(".mcp.json"), PROJECT_MCP).unwrap();
    std::fs::write(
        Path::new(h.root()).join(".fold/config.toml"),
        "[roles.task_worker]\nmodel = \"haiku\"\nmcp = [\"fold\"]\n",
    )
    .unwrap();
    h.call(
        MAIN,
        "create_label",
        json!({"name": "design", "color": "sage"}),
    )
    .await;
    let mut define = json!({"name": "design", "label": "design"});
    define
        .as_object_mut()
        .unwrap()
        .extend(profile.as_object().unwrap().clone());
    let defined = h.call(MAIN, "define_area", define).await;
    for title in ["Draw the sign-in screen", "Draw the board screen"] {
        h.call(
            MAIN,
            "create_task",
            json!({"title": title, "labels": ["design"]}),
        )
        .await;
    }
    h.call(
        MAIN,
        "start_worker",
        json!({"role": "area_lead", "area": "design"}),
    )
    .await;
    until_state(h, LEAD, WorkerState::Idle).await;
    h.connect(LEAD).await;
    defined
}

async fn start_task_worker(h: &Harness, task: &str, extra: Value) -> String {
    let mut start = json!({"role": "task_worker", "tasks": [task]});
    start
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let started = h.call(LEAD, "start_worker", start).await;
    started["worker"]["name"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn a_task_worker_of_the_design_area_starts_with_the_areas_servers_model_and_instructions() {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main(MAIN).await;
    let defined = design_area(
        &mut h,
        json!({"instructions": "  Draw screens in Pencil.  ", "model": "opus", "mcp": ["fold", "pencil"]}),
    )
    .await;
    assert_eq!(
        defined["area"],
        json!({
            "name": "design",
            "label": "design",
            "lead": null,
            "instructions": "Draw screens in Pencil.",
            "model": "opus",
            "mcp": ["fold", "pencil"],
        })
    );
    let lead = first_run(&fake, LEAD).await;
    assert_eq!(lead.flag("--model"), Some("opus"));
    assert!(lead.has("--strict-mcp-config"));
    assert_eq!(servers(&lead), ["fold", "pencil"]);
    assert!(prompt(&lead).ends_with("\n\nDraw screens in Pencil.\n"));

    let task_worker =
        start_task_worker(&h, "T-1", json!({"instructions": "Start with the form."})).await;
    let run = first_run(&fake, &task_worker).await;
    assert_eq!(run.flag("--model"), Some("opus"));
    assert!(run.has("--strict-mcp-config"));
    assert_eq!(servers(&run), ["fold", "pencil"]);
    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(run.flag("--mcp-config").unwrap()).unwrap())
            .unwrap();
    assert_eq!(
        config["mcpServers"]["pencil"],
        json!({"command": "pencil", "args": ["--stdio"]})
    );
    let text = prompt(&run);
    assert!(text.starts_with(fold_core::Role::TaskWorker.prompt().trim_end()));
    assert!(text.contains("## Area design"), "{text}");
    assert!(text.ends_with("\n\nDraw screens in Pencil.\n"));
    assert!(run.stdin[1].contains("Start with the form."));

    let own = start_task_worker(&h, "T-2", json!({"model": "sonnet"})).await;
    let run = first_run(&fake, &own).await;
    assert_eq!(run.flag("--model"), Some("sonnet"));
    assert_eq!(servers(&run), ["fold", "pencil"]);

    let listed = h.call(MAIN, "list_workers", json!({})).await;
    assert_eq!(listed["areas"][0]["model"], "opus");
    assert_eq!(listed["areas"][0]["lead"], LEAD);
}

#[tokio::test]
async fn a_changed_profile_reaches_the_next_start_and_never_a_running_worker() {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main(MAIN).await;
    design_area(&mut h, json!({"model": "opus", "mcp": ["fold", "pencil"]})).await;
    let first = start_task_worker(&h, "T-1", json!({})).await;
    let before = first_run(&fake, &first).await;
    let running = until_state(&h, &first, WorkerState::Idle).await;
    let first_config = std::fs::read_to_string(before.flag("--mcp-config").unwrap()).unwrap();

    let updated = h
        .call(
            MAIN,
            "update_area",
            json!({"name": "design", "model": "sonnet", "mcp": ["browser"], "instructions": "Use the browser."}),
        )
        .await;
    assert_eq!(updated["area"]["model"], "sonnet");
    assert_eq!(updated["area"]["mcp"], json!(["browser"]));
    assert_eq!(updated["area"]["instructions"], "Use the browser.");
    let after_update = worker(&h, &first).await;
    assert_eq!(after_update.pid, running.pid);
    assert_eq!(after_update.model, running.model);
    assert_eq!(fake.runs_of(&first).len(), 1);
    assert_eq!(
        std::fs::read_to_string(before.flag("--mcp-config").unwrap()).unwrap(),
        first_config
    );

    let second = start_task_worker(&h, "T-2", json!({})).await;
    let run = first_run(&fake, &second).await;
    assert_eq!(run.flag("--model"), Some("sonnet"));
    assert_eq!(servers(&run), ["browser", "fold"]);
    assert!(prompt(&run).ends_with("\n\nUse the browser.\n"));

    let cleared = h
        .call(
            MAIN,
            "update_area",
            json!({"name": "design", "clear_model": true, "clear_mcp": true, "clear_instructions": true}),
        )
        .await;
    assert_eq!(cleared["area"]["model"], Value::Null);
    assert_eq!(cleared["area"]["mcp"], Value::Null);
    let third = start_task_worker(&h, "T-1", json!({})).await;
    let run = first_run(&fake, &third).await;
    assert_eq!(run.flag("--model"), Some("haiku"));
    assert_eq!(servers(&run), ["fold"]);
    assert_eq!(prompt(&run), fold_core::Role::TaskWorker.prompt());

    let changes: Vec<Value> = h
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "area.updated")
        .map(|event| event.data)
        .collect();
    assert_eq!(
        changes,
        [
            json!({"area": "design", "fields": ["instructions", "model", "mcp"]}),
            json!({"area": "design", "fields": ["instructions", "model", "mcp"]}),
        ]
    );
}

#[tokio::test]
async fn a_project_role_prompt_comes_before_the_area_instructions() {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main(MAIN).await;
    let roles = Path::new(h.root()).join(".fold/roles");
    std::fs::create_dir_all(&roles).unwrap();
    std::fs::write(roles.join("task_worker.md"), "You draw for this project.\n").unwrap();
    design_area(&mut h, json!({"instructions": "Draw screens in Pencil."})).await;
    let task_worker = start_task_worker(&h, "T-1", json!({})).await;
    let run = first_run(&fake, &task_worker).await;
    let text = prompt(&run);
    assert!(
        text.starts_with("You draw for this project.\n\n## Authority and recovery"),
        "{text}"
    );
    let shared = text.find(fold_core::shared_prompt()).unwrap();
    let area = text.find("## Area design").unwrap();
    assert!(shared < area, "the shared rules come before the area");
    assert!(text.ends_with("\n\nDraw screens in Pencil.\n"));
    assert_eq!(run.flag("--model"), Some("haiku"));
}

#[tokio::test]
async fn only_the_main_agent_or_a_human_changes_a_profile_and_bad_values_are_refused() {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main(MAIN).await;
    design_area(&mut h, json!({})).await;
    let denied = h
        .refused(
            LEAD,
            "update_area",
            json!({"name": "design", "model": "opus"}),
        )
        .await;
    assert_eq!(denied["code"], codes::MAIN_AGENT_REQUIRED);
    let missing = h
        .refused(MAIN, "update_area", json!({"name": "web", "model": "opus"}))
        .await;
    assert_eq!(missing["code"], codes::AREA_NOT_FOUND);
    for bad in [
        json!({"name": "design"}),
        json!({"name": "design", "mcp": []}),
        json!({"name": "design", "model": "two words"}),
        json!({"name": "design", "model": "opus", "clear_model": true}),
    ] {
        let refused = h.refused(MAIN, "update_area", bad).await;
        assert_eq!(refused["code"], codes::INVALID_REQUEST);
    }
    h.call(
        MAIN,
        "create_label",
        json!({"name": "api", "color": "sage"}),
    )
    .await;
    let bad_define = h
        .refused(
            MAIN,
            "define_area",
            json!({"name": "api", "label": "api", "mcp": ["a b"]}),
        )
        .await;
    assert_eq!(bad_define["code"], codes::INVALID_REQUEST);

    let path = format!("/api/v1/projects/{SLUG}/areas/design");
    let (status, area) = h
        .http(
            Method::PATCH,
            &path,
            Some(&json!({"instructions": "Draw screens in Pencil.", "mcp": ["pencil"]})),
            "tanim",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{area}");
    assert_eq!(area["mcp"], json!(["pencil"]));
    assert_eq!(area["model"], Value::Null);
    let (status, refused) = h
        .http(Method::PATCH, &path, Some(&json!({})), "tanim")
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    let (status, created) = h
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/areas"),
            Some(&json!({"name": "api", "label": "api", "model": "sonnet"})),
            "tanim",
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["model"], "sonnet");
    let (status, listed) = h
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/workers"),
            None,
            "tanim",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(
        listed["areas"][1]["instructions"],
        "Draw screens in Pencil."
    );
    assert_eq!(listed["areas"][0]["model"], "sonnet");
}

#[tokio::test]
async fn an_agent_outside_the_team_leaves_profiles_and_task_kinds_to_the_main_agent() {
    let h = Harness::start().await;
    h.make_main(MAIN).await;
    h.call(
        MAIN,
        "create_label",
        json!({"name": "design", "color": "sage"}),
    )
    .await;
    h.call(
        MAIN,
        "define_area",
        json!({"name": "design", "label": "design"}),
    )
    .await;
    let registered = h
        .call(
            MAIN,
            "register_agent",
            json!({"name": MAIN, "tags": ["main"]}),
        )
        .await;
    assert!(registered.to_string().contains("\"main\""), "{registered}");

    let profile = h
        .refused(
            "backend",
            "update_area",
            json!({"name": "design", "model": "opus"}),
        )
        .await;
    assert_eq!(profile["code"], codes::ORCHESTRATOR_REQUIRED, "{profile}");
    assert!(
        profile["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("update_area"),
        "{profile}"
    );
    let kind = h
        .refused(
            "backend",
            "create_task",
            json!({"title": "Draw the icons", "labels": ["design"], "kind": "no_proposal"}),
        )
        .await;
    assert_eq!(kind["code"], codes::ORCHESTRATOR_REQUIRED, "{kind}");

    let changed = h
        .call(
            MAIN,
            "update_area",
            json!({"name": "design", "model": "opus"}),
        )
        .await;
    assert_eq!(changed["area"]["model"], "opus");
}
