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
const REVIEW_LEAD: &str = "review-lead";

/// The recorded turn reports a context of 25310 tokens in a window of 1000000; the grown one reports 700192, past the default of 60 percent for that window.
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
    harness.make_main(MAIN).await;
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
    assert_eq!(listed["handoff"]["chars"], note.chars().count());
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
async fn a_project_handoff_at_wins_over_the_default() {
    let turn = Turn::new();
    let fake = turn.fake();
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    std::fs::write(
        std::path::Path::new(harness.root()).join(".fold/config.toml"),
        "[roles.area_lead]\nhandoff_at = 80\n",
    )
    .unwrap();
    lead(&mut harness).await;
    turn.grow();
    harness.send(MAIN, &[LEAD], "Look at the page.").await;
    assert_eq!(idle_after(&harness, 2).await.context_tokens, 700_192);
    harness.send(MAIN, &[LEAD], "One more look.").await;
    idle_after(&harness, 3).await;
    let run = runs(&fake, 1, 4).await.remove(0);
    assert_eq!(asks(&run), 0, "70 percent is below the project's 80");
}

#[tokio::test]
async fn a_lead_that_writes_no_note_starts_fresh_with_the_summary_alone() {
    let turn = Turn::new();
    let fake = turn.fake();
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
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
    assert!(
        first_turn.contains("Unread messages: 0."),
        "each message reached the lead whole in its turn, so none is unread: {first_turn}"
    );
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
    harness.make_main(MAIN).await;
    let started = lead(&mut harness).await;
    let note = "Mid-turn note. Next: split T-1.";
    // A lead calls write_handoff inside a turn, so the note is stored without a refresh of its own and the refresh waits for a turn's end.
    harness
        .handle
        .store
        .write(async |tx| {
            agents::insert_handoff(
                tx,
                started.agent_id,
                &started.session_id,
                note,
                fold_core::Timestamp::from_millis(1).unwrap(),
            )
            .await
        })
        .await
        .unwrap();
    harness.send(MAIN, &[LEAD], "Look at the page.").await;
    let fresh_runs = runs(&fake, 2, 2).await;
    let fresh = &fresh_runs[1];
    let fresh_session = fresh.flag("--session-id").unwrap().to_owned();
    assert_ne!(fresh_session, started.session_id);
    assert!(fresh.stdin[1].contains(note), "{}", fresh.stdin[1]);
    // The check after the first turn can still be pending when the note is stored, so the refresh comes at the end of the first turn or of the message's turn.
    let row = eventually(WAIT, async || {
        let row = worker(&harness).await;
        if row.state == WorkerState::Idle && row.session_id == fresh_session {
            Ok(row)
        } else {
            Err((row.state, row.turns))
        }
    })
    .await;
    assert!(matches!(row.turns, 2 | 3), "{}", row.turns);
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

/// Sets the lead's context as a turn that is still under way reports it, with no result yet.
async fn grow_inside_the_turn(harness: &Harness, tokens: i64) {
    let row = worker(harness).await;
    harness
        .handle
        .store
        .write(async |tx| {
            sqlx::query("UPDATE worker_processes SET context_tokens = ?1 WHERE id = ?2")
                .bind(tokens)
                .bind(row.id)
                .execute(tx.conn())
                .await
                .map_err(fold_store::StoreError::from)
        })
        .await
        .unwrap();
}

/// The result of a short wait of `agent` once one carries `handoff`. The windows of a turn's result are learned just after its row turns idle, so the first wait can come before them.
async fn handoff_wait(harness: &Harness, agent: &str) -> Value {
    eventually(WAIT, async || {
        let woke = harness
            .call(agent, "wait_for_events", json!({"timeout_s": 1}))
            .await;
        if woke.get("handoff").is_some() {
            Ok(woke)
        } else {
            Err(woke)
        }
    })
    .await
}

#[tokio::test]
async fn a_lead_past_its_threshold_gets_the_handoff_request_in_a_wait_result() {
    let turn = Turn::new();
    let fake = turn.fake();
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    lead(&mut harness).await;
    let small = harness
        .call(LEAD, "wait_for_events", json!({"timeout_s": 1}))
        .await;
    assert_eq!(small.get("handoff"), None, "{small}");

    // The first turn's result reported a window of 1000000, so 700000 tokens are past the default of 60 percent.
    grow_inside_the_turn(&harness, 700_000).await;
    let woke = handoff_wait(&harness, LEAD).await;
    assert_eq!(woke["timed_out"], true, "{woke}");
    assert_eq!(woke["handoff"], HANDOFF_REQUEST, "{woke}");
    let asked = worker(&harness).await;
    let turns = agents::handoff_asked(harness.handle.store.read(), asked.id)
        .await
        .unwrap();
    assert_eq!(turns, Some(1), "the ask counts from the turn it came in");

    harness
        .call(LEAD, "write_handoff", json!({"text": "Next: split T-1."}))
        .await;
    let after = harness
        .call(LEAD, "wait_for_events", json!({"timeout_s": 1}))
        .await;
    assert_eq!(
        after.get("handoff"),
        None,
        "a session with a note is not asked again: {after}"
    );
}

#[tokio::test]
async fn the_review_lead_hands_off_at_its_own_lower_default_in_a_wait() {
    let turn = Turn::new();
    let fake = turn.fake();
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    lead(&mut harness).await;
    grow_inside_the_turn(&harness, 450_000).await;

    harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    harness.connect(REVIEW_LEAD).await;
    let review_lead = eventually(WAIT, async || {
        let row = harness
            .handle
            .agents()
            .get_worker(&AgentName::parse(REVIEW_LEAD).unwrap())
            .await
            .unwrap();
        if row.state == WorkerState::Idle && row.turns == 1 {
            Ok(row)
        } else {
            Err((row.state, row.turns))
        }
    })
    .await;
    harness
        .handle
        .store
        .write(async |tx| {
            sqlx::query("UPDATE worker_processes SET context_tokens = 450000 WHERE id = ?1")
                .bind(review_lead.id)
                .execute(tx.conn())
                .await
                .map_err(fold_store::StoreError::from)
        })
        .await
        .unwrap();
    let woke = handoff_wait(&harness, REVIEW_LEAD).await;
    assert_eq!(
        woke["handoff"], HANDOFF_REQUEST,
        "45 percent is past the review lead's 40: {woke}"
    );
    // fold knows the window now, so the area lead's wait is sized by it too.
    let area_lead = harness
        .call(LEAD, "wait_for_events", json!({"timeout_s": 1}))
        .await;
    assert_eq!(
        area_lead.get("handoff"),
        None,
        "45 percent is below an area lead's 60: {area_lead}"
    );
}
