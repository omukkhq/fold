#![cfg(test)]

use crate::common::roles::give_role;
use fold_core::roles::Role;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::{Actor, AgentActor, AgentName, Clock, HumanName, WorkerName, WorkerState, codes};
use fold_daemon::services::agents::{Launcher, MCP_RETRY_TURN, PAUSE_TURN, RESTART_TURN};
use fold_daemon::services::events::BusMessage;
use fold_store::agents::WorkerProcessRow;
use fold_store::testing::{eventually, remove_dir_eventually};
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, SLUG, WAIT};
use crate::workers_support::{FakeClaude, Run};

const MAIN: &str = "frontend";

fn name(text: &str) -> AgentName {
    AgentName::parse(text).unwrap()
}

/// Labels `ui` and `server`, areas `web` over `ui` and `api` over `server`, task `T-1` labelled `ui` and `T-2` labelled `server`, all made by the main agent `frontend`.
async fn areas(harness: &Harness) {
    for label in ["ui", "server"] {
        harness
            .call(
                MAIN,
                "create_label",
                json!({"name": label, "color": "sage"}),
            )
            .await;
    }
    for (area, label) in [("web", "ui"), ("api", "server")] {
        let defined = harness
            .call(MAIN, "define_area", json!({"name": area, "label": label}))
            .await;
        assert_eq!(defined["area"]["label"], label);
    }
    for (title, label) in [("Draw the page", "ui"), ("Serve the page", "server")] {
        harness
            .call(
                MAIN,
                "create_task",
                json!({"title": title, "labels": [label]}),
            )
            .await;
    }
}

async fn worker(harness: &Harness, agent: &str) -> WorkerProcessRow {
    harness
        .handle
        .agents()
        .get_worker(&name(agent))
        .await
        .unwrap()
}

async fn until_state(harness: &Harness, agent: &str, state: WorkerState) -> WorkerProcessRow {
    eventually(WAIT, async || {
        let row = worker(harness, agent).await;
        if row.state == state {
            Ok(row)
        } else {
            Err(row.state)
        }
    })
    .await
}

/// The runs of `agent` once there are `count` of them and the newest has read `lines` stdin lines.
async fn until_runs(fake: &FakeClaude, agent: &str, count: usize, lines: usize) -> Vec<Run> {
    eventually(WAIT, async || {
        let runs = fake.runs_of(agent);
        match runs.last() {
            Some(last) if runs.len() == count && last.stdin.len() >= lines => Ok(runs),
            _ => Err(runs.len()),
        }
    })
    .await
}

/// The main agent starts `web-lead`, whose own session then connects as Claude Code's `fld mcp` would.
async fn start_lead(harness: &mut Harness, area: &str) -> String {
    let started = harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": area}),
        )
        .await;
    let lead = started["worker"]["name"].as_str().unwrap().to_owned();
    assert_eq!(lead, format!("{area}-lead"));
    harness.connect(&lead).await;
    lead
}

async fn stored_events(harness: &Harness) -> Vec<(String, Value)> {
    harness
        .events()
        .await
        .into_iter()
        .map(|event| (event.kind, event.data))
        .collect()
}

#[tokio::test]
async fn an_area_lead_starts_task_workers_only_for_tasks_of_its_own_area() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let started = harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"], "instructions": "Keep the page small."}),
        )
        .await;
    assert_eq!(started["worker"]["name"], "web-worker");
    assert_eq!(started["worker"]["area"], "web");
    assert_eq!(started["worker"]["tasks"], json!(["T-1"]));

    let outside = harness
        .refused(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-2"]}),
        )
        .await;
    assert_eq!(outside["code"], codes::TASK_OUTSIDE_AREA);
    let other_area = harness
        .refused(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "area": "api", "tasks": ["T-2"]}),
        )
        .await;
    assert_eq!(other_area["code"], codes::WORKER_START_DENIED);
    let by_main = harness
        .refused(
            MAIN,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    assert_eq!(by_main["code"], codes::WORKER_START_DENIED);
    harness.connect("web-worker").await;
    let by_task_worker = harness
        .refused(
            "web-worker",
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    assert_eq!(by_task_worker["code"], codes::WORKER_START_DENIED);
    let lead_by_lead = harness
        .refused(
            &lead,
            "start_worker",
            json!({"role": "area_lead", "area": "api"}),
        )
        .await;
    assert_eq!(lead_by_lead["code"], codes::MAIN_AGENT_REQUIRED);
    let second_lead = harness
        .refused(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    assert_eq!(second_lead["code"], codes::AREA_HAS_LEAD);

    let lead_run = until_runs(&fake, &lead, 1, 2).await.remove(0);
    let task_worker_run = until_runs(&fake, "web-worker", 1, 2).await.remove(0);
    for (run, ttl) in [(&lead_run, "1h"), (&task_worker_run, "5m")] {
        assert_eq!(run.env["CLAUDE_CODE_PROMPT_CACHE_TTL"], ttl);
        assert_eq!(run.env["CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL"], ttl);
        assert_eq!(run.flag("--permission-mode"), Some("auto"));
        assert_eq!(run.flag("--permission-prompt-tool"), Some("stdio"));
        assert_eq!(run.flag("--input-format"), Some("stream-json"));
        assert!(run.has("-p") && run.has("--verbose"));
        assert!(!run.has("--strict-mcp-config"));
        assert!(!run.has("--resume"));
        assert!(run.flag("--append-system-prompt-file").is_some());
        assert_eq!(
            std::path::Path::new(&run.cwd),
            std::path::Path::new(harness.root())
        );
    }
    let task_worker = worker(&harness, "web-worker").await;
    assert_eq!(
        task_worker_run.flag("--session-id"),
        Some(task_worker.session_id.as_str())
    );
    assert!(task_worker_run.stdin[0].contains("\"initialize\""));
    assert!(task_worker_run.stdin[1].contains("task worker for T-1"));
    assert!(task_worker_run.stdin[1].contains("Keep the page small."));
    let prompt =
        std::fs::read_to_string(task_worker_run.flag("--append-system-prompt-file").unwrap())
            .unwrap();
    assert!(prompt.contains("never edit the project's main files"));
    let config: Value = serde_json::from_str(
        &std::fs::read_to_string(task_worker_run.flag("--mcp-config").unwrap()).unwrap(),
    )
    .unwrap();
    let fold = fake.launcher.fold.clone().unwrap();
    assert_eq!(
        config,
        json!({"mcpServers": {"fold": {"command": fold, "args": ["mcp"], "env": {"FOLD_AGENT": "web-worker"}}}})
    );
    for run in [&lead_run, &task_worker_run] {
        let own: Value = serde_json::from_str(
            &std::fs::read_to_string(run.flag("--settings").unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(own, fold_core::layout::session_settings());
        assert_eq!(own["permissions"]["allow"], json!(["mcp__fold"]));
        assert_eq!(
            own["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "fld hook pre-tool-use"
        );
    }
    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    assert_eq!(listed["areas"][1]["lead"], "web-lead");
    let names: Vec<&str> = listed["workers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|worker| worker["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["web-lead", "web-worker"]);
    let ttls: Vec<&str> = listed["workers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|worker| worker["cache_ttl"].as_str().unwrap())
        .collect();
    assert_eq!(ttls, ["1h", "5m"]);
}

#[tokio::test]
async fn a_task_worker_stops_once_its_task_has_an_open_proposal() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    until_state(&harness, "web-worker", WorkerState::Idle).await;
    harness.connect("web-worker").await;
    let work = harness.start_work("web-worker", "T-1").await;
    std::fs::write(work.dir.join("page.html"), "<p>page</p>\n").unwrap();
    harness.propose("web-worker", &work, "Draw the page").await;
    let stopped = until_state(&harness, "web-worker", WorkerState::Stopped).await;
    assert_eq!(stopped.exit_code, Some(0));
    assert_eq!(stopped.turns, 1);
    assert_eq!(worker(&harness, &lead).await.state, WorkerState::Idle);
    let events = stored_events(&harness).await;
    let stopped_event = events
        .iter()
        .find(|(kind, _)| kind == "worker.stopped")
        .unwrap();
    assert_eq!(stopped_event.1["agent"], "web-worker");
    assert_eq!(stopped_event.1["exit_code"], 0);
    assert!(!harness.handle.workers.is_running(&name("web-worker")));
}

#[tokio::test]
async fn a_task_worker_stops_when_its_last_proposal_opens_after_an_earlier_one_integrated() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    give_role(&harness.handle.store, "review", Role::ReviewLead, None).await;
    areas(&harness).await;
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Style the page", "labels": ["ui"]}),
        )
        .await;
    let lead = start_lead(&mut harness, "web").await;
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1", "T-3"]}),
        )
        .await;
    until_state(&harness, "web-worker", WorkerState::Idle).await;
    harness.connect("web-worker").await;
    let first = harness.start_work("web-worker", "T-1").await;
    std::fs::write(first.dir.join("page.html"), "<p>page</p>\n").unwrap();
    let proposal = harness.propose("web-worker", &first, "Draw the page").await;
    harness.approve(&proposal).await;
    harness
        .call("review", "integrate_proposal", json!({"id": proposal}))
        .await;
    let waiting = worker(&harness, "web-worker").await.state;
    assert!(
        matches!(waiting, WorkerState::Idle | WorkerState::Working),
        "T-3 still waits for its proposal: {waiting}"
    );
    let second = harness.start_work("web-worker", "T-3").await;
    std::fs::write(second.dir.join("style.css"), "p {}\n").unwrap();
    harness
        .propose("web-worker", &second, "Style the page")
        .await;
    let stopped = until_state(&harness, "web-worker", WorkerState::Stopped).await;
    assert_eq!(stopped.exit_code, Some(0));
}

#[tokio::test]
async fn an_idle_task_worker_stops_after_its_idle_minutes_and_resumes_with_a_turn() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    let idle = until_state(&harness, "web-worker", WorkerState::Idle).await;
    assert_eq!(idle.turns, 1);
    assert!(idle.cost_micro_usd > 0);
    assert!(idle.cache_read_tokens > 0);
    assert!(idle.context_tokens > 0);
    assert_eq!(idle.model.as_deref(), Some("claude-haiku-5-5"));
    let workers = harness.handle.workers.clone();
    let clock = harness.project.clock().clone();
    let early = workers
        .sweep_idle(clock.advance(Duration::from_secs(14 * 60)).unwrap())
        .await
        .unwrap();
    assert!(early.is_empty());
    let late = workers
        .sweep_idle(clock.advance(Duration::from_secs(60)).unwrap())
        .await
        .unwrap();
    assert_eq!(late, [name("web-worker")]);
    let stopped = worker(&harness, "web-worker").await;
    assert_eq!(stopped.state, WorkerState::Stopped);
    assert_eq!(stopped.stopped_at, Some(clock.now()));
    assert_eq!(
        worker(&harness, &lead).await.state,
        WorkerState::Idle,
        "a lead never stops for being idle"
    );

    let itself = Actor::Agent(AgentActor {
        agent: name("web-worker"),
        worker: WorkerName::parse("web-worker").unwrap(),
        tags: Vec::new(),
    });
    let denied = harness
        .handle
        .agents()
        .resume_worker(&itself, &name("web-worker"), "go on".to_owned())
        .await
        .unwrap_err();
    assert!(
        matches!(&denied, fold_daemon::services::agents::AgentError::Fold(error) if error.code() == codes::WORKER_STOP_DENIED),
        "only whoever may stop a worker resumes it: {denied:?}"
    );
    let human = Actor::human(HumanName::parse("ana").unwrap());
    let resumed = harness
        .handle
        .agents()
        .resume_worker(
            &human,
            &name("web-worker"),
            "Your proposal needs a second look.".to_owned(),
        )
        .await
        .unwrap();
    assert!(resumed.state.is_running());
    let runs = until_runs(&fake, "web-worker", 2, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(stopped.session_id.as_str()));
    assert!(!runs[1].has("--session-id"));
    assert!(runs[1].stdin[1].contains("Your proposal needs a second look."));
    let again = until_state(&harness, "web-worker", WorkerState::Idle).await;
    assert_eq!(again.turns, 2);
    assert!(again.cost_micro_usd > idle.cost_micro_usd);
    let running = harness
        .handle
        .agents()
        .resume_worker(&human, &name("web-worker"), "again".to_owned())
        .await
        .unwrap_err();
    assert!(
        matches!(running, fold_daemon::services::agents::AgentError::Fold(error) if error.code() == codes::WORKER_EXISTS)
    );
}

#[tokio::test]
async fn a_fold_notice_reaches_a_worker_once_as_a_user_turn() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    until_state(&harness, &lead, WorkerState::Idle).await;
    let mut inbox = harness.register_inbox(&lead).await;
    let sent = harness
        .call(
            MAIN,
            "send_message",
            json!({"to": [&lead], "body": "the plover task"}),
        )
        .await;
    let message = sent["id"].as_str().unwrap().to_owned();
    let run = until_runs(&fake, &lead, 1, 3).await.remove(0);
    assert!(
        run.stdin[2].contains("\"type\":\"user\""),
        "{}",
        run.stdin[2]
    );
    assert!(
        run.stdin[2].contains(&format!("message {message} arrived")),
        "{}",
        run.stdin[2]
    );
    assert!(!run.stdin[2].contains("plover"));
    eventually(WAIT, async || {
        let row = worker(&harness, &lead).await;
        if row.turns == 2 && row.state == WorkerState::Idle {
            Ok(())
        } else {
            Err((row.turns, row.state))
        }
    })
    .await;
    assert_eq!(
        inbox.try_next(),
        None,
        "a supervised worker gets its notices on stdin only"
    );
}

#[tokio::test]
async fn the_fifth_task_worker_and_the_fourth_reviewer_are_refused_and_a_lead_never_is() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let mut proposals = Vec::new();
    for title in ["Write the docs", "Write the guide", "Write the notes"] {
        let task = harness.create_task(MAIN, title).await;
        let work = harness.start_work(MAIN, &task).await;
        std::fs::write(work.dir.join("docs.txt"), format!("{title}\n")).unwrap();
        proposals.push(harness.propose(MAIN, &work, title).await);
    }
    let mut pages = vec!["T-1".to_owned()];
    for title in [
        "Draw the menu",
        "Draw the footer",
        "Draw the form",
        "Draw the list",
    ] {
        let created = harness
            .call(
                MAIN,
                "create_task",
                json!({"title": title, "labels": ["ui"]}),
            )
            .await;
        pages.push(created["id"].as_str().unwrap().to_owned());
    }
    let lead = start_lead(&mut harness, "web").await;
    let task_worker = |task: &str| json!({"role": "task_worker", "tasks": [task]});
    for task in &pages[..4] {
        harness.call(&lead, "start_worker", task_worker(task)).await;
    }
    let refused = harness
        .refused(&lead, "start_worker", task_worker(&pages[4]))
        .await;
    assert_eq!(refused["code"], codes::WORKER_LIMIT_REACHED);
    assert_eq!(refused["details"], json!({"role": "task_worker", "max": 4}));
    assert!(
        refused["hint"]
            .as_str()
            .unwrap()
            .contains("task_worker limit is 4"),
        "{refused}"
    );
    assert!(
        harness
            .handle
            .agents()
            .get_worker(&name("web-worker-5"))
            .await
            .is_err()
    );

    start_lead(&mut harness, "api").await;
    let review = harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    assert_eq!(review["worker"]["name"], "review-lead");
    harness.connect("review-lead").await;
    let reviewer = |proposal: &str| json!({"role": "reviewer", "proposals": [proposal]});
    for proposal in &proposals {
        harness
            .call("review-lead", "start_worker", reviewer(proposal))
            .await;
    }
    // The limit is checked before the claims, so a held proposal is refused for the limit.
    let full = harness
        .refused("review-lead", "start_worker", reviewer(&proposals[0]))
        .await;
    assert_eq!(full["code"], codes::WORKER_LIMIT_REACHED);
    assert_eq!(full["details"], json!({"role": "reviewer", "max": 3}));
    assert!(
        full["hint"]
            .as_str()
            .unwrap()
            .contains("reviewer limit is 3"),
        "{full}"
    );

    harness
        .call(&lead, "stop_worker", json!({"name": "web-worker"}))
        .await;
    let fifth = harness
        .call(&lead, "start_worker", task_worker(&pages[4]))
        .await;
    assert_eq!(fifth["worker"]["name"], "web-worker-5");
}

#[tokio::test]
async fn a_daemon_restart_resumes_a_working_lead() {
    let fake = FakeClaude::new("init");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let working = until_state(&harness, &lead, WorkerState::Working).await;
    until_runs(&fake, &lead, 1, 2).await;
    harness.stop().await;
    harness.start_again().await;
    let runs = until_runs(&fake, &lead, 2, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(working.session_id.as_str()));
    assert!(runs[1].stdin[1].contains(RESTART_TURN));
    assert!(
        runs[1].stdin[1]
            .contains("These tasks of area web are ready and wait for a task worker: T-1."),
        "the restart turn also names the ready task: {:?}",
        runs[1].stdin
    );
    assert_eq!(user_turns(&runs[1]).len(), 1, "{:?}", runs[1].stdin);
    let resumed = until_state(&harness, &lead, WorkerState::Working).await;
    assert_eq!(resumed.session_id, working.session_id);
    assert!(harness.handle.workers.is_running(&name(&lead)));
    let events = stored_events(&harness).await;
    assert!(
        events
            .iter()
            .all(|(kind, _)| kind != "worker.stopped" && kind != "worker.failed"),
        "the daemon's own stop is not a worker stop"
    );
}

#[tokio::test]
async fn a_role_that_names_its_mcp_servers_gets_only_those() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    let root = std::path::PathBuf::from(harness.root());
    std::fs::write(
        root.join(".fold/config.toml"),
        "[roles.area_lead]\nmcp = [\"fold\", \"pencil\"]\nmodel = \"sonnet\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".mcp.json"),
        r#"{"mcpServers":{"pencil":{"command":"pencil-mcp"},"other":{"command":"other-mcp"}}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".fold/roles")).unwrap();
    std::fs::write(
        root.join(".fold/roles/area_lead.md"),
        "Our own lead prompt.\n",
    )
    .unwrap();
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let run = until_runs(&fake, &lead, 1, 2).await.remove(0);
    assert!(run.has("--strict-mcp-config"));
    assert_eq!(run.flag("--model"), Some("sonnet"));
    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(run.flag("--mcp-config").unwrap()).unwrap())
            .unwrap();
    let servers: Vec<&String> = config["mcpServers"].as_object().unwrap().keys().collect();
    assert_eq!(servers, ["fold", "pencil"]);
    assert_eq!(
        config["mcpServers"]["fold"]["env"]["FOLD_AGENT"],
        "web-lead"
    );
    assert_eq!(
        std::fs::read_to_string(run.flag("--append-system-prompt-file").unwrap()).unwrap(),
        format!("Our own lead prompt.\n\n{}", fold_core::shared_prompt())
    );
}

/// Collects every formatted log line of the process.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn no_log_line_or_stored_event_holds_output_text() {
    let logs = Logs::default();
    let writer = logs.clone();
    use tracing_subscriber::layer::SubscriberExt as _;
    // rmcp traces every request with its arguments, so only fold's own crates are read.
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || writer.clone()),
        )
        .with(
            tracing_subscriber::filter::Targets::new()
                .with_target("fold_daemon", tracing::Level::TRACE)
                .with_target("fold_store", tracing::Level::TRACE)
                .with_target("fold_vcs", tracing::Level::TRACE),
        );
    let _guard = tracing::subscriber::set_default(subscriber);
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    let mut bus = harness.handle.bus.subscribe();
    areas(&harness).await;
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web", "instructions": "The secret plan is plover."}),
        )
        .await;
    harness.connect("web-lead").await;
    until_state(&harness, "web-lead", WorkerState::Idle).await;
    let mut steps = Vec::new();
    while let Ok(message) = bus.try_recv() {
        if let BusMessage::Live(event) = message
            && event.event_type == fold_core::events::types::WORKER_OUTPUT
        {
            steps.push(event.data.clone());
        }
    }
    assert!(
        steps.contains(&json!({"agent": "web-lead", "step": "text", "text_length": 8})),
        "{steps:?}"
    );
    assert!(steps.iter().any(|step| step["step"] == "result"));
    harness
        .call(MAIN, "stop_worker", json!({"name": "web-lead"}))
        .await;
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    let events = serde_json::to_string(&stored_events(&harness).await).unwrap();
    let live = serde_json::to_string(&steps).unwrap();
    for secret in ["heron", "plover", "ok heron", "Stop hook"] {
        assert!(!text.contains(secret), "a log line holds {secret:?}");
        assert!(!events.contains(secret), "a stored event holds {secret:?}");
        assert!(!live.contains(secret), "a live event holds {secret:?}");
    }
    assert!(events.contains("worker.started") && events.contains("worker.stopped"));
}

#[tokio::test]
async fn a_stop_escalates_to_signals_on_the_projects_clock() {
    let fake = FakeClaude::new("result").stubborn();
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    until_state(&harness, &lead, WorkerState::Idle).await;
    let agents = harness.handle.agents();
    let human = Actor::human(HumanName::parse("ana").unwrap());
    let target = name(&lead);
    let stop = tokio::spawn(async move { agents.stop_worker(&human, &target).await });
    let clock = harness.project.clock().clone();
    eventually(WAIT, async || {
        if stop.is_finished() {
            Ok(())
        } else {
            clock.advance(Duration::from_secs(6)).unwrap();
            Err(clock.now())
        }
    })
    .await;
    let row = worker(&harness, &lead).await;
    assert_eq!(row.state, WorkerState::Stopped);
    assert_eq!(
        row.exit_code,
        Some(143),
        "SIGTERM ends the stand-in after SIGINT was ignored"
    );
}

#[tokio::test]
async fn a_stop_that_needs_signals_leaves_no_last_error() {
    let fake = FakeClaude::new("result")
        .stubborn()
        .warning_on_stderr("warning: slow disk");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    until_state(&harness, &lead, WorkerState::Idle).await;
    let agents = harness.handle.agents();
    let human = Actor::human(HumanName::parse("ana").unwrap());
    let target = name(&lead);
    let stop = tokio::spawn(async move { agents.stop_worker(&human, &target).await });
    let clock = harness.project.clock().clone();
    eventually(WAIT, async || {
        if stop.is_finished() {
            Ok(())
        } else {
            clock.advance(Duration::from_secs(6)).unwrap();
            Err(clock.now())
        }
    })
    .await;
    let row = worker(&harness, &lead).await;
    assert_eq!(
        (row.state, row.exit_code),
        (WorkerState::Stopped, Some(143))
    );
    assert_eq!(
        (row.last_error, row.last_error_at),
        (None, None),
        "the stderr of a process that fold stopped is no error"
    );
}

/// Whether `pid` is gone or only waits to be reaped. `ps` prints the state of a listed process and nothing for one that is gone, on Linux and on macOS.
fn ended(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&output.stdout);
    state.trim().is_empty() || state.trim_start().starts_with('Z')
}

#[tokio::test]
async fn a_stop_signals_every_process_the_worker_started() {
    let fake = FakeClaude::new("result").stubborn().with_child();
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    until_state(&harness, &lead, WorkerState::Idle).await;
    let run = fake.runs_of(&lead).remove(0);
    let child = eventually(WAIT, async || fake.child_of(&run).ok_or("no child yet")).await;
    assert!(!ended(child));
    let agents = harness.handle.agents();
    let human = Actor::human(HumanName::parse("ana").unwrap());
    let target = name(&lead);
    let stop = tokio::spawn(async move { agents.stop_worker(&human, &target).await });
    let clock = harness.project.clock().clone();
    eventually(WAIT, async || {
        if stop.is_finished() {
            Ok(())
        } else {
            clock.advance(Duration::from_secs(6)).unwrap();
            Err(clock.now())
        }
    })
    .await;
    eventually(WAIT, async || {
        if ended(child) {
            Ok(())
        } else {
            Err("the child still runs")
        }
    })
    .await;
}

#[tokio::test]
async fn shutdown_ends_every_worker_at_once() {
    let fake = FakeClaude::new("result").stubborn();
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let web = start_lead(&mut harness, "web").await;
    let api = start_lead(&mut harness, "api").await;
    until_state(&harness, &web, WorkerState::Idle).await;
    until_state(&harness, &api, WorkerState::Idle).await;
    let workers = harness.handle.workers.clone();
    let shutdown = tokio::spawn(async move { workers.shutdown().await });
    // The stand-ins ignore a closed stdin and SIGINT, and the clock stands still, so a shutdown that ended one worker before the next would close only the first one's stdin.
    eventually(WAIT, async || {
        let closed = [&web, &api]
            .iter()
            .filter(|lead| fake.saw_eof(&fake.runs_of(lead).remove(0)))
            .count();
        if closed == 2 { Ok(()) } else { Err(closed) }
    })
    .await;
    let clock = harness.project.clock().clone();
    eventually(WAIT, async || {
        if shutdown.is_finished() {
            Ok(())
        } else {
            clock.advance(Duration::from_secs(6)).unwrap();
            Err(clock.now())
        }
    })
    .await;
}

#[tokio::test]
async fn opening_a_project_whose_fold_data_was_deleted_removes_it_and_ends_its_workers() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    until_state(&harness, &lead, WorkerState::Idle).await;
    let run = fake.runs_of(&lead).remove(0);
    remove_dir_eventually(&harness.project.paths().fold_dir(), WAIT).await;
    let state = harness.state.clone();
    let open =
        tokio::spawn(async move { fold_daemon::services::projects::open(&state, SLUG).await });
    let clock = harness.project.clock().clone();
    eventually(WAIT, async || {
        if open.is_finished() {
            Ok(())
        } else {
            clock.advance(Duration::from_secs(6)).unwrap();
            Err(clock.now())
        }
    })
    .await;
    let error = open.await.unwrap().unwrap_err();
    assert_eq!(error.code(), Some(codes::PROJECT_NOT_FOUND));
    assert!(fake.saw_eof(&run));
    assert!(harness.handle.store.read().is_closed());
}

#[tokio::test]
async fn a_line_that_is_not_utf8_or_too_long_does_not_stop_the_reader() {
    let fake = FakeClaude::new("result").noisy();
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let idle = until_state(&harness, &lead, WorkerState::Idle).await;
    assert_eq!(idle.turns, 1, "the turn after the noise is still recorded");
}

#[tokio::test]
async fn a_human_defines_areas_and_starts_and_stops_leads_over_http() {
    let fake = FakeClaude::new("result");
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    harness
        .call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    let (status, area) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/areas"),
            Some(&json!({"name": "web", "label": "ui"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{area}");
    assert_eq!(
        area,
        json!({"name": "web", "label": "ui", "lead": null, "instructions": null, "model": null, "mcp": null})
    );
    let (status, again) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/areas"),
            Some(&json!({"name": "site", "label": "ui"})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");
    assert_eq!(again["error"]["code"], codes::AREA_EXISTS);
    let (status, missing) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/areas"),
            Some(&json!({"name": "api", "label": "server"})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    let (status, started) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/workers"),
            Some(&json!({"role": "area_lead", "area": "web", "name": "ui-lead"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{started}");
    assert_eq!(started["name"], "ui-lead");
    assert_eq!(started["started_by"], "human:ana");
    assert_eq!(started["cache_ttl"], "1h");
    let (status, task_worker) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/workers"),
            Some(&json!({"role": "task_worker"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{task_worker}");
    until_state(&harness, "ui-lead", WorkerState::Idle).await;
    let (status, stopped) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/workers/ui-lead/stop"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["state"], "stopped");
    let (status, listed) = harness
        .socket(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/workers"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["areas"][0]["lead"], "ui-lead");
    assert_eq!(listed["workers"][0]["turns"], 1);
    assert_eq!(listed["workers"][0]["cache_ttl"], "1h");
    assert_eq!(
        listed["roles"],
        json!([
            {"role": "area_lead", "cache_ttl": "1h", "model": null},
            {"role": "review_lead", "cache_ttl": "1h", "model": null},
            {"role": "task_worker", "cache_ttl": "5m", "model": null},
            {"role": "reviewer", "cache_ttl": "1h", "model": null},
        ])
    );
    let config = std::path::Path::new(harness.root()).join(".fold/config.toml");
    let kept = std::fs::read_to_string(&config).ok();
    std::fs::write(&config, "[roles.task_worker]\nmodel = \"haiku\"\n").unwrap();
    let (_, modelled) = harness
        .socket(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/workers"),
            None,
        )
        .await;
    assert_eq!(modelled["roles"][2]["model"], "haiku");
    assert_eq!(modelled["roles"][0]["model"], Value::Null);
    std::fs::write(&config, "[roles.task_worker]\ncache_ttl = \"2h\"\n").unwrap();
    let (status, unreadable) = harness
        .socket(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/workers"),
            None,
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a config fold cannot read still lists the workers"
    );
    assert_eq!(unreadable["workers"][0]["cache_ttl"], Value::Null);
    assert_eq!(unreadable["roles"], json!([]));
    match kept {
        Some(text) => std::fs::write(&config, text).unwrap(),
        None => std::fs::remove_file(&config).unwrap(),
    }
    let (status, unknown) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/workers/nobody/stop"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{unknown}");

    let (status, resumed) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/workers"),
            Some(&json!({"role": "area_lead", "area": "web"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{resumed}");
    assert_eq!(
        resumed["name"], "ui-lead",
        "a start of the area's stopped lead resumes it"
    );
    let runs = until_runs(&fake, "ui-lead", 2, 2).await;
    assert!(runs[1].has("--resume"));
}

#[tokio::test]
async fn a_reviewer_stops_once_its_batch_review_gives_every_proposal_a_verdict() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    for (number, title) in [(1, "Draw the page"), (2, "Serve the page")] {
        let task = harness.create_task(MAIN, title).await;
        assert_eq!(task, format!("T-{number}"));
        let work = harness.start_work(MAIN, &task).await;
        std::fs::write(work.dir.join(format!("part{number}.txt")), "part\n").unwrap();
        harness.propose(MAIN, &work, title).await;
    }
    harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    harness.connect("review-lead").await;
    let started = harness
        .call(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-1", "P-2"]}),
        )
        .await;
    assert_eq!(started["worker"]["name"], "reviewer");
    assert_eq!(started["worker"]["proposals"], json!(["P-1", "P-2"]));
    let run = until_runs(&fake, "reviewer", 1, 2).await.remove(0);
    assert_eq!(run.env["CLAUDE_CODE_PROMPT_CACHE_TTL"], "1h");
    assert!(run.stdin[1].contains("reviewer for P-1, P-2"));
    assert!(run.stdin[1].contains("in batch rb-1"), "{}", run.stdin[1]);
    let by_lead = harness
        .refused(
            "review-lead",
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    assert_eq!(by_lead["code"], codes::WORKER_START_DENIED);
    until_state(&harness, "reviewer", WorkerState::Idle).await;
    harness.connect("reviewer").await;
    let refused = harness
        .refused(
            "reviewer",
            "create_review_batch",
            json!({"proposals": ["P-1", "P-2"]}),
        )
        .await;
    assert_eq!(refused["code"], codes::WORKER_CLAIM_DENIED);
    let batch = harness
        .call("reviewer", "get_review_batch", json!({"id": "rb-1"}))
        .await;
    assert_eq!(batch["holder"], "reviewer", "{batch}");
    let id = batch["id"].as_str().unwrap().to_owned();
    harness
        .call(
            "reviewer",
            "submit_batch_review",
            json!({
                "id": id,
                "verdicts": [
                    {"proposal": "P-1", "verdict": "request_changes", "body": "split it"},
                    {"proposal": "P-2", "verdict": "request_changes", "body": "name it"}
                ]
            }),
        )
        .await;
    let stopped = until_state(&harness, "reviewer", WorkerState::Stopped).await;
    assert_eq!(stopped.exit_code, Some(0));
    assert_eq!(
        worker(&harness, "review-lead").await.state,
        WorkerState::Idle
    );
}

#[tokio::test]
async fn an_archived_task_stops_its_task_worker_at_once() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    until_state(&harness, "web-worker", WorkerState::Idle).await;
    let held = harness
        .refused(MAIN, "archive_task", json!({"id": "T-1"}))
        .await;
    assert_eq!(
        held["code"],
        codes::NOT_CLAIM_HOLDER,
        "the task worker holds T-1"
    );
    harness
        .call(&lead, "archive_task", json!({"id": "T-1"}))
        .await;
    let stopped = until_state(&harness, "web-worker", WorkerState::Stopped).await;
    assert_eq!(stopped.exit_code, Some(0));
    assert_eq!(worker(&harness, &lead).await.state, WorkerState::Idle);
}

#[tokio::test]
async fn a_restart_that_cannot_read_the_configuration_fails_each_worker_and_goes_on() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let leads = [
        start_lead(&mut harness, "web").await,
        start_lead(&mut harness, "api").await,
    ];
    for lead in &leads {
        until_state(&harness, lead, WorkerState::Idle).await;
    }
    harness.stop().await;
    std::fs::write(
        std::path::Path::new(harness.root()).join(".fold/config.toml"),
        "[workers]\ntask_worker = \"many\"\n",
    )
    .unwrap();
    harness.start_again().await;
    for lead in &leads {
        let failed = until_state(&harness, lead, WorkerState::Failed).await;
        assert_eq!(failed.pid, None);
        assert_eq!(fake.runs_of(lead).len(), 1, "no second process started");
    }
    let reasons: Vec<Value> = stored_events(&harness)
        .await
        .into_iter()
        .filter(|(kind, _)| kind == "worker.failed")
        .map(|(_, data)| data)
        .collect();
    assert_eq!(reasons.len(), 2, "{reasons:?}");
    for (lead, data) in leads.iter().zip(&reasons) {
        assert_eq!(data["agent"], lead.as_str());
        assert_eq!(data["reason"], "config");
    }
}

#[tokio::test]
async fn a_start_gives_the_task_worker_its_claims_or_starts_nothing() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Style the page", "labels": ["ui"]}),
        )
        .await;
    let lead = start_lead(&mut harness, "web").await;
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    let view = harness.call(MAIN, "get_task", json!({"id": "T-1"})).await;
    assert_eq!(view["claim"]["holder"], "web-worker", "{view}");
    let events = stored_events(&harness).await;
    let started = events
        .iter()
        .position(|(kind, data)| kind == "worker.started" && data["agent"] == "web-worker")
        .unwrap();
    let claimed = events
        .iter()
        .position(|(kind, data)| kind == "claim.acquired" && data["worker"] == "web-worker")
        .unwrap();
    assert!(
        started < claimed,
        "the claim follows the start in one write"
    );

    let before = stored_events(&harness).await.len();
    let refused = harness
        .refused(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-3", "T-1"]}),
        )
        .await;
    assert_eq!(refused["code"], codes::ALREADY_CLAIMED, "{refused}");
    assert_eq!(refused["details"]["holder"], "web-worker");
    assert!(
        harness
            .handle
            .agents()
            .get_worker(&name("web-worker-2"))
            .await
            .is_err(),
        "a refused claim starts no worker"
    );
    let unclaimed = harness.call(MAIN, "get_task", json!({"id": "T-3"})).await;
    assert_eq!(unclaimed["claim"], Value::Null, "{unclaimed}");
    let agents = harness.call(MAIN, "list_agents", json!({})).await;
    assert!(
        agents["agents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|agent| agent["name"] != "web-worker-2"),
        "{agents}"
    );
    assert_eq!(stored_events(&harness).await.len(), before);
    assert_eq!(fake.runs_of("web-worker-2").len(), 0);
    let outside = harness
        .refused(&lead, "claim_task", json!({"id": "T-2"}))
        .await;
    assert_eq!(outside["code"], codes::TASK_OUTSIDE_AREA, "{outside}");

    // A workspace create takes the claim on its way, so it is refused for a task the worker was not given.
    until_state(&harness, "web-worker", WorkerState::Idle).await;
    harness.connect("web-worker").await;
    let other = harness
        .refused("web-worker", "create_workspace", json!({"task": "T-3"}))
        .await;
    assert_eq!(other["code"], codes::WORKER_CLAIM_DENIED, "{other}");
    let unclaimed = harness.call(MAIN, "get_task", json!({"id": "T-3"})).await;
    assert_eq!(unclaimed["claim"], Value::Null, "{unclaimed}");
    let own = harness
        .call("web-worker", "create_workspace", json!({"task": "T-1"}))
        .await;
    assert!(own["workspace"].is_string(), "{own}");
}

#[tokio::test]
async fn a_start_takes_over_a_stale_claim_and_adopts_a_released_workspace() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Style the page", "labels": ["ui"]}),
        )
        .await;
    let released = harness.start_work(MAIN, "T-1").await;
    harness
        .call(MAIN, "release_task", json!({"id": "T-1"}))
        .await;
    let stale = harness.start_work(MAIN, "T-3").await;
    let lead = start_lead(&mut harness, "web").await;
    let early = harness
        .refused(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-3"]}),
        )
        .await;
    assert_eq!(early["code"], codes::ALREADY_CLAIMED);
    harness
        .project
        .clock()
        .advance(Duration::from_secs(31 * 60))
        .unwrap();
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1", "T-3"]}),
        )
        .await;
    for (task, work) in [("T-1", &released), ("T-3", &stale)] {
        let view = harness.call(MAIN, "get_task", json!({"id": task})).await;
        assert_eq!(view["claim"]["holder"], "web-worker", "{view}");
        let workspace = harness
            .call(MAIN, "get_workspace", json!({"ws": work.workspace}))
            .await;
        assert_eq!(workspace["worker"], "web-worker", "{workspace}");
    }
    let events = stored_events(&harness).await;
    assert!(events.iter().any(|(kind, data)| kind == "claim.taken_over"
        && data["from"] == MAIN
        && data["to"] == "web-worker"));
    let adopted: Vec<&Value> = events
        .iter()
        .filter(|(kind, _)| kind == "workspace.adopted")
        .map(|(_, data)| data)
        .collect();
    assert_eq!(adopted.len(), 2, "{adopted:?}");
}

#[tokio::test]
async fn a_start_that_names_a_task_twice_is_refused_and_holds_no_workspace() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let work = harness.start_work(MAIN, "T-1").await;
    let lead = start_lead(&mut harness, "web").await;
    let refused = tokio::time::timeout(
        WAIT,
        harness.refused(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1", "T-1"]}),
        ),
    )
    .await
    .expect("the start answers");
    assert_eq!(refused["code"], codes::INVALID_REQUEST, "{refused}");
    assert_eq!(refused["details"]["field"], "tasks");
    // The holder still records in its workspace: the refused start holds no workspace lock.
    std::fs::write(work.dir.join("page.html"), "x\n").unwrap();
    tokio::time::timeout(
        WAIT,
        harness.call(
            MAIN,
            "record_checkpoint",
            json!({"ws": work.workspace, "label": "page"}),
        ),
    )
    .await
    .expect("the checkpoint answers");
    assert!(
        harness
            .handle
            .agents()
            .get_worker(&name("web-worker"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn of_two_starts_for_the_same_tasks_one_gets_them_and_the_other_is_refused() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Style the page", "labels": ["ui"]}),
        )
        .await;
    for task in ["T-1", "T-3"] {
        harness.start_work(MAIN, task).await;
        harness
            .call(MAIN, "release_task", json!({"id": task}))
            .await;
    }
    let lead = start_lead(&mut harness, "web").await;
    let start = |tasks: [&str; 2]| {
        harness.call_raw(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": tasks}),
        )
    };
    let (first, second) = tokio::time::timeout(WAIT, async {
        tokio::join!(start(["T-1", "T-3"]), start(["T-3", "T-1"]))
    })
    .await
    .expect("both starts answer");
    let mut results = [first, second];
    results.sort_by_key(|result| result.is_error);
    assert_eq!(results[0].is_error, Some(false), "{:?}", results[0]);
    assert_eq!(results[1].is_error, Some(true), "{:?}", results[1]);
    let refused = crate::common::acceptance::body(&results[1]);
    assert_eq!(
        refused["error"]["code"],
        codes::ALREADY_CLAIMED,
        "{refused}"
    );
    let started = crate::common::acceptance::body(&results[0]);
    let holder = started["worker"]["name"].as_str().unwrap().to_owned();
    for task in ["T-1", "T-3"] {
        let view = harness.call(MAIN, "get_task", json!({"id": task})).await;
        assert_eq!(view["claim"]["holder"], holder.as_str(), "{view}");
    }
}

#[tokio::test]
async fn a_worker_claims_through_no_path_of_its_own() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Style the page", "labels": ["ui"]}),
        )
        .await;
    let lead = start_lead(&mut harness, "web").await;
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    until_state(&harness, "web-worker", WorkerState::Idle).await;
    harness.connect("web-worker").await;
    give_role(&harness.handle.store, "backend", Role::Reviewer, None).await;
    let denied = |refused: &Value| {
        assert_eq!(refused["code"], codes::WORKER_CLAIM_DENIED, "{refused}");
    };

    denied(
        &harness
            .refused("backend", "claim_task", json!({"id": "T-3"}))
            .await,
    );
    harness
        .call("web-worker", "register_worker", json!({"name": "tests"}))
        .await;
    denied(
        &harness
            .refused(
                "web-worker",
                "claim_task",
                json!({"id": "T-3", "as": "web-worker/tests"}),
            )
            .await,
    );

    // A workspace left by a released claim is adopted only by an agent that may claim the task.
    let left = harness.start_work(MAIN, "T-3").await;
    harness
        .call(MAIN, "release_task", json!({"id": "T-3"}))
        .await;
    denied(
        &harness
            .refused("web-worker", "create_workspace", json!({"task": "T-3"}))
            .await,
    );
    let unclaimed = harness.call(MAIN, "get_task", json!({"id": "T-3"})).await;
    assert_eq!(unclaimed["claim"], Value::Null, "{unclaimed}");
    let orphan = harness
        .call(MAIN, "get_workspace", json!({"ws": left.workspace}))
        .await;
    assert_ne!(orphan["worker"], "web-worker", "{orphan}");

    // A stale claim is taken over only by the lead of the task's area or the main agent.
    harness.call(MAIN, "claim_task", json!({"id": "T-3"})).await;
    harness
        .project
        .clock()
        .advance(Duration::from_secs(31 * 60))
        .unwrap();
    denied(
        &harness
            .refused("web-worker", "takeover_claim", json!({"id": "T-3"}))
            .await,
    );
    let view = harness.call(MAIN, "get_task", json!({"id": "T-3"})).await;
    assert_eq!(view["claim"]["holder"], MAIN, "{view}");
}

#[tokio::test]
async fn a_start_moves_the_leads_own_claim_and_an_ended_workers_claim_but_never_a_running_one() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Style the page", "labels": ["ui"]}),
        )
        .await;
    let lead = start_lead(&mut harness, "web").await;
    let own = harness.start_work(&lead, "T-1").await;
    let taken_over = |events: &[(String, Value)], from: &str, to: &str| {
        events.iter().any(|(kind, data)| {
            kind == "claim.taken_over" && data["from"] == from && data["to"] == to
        })
    };

    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    let view = harness.call(MAIN, "get_task", json!({"id": "T-1"})).await;
    assert_eq!(view["claim"]["holder"], "web-worker", "{view}");
    let workspace = harness
        .call(MAIN, "get_workspace", json!({"ws": own.workspace}))
        .await;
    assert_eq!(workspace["worker"], "web-worker", "{workspace}");
    assert!(taken_over(
        &stored_events(&harness).await,
        &lead,
        "web-worker"
    ));

    until_state(&harness, "web-worker", WorkerState::Idle).await;
    harness
        .call(&lead, "stop_worker", json!({"name": "web-worker"}))
        .await;
    until_state(&harness, "web-worker", WorkerState::Stopped).await;
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    let view = harness.call(MAIN, "get_task", json!({"id": "T-1"})).await;
    assert_eq!(view["claim"]["holder"], "web-worker-2", "{view}");
    assert!(taken_over(
        &stored_events(&harness).await,
        "web-worker",
        "web-worker-2"
    ));

    let refused = harness
        .refused(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    assert_eq!(refused["code"], codes::ALREADY_CLAIMED, "{refused}");
    assert_eq!(refused["details"]["holder"], "web-worker-2");
    assert!(
        refused["hint"].as_str().unwrap().contains("once it ends"),
        "{refused}"
    );
    harness.start_work(MAIN, "T-3").await;
    let held = harness
        .refused(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-3"]}),
        )
        .await;
    assert_eq!(held["code"], codes::ALREADY_CLAIMED, "{held}");
    assert_eq!(held["details"]["holder"], MAIN);
}

#[tokio::test]
async fn a_review_lead_gives_an_ended_reviewers_review_again_and_a_marked_proposal_only_alone() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    for (number, title) in [(1, "Draw the page"), (2, "Serve the page"), (3, "Sign in")] {
        let task = harness.create_task(MAIN, title).await;
        assert_eq!(task, format!("T-{number}"));
    }
    let (status, marked) = harness
        .http(
            Method::PUT,
            &format!("/api/v1/projects/{SLUG}/tasks/T-3/human-review"),
            Some(&json!({"reviewer": null, "reason": "touches sign-in"})),
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{marked}");
    for number in 1..=3 {
        let task = format!("T-{number}");
        let work = harness.start_work(MAIN, &task).await;
        std::fs::write(work.dir.join(format!("part{number}.txt")), "part\n").unwrap();
        harness.propose(MAIN, &work, &task).await;
    }
    harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    harness.connect("review-lead").await;

    let group = harness
        .refused(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-2", "P-3"]}),
        )
        .await;
    assert_eq!(group["code"], codes::INVALID_REQUEST, "{group}");
    assert_eq!(group["details"]["proposal"], "P-3");
    assert!(group["hint"].as_str().unwrap().contains("alone"), "{group}");
    let own = harness
        .refused(
            "review-lead",
            "create_review_batch",
            json!({"proposals": ["P-3", "P-2"]}),
        )
        .await;
    assert_eq!(own["code"], codes::INVALID_REQUEST, "{own}");
    let unclaimed = harness
        .call(MAIN, "get_proposal", json!({"id": "P-2"}))
        .await;
    assert_eq!(unclaimed["claim"], Value::Null, "{unclaimed}");

    harness
        .call(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-1", "P-2"]}),
        )
        .await;
    harness
        .call(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-3"]}),
        )
        .await;
    let running = harness
        .refused(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-3"]}),
        )
        .await;
    assert_eq!(running["code"], codes::ALREADY_CLAIMED, "{running}");
    assert_eq!(running["details"]["holder"], "reviewer-2");

    until_state(&harness, "reviewer", WorkerState::Idle).await;
    harness
        .call("review-lead", "stop_worker", json!({"name": "reviewer"}))
        .await;
    until_state(&harness, "reviewer", WorkerState::Stopped).await;
    let again = harness
        .call(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-1", "P-2"]}),
        )
        .await;
    assert_eq!(again["worker"]["name"], "reviewer-3", "{again}");
    for proposal in ["P-1", "P-2"] {
        let view = harness
            .call(MAIN, "get_proposal", json!({"id": proposal}))
            .await;
        assert_eq!(view["claim"]["holder"], "reviewer-3", "{view}");
    }
    let old = harness
        .call("review-lead", "get_review_batch", json!({"id": "rb-1"}))
        .await;
    assert_eq!(old["state"], "closed", "{old}");
    let new = harness
        .call("review-lead", "get_review_batch", json!({"id": "rb-2"}))
        .await;
    assert_eq!(new["holder"], "reviewer-3", "{new}");
}

#[tokio::test]
async fn a_worker_that_fails_to_launch_gives_its_claims_back() {
    let mut harness = Harness::start_without_roles().await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let started = harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    assert_eq!(started["worker"]["state"], "failed", "{started}");
    let view = harness.call(MAIN, "get_task", json!({"id": "T-1"})).await;
    assert_eq!(view["claim"], Value::Null, "{view}");
    let events = stored_events(&harness).await;
    assert!(events.iter().any(|(kind, data)| kind == "claim.released"
        && data["worker"] == "web-worker"
        && data["reason"] == "worker_failed"));
    let again = harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    assert_eq!(again["worker"]["name"], "web-worker-2", "{again}");
}

#[tokio::test]
async fn a_task_worker_stops_when_its_task_is_done_unless_it_holds_another_claim() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    give_role(&harness.handle.store, "review", Role::ReviewLead, None).await;
    areas(&harness).await;
    for title in ["Style the page", "Print the page"] {
        harness
            .call(
                MAIN,
                "create_task",
                json!({"title": title, "labels": ["ui"]}),
            )
            .await;
    }
    let lead = start_lead(&mut harness, "web").await;
    let human = Actor::human(HumanName::parse("ana").unwrap());
    let mut proposals = Vec::new();
    for (agent, tasks) in [
        ("web-worker", vec!["T-1"]),
        ("web-worker-2", vec!["T-3", "T-4"]),
    ] {
        harness
            .call(
                &lead,
                "start_worker",
                json!({"role": "task_worker", "tasks": tasks}),
            )
            .await;
        until_state(&harness, agent, WorkerState::Idle).await;
        harness.connect(agent).await;
        for task in tasks {
            let work = harness.start_work(agent, task).await;
            std::fs::write(work.dir.join(format!("{task}.html")), "x\n").unwrap();
            proposals.push(harness.propose(agent, &work, "Draw").await);
        }
        until_state(&harness, agent, WorkerState::Stopped).await;
        // A resumed worker runs while its proposals are reviewed.
        harness
            .handle
            .agents()
            .resume_worker(&human, &name(agent), "Wait for the review.".to_owned())
            .await
            .unwrap();
        until_state(&harness, agent, WorkerState::Idle).await;
    }
    let session = worker(&harness, "web-worker").await.session_id;
    let before = stored_events(&harness).await.len();
    // T-3 first: fold handles the events in order, so once the integration of T-1 has stopped web-worker, the one of T-3 has been handled too.
    for proposal in proposals[..2].iter().rev() {
        harness.approve(proposal).await;
        harness
            .call("review", "integrate_proposal", json!({"id": proposal}))
            .await;
    }
    for task in ["T-1", "T-3"] {
        assert_eq!(harness.completions(task).await, 1, "{task} is Done");
    }
    let stopped = until_state(&harness, "web-worker", WorkerState::Stopped).await;
    assert_eq!(stopped.exit_code, Some(0));
    assert_eq!(
        stopped.session_id, session,
        "the session stays for its history"
    );
    let kept = worker(&harness, "web-worker-2").await;
    assert_eq!(
        kept.state,
        WorkerState::Idle,
        "a worker that holds the claim on T-4 keeps running"
    );
    let stops: Vec<Value> = stored_events(&harness).await[before..]
        .iter()
        .filter(|(kind, _)| kind == "worker.stopped")
        .map(|(_, data)| data["agent"].clone())
        .collect();
    assert_eq!(stops, [json!("web-worker")]);
}

/// Points `fake`'s user turns at a copy of the fixture `name` in `dir`, which a test can replace while workers run.
fn writable_turn(fake: &mut FakeClaude, dir: &tempfile::TempDir, name: &str) -> std::path::PathBuf {
    let turn = dir.path().join("turn.jsonl");
    std::fs::copy(crate::workers_support::fixture(name), &turn).unwrap();
    for (key, value) in &mut fake.launcher.env {
        if key == "FOLD_FAKE_CLAUDE_TURN" {
            *value = turn.to_string_lossy().into_owned();
        }
    }
    turn
}

#[tokio::test]
async fn a_task_worker_that_cannot_reach_fold_keeps_its_claim_until_a_daemon_start_resumes_it() {
    let mut fake = FakeClaude::new("mcp_failed");
    let turns = tempfile::TempDir::new().unwrap();
    let turn = writable_turn(&mut fake, &turns, "mcp_failed");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    give_role(
        &harness.handle.store,
        "backend",
        Role::AreaLead,
        Some("web"),
    )
    .await;
    let started = harness
        .call(
            "backend",
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    assert_eq!(started["worker"]["name"], "web-worker", "{started}");

    let failed = until_state(&harness, "web-worker", WorkerState::Failed).await;
    assert_eq!(failed.failed_reason.as_deref(), Some("mcp_unavailable"));
    assert_eq!(
        failed.turns, 0,
        "it failed at the first step of its first turn"
    );
    let view = harness.call(MAIN, "get_task", json!({"id": "T-1"})).await;
    assert_eq!(view["claim"]["holder"], "web-worker", "{view}");
    let events = stored_events(&harness).await;
    assert!(
        !events.iter().any(|(kind, _)| kind == "claim.released"),
        "the task does not turn ready, so no lead hears of it: {events:?}"
    );

    std::fs::copy(crate::workers_support::fixture("result"), &turn).unwrap();
    harness.restart().await;
    let runs = until_runs(&fake, "web-worker", 2, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(failed.session_id.as_str()));
    assert!(
        runs[1].stdin[1].contains(MCP_RETRY_TURN),
        "{:?}",
        runs[1].stdin
    );
    until_state(&harness, "web-worker", WorkerState::Idle).await;
    let view = harness.call(MAIN, "get_task", json!({"id": "T-1"})).await;
    assert_eq!(view["claim"]["holder"], "web-worker", "{view}");
}

#[tokio::test]
async fn a_daemon_start_during_a_pause_leaves_a_worker_that_could_not_reach_fold_to_the_resume() {
    let mut fake = FakeClaude::new("mcp_failed");
    let turns = tempfile::TempDir::new().unwrap();
    let turn = writable_turn(&mut fake, &turns, "mcp_failed");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let failed = until_state(&harness, &lead, WorkerState::Failed).await;
    eventually(WAIT, async || {
        if harness.handle.workers.is_running(&name(&lead)) {
            Err(())
        } else {
            Ok(())
        }
    })
    .await;
    harness.call(MAIN, "pause_project", json!({})).await;
    std::fs::copy(crate::workers_support::fixture("result"), &turn).unwrap();

    harness.restart().await;
    assert_eq!(
        fake.runs_of(&lead).len(),
        1,
        "a paused project runs no worker"
    );
    assert_eq!(worker(&harness, &lead).await.state, WorkerState::Failed);

    harness.call(MAIN, "resume_project", json!({})).await;
    let runs = until_runs(&fake, &lead, 2, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(failed.session_id.as_str()));
    assert!(
        runs[1].stdin[1].contains(MCP_RETRY_TURN)
            && runs[1].stdin[1]
                .contains("These tasks of area web are ready and wait for a task worker: T-1."),
        "the retry turn also names the ready task: {:?}",
        runs[1].stdin
    );
    assert_eq!(user_turns(&runs[1]).len(), 1, "{:?}", runs[1].stdin);
    until_state(&harness, &lead, WorkerState::Idle).await;
}

#[tokio::test]
async fn a_resume_waits_until_the_process_that_could_not_reach_fold_has_ended() {
    let mut fake = FakeClaude::new("mcp_failed").stubborn();
    let turns = tempfile::TempDir::new().unwrap();
    let turn = writable_turn(&mut fake, &turns, "mcp_failed");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    until_state(&harness, &lead, WorkerState::Failed).await;
    let old = fake.runs_of(&lead).pop().unwrap();
    assert!(
        harness.handle.workers.is_running(&name(&lead)),
        "the stand-in ignores the closed stdin and SIGINT"
    );
    std::fs::copy(crate::workers_support::fixture("result"), &turn).unwrap();

    let agents = harness.handle.agents();
    let human = Actor::human(HumanName::parse("ana").unwrap());
    let target = name(&lead);
    let resume = tokio::spawn(async move {
        agents
            .resume_worker(&human, &target, "Go on.".to_owned())
            .await
    });
    let early =
        tokio::time::timeout(Duration::from_millis(300), until_runs(&fake, &lead, 2, 0)).await;
    assert!(early.is_err(), "no second process runs beside the first");
    let clock = harness.project.clock().clone();
    eventually(WAIT, async || {
        if resume.is_finished() {
            Ok(())
        } else {
            clock.advance(Duration::from_secs(6)).unwrap();
            Err(clock.now())
        }
    })
    .await;
    resume.await.unwrap().unwrap();
    assert!(ended(old.pid), "the first process ended before the resume");
    let runs = until_runs(&fake, &lead, 2, 2).await;
    let resumed = until_state(&harness, &lead, WorkerState::Idle).await;
    assert_eq!(
        resumed.pid,
        Some(i64::from(runs[1].pid)),
        "the ended process left the new pid in place"
    );
}

/// The `command` of the fold server in the MCP configuration of `agent`'s latest launch.
fn mcp_command(harness: &Harness, agent: &str) -> String {
    let path = harness
        .project
        .paths()
        .worker_dir(&name(agent))
        .join("mcp.json");
    let config: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    config["mcpServers"]["fold"]["command"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn a_worker_whose_fold_server_failed_ends_failed_and_asks_for_a_restart() {
    let fake = FakeClaude::new("mcp_failed");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let mut inbox = harness.register_inbox(MAIN).await;
    let lead = start_lead(&mut harness, "web").await;

    let row = until_state(&harness, &lead, WorkerState::Failed).await;
    assert_eq!(row.failed_reason.as_deref(), Some("mcp_unavailable"));
    assert_eq!(row.turns, 0, "the turn without fold tools counts no turn");
    let events = stored_events(&harness).await;
    let failed: Vec<&Value> = events
        .iter()
        .filter(|(kind, _)| kind == "worker.failed")
        .map(|(_, data)| data)
        .collect();
    assert_eq!(
        failed,
        [&json!({"agent": "web-lead", "reason": "mcp_unavailable"})]
    );
    let run = fake.runs_of(&lead).pop().unwrap();
    eventually(
        WAIT,
        async || {
            if fake.saw_eof(&run) { Ok(()) } else { Err(()) }
        },
    )
    .await;
    eventually(WAIT, async || {
        if harness.handle.workers.is_running(&name(&lead)) {
            Err(())
        } else {
            Ok(())
        }
    })
    .await;
    assert_eq!(
        worker(&harness, &lead).await.state,
        WorkerState::Failed,
        "the end of the stopped process keeps the failure"
    );

    let notice = loop {
        let post = inbox.next().await;
        if post.contains("worker web-lead failed") {
            break post;
        }
    };
    assert!(
        notice.contains(
            "its fold MCP server did not connect; ask the human to run `fld restart`, which resumes it"
        ),
        "{notice}"
    );

    let (status, list) = harness
        .http(Method::GET, "/api/v1/projects/demo/needs-you", None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let items: Vec<&Value> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == "worker_unreachable")
        .collect();
    assert_eq!(items.len(), 1, "{list}");
    assert_eq!(items[0]["agent"], "web-lead");
    assert_eq!(items[0]["screen"], "worker");
    assert_eq!(
        items[0]["reason"],
        "run `fld restart`; the daemon then resumes the worker"
    );
    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    assert_eq!(listed["workers"][0]["failed_reason"], "mcp_unavailable");
    let (status, agents) = harness
        .http(Method::GET, "/api/v1/projects/demo/agents", None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK, "{agents}");
    let process_of = |agent: &str| {
        agents["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == agent)
            .map(|entry| entry["process"].clone())
            .unwrap()
    };
    assert_eq!(
        process_of("web-lead"),
        json!({
            "state": "failed",
            "failed_reason": "mcp_unavailable",
            "last_error": null,
            "model_unreachable": false,
        })
    );
    assert_eq!(
        process_of(MAIN),
        Value::Null,
        "fold does not run the main agent"
    );
}

#[tokio::test]
async fn a_worker_whose_claude_is_missing_fails_with_the_reason_and_the_fix() {
    let fake = FakeClaude::new("result");
    let missing = tempfile::TempDir::new().unwrap();
    let launcher = Launcher {
        claude: missing.path().join("bin/claude"),
        ..fake.launcher.clone()
    };
    let harness = Harness::start_with_launcher(launcher).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let mut inbox = harness.register_inbox(MAIN).await;
    let started = harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    assert_eq!(started["worker"]["state"], "failed", "{started}");
    let row = worker(&harness, "web-lead").await;
    assert_eq!(row.failed_reason.as_deref(), Some("claude_not_found"));
    let notice = loop {
        let post = inbox.next().await;
        if post.contains("worker web-lead failed") {
            break post;
        }
    };
    assert!(
        notice.contains("the daemon finds no claude") && notice.contains("`fld restart`"),
        "{notice}"
    );
    let (status, list) = harness
        .http(Method::GET, "/api/v1/projects/demo/needs-you", None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let items: Vec<&Value> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["agent"] == "web-lead")
        .collect();
    assert_eq!(items.len(), 1, "{list}");
    assert_eq!(items[0]["kind"], "claude_not_found");
    assert_eq!(items[0]["screen"], "worker");
    let reason = items[0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("fld restart") && reason.contains("fld doctor"),
        "{reason}"
    );
}

#[tokio::test]
async fn a_launch_names_the_fld_path_and_fails_once_no_fld_is_there() {
    let fake = FakeClaude::new("result");
    let fold = fake.launcher.fold.clone().unwrap();
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    until_state(&harness, &lead, WorkerState::Idle).await;
    assert_eq!(mcp_command(&harness, &lead), fold.to_string_lossy());

    // An installer puts the new fld in place by a rename, so the path names the new file.
    let staged = fold.with_file_name(".fld.new");
    std::fs::copy(&fold, &staged).unwrap();
    std::fs::rename(&staged, &fold).unwrap();
    harness
        .call(MAIN, "stop_worker", json!({"name": lead}))
        .await;
    until_state(&harness, &lead, WorkerState::Stopped).await;
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    until_state(&harness, &lead, WorkerState::Idle).await;
    assert_eq!(mcp_command(&harness, &lead), fold.to_string_lossy());
    assert_eq!(fake.runs_of(&lead).len(), 2);

    std::fs::remove_file(&fold).unwrap();
    harness
        .call(MAIN, "stop_worker", json!({"name": lead}))
        .await;
    until_state(&harness, &lead, WorkerState::Stopped).await;
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    let row = until_state(&harness, &lead, WorkerState::Failed).await;
    assert_eq!(row.failed_reason.as_deref(), Some("fld_missing"));
    assert_eq!(fake.runs_of(&lead).len(), 2, "no process ran without fld");
    let (_, list) = harness
        .http(Method::GET, "/api/v1/projects/demo/needs-you", None, "ana")
        .await;
    let item = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "worker_unreachable" && item["agent"] == "web-lead")
        .unwrap_or_else(|| panic!("{list}"));
    assert_eq!(
        item["reason"],
        "install fld and run `fld restart`; the daemon then resumes the worker, or an agent resumes it with `resume_worker`"
    );

    harness.restart().await;
    let still = worker(&harness, &lead).await;
    assert_eq!(
        (still.state, still.failed_reason.as_deref()),
        (WorkerState::Failed, Some("fld_missing")),
        "a daemon start leaves a worker failed while its fld is gone"
    );
    assert_eq!(fake.runs_of(&lead).len(), 2);

    std::fs::write(&fold, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::set_permissions(
        &fold,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .unwrap();
    harness.restart().await;
    let runs = until_runs(&fake, &lead, 3, 2).await;
    assert_eq!(runs[2].flag("--resume"), Some(row.session_id.as_str()));
    assert!(
        runs[2].stdin[1].contains(MCP_RETRY_TURN),
        "{:?}",
        runs[2].stdin
    );
    let resumed = until_state(&harness, &lead, WorkerState::Idle).await;
    assert_eq!(resumed.failed_reason, None);
    let (_, list) = harness
        .http(Method::GET, "/api/v1/projects/demo/needs-you", None, "ana")
        .await;
    assert!(
        !list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "worker_unreachable"),
        "{list}"
    );
}

#[tokio::test]
async fn a_daemon_start_resumes_a_worker_whose_fold_connection_failed_once_per_start() {
    let mut fake = FakeClaude::new("mcp_failed");
    let turns = tempfile::TempDir::new().unwrap();
    let turn = turns.path().join("turn.jsonl");
    std::fs::copy(crate::workers_support::fixture("mcp_failed"), &turn).unwrap();
    for (key, value) in &mut fake.launcher.env {
        if key == "FOLD_FAKE_CLAUDE_TURN" {
            *value = turn.to_string_lossy().into_owned();
        }
    }
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let failed = until_state(&harness, &lead, WorkerState::Failed).await;
    assert_eq!(failed.failed_reason.as_deref(), Some("mcp_unavailable"));
    until_runs(&fake, &lead, 1, 2).await;

    harness.restart().await;
    let runs = until_runs(&fake, &lead, 2, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(failed.session_id.as_str()));
    assert!(
        runs[1].stdin[1].contains(MCP_RETRY_TURN),
        "{:?}",
        runs[1].stdin
    );
    eventually(WAIT, async || {
        if fake.saw_eof(&runs[1]) {
            Ok(())
        } else {
            Err(())
        }
    })
    .await;
    let again = until_state(&harness, &lead, WorkerState::Failed).await;
    assert_eq!(again.failed_reason.as_deref(), Some("mcp_unavailable"));
    eventually(WAIT, async || {
        if harness.handle.workers.is_running(&name(&lead)) {
            Err(())
        } else {
            Ok(())
        }
    })
    .await;
    assert_eq!(
        fake.runs_of(&lead).len(),
        2,
        "a failure on the resumed turn waits for the next start"
    );

    std::fs::copy(crate::workers_support::fixture("result"), &turn).unwrap();
    harness.restart().await;
    let runs = until_runs(&fake, &lead, 3, 2).await;
    assert_eq!(runs[2].flag("--resume"), Some(failed.session_id.as_str()));
    let resumed = until_state(&harness, &lead, WorkerState::Idle).await;
    assert_eq!(resumed.failed_reason, None);
    let (_, list) = harness
        .http(Method::GET, "/api/v1/projects/demo/needs-you", None, "ana")
        .await;
    assert!(
        !list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "worker_unreachable"),
        "{list}"
    );
}

#[tokio::test]
async fn a_worker_whose_turns_reach_no_model_shows_the_error_and_asks_a_human() {
    let mut fake = FakeClaude::new("error_result");
    let turns = tempfile::TempDir::new().unwrap();
    let turn = writable_turn(&mut fake, &turns, "error_result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let mut inbox = harness.register_inbox(MAIN).await;
    let lead = start_lead(&mut harness, "web").await;
    let error = "No conversation found with session ID: 00000000-0000-4000-8000-000000000074";
    let until_failed_turns = async |count: u64| {
        eventually(WAIT, async || {
            let row = worker(&harness, &lead).await;
            if row.failed_turns == count && row.state == WorkerState::Idle {
                Ok(row)
            } else {
                Err((row.failed_turns, row.state))
            }
        })
        .await
    };
    let first = until_failed_turns(1).await;
    assert_eq!(first.last_error.as_deref(), Some(error));
    assert!(first.last_error_at.is_some());
    assert_eq!(first.turns, 1, "a failed turn is still a turn");
    let unreachable_events = async || {
        stored_events(&harness)
            .await
            .into_iter()
            .filter(|(kind, _)| kind == "worker.model_unreachable")
            .map(|(_, data)| data)
            .collect::<Vec<_>>()
    };
    assert!(unreachable_events().await.is_empty());
    let nudge = async || {
        harness
            .call(
                MAIN,
                "send_message",
                json!({"to": [&lead], "body": "go on"}),
            )
            .await;
    };
    nudge().await;
    until_failed_turns(2).await;
    nudge().await;
    until_failed_turns(3).await;
    assert_eq!(
        unreachable_events().await,
        [json!({"agent": "web-lead", "turns": 3})]
    );
    let notice = loop {
        let post = inbox.next().await;
        if post.contains("worker web-lead cannot reach the model") {
            break post;
        }
    };
    assert!(notice.contains("run `fld doctor`"), "{notice}");
    assert!(!notice.contains("No conversation"), "{notice}");

    let (status, list) = harness
        .http(Method::GET, "/api/v1/projects/demo/needs-you", None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let items: Vec<&Value> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["kind"] == "worker_model_unreachable")
        .collect();
    assert_eq!(items.len(), 1, "{list}");
    assert_eq!(items[0]["agent"], "web-lead");
    assert_eq!(items[0]["screen"], "worker");
    assert_eq!(items[0]["title"], error);
    assert_eq!(
        items[0]["reason"],
        "check the Claude Code sign-in, model and usage limit, or run `fld doctor`"
    );
    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    let entry = &listed["workers"][0];
    assert_eq!(entry["last_error"], error);
    assert_eq!(entry["failed_turns"], 3);
    assert_eq!(
        entry["hint"],
        "check the Claude Code sign-in, model and usage limit, or run `fld doctor`"
    );
    let (status, workers) = harness
        .http(Method::GET, "/api/v1/projects/demo/workers", None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK, "{workers}");
    let entry = &workers["workers"][0];
    assert_eq!(
        (
            &entry["last_error"],
            &entry["failed_turns"],
            &entry["model_unreachable"]
        ),
        (&json!(error), &json!(3), &json!(true))
    );
    assert!(entry["last_error_at"].is_i64());
    let (_, agents) = harness
        .http(Method::GET, "/api/v1/projects/demo/agents", None, "ana")
        .await;
    let process = agents["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["name"] == "web-lead")
        .map(|agent| agent["process"].clone())
        .unwrap();
    assert_eq!(
        process,
        json!({"state": "idle", "failed_reason": null, "last_error": error, "model_unreachable": true})
    );

    nudge().await;
    until_failed_turns(4).await;
    assert_eq!(
        unreachable_events().await.len(),
        1,
        "the push comes once for a run of failed turns"
    );

    std::fs::copy(crate::workers_support::fixture("result"), &turn).unwrap();
    nudge().await;
    let healed = until_failed_turns(0).await;
    assert_eq!((healed.last_error, healed.last_error_at), (None, None));
    let (_, list) = harness
        .http(Method::GET, "/api/v1/projects/demo/needs-you", None, "ana")
        .await;
    assert!(
        !list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "worker_model_unreachable"),
        "{list}"
    );
}

#[tokio::test]
async fn a_process_that_exits_with_an_error_leaves_its_last_stderr_lines() {
    let fake = FakeClaude::new("result").failing_with_stderr("Error: no access to the model");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let failed = until_state(&harness, &lead, WorkerState::Failed).await;
    assert_eq!(failed.failed_reason.as_deref(), Some("exited"));
    assert_eq!(failed.exit_code, Some(1));
    assert_eq!(
        failed.last_error.as_deref(),
        Some("starting\nError: no access to the model")
    );
    assert_eq!(failed.failed_turns, 0, "no turn ended");
    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    assert_eq!(
        listed["workers"][0]["last_error"],
        "starting\nError: no access to the model"
    );
    assert!(listed["workers"][0].get("hint").is_none());
}

#[tokio::test]
async fn a_synthetic_error_message_never_becomes_the_workers_model() {
    let fake = FakeClaude::new("synthetic_error");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let failed = eventually(WAIT, async || {
        let row = worker(&harness, &lead).await;
        if row.failed_turns == 1 && row.state == WorkerState::Idle {
            Ok(row)
        } else {
            Err((row.failed_turns, row.state))
        }
    })
    .await;
    assert_eq!(failed.model.as_deref(), Some("claude-haiku-5-5"));
    assert_eq!(
        failed.last_error.as_deref(),
        Some("Not logged in · Please run /login")
    );
    harness
        .call(MAIN, "stop_worker", json!({"name": lead}))
        .await;
    until_state(&harness, &lead, WorkerState::Stopped).await;
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    let runs = until_runs(&fake, &lead, 2, 2).await;
    assert_eq!(runs[1].flag("--model"), Some("claude-haiku-5-5"));
}

#[tokio::test]
async fn a_row_that_holds_a_synthetic_model_launches_without_it_and_forgets_it() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    until_state(&harness, &lead, WorkerState::Idle).await;
    harness
        .call(MAIN, "stop_worker", json!({"name": lead}))
        .await;
    until_state(&harness, &lead, WorkerState::Stopped).await;
    fold_store::testing::execute(
        &harness.handle.store,
        "UPDATE worker_processes SET model = '<synthetic>'",
    )
    .await;
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    let runs = until_runs(&fake, &lead, 2, 2).await;
    assert!(!runs[1].has("--model"), "{:?}", runs[1].args);
    let healed = eventually(WAIT, async || {
        let row = worker(&harness, &lead).await;
        if row.state == WorkerState::Idle && row.turns == 2 {
            Ok(row)
        } else {
            Err((row.state, row.turns))
        }
    })
    .await;
    assert_eq!(
        healed.model.as_deref(),
        Some("claude-haiku-5-5"),
        "the row holds the model that answered"
    );
}

#[tokio::test]
async fn a_start_for_an_area_whose_lead_ended_resumes_that_lead_and_says_the_name_went_unused() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let idle = until_state(&harness, &lead, WorkerState::Idle).await;
    let start = json!({"role": "area_lead", "area": "web", "name": "web-lead2"});
    let refused = harness.refused(MAIN, "start_worker", start.clone()).await;
    assert_eq!(refused["code"], codes::AREA_HAS_LEAD, "{refused}");

    harness
        .call(MAIN, "stop_worker", json!({"name": lead}))
        .await;
    until_state(&harness, &lead, WorkerState::Stopped).await;
    let resumed = harness.call(MAIN, "start_worker", start).await;
    assert_eq!(resumed["worker"]["name"], lead.as_str(), "{resumed}");
    assert_eq!(resumed["resumed"], true, "{resumed}");
    assert_eq!(resumed["session"], idle.session_id.as_str(), "{resumed}");
    assert_eq!(resumed["unused_name"], "web-lead2", "{resumed}");
    let hint = resumed["hint"].as_str().unwrap();
    assert!(
        hint.contains("fold resumed web-lead, the existing lead (turns so far: 1), in its session")
            && hint
                .contains("or in a new session with its hand-off note when that session is gone")
            && hint.contains("did not use the name web-lead2")
            && hint.contains("send_message"),
        "{hint}"
    );
    let runs = until_runs(&fake, &lead, 2, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(idle.session_id.as_str()));
    let agents = harness.call(MAIN, "list_agents", json!({})).await;
    assert!(
        !agents.to_string().contains("web-lead2"),
        "no agent of the unused name: {agents}"
    );

    until_state(&harness, &lead, WorkerState::Idle).await;
    harness
        .call(MAIN, "stop_worker", json!({"name": lead}))
        .await;
    until_state(&harness, &lead, WorkerState::Stopped).await;
    let unnamed = harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    assert_eq!(unnamed["resumed"], true, "{unnamed}");
    assert!(unnamed.get("unused_name").is_none(), "{unnamed}");
    assert!(
        !unnamed["hint"].as_str().unwrap().contains("did not use"),
        "{unnamed}"
    );
    let fresh = harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    assert!(fresh.get("resumed").is_none(), "{fresh}");
}

/// The user turns among the stdin lines of `run`.
fn user_turns(run: &Run) -> Vec<&String> {
    run.stdin
        .iter()
        .filter(|line| line.contains(r#""type":"user""#))
        .collect()
}

#[tokio::test]
async fn a_task_made_ready_while_its_lead_could_not_reach_fold_gets_a_worker_after_the_daemon_start()
 {
    let mut fake = FakeClaude::new("mcp_failed");
    let turns = tempfile::TempDir::new().unwrap();
    let turn = writable_turn(&mut fake, &turns, "mcp_failed");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    until_state(&harness, &lead, WorkerState::Failed).await;
    eventually(WAIT, async || {
        if harness.handle.workers.is_running(&name(&lead)) {
            Err(())
        } else {
            Ok(())
        }
    })
    .await;
    let created = harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Style the page", "labels": ["ui"]}),
        )
        .await;
    assert_eq!(created["id"], "T-3", "{created}");

    std::fs::copy(crate::workers_support::fixture("result"), &turn).unwrap();
    harness.restart().await;
    until_runs(&fake, &lead, 2, 2).await;
    until_state(&harness, &lead, WorkerState::Idle).await;
    let runs = fake.runs_of(&lead);
    let resumed = user_turns(&runs[1]);
    assert_eq!(resumed.len(), 1, "one turn: {resumed:?}");
    assert!(resumed[0].contains(MCP_RETRY_TURN), "{}", resumed[0]);
    assert!(
        resumed[0]
            .contains("These tasks of area web are ready and wait for a task worker: T-1, T-3."),
        "{}",
        resumed[0]
    );

    harness.connect(&lead).await;
    let started = harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-3"]}),
        )
        .await;
    assert_eq!(started["worker"]["name"], "web-worker", "{started}");
    let view = harness.call(MAIN, "get_task", json!({"id": "T-3"})).await;
    assert_eq!(view["claim"]["holder"], "web-worker", "{view}");
}

#[tokio::test]
async fn an_idle_lead_gets_a_turn_at_a_daemon_start_only_when_ready_tasks_wait_for_it() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    until_state(&harness, &lead, WorkerState::Idle).await;

    harness.stop().await;
    harness.start_again().await;
    until_runs(&fake, &lead, 2, 2).await;
    let idle = until_state(&harness, &lead, WorkerState::Idle).await;
    assert_eq!(idle.turns, 2);
    let runs = fake.runs_of(&lead);
    let caught_up = user_turns(&runs[1]);
    assert_eq!(caught_up.len(), 1, "{caught_up:?}");
    assert!(
        caught_up[0].contains("These tasks of area web are ready and wait for a task worker: T-1."),
        "{}",
        caught_up[0]
    );
    assert!(!caught_up[0].contains(RESTART_TURN), "{}", caught_up[0]);

    harness.connect(&lead).await;
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    until_state(&harness, "web-worker", WorkerState::Idle).await;
    harness.stop().await;
    harness.start_again().await;
    let row = worker(&harness, &lead).await;
    assert_eq!(
        (row.state, row.turns),
        (WorkerState::Idle, 2),
        "nothing waits for the lead, so it gets no turn"
    );
    let runs = until_runs(&fake, &lead, 3, 1).await;
    assert!(user_turns(&runs[2]).is_empty(), "{:?}", runs[2].stdin);
}

#[tokio::test]
async fn a_pause_resume_names_the_tasks_that_turned_ready_in_the_same_turn() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    until_state(&harness, &lead, WorkerState::Idle).await;
    harness.call(MAIN, "pause_project", json!({})).await;
    eventually(WAIT, async || {
        if harness.handle.workers.is_running(&name(&lead)) {
            Err(())
        } else {
            Ok(())
        }
    })
    .await;
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Style the page", "labels": ["ui"]}),
        )
        .await;

    harness.call(MAIN, "resume_project", json!({})).await;
    until_runs(&fake, &lead, 2, 2).await;
    let idle = until_state(&harness, &lead, WorkerState::Idle).await;
    assert_eq!(idle.turns, 2, "one turn after the resume");
    let runs = fake.runs_of(&lead);
    let resumed = user_turns(&runs[1]);
    assert_eq!(resumed.len(), 1, "{resumed:?}");
    assert!(resumed[0].contains(PAUSE_TURN), "{}", resumed[0]);
    assert!(
        resumed[0].contains("wait for a task worker: T-1, T-3."),
        "{}",
        resumed[0]
    );
}

#[tokio::test]
async fn a_review_lead_hears_at_a_daemon_start_of_the_proposals_that_wait_for_a_review() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let started = harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    let review_lead = started["worker"]["name"].as_str().unwrap().to_owned();
    until_state(&harness, &review_lead, WorkerState::Idle).await;
    let lead = start_lead(&mut harness, "web").await;
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    until_state(&harness, "web-worker", WorkerState::Idle).await;
    harness.connect("web-worker").await;
    let work = harness.start_work("web-worker", "T-1").await;
    std::fs::write(work.dir.join("page.html"), "<p>page</p>\n").unwrap();
    let proposal = harness.propose("web-worker", &work, "Draw the page").await;
    until_state(&harness, "web-worker", WorkerState::Stopped).await;
    until_state(&harness, &review_lead, WorkerState::Idle).await;
    until_state(&harness, &lead, WorkerState::Idle).await;
    let before = worker(&harness, &lead).await.turns;

    harness.restart().await;
    let runs = until_runs(&fake, &review_lead, 2, 2).await;
    let caught_up = user_turns(&runs[1]);
    assert_eq!(caught_up.len(), 1, "{caught_up:?}");
    assert!(
        caught_up[0].contains(&format!(
            "These open proposals wait for a review: {proposal}."
        )),
        "{}",
        caught_up[0]
    );
    let row = worker(&harness, &lead).await;
    assert_eq!(
        (row.state, row.turns),
        (WorkerState::Idle, before),
        "T-1 is in review, so nothing waits for its area lead"
    );
}

#[tokio::test]
async fn a_catch_up_names_in_ready_order_only_the_ready_tasks_of_the_area_that_no_live_worker_holds()
 {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    until_state(&harness, &lead, WorkerState::Idle).await;
    for task in [
        json!({"title": "Check the page on a phone", "labels": ["ui"], "kind": "human"}),
        json!({"title": "Pick the font", "labels": ["ui"]}),
        json!({"title": "Size the logo", "labels": ["ui"]}),
        json!({"title": "Name the page"}),
        json!({"title": "Draw the icon", "labels": ["ui"]}),
        json!({"title": "Draw the footer", "labels": ["ui"]}),
        json!({"title": "Draw the header", "labels": ["ui"]}),
    ] {
        harness.call(MAIN, "create_task", task).await;
    }
    harness
        .call(
            MAIN,
            "set_blocker",
            json!({"id": "T-4", "reason": "Wait for the brand colours"}),
        )
        .await;
    harness
        .call(MAIN, "archive_task", json!({"id": "T-5"}))
        .await;
    harness
        .call(
            MAIN,
            "add_dependency",
            json!({"task": "T-9", "needs": "T-8"}),
        )
        .await;
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    until_state(&harness, "web-worker", WorkerState::Idle).await;
    until_state(&harness, &lead, WorkerState::Idle).await;
    let before = fake.runs_of(&lead).len();

    harness.stop().await;
    harness.start_again().await;
    let runs = until_runs(&fake, &lead, before + 1, 2).await;
    let caught_up = user_turns(&runs[before]);
    assert_eq!(caught_up.len(), 1, "{caught_up:?}");
    assert!(
        caught_up[0]
            .contains("These tasks of area web are ready and wait for a task worker: T-8, T-7."),
        "T-8 frees T-9, so it comes first; the running worker's T-1, the human T-3, the blocked T-4, the archived T-5, T-6 in no area and T-2 of another area are left out: {}",
        caught_up[0]
    );
}

#[tokio::test]
async fn a_claim_of_a_stopped_worker_hides_no_work_from_the_catch_up_and_a_live_one_does() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let started = harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    let review_lead = started["worker"]["name"].as_str().unwrap().to_owned();
    until_state(&harness, &review_lead, WorkerState::Idle).await;
    harness.connect(&review_lead).await;
    let lead = start_lead(&mut harness, "web").await;
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Draw the footer", "labels": ["ui"]}),
        )
        .await;
    let mut proposals = Vec::new();
    for (worker, task) in [("web-worker", "T-1"), ("web-worker-2", "T-3")] {
        harness
            .call(
                &lead,
                "start_worker",
                json!({"role": "task_worker", "tasks": [task]}),
            )
            .await;
        until_state(&harness, worker, WorkerState::Idle).await;
        harness.connect(worker).await;
        let work = harness.start_work(worker, task).await;
        std::fs::write(work.dir.join(format!("{task}.html")), "<p>page</p>\n").unwrap();
        proposals.push(harness.propose(worker, &work, "Draw the page").await);
        until_state(&harness, worker, WorkerState::Stopped).await;
    }
    for (reviewer, proposal) in [("reviewer", &proposals[0]), ("reviewer-2", &proposals[1])] {
        let given = harness
            .call(
                &review_lead,
                "start_worker",
                json!({"role": "reviewer", "proposals": [proposal]}),
            )
            .await;
        assert_eq!(given["worker"]["name"], reviewer, "{given}");
        until_state(&harness, reviewer, WorkerState::Idle).await;
    }
    harness
        .call(&review_lead, "stop_worker", json!({"name": "reviewer"}))
        .await;
    until_state(&harness, "reviewer", WorkerState::Stopped).await;
    let held = harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Draw the header", "labels": ["ui"]}),
        )
        .await;
    assert_eq!(held["id"], "T-4", "{held}");
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-4"]}),
        )
        .await;
    until_state(&harness, "web-worker-3", WorkerState::Idle).await;
    harness
        .call(&lead, "stop_worker", json!({"name": "web-worker-3"}))
        .await;
    until_state(&harness, "web-worker-3", WorkerState::Stopped).await;
    let view = harness.call(MAIN, "get_task", json!({"id": "T-4"})).await;
    assert_eq!(
        (&view["status"], &view["claim"]["holder"]),
        (&json!("todo"), &json!("web-worker-3")),
        "{view}"
    );
    until_state(&harness, &lead, WorkerState::Idle).await;
    until_state(&harness, &review_lead, WorkerState::Idle).await;
    let lead_runs = fake.runs_of(&lead).len();
    let review_runs = fake.runs_of(&review_lead).len();

    harness.restart().await;
    let runs = until_runs(&fake, &review_lead, review_runs + 1, 2).await;
    let caught_up = user_turns(&runs[review_runs]);
    assert_eq!(caught_up.len(), 1, "{caught_up:?}");
    assert!(
        caught_up[0].contains(&format!(
            "These open proposals wait for a review: {}.",
            proposals[0]
        )),
        "the stopped reviewer's proposal waits, the running reviewer-2 keeps the other: {}",
        caught_up[0]
    );
    let runs = until_runs(&fake, &lead, lead_runs + 1, 2).await;
    let caught_up = user_turns(&runs[lead_runs]);
    assert_eq!(caught_up.len(), 1, "{caught_up:?}");
    assert!(
        caught_up[0].contains("These tasks of area web are ready and wait for a task worker: T-4."),
        "{}",
        caught_up[0]
    );

    harness.connect(&lead).await;
    let moved = harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-4"]}),
        )
        .await;
    assert_eq!(moved["worker"]["name"], "web-worker-4", "{moved}");
    let view = harness.call(MAIN, "get_task", json!({"id": "T-4"})).await;
    assert_eq!(view["claim"]["holder"], "web-worker-4", "{view}");
}
