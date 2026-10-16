#![cfg(test)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fold_core::roles::HANDOFF_REQUEST;
use fold_core::{AgentName, WorkerState, codes};
use fold_daemon::services::events::BusMessage;
use fold_store::agents::{self, WorkerProcessRow};
use fold_store::testing::eventually;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::common::acceptance::{Harness, WAIT};
use crate::workers_support::{FakeClaude, Run, fixture};

const MAIN: &str = "frontend";
const LEAD: &str = "web-lead";

/// The recorded turn reports a context of 25310 tokens in a window of 1000000; the grown one reports 700192, past the default `handoff_at` of 60 percent.
const SMALL: &str = "\"cache_read_input_tokens\":25118";
const GROWN: &str = "\"cache_read_input_tokens\":700000";

/// The turn the stand-in replays: a file the test rewrites between turns, so the context it reports grows.
struct Turn {
    _dir: TempDir,
    path: PathBuf,
    small: String,
}

impl Turn {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("turn.jsonl");
        let small = std::fs::read_to_string(fixture("result")).unwrap();
        assert!(small.contains(SMALL));
        std::fs::write(&path, &small).unwrap();
        Self {
            _dir: dir,
            path,
            small,
        }
    }

    fn grow(&self) {
        std::fs::write(&self.path, self.small.replace(SMALL, GROWN)).unwrap();
    }

    fn shrink(&self) {
        std::fs::write(&self.path, &self.small).unwrap();
    }

    fn fake(&self) -> FakeClaude {
        let mut fake = FakeClaude::new("result");
        for (key, value) in &mut fake.launcher.env {
            if key == "FOLD_FAKE_CLAUDE_TURN" {
                *value = self.path.to_string_lossy().into_owned();
            }
        }
        fake
    }
}

async fn worker(harness: &Harness) -> WorkerProcessRow {
    harness
        .handle
        .agents()
        .get_worker(&AgentName::parse(LEAD).unwrap())
        .await
        .unwrap()
}

/// The lead's row once it is idle after `turns` turns.
async fn idle_after(harness: &Harness, turns: u64) -> WorkerProcessRow {
    eventually(WAIT, async || {
        let row = worker(harness).await;
        if row.state == WorkerState::Idle && row.turns == turns {
            Ok(row)
        } else {
            Err((row.state, row.turns))
        }
    })
    .await
}

/// The lead's runs once there are `count` and the newest has read `lines` stdin lines.
async fn runs(fake: &FakeClaude, count: usize, lines: usize) -> Vec<Run> {
    eventually(WAIT, async || {
        let runs = fake.runs_of(LEAD);
        match runs.last() {
            Some(last) if runs.len() == count && last.stdin.len() >= lines => Ok(runs),
            _ => Err(runs.len()),
        }
    })
    .await
}

fn asks(run: &Run) -> usize {
    let request = serde_json::to_string(HANDOFF_REQUEST).unwrap();
    let request = request.trim_matches('"');
    run.stdin
        .iter()
        .filter(|line| line.contains(request))
        .count()
}

/// Label `ui`, area `web` over it, task `T-1` labelled `ui`, and the lead `web-lead` started by the main agent and connected; returns once its first turn has ended.
async fn lead(harness: &mut Harness) -> WorkerProcessRow {
    harness
        .call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    harness
        .call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Draw the page", "labels": ["ui"]}),
        )
        .await;
    harness
        .call(MAIN, "create_task", json!({"title": "Serve the page"}))
        .await;
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    harness.connect(LEAD).await;
    idle_after(harness, 1).await
}

/// The context grows past the threshold in the second turn, so fold asks in the third.
async fn grow_until_asked(harness: &Harness, turn: &Turn, fake: &FakeClaude) -> WorkerProcessRow {
    let first = runs(fake, 1, 2).await;
    assert_eq!(asks(&first[0]), 0, "a small context is not asked");
    turn.grow();
    harness.send(MAIN, &[LEAD], "Look at the page.").await;
    let asked = idle_after(harness, 3).await;
    assert_eq!(asked.context_tokens, 700_192);
    let run = runs(fake, 1, 4).await.remove(0);
    assert_eq!(asks(&run), 1);
    asked
}

async fn stored_events(harness: &Harness) -> Vec<(String, Value)> {
    harness
        .events()
        .await
        .into_iter()
        .map(|event| (event.kind, event.data))
        .collect()
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
async fn a_lead_past_its_threshold_hands_off_to_a_fresh_session() {
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
    let turn = Turn::new();
    let fake = turn.fake();
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let mut bus = harness.handle.bus.subscribe();
    let started = lead(&mut harness).await;
    let asked = grow_until_asked(&harness, &turn, &fake).await;
    assert_eq!(asked.session_id, started.session_id);

    harness.send(MAIN, &[LEAD], "One more look.").await;
    idle_after(&harness, 4).await;
    assert_eq!(asks(&runs(&fake, 1, 5).await[0]), 1, "fold asks once");

    turn.shrink();
    let note = "Working on the page. Open: plover layout. Next: split T-1.";
    let stored = harness
        .call(LEAD, "write_handoff", json!({"text": note}))
        .await;
    assert_eq!(stored["stored"], true);
    let fresh_runs = runs(&fake, 2, 2).await;
    let fresh = &fresh_runs[1];
    let fresh_session = fresh.flag("--session-id").unwrap().to_owned();
    assert_ne!(fresh_session, started.session_id);
    assert!(!fresh.has("--resume"));
    assert_eq!(fresh.env["CLAUDE_CODE_PROMPT_CACHE_TTL"], "1h");
    let first_turn = &fresh.stdin[1];
    assert!(first_turn.contains("fold started you again as web-lead"));
    assert!(first_turn.contains(note), "{first_turn}");
    assert!(first_turn.contains("Open tasks of your area (1):"));
    assert!(first_turn.contains("T-1 todo Draw the page"));
    assert!(!first_turn.contains("Serve the page"));
    assert!(first_turn.contains("Open proposals: none."));
    let row = idle_after(&harness, 5).await;
    assert_eq!(row.session_id, fresh_session);
    assert_eq!(row.area.as_ref().map(|area| area.as_str()), Some("web"));

    let events = stored_events(&harness).await;
    let refreshed: Vec<&Value> = events
        .iter()
        .filter(|(kind, _)| kind == "worker.refreshed")
        .map(|(_, data)| data)
        .collect();
    assert_eq!(refreshed.len(), 1);
    assert_eq!(refreshed[0]["agent"], LEAD);
    assert_eq!(refreshed[0]["old_session"], started.session_id.as_str());
    assert_eq!(refreshed[0]["new_session"], fresh_session.as_str());
    assert_eq!(refreshed[0]["context_tokens"], 700_192);
    assert_eq!(refreshed[0]["handoff"], true);
    assert!(
        events
            .iter()
            .all(|(kind, _)| kind != "worker.stopped" && kind != "worker.failed"),
        "a refresh is not a stop"
    );
    let listed = harness.call(MAIN, "list_workers", json!({})).await;
    let listed = &listed["workers"][0];
    assert_eq!(listed["handoff"]["text"], note);
    assert_eq!(listed["handoff"]["session"], started.session_id.as_str());

    let mut live = Vec::new();
    while let Ok(message) = bus.try_recv() {
        if let BusMessage::Live(event) = message {
            live.push(event.data.clone());
        }
    }
    let text = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    let events = serde_json::to_string(&events).unwrap();
    let live = serde_json::to_string(&live).unwrap();
    assert!(!text.contains("plover"), "a log line holds the note");
    assert!(!events.contains("plover"), "a stored event holds the note");
    assert!(!live.contains("plover"), "a live event holds the note");
}

#[tokio::test]
async fn a_lead_that_writes_no_note_starts_fresh_with_the_summary_alone() {
    let turn = Turn::new();
    let fake = turn.fake();
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let started = lead(&mut harness).await;
    grow_until_asked(&harness, &turn, &fake).await;
    harness.send(MAIN, &[LEAD], "Still there?").await;
    idle_after(&harness, 4).await;
    assert_eq!(fake.runs_of(LEAD).len(), 1, "the lead has turns left");
    turn.shrink();
    harness.send(MAIN, &[LEAD], "Last call.").await;
    let fresh_runs = runs(&fake, 2, 2).await;
    let first_turn = &fresh_runs[1].stdin[1];
    assert!(first_turn.contains("Your last session wrote no hand-off note."));
    assert!(first_turn.contains("T-1 todo Draw the page"));
    assert!(first_turn.contains("Unread messages: 3. Call get_inbox."));
    let events = stored_events(&harness).await;
    let refreshed = events
        .iter()
        .find(|(kind, _)| kind == "worker.refreshed")
        .map(|(_, data)| data)
        .unwrap();
    assert_eq!(refreshed["handoff"], false);
    assert_eq!(refreshed["old_session"], started.session_id.as_str());

    let main = harness
        .refused(MAIN, "write_handoff", json!({"text": "my plan"}))
        .await;
    assert_eq!(main["code"], codes::HANDOFF_DENIED);
    let empty = harness
        .refused(LEAD, "write_handoff", json!({"text": "  "}))
        .await;
    assert_eq!(empty["code"], codes::INVALID_REQUEST);
}

#[tokio::test]
async fn a_note_written_during_a_turn_refreshes_the_lead_when_the_turn_ends() {
    let turn = Turn::new();
    let fake = turn.fake();
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let started = lead(&mut harness).await;
    let note = "Mid-turn note. Next: split T-1.";
    // A lead calls write_handoff inside a turn, so the note is stored while the row is working and the refresh waits for the turn's end.
    harness
        .handle
        .store
        .write(async |tx| {
            agents::insert_handoff(
                tx.conn(),
                started.agent_id,
                &started.session_id,
                note,
                fold_core::Timestamp::from_millis(1).unwrap(),
            )
            .await
        })
        .await
        .unwrap();
    assert_eq!(fake.runs_of(LEAD).len(), 1);
    harness.send(MAIN, &[LEAD], "Look at the page.").await;
    let fresh_runs = runs(&fake, 2, 2).await;
    let fresh = &fresh_runs[1];
    let fresh_session = fresh.flag("--session-id").unwrap().to_owned();
    assert_ne!(fresh_session, started.session_id);
    assert!(fresh.stdin[1].contains(note), "{}", fresh.stdin[1]);
    let row = idle_after(&harness, 3).await;
    assert_eq!(row.session_id, fresh_session);
    let events = stored_events(&harness).await;
    let refreshed = events
        .iter()
        .find(|(kind, _)| kind == "worker.refreshed")
        .map(|(_, data)| data)
        .unwrap();
    assert_eq!(refreshed["handoff"], true);
    assert_eq!(refreshed["new_session"], fresh_session.as_str());
    assert!(
        events
            .iter()
            .all(|(kind, _)| kind != "worker.stopped" && kind != "worker.failed")
    );
}
