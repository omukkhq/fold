#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_core::{AgentName, Role, WorkerState};
use fold_daemon::mcp::handshake::{self, Answer, PROTOCOL_VERSION};
use fold_store::testing::eventually;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::common::acceptance::{HUMAN, Harness, WAIT};
use crate::worker_resume::{until_runs, until_state, worker};
use crate::workers_support::{FakeClaude, Run};

const MAIN: &str = "main";
const BASE: &str = "/api/v1/projects/demo";

fn supervise(harness: &Harness, on: bool) {
    std::fs::write(
        std::path::Path::new(harness.root()).join(".fold/config.toml"),
        format!("[main]\nsupervised = {on}\n"),
    )
    .unwrap();
}

/// A project whose setting has fold run the main agent, after a daemon start ran it: `main` is idle after its first turn, and [`HUMAN`] is a human of the project.
async fn supervised(fake: &FakeClaude) -> Harness {
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    fold_daemon::services::projects::ensure_human(
        harness.project.registry(),
        harness.project.clock(),
        &fold_core::HumanName::parse(HUMAN).unwrap(),
    )
    .await
    .unwrap();
    supervise(&harness, true);
    harness.restart().await;
    until_runs(fake, MAIN, 1).await;
    until_state(&harness, MAIN, WorkerState::Idle).await;
    harness
}

async fn http(
    harness: &Harness,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    harness
        .http(method, &format!("{BASE}{path}"), body.as_ref(), HUMAN)
        .await
}

/// A human's reply in the thread of `message`, returned by id.
async fn reply(harness: &Harness, message: &str, body: &str) -> String {
    let (status, sent) = http(
        harness,
        Method::POST,
        "/feedback",
        Some(json!({"reply_to": message, "body": body})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    sent["id"].as_str().unwrap().to_owned()
}

/// A session of the supervised main's own adapter, from inside the process tree of its newest run.
async fn connect_main(harness: &mut Harness, fake: &FakeClaude) {
    let run = fake.runs_of(MAIN).pop().expect("the supervised main runs");
    harness.connect_under(MAIN, run.pid).await;
}

/// The supervised main asks the humans to approve its plan, linked to no task, and ends its turn; returns the ask.
async fn ask_for_the_plan(harness: &mut Harness, fake: &FakeClaude) -> String {
    connect_main(harness, fake).await;
    let ask = harness
        .send_with(
            MAIN,
            json!({
                "to": ["humans"],
                "body": "Plan: one task, T-1, for the search box. OK?",
                "choices": ["OK", "Change it"],
                "asks_human": true,
            }),
        )
        .await;
    harness.disconnect(MAIN).await;
    ask
}

/// A human's stop of the supervised main on the Agents screen, which holds it.
async fn stop(harness: &Harness) {
    let (status, body) = http(harness, Method::POST, "/workers/main/stop", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    until_state(harness, MAIN, WorkerState::Stopped).await;
}

/// A stop of the supervised main that fold makes itself, such as the stop once it has answered its messages: the row is not held. It returns once the exit is recorded; work that waits can start the main again at once.
async fn stop_by_fold(harness: &Harness) {
    harness
        .handle
        .agents()
        .stop_worker(&fold_core::Actor::System, &AgentName::parse(MAIN).unwrap())
        .await
        .unwrap();
}

/// Whether a human's stop holds the supervised main.
async fn held(harness: &Harness) -> bool {
    let row = worker(harness, MAIN).await;
    fold_store::agents::worker_held(harness.handle.store.read(), row.id)
        .await
        .unwrap()
}

/// One handshake of `agent` with `tags` and the daemon's answer; an accepted session is closed again.
async fn handshake_of(harness: &Harness, agent: &str, tags: &[&str]) -> Answer {
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(handshake::accept(
        harness.state.clone(),
        server_end,
        Default::default(),
    ));
    let (read, mut write) = tokio::io::split(client_end);
    let hello = json!({"fold": PROTOCOL_VERSION, "cwd": harness.root(), "pid": 910_000, "agent": agent, "tags": tags});
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
    drop((read, write));
    tokio::time::timeout(WAIT, server)
        .await
        .expect("the session ends within the wait")
        .unwrap();
    serde_json::from_str(&line).unwrap()
}

/// The user turns of `run` that hold `text`.
fn turns_with<'a>(run: &'a Run, text: &str) -> Vec<&'a String> {
    run.stdin
        .iter()
        .filter(|line| line.contains(text))
        .collect()
}

/// The newest run of `main` once one of its stdin lines holds `text`.
async fn until_told(fake: &FakeClaude, text: &str) -> Run {
    eventually(WAIT, async || {
        let run = fake.runs_of(MAIN).pop();
        match run {
            Some(run) if !turns_with(&run, text).is_empty() => Ok(run),
            other => Err(other.map(|run| run.stdin)),
        }
    })
    .await
}

#[tokio::test]
async fn a_daemon_start_runs_the_supervised_main_with_its_role_and_the_main_tag() {
    let fake = FakeClaude::new("result");
    let harness = supervised(&fake).await;
    let runs = fake.runs_of(MAIN);
    assert_eq!(runs.len(), 1);
    assert!(runs[0].has("--session-id"));
    assert!(
        runs[0].stdin[1].contains("fold started you as main, the main agent of this project"),
        "{}",
        runs[0].stdin[1]
    );
    let prompt =
        std::fs::read_to_string(runs[0].flag("--append-system-prompt-file").unwrap()).unwrap();
    assert!(prompt.starts_with("# Main agent\n"), "{prompt}");
    assert!(prompt.contains("## Supervised main: no terminal"));
    let row = worker(&harness, MAIN).await;
    assert_eq!((row.role, row.area), (Role::Main, None));
    let agents = harness.handle.agents();
    let starter = agents
        .starter(&agents.actor_of(&main_worker(&harness).await).await.unwrap())
        .await
        .unwrap();
    assert_eq!(starter, fold_core::policy::Starter::MainAgent);
    let refused = harness
        .refused("frontend", "start_worker", json!({"role": "main"}))
        .await;
    assert_eq!(refused["code"], "worker_start_denied", "{refused}");
}

async fn main_worker(harness: &Harness) -> fold_store::agents::WorkerRow {
    let store = &harness.handle.store;
    let agent = fold_store::agents::find_agent(store.read(), &AgentName::parse(MAIN).unwrap())
        .await
        .unwrap()
        .unwrap();
    fold_store::agents::main_worker(store.read(), agent.id)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn a_message_to_the_stopped_supervised_main_resumes_it_once() {
    let fake = FakeClaude::new("result");
    let mut harness = supervised(&fake).await;
    let session = worker(&harness, MAIN).await.session_id;
    let ask = ask_for_the_plan(&mut harness, &fake).await;
    stop_by_fold(&harness).await;

    let answer = reply(&harness, &ask, "OK").await;
    let runs = until_runs(&fake, MAIN, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(session.as_str()));
    assert!(
        runs[1].stdin[1].contains(&format!("- {answer} from human:{HUMAN}")),
        "{}",
        runs[1].stdin[1]
    );
    until_state(&harness, MAIN, WorkerState::Idle).await;
    assert_eq!(fake.runs_of(MAIN).len(), 2, "one start for the message");
}

#[tokio::test]
async fn a_humans_stop_holds_the_supervised_main_until_a_human_starts_it_again() {
    let fake = FakeClaude::new("result");
    let mut harness = supervised(&fake).await;
    let ask = ask_for_the_plan(&mut harness, &fake).await;
    stop(&harness).await;
    assert!(held(&harness).await);

    let answer = reply(&harness, &ask, "OK").await;
    let (status, created) = harness
        .create_request_form(&[("body", None, None, b"Search the board")], Some(HUMAN))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    harness.restart().await;
    assert_eq!(worker(&harness, MAIN).await.state, WorkerState::Stopped);
    assert!(held(&harness).await, "a daemon start keeps the hold");

    let (status, body) = http(
        &harness,
        Method::POST,
        "/workers/main/restart",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let runs = until_runs(&fake, MAIN, 2).await;
    assert!(
        !runs[1].has("--resume"),
        "no reply, request or daemon start resumed the held main before the human's new session"
    );
    until_state(&harness, MAIN, WorkerState::Idle).await;
    assert!(!held(&harness).await, "the human's start clears the hold");

    stop_by_fold(&harness).await;
    assert!(!held(&harness).await);
    let run = until_told(&fake, &format!("- {answer} from human:{HUMAN}")).await;
    assert!(
        run.has("--resume"),
        "fold's own stop holds nothing: the reply the held main owed resumes it"
    );
}

#[tokio::test]
async fn a_paused_project_holds_the_supervised_main_until_its_resume() {
    let fake = FakeClaude::new("result");
    let mut harness = supervised(&fake).await;
    let ask = ask_for_the_plan(&mut harness, &fake).await;
    stop_by_fold(&harness).await;
    let (status, body) = http(&harness, Method::POST, "/pause", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let answer = reply(&harness, &ask, "OK").await;
    let (status, created) = harness
        .create_request_form(&[("body", None, None, b"Search the board")], Some(HUMAN))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(
        fake.runs_of(MAIN).len(),
        1,
        "a paused project starts nobody"
    );

    let (status, body) = http(&harness, Method::POST, "/resume", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // The resume serves both what waits, the reply and the request, in one resumed run: a start for the reply names it, one for the request counts it as unread.
    let run = until_told(&fake, "RQ-1").await;
    assert!(run.has("--resume"));
    assert!(
        !turns_with(&run, &answer).is_empty()
            || !turns_with(&run, "Unread messages to you: 1.").is_empty(),
        "{:?}",
        run.stdin
    );
    assert_eq!(fake.runs_of(MAIN).len(), 2);
}

#[tokio::test]
async fn the_plan_waits_for_a_human_reply_in_its_thread() {
    let fake = FakeClaude::new("result");
    let mut harness = supervised(&fake).await;
    let ask = ask_for_the_plan(&mut harness, &fake).await;
    let (status, needs) = http(&harness, Method::GET, "/needs-you", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        needs["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["kind"] == "question" && item["message"] == ask.as_str()),
        "{needs}"
    );
    let run = fake.runs_of(MAIN).pop().unwrap();
    assert_eq!(
        run.stdin.len(),
        2,
        "no turn before the reply: {:?}",
        run.stdin
    );
    assert_eq!(worker(&harness, MAIN).await.state, WorkerState::Idle);

    let answer = reply(&harness, &ask, "OK").await;
    let told = until_told(&fake, &answer).await;
    assert_eq!(
        told.pid, run.pid,
        "the idle main gets the reply as its next turn"
    );
    assert_eq!(fake.runs_of(MAIN).len(), 1);
}

#[tokio::test]
async fn the_interactive_and_the_supervised_main_refuse_each_other() {
    let fake = FakeClaude::new("result");
    let mut harness = supervised(&fake).await;

    let refused = handshake_of(&harness, MAIN, &["main"]).await;
    assert!(!refused.ok);
    let error = refused.error.unwrap();
    assert_eq!(error.code, "main_supervised", "{error:?}");
    let untagged = handshake_of(&harness, MAIN, &[]).await;
    assert_eq!(
        untagged.error.map(|error| error.code),
        Some("main_supervised".to_owned()),
        "a terminal session that names the running supervised main is a second main agent"
    );
    connect_main(&mut harness, &fake).await;
    harness.call(MAIN, "get_inbox", json!({})).await;
    harness.disconnect(MAIN).await;
    harness
        .call(
            "frontend",
            "create_label",
            json!({"name": "ui", "color": "sage"}),
        )
        .await;
    let (status, body) = http(
        &harness,
        Method::POST,
        "/areas",
        Some(json!({"name": "web", "label": "ui"})),
    )
    .await;
    assert!(status.is_success(), "{body}");
    let (status, body) = http(
        &harness,
        Method::POST,
        "/workers",
        Some(json!({"role": "area_lead", "area": "web"})),
    )
    .await;
    assert!(status.is_success(), "{body}");
    until_state(&harness, "web-lead", WorkerState::Idle).await;
    let lead = handshake_of(&harness, "web-lead", &["main"]).await;
    assert_eq!(
        lead.error.map(|error| error.code),
        Some("main_tag_denied".to_owned()),
        "a running lead is no main agent"
    );

    stop(&harness).await;
    harness.connect_tagged(MAIN, &["main"]).await;
    let (status, body) = http(
        &harness,
        Method::POST,
        "/workers/main/restart",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "main_interactive", "{body}");
    let (status, created) = harness
        .create_request_form(&[("body", None, None, b"Search the board")], Some(HUMAN))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(
        fake.runs_of(MAIN).len(),
        1,
        "an interactive main keeps fold from starting one"
    );

    harness.disconnect(MAIN).await;
    let (status, body) = http(
        &harness,
        Method::POST,
        "/workers/main/restart",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let first = fake.runs_of(MAIN)[0]
        .flag("--session-id")
        .unwrap()
        .to_owned();
    eventually(WAIT, async || {
        let runs = fake.runs_of(MAIN);
        match runs.last().and_then(|run| run.flag("--session-id")) {
            Some(session) if session != first => Ok(()),
            _ => Err(runs.len()),
        }
    })
    .await;
}

#[tokio::test]
async fn a_request_starts_the_supervised_main_and_never_while_the_setting_is_off() {
    let fake = FakeClaude::new("result");
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let (status, created) = harness
        .create_request_form(&[("body", None, None, b"Search the board")], Some(HUMAN))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let (status, workers) = http(&harness, Method::GET, "/workers", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !workers["workers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["role"] == "main"),
        "{workers}"
    );
    assert!(fake.runs_of(MAIN).is_empty());

    supervise(&harness, true);
    let (status, created) = harness
        .create_request_form(&[("body", None, None, b"Filter the board")], Some(HUMAN))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let runs = until_runs(&fake, MAIN, 1).await;
    let turn = &runs[0].stdin[1];
    assert!(turn.contains("fold started you as main"), "{turn}");
    assert!(turn.contains("RQ-2 created. Call claim_request."), "{turn}");
    let (_, workers) = http(&harness, Method::GET, "/workers", None).await;
    let main = workers["workers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == MAIN)
        .cloned()
        .unwrap();
    assert_eq!(main["role"], "main", "{main}");

    until_state(&harness, MAIN, WorkerState::Idle).await;
    stop(&harness).await;
    supervise(&harness, false);
    let (status, body) = http(
        &harness,
        Method::POST,
        "/workers/main/restart",
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"]["code"], "main_not_supervised", "{body}");
    assert_eq!(fake.runs_of(MAIN).len(), 1);
}

#[tokio::test]
async fn a_closed_workspace_of_the_supervised_main_goes_once_its_turn_ends() {
    let fake = FakeClaude::new("result");
    let mut harness = supervised(&fake).await;
    connect_main(&mut harness, &fake).await;
    let opened = harness.call(MAIN, "open_main_workspace", json!({})).await;
    let ws = opened["workspace"].as_str().unwrap().to_owned();
    let dir = std::path::Path::new(harness.root()).join(opened["path"].as_str().unwrap());
    assert!(dir.is_dir());
    let row = worker(&harness, MAIN).await;
    assert!(
        harness
            .handle
            .store
            .write(async |tx| fold_store::agents::mark_worker_working(tx, row.id).await)
            .await
            .unwrap()
    );

    harness
        .call(
            MAIN,
            "abandon_workspace",
            json!({"ws": ws, "reason": "The plan changed."}),
        )
        .await;
    assert!(dir.is_dir(), "a working main keeps its closed workspace");
    let message = harness.send("frontend", &[MAIN], "Done with T-1.").await;
    until_told(&fake, &message).await;
    eventually(WAIT, async || if dir.exists() { Err(()) } else { Ok(()) }).await;
    assert_eq!(worker(&harness, MAIN).await.state, WorkerState::Idle);
}

/// A task of main, ready at once; returns its id.
async fn main_task(harness: &Harness, title: &str) -> String {
    let created = harness
        .call(
            "frontend",
            "create_task",
            json!({"title": title, "labels": ["main"]}),
        )
        .await;
    created["id"].as_str().unwrap().to_owned()
}

/// What the catch-up turn says of the ready task `task` of main.
fn ready_for_main(task: &str) -> String {
    format!("These tasks of main are ready for you: {task}.")
}

#[tokio::test]
async fn a_daemon_start_tells_the_stopped_main_its_messages_with_the_work_that_waits() {
    let fake = FakeClaude::new("result");
    let mut harness = supervised(&fake).await;
    supervise(&harness, false);
    harness.restart().await;
    assert_eq!(
        worker(&harness, MAIN).await.state,
        WorkerState::Stopped,
        "a daemon start with the setting off ends the running row"
    );
    let task = main_task(&harness, "Add the checks").await;
    let message = harness.send("frontend", &[MAIN], "Which checks?").await;

    // The setting turns on while the daemon is down, so only the start serves what waits.
    harness.stop().await;
    supervise(&harness, true);
    harness.start_again().await;
    harness.connect("frontend").await;
    let run = until_told(&fake, &message).await;
    assert!(run.has("--resume"));
    assert_eq!(fake.runs_of(MAIN).len(), 2);
    let first = &run.stdin[1];
    assert!(first.contains(&ready_for_main(&task)), "{first}");
    assert!(
        first.contains(&format!("- {message} from frontend")),
        "{first}"
    );
    until_state(&harness, MAIN, WorkerState::Idle).await;

    connect_main(&mut harness, &fake).await;
    harness
        .send_with(
            MAIN,
            json!({"to": ["frontend"], "reply_to": message, "body": "Lint and tests."}),
        )
        .await;
    let later = harness.send("frontend", &[MAIN], "Thanks.").await;
    let told = until_told(&fake, &later).await;
    assert_eq!(
        told.pid, run.pid,
        "a main told of other work stays after its answer"
    );
}

#[tokio::test]
async fn work_that_reaches_the_main_while_it_answers_starts_it_again_after_its_stop() {
    let fake = FakeClaude::new("result");
    let mut harness = supervised(&fake).await;
    stop_by_fold(&harness).await;
    let message = harness.send("frontend", &[MAIN], "Which checks?").await;
    let answering = until_told(&fake, &message).await;
    assert!(answering.has("--resume"));
    let task = main_task(&harness, "Add the checks").await;
    until_told(&fake, &format!("{task} is ready in area main")).await;
    until_state(&harness, MAIN, WorkerState::Idle).await;

    connect_main(&mut harness, &fake).await;
    harness
        .send_with(
            MAIN,
            json!({"to": ["frontend"], "reply_to": message, "body": "Lint and tests."}),
        )
        .await;
    harness.disconnect(MAIN).await;
    let again = until_told(&fake, &ready_for_main(&task)).await;
    assert_ne!(
        again.pid, answering.pid,
        "fold stopped the main once it answered, then started it for the task"
    );
    assert!(again.has("--resume"));
}

#[tokio::test]
async fn the_end_of_the_interactive_main_session_starts_the_supervised_main() {
    let fake = FakeClaude::new("result");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.connect_tagged(MAIN, &["main"]).await;
    supervise(&harness, true);
    let (status, created) = harness
        .create_request_form(&[("body", None, None, b"Search the board")], Some(HUMAN))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert!(
        fake.runs_of(MAIN).is_empty(),
        "the interactive main keeps fold from starting one"
    );

    harness.disconnect(MAIN).await;
    let run = until_told(&fake, "RQ-1").await;
    let first = &run.stdin[1];
    assert!(first.contains("fold started you as main"), "{first}");
    assert!(
        first.contains("These open requests wait for a drafter: RQ-1."),
        "{first}"
    );
}

#[tokio::test]
async fn the_supervised_main_hands_off_through_its_note_and_is_asked_again_without_one() {
    let fake = FakeClaude::new("result");
    let mut harness = supervised(&fake).await;
    let row = worker(&harness, MAIN).await;
    let first_session = row.session_id.clone();
    harness
        .handle
        .store
        .write(async |tx| {
            fold_store::agents::set_handoff_asked(tx, row.id, row.turns, row.started_at).await
        })
        .await
        .unwrap();
    let asked = row.turns;
    let run = fake.runs_of(MAIN).pop().unwrap();
    for n in 1..=3 {
        let message = harness
            .send("frontend", &[MAIN], &format!("Status {n}?"))
            .await;
        until_told(&fake, &message).await;
        eventually(WAIT, async || {
            let turns = worker(&harness, MAIN).await.turns;
            if turns >= asked + n {
                Ok(())
            } else {
                Err(turns)
            }
        })
        .await;
    }
    let reasked = until_told(&fake, "Call write_handoff now").await;
    assert_eq!(
        reasked.pid, run.pid,
        "the ask comes again in the same session"
    );
    assert_eq!(worker(&harness, MAIN).await.session_id, first_session);

    connect_main(&mut harness, &fake).await;
    harness
        .call(
            MAIN,
            "write_handoff",
            json!({"text": "Waiting for the OK to the plan in m-1."}),
        )
        .await;
    harness.disconnect(MAIN).await;
    let fresh = until_told(&fake, "Waiting for the OK to the plan in m-1.").await;
    assert!(!fresh.has("--resume"), "the note starts a fresh session");
    assert_ne!(worker(&harness, MAIN).await.session_id, first_session);
}
