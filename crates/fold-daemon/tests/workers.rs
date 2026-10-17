#![cfg(test)]

use crate::common::roles::give_role;
use fold_core::roles::Role;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::{Actor, AgentActor, AgentName, Clock, HumanName, WorkerName, WorkerState, codes};
use fold_daemon::services::agents::RESTART_TURN;
use fold_daemon::services::events::BusMessage;
use fold_store::agents::WorkerProcessRow;
use fold_store::testing::eventually;
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
async fn an_area_lead_starts_coders_only_for_tasks_of_its_own_area() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    let started = harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_coder", "tasks": ["T-1"], "instructions": "Keep the page small."}),
        )
        .await;
    assert_eq!(started["worker"]["name"], "web-coder");
    assert_eq!(started["worker"]["area"], "web");
    assert_eq!(started["worker"]["tasks"], json!(["T-1"]));

    let outside = harness
        .refused(
            &lead,
            "start_worker",
            json!({"role": "task_coder", "tasks": ["T-2"]}),
        )
        .await;
    assert_eq!(outside["code"], codes::TASK_OUTSIDE_AREA);
    let other_area = harness
        .refused(
            &lead,
            "start_worker",
            json!({"role": "task_coder", "area": "api", "tasks": ["T-2"]}),
        )
        .await;
    assert_eq!(other_area["code"], codes::WORKER_START_DENIED);
    let by_main = harness
        .refused(
            MAIN,
            "start_worker",
            json!({"role": "task_coder", "tasks": ["T-1"]}),
        )
        .await;
    assert_eq!(by_main["code"], codes::WORKER_START_DENIED);
    harness.connect("web-coder").await;
    let by_coder = harness
        .refused(
            "web-coder",
            "start_worker",
            json!({"role": "task_coder", "tasks": ["T-1"]}),
        )
        .await;
    assert_eq!(by_coder["code"], codes::WORKER_START_DENIED);
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
    let coder_run = until_runs(&fake, "web-coder", 1, 2).await.remove(0);
    for (run, ttl) in [(&lead_run, "1h"), (&coder_run, "5m")] {
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
    let coder = worker(&harness, "web-coder").await;
    assert_eq!(
        coder_run.flag("--session-id"),
        Some(coder.session_id.as_str())
    );
    assert!(coder_run.stdin[0].contains("\"initialize\""));
    assert!(coder_run.stdin[1].contains("task coder for T-1"));
    assert!(coder_run.stdin[1].contains("Keep the page small."));
    let prompt =
        std::fs::read_to_string(coder_run.flag("--append-system-prompt-file").unwrap()).unwrap();
    assert!(prompt.contains("never edit the project's main files"));
    let config: Value = serde_json::from_str(
        &std::fs::read_to_string(coder_run.flag("--mcp-config").unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        config,
        json!({"mcpServers": {"fold": {"command": "/usr/local/bin/fld", "args": ["mcp"], "env": {"FOLD_AGENT": "web-coder"}}}})
    );
    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    assert_eq!(listed["areas"][1]["lead"], "web-lead");
    let names: Vec<&str> = listed["workers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|worker| worker["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["web-lead", "web-coder"]);
    let ttls: Vec<&str> = listed["workers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|worker| worker["cache_ttl"].as_str().unwrap())
        .collect();
    assert_eq!(ttls, ["1h", "5m"]);
}

#[tokio::test]
async fn a_task_coder_stops_once_its_task_has_an_open_proposal() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_coder", "tasks": ["T-1"]}),
        )
        .await;
    until_state(&harness, "web-coder", WorkerState::Idle).await;
    harness.connect("web-coder").await;
    let work = harness.start_work("web-coder", "T-1").await;
    std::fs::write(work.dir.join("page.html"), "<p>page</p>\n").unwrap();
    harness.propose("web-coder", &work, "Draw the page").await;
    let stopped = until_state(&harness, "web-coder", WorkerState::Stopped).await;
    assert_eq!(stopped.exit_code, Some(0));
    assert_eq!(stopped.turns, 1);
    assert_eq!(worker(&harness, &lead).await.state, WorkerState::Idle);
    let events = stored_events(&harness).await;
    let stopped_event = events
        .iter()
        .find(|(kind, _)| kind == "worker.stopped")
        .unwrap();
    assert_eq!(stopped_event.1["agent"], "web-coder");
    assert_eq!(stopped_event.1["exit_code"], 0);
    assert!(!harness.handle.workers.is_running(&name("web-coder")));
}

#[tokio::test]
async fn a_coder_stops_when_its_last_proposal_opens_after_an_earlier_one_integrated() {
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
            json!({"role": "task_coder", "tasks": ["T-1", "T-3"]}),
        )
        .await;
    until_state(&harness, "web-coder", WorkerState::Idle).await;
    harness.connect("web-coder").await;
    let first = harness.start_work("web-coder", "T-1").await;
    std::fs::write(first.dir.join("page.html"), "<p>page</p>\n").unwrap();
    let proposal = harness.propose("web-coder", &first, "Draw the page").await;
    harness.approve(&proposal).await;
    harness
        .call("review", "integrate_proposal", json!({"id": proposal}))
        .await;
    let waiting = worker(&harness, "web-coder").await.state;
    assert!(
        matches!(waiting, WorkerState::Idle | WorkerState::Working),
        "T-3 still waits for its proposal: {waiting}"
    );
    let second = harness.start_work("web-coder", "T-3").await;
    std::fs::write(second.dir.join("style.css"), "p {}\n").unwrap();
    harness
        .propose("web-coder", &second, "Style the page")
        .await;
    let stopped = until_state(&harness, "web-coder", WorkerState::Stopped).await;
    assert_eq!(stopped.exit_code, Some(0));
}

#[tokio::test]
async fn an_idle_coder_stops_after_its_idle_minutes_and_resumes_with_a_turn() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_coder", "tasks": ["T-1"]}),
        )
        .await;
    let idle = until_state(&harness, "web-coder", WorkerState::Idle).await;
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
    assert_eq!(late, [name("web-coder")]);
    let stopped = worker(&harness, "web-coder").await;
    assert_eq!(stopped.state, WorkerState::Stopped);
    assert_eq!(stopped.stopped_at, Some(clock.now()));
    assert_eq!(
        worker(&harness, &lead).await.state,
        WorkerState::Idle,
        "a lead never stops for being idle"
    );

    let itself = Actor::Agent(AgentActor {
        agent: name("web-coder"),
        worker: WorkerName::parse("web-coder").unwrap(),
        tags: Vec::new(),
    });
    let denied = harness
        .handle
        .agents()
        .resume_worker(&itself, &name("web-coder"), "go on".to_owned())
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
            &name("web-coder"),
            "Your proposal needs a second look.".to_owned(),
        )
        .await
        .unwrap();
    assert!(resumed.state.is_running());
    let runs = until_runs(&fake, "web-coder", 2, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(stopped.session_id.as_str()));
    assert!(!runs[1].has("--session-id"));
    assert!(runs[1].stdin[1].contains("Your proposal needs a second look."));
    let again = until_state(&harness, "web-coder", WorkerState::Idle).await;
    assert_eq!(again.turns, 2);
    assert!(again.cost_micro_usd > idle.cost_micro_usd);
    let running = harness
        .handle
        .agents()
        .resume_worker(&human, &name("web-coder"), "again".to_owned())
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
async fn the_fifth_task_coder_and_the_fourth_batch_reviewer_are_refused_and_a_lead_never_is() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let task = harness.create_task(MAIN, "Write the docs").await;
    let work = harness.start_work("backend", &task).await;
    std::fs::write(work.dir.join("docs.txt"), "docs\n").unwrap();
    let proposal = harness.propose("backend", &work, "Write the docs").await;
    let lead = start_lead(&mut harness, "web").await;
    let coder = json!({"role": "task_coder", "tasks": ["T-1"]});
    for _ in 0..4 {
        harness.call(&lead, "start_worker", coder.clone()).await;
    }
    let refused = harness.refused(&lead, "start_worker", coder.clone()).await;
    assert_eq!(refused["code"], codes::WORKER_LIMIT_REACHED);
    assert_eq!(refused["details"], json!({"role": "task_coder", "max": 4}));
    assert!(
        refused["hint"]
            .as_str()
            .unwrap()
            .contains("task_coder limit is 4"),
        "{refused}"
    );
    assert!(
        harness
            .handle
            .agents()
            .get_worker(&name("web-coder-5"))
            .await
            .is_err()
    );

    start_lead(&mut harness, "api").await;
    let review = harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    assert_eq!(review["worker"]["name"], "review-lead");
    harness.connect("review-lead").await;
    let reviewer = json!({"role": "batch_reviewer", "proposals": [proposal]});
    for _ in 0..3 {
        harness
            .call("review-lead", "start_worker", reviewer.clone())
            .await;
    }
    let full = harness
        .refused("review-lead", "start_worker", reviewer)
        .await;
    assert_eq!(full["code"], codes::WORKER_LIMIT_REACHED);
    assert_eq!(full["details"], json!({"role": "batch_reviewer", "max": 3}));
    assert!(
        full["hint"]
            .as_str()
            .unwrap()
            .contains("batch_reviewer limit is 3"),
        "{full}"
    );

    harness
        .call(&lead, "stop_worker", json!({"name": "web-coder"}))
        .await;
    let fifth = harness.call(&lead, "start_worker", coder).await;
    assert_eq!(fifth["worker"]["name"], "web-coder-5");
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

/// Whether `pid` is gone or only waits to be reaped.
fn ended(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
        stat.rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
    })
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
    let (status, coder) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/workers"),
            Some(&json!({"role": "task_coder"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{coder}");
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
            {"role": "task_coder", "cache_ttl": "5m", "model": null},
            {"role": "batch_reviewer", "cache_ttl": "1h", "model": null},
        ])
    );
    let config = std::path::Path::new(harness.root()).join(".fold/config.toml");
    let kept = std::fs::read_to_string(&config).ok();
    std::fs::write(&config, "[roles.task_coder]\nmodel = \"haiku\"\n").unwrap();
    let (_, modelled) = harness
        .socket(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/workers"),
            None,
        )
        .await;
    assert_eq!(modelled["roles"][2]["model"], "haiku");
    assert_eq!(modelled["roles"][0]["model"], Value::Null);
    std::fs::write(&config, "[roles.task_coder]\ncache_ttl = \"2h\"\n").unwrap();
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
async fn a_batch_reviewer_stops_once_its_batch_review_gives_every_proposal_a_verdict() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    for (number, title) in [(1, "Draw the page"), (2, "Serve the page")] {
        let task = harness.create_task(MAIN, title).await;
        assert_eq!(task, format!("T-{number}"));
        let work = harness.start_work("backend", &task).await;
        std::fs::write(work.dir.join(format!("part{number}.txt")), "part\n").unwrap();
        harness.propose("backend", &work, title).await;
    }
    harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    harness.connect("review-lead").await;
    let started = harness
        .call(
            "review-lead",
            "start_worker",
            json!({"role": "batch_reviewer", "proposals": ["P-1", "P-2"]}),
        )
        .await;
    assert_eq!(started["worker"]["name"], "batch-reviewer");
    assert_eq!(started["worker"]["proposals"], json!(["P-1", "P-2"]));
    let run = until_runs(&fake, "batch-reviewer", 1, 2).await.remove(0);
    assert_eq!(run.env["CLAUDE_CODE_PROMPT_CACHE_TTL"], "1h");
    assert!(run.stdin[1].contains("batch reviewer for P-1, P-2"));
    let by_lead = harness
        .refused(
            "review-lead",
            "start_worker",
            json!({"role": "task_coder", "tasks": ["T-1"]}),
        )
        .await;
    assert_eq!(by_lead["code"], codes::WORKER_START_DENIED);
    until_state(&harness, "batch-reviewer", WorkerState::Idle).await;
    harness.connect("batch-reviewer").await;
    let batch = harness
        .call(
            "batch-reviewer",
            "create_review_batch",
            json!({"proposals": ["P-1", "P-2"]}),
        )
        .await;
    let id = batch["id"].as_str().unwrap().to_owned();
    harness
        .call(
            "batch-reviewer",
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
    let stopped = until_state(&harness, "batch-reviewer", WorkerState::Stopped).await;
    assert_eq!(stopped.exit_code, Some(0));
    assert_eq!(
        worker(&harness, "review-lead").await.state,
        WorkerState::Idle
    );
}

#[tokio::test]
async fn an_archived_task_stops_its_coder_at_once() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    areas(&harness).await;
    let lead = start_lead(&mut harness, "web").await;
    harness
        .call(
            &lead,
            "start_worker",
            json!({"role": "task_coder", "tasks": ["T-1"]}),
        )
        .await;
    until_state(&harness, "web-coder", WorkerState::Idle).await;
    harness
        .call(MAIN, "archive_task", json!({"id": "T-1"}))
        .await;
    let stopped = until_state(&harness, "web-coder", WorkerState::Stopped).await;
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
        "[workers]\ntask_coder = \"many\"\n",
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
