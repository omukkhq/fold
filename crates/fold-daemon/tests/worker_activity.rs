#![cfg(test)]

use std::time::Duration;

use fold_core::WorkerState;
use fold_daemon::live::{ActivityState, LiveActivity, LiveCall, LiveSlot, WorkspaceLive};
use fold_daemon::services::workspaces::check_stalls;
use fold_store::testing::eventually;
use serde_json::json;

use crate::common::acceptance::{HUMAN, Harness, WAIT, Work};
use crate::worker_resume::{
    MAIN, TASK_WORKER, proposed_by_a_task_worker, request_changes, task_worker_for, until_runs,
    until_state, worker,
};
use crate::workers_support::FakeClaude;

const MINUTE: Duration = Duration::from_secs(60);

/// The default `stall_minutes`.
const STALL: Duration = Duration::from_secs(5 * 60);

async fn state(harness: &Harness, work: &Work) -> WorkspaceLive {
    check_stalls(&harness.handle, harness.project.now())
        .await
        .unwrap();
    let workspace = work.workspace.parse().unwrap();
    WorkspaceLive::clone(&harness.handle.live.get(workspace).unwrap())
}

fn advance(harness: &Harness, by: Duration) {
    harness.project.clock().advance(by).unwrap();
}

async fn stalled_events(harness: &Harness) -> Vec<serde_json::Value> {
    harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "worker.stalled")
        .map(|event| event.data)
        .collect()
}

/// A task worker whose turn ends: with no call open and no output for `stall_minutes` it is silent, fold tells its lead once, and the stall waits for a person one window later.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_without_output_for_the_window_is_silent_and_its_lead_hears_first() {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    let start = harness.project.now();

    advance(&harness, STALL - Duration::from_millis(1));
    assert_eq!(state(&harness, &work).await.view.activity, None);

    advance(&harness, Duration::from_millis(1));
    let silent = state(&harness, &work).await.view;
    let seen = silent.activity.expect("a silent worker");
    assert_eq!(seen.state, ActivityState::Silent);
    assert_eq!(seen.call, None);
    assert!(seen.since >= start, "counted from the newest step");
    assert!(seen.supervised);
    assert_eq!(seen.told, ["web-lead"]);
    assert!(!silent.stalled, "the lead acts first");
    let told = stalled_events(&harness).await;
    assert_eq!(told.len(), 1);
    assert_eq!(told[0]["agent"], TASK_WORKER);
    assert_eq!(told[0]["reason"], "silent");
    assert_eq!(told[0]["told"], json!(["web-lead"]));

    advance(&harness, STALL - Duration::from_millis(1));
    assert!(!state(&harness, &work).await.view.stalled);
    advance(&harness, Duration::from_millis(1));
    assert!(
        state(&harness, &work).await.view.stalled,
        "one window after the notice"
    );
    assert_eq!(stalled_events(&harness).await.len(), 1, "told once");
}

/// A Bash call without a result is work: the worker never stalls by silence while it runs, shows as working after a minute, and its call counts as long at twice Claude Code's ten-minute limit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_open_command_is_work_until_it_runs_twice_claude_codes_limit() {
    let fake = FakeClaude::new("bash_open");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Working).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;

    advance(&harness, 2 * MINUTE);
    let working = state(&harness, &work).await.view;
    assert_eq!(
        working
            .activity
            .as_ref()
            .map(|seen| (seen.state, seen.call)),
        Some((ActivityState::Working, Some(LiveCall::Command)))
    );
    assert!(
        working
            .activity
            .as_ref()
            .is_some_and(|seen| seen.told.is_empty())
    );
    assert!(!working.stalled);

    advance(&harness, 17 * MINUTE);
    let still: Option<LiveActivity> = state(&harness, &work).await.view.activity;
    assert_eq!(
        still.map(|seen| seen.state),
        Some(ActivityState::Working),
        "19 min"
    );
    assert!(stalled_events(&harness).await.is_empty());

    advance(&harness, MINUTE);
    let long = state(&harness, &work).await.view;
    let seen = long.activity.expect("a long call");
    assert_eq!(
        (seen.state, seen.call),
        (ActivityState::LongCall, Some(LiveCall::Command))
    );
    assert!(!long.stalled);
    let told = stalled_events(&harness).await;
    assert_eq!(told[0]["reason"], "long_call");
    assert_eq!(told[0]["call"], "command");
    assert_eq!(told[0]["minutes"], 20);
}

/// A stop that the lead asked for is a choice: the worker shows stopped in the neutral tone, fold tells nobody and no person is needed, however long it stays stopped. A session of the agent that fold does not run keeps it from counting as stopped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_that_someone_asked_for_is_shown_and_never_a_stall() {
    let fake = FakeClaude::new("result");
    let mut harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    harness
        .call("web-lead", "stop_worker", json!({"name": TASK_WORKER}))
        .await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    assert_eq!(
        state(&harness, &work).await.view.activity,
        None,
        "the harness's own session of the worker still acts as it"
    );

    harness.disconnect(TASK_WORKER).await;
    let stopped = eventually(WAIT, async || {
        let view = state(&harness, &work).await.view;
        match view.activity {
            Some(seen) if seen.state == ActivityState::Stopped => Ok(seen),
            other => Err(other),
        }
    })
    .await;
    assert!(stopped.told.is_empty());
    assert!(!stopped.is_stall());
    advance(&harness, 3 * STALL);
    let view = state(&harness, &work).await.view;
    assert_eq!(
        view.activity.map(|seen| seen.state),
        Some(ActivityState::Stopped)
    );
    assert!(!view.stalled, "no person is needed");
    assert!(
        stalled_events(&harness).await.is_empty(),
        "fold tells nobody"
    );
}

/// A process that ended without anyone asking, such as by a crash, and without a report fails at once, and its lead hears of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_process_that_failed_without_a_report_is_a_stall_at_once() {
    let fake = FakeClaude::new("result").failing_with_stderr("Segmentation fault");
    let mut harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Failed).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    harness.disconnect(TASK_WORKER).await;
    let failed = eventually(WAIT, async || {
        let view = state(&harness, &work).await.view;
        match view.activity {
            Some(seen) if seen.state == ActivityState::Failed => Ok(seen),
            other => Err(other),
        }
    })
    .await;
    assert!(failed.is_stall());
    assert_eq!(failed.told, ["web-lead"]);
    let told = stalled_events(&harness).await;
    assert_eq!(told.len(), 1);
    assert_eq!(told[0]["reason"], "failed");
    assert_eq!(
        fold_core::stall::activity_text(failed.activity(), TASK_WORKER, harness.project.now())
            .as_deref(),
        Some("@web-worker failed")
    );

    // A file that the watcher sees after the end, such as the process's last write, is not the work of a running worker.
    let before = state(&harness, &work).await.view.last_activity_at;
    advance(&harness, Duration::from_secs(1));
    work.write(&[("late.txt", "written as the process ended\n")]);
    // The scan that takes the write drops a stall from the view until the next decision, which can read the workspace before the scan, so wait for a decision after it.
    let late = eventually(WAIT, async || {
        let view = state(&harness, &work).await.view;
        if view.last_activity_at > before && view.activity.is_some() {
            Ok(view)
        } else {
            Err(view.last_activity_at)
        }
    })
    .await;
    assert_eq!(
        late.activity.map(|seen| seen.state),
        Some(ActivityState::Failed)
    );
    advance(&harness, 2 * STALL);
    let view = state(&harness, &work).await.view;
    assert_eq!(
        view.activity.map(|seen| seen.state),
        Some(ActivityState::Failed)
    );
    assert_eq!(stalled_events(&harness).await.len(), 1, "told once");
}

/// A worker whose ask about its task waits for a person is never stalled, however long it waits and whatever call it left open.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_that_waits_for_an_answer_is_not_stalled() {
    let fake = FakeClaude::new("bash_open");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Working).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    fold_daemon::services::projects::ensure_human(
        harness.project.registry(),
        harness.project.clock(),
        &fold_core::HumanName::parse(HUMAN).unwrap(),
    )
    .await
    .unwrap();
    harness
        .send_with(
            TASK_WORKER,
            json!({"to": [format!("human:{HUMAN}")], "body": "Which colour?", "task": "T-1", "asks_human": true}),
        )
        .await;
    advance(&harness, 60 * MINUTE);
    let view = state(&harness, &work).await.view;
    assert_eq!(view.activity, None);
    assert!(!view.stalled);
    assert!(stalled_events(&harness).await.is_empty());
}

/// The lead that fold tells of a stall started the worker, so it may restart it with restart_worker, which ends the stall; the lead of another area may not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lead_that_hears_of_a_stall_restarts_its_task_worker() {
    let fake = FakeClaude::new("result");
    let mut harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    advance(&harness, STALL);
    let told = state(&harness, &work).await.view.activity.expect("a stall");
    assert_eq!(told.told, ["web-lead"]);

    crate::common::roles::give_role(
        &harness.handle.store,
        "api-lead",
        fold_core::Role::AreaLead,
        Some("api"),
    )
    .await;
    harness.connect("api-lead").await;
    let refused = harness
        .refused("api-lead", "restart_worker", json!({"name": TASK_WORKER}))
        .await;
    assert_eq!(refused["code"], fold_core::codes::MAIN_AGENT_REQUIRED);

    harness
        .call("web-lead", "restart_worker", json!({"name": TASK_WORKER}))
        .await;
    crate::worker_resume::until_runs(&fake, TASK_WORKER, 2).await;
    eventually(WAIT, async || {
        match state(&harness, &work).await.view.activity {
            None => Ok(()),
            other => Err(other),
        }
    })
    .await;
    assert!(!state(&harness, &work).await.view.stalled);
}

/// With one task worker place held by another worker, a task worker whose proposal comes back waits for a free slot: the neutral activity says so however long it waits, fold tells nobody and no person is needed, a lead's start of a new task worker is refused for it, and the stop of the other worker resumes it first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resume_the_limits_hold_waits_for_a_slot_and_is_never_a_stall() {
    let fake = FakeClaude::new("result");
    let (harness, work) = proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    std::fs::write(
        std::path::Path::new(harness.root()).join(".fold/config.toml"),
        "[workers]\ntask_worker = 1\n",
    )
    .unwrap();
    for title in ["Draw the menu", "Draw the footer"] {
        harness
            .call(
                MAIN,
                "create_task",
                json!({"title": title, "labels": ["ui"]}),
            )
            .await;
    }
    let other = harness
        .call(
            "web-lead",
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-2"]}),
        )
        .await;
    let other = other["worker"]["name"].as_str().unwrap().to_owned();
    until_state(&harness, &other, WorkerState::Idle).await;
    advance(&harness, Duration::from_secs(1));
    request_changes(&harness, "P-1", "Split the page.").await;
    assert_eq!(
        worker(&harness, TASK_WORKER).await.state,
        WorkerState::Stopped
    );

    for _ in 0..3 {
        let view = state(&harness, &work).await.view;
        let seen = view.activity.expect("the owed resume waits for a slot");
        assert_eq!(seen.state, ActivityState::WaitsForSlot);
        assert_eq!(seen.slot, Some(LiveSlot::TaskWorker));
        assert!(seen.told.is_empty());
        assert!(!seen.is_stall());
        assert!(!view.stalled, "no person is needed");
        assert_eq!(
            fold_core::stall::activity_text(seen.activity(), TASK_WORKER, harness.project.now())
                .as_deref(),
            Some("Waiting for a free slot")
        );
        advance(&harness, STALL);
    }
    assert!(
        stalled_events(&harness).await.is_empty(),
        "fold tells nobody"
    );

    let refused = harness
        .refused(
            "web-lead",
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-3"]}),
        )
        .await;
    assert_eq!(refused["details"]["reason"], "owed_resume_waits");
    assert_eq!(refused["details"]["agent"], TASK_WORKER);

    harness
        .call("web-lead", "stop_worker", json!({"name": other}))
        .await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert!(runs[1].stdin[1].contains("Split the page."));
    let after = state(&harness, &work).await.view;
    assert_ne!(
        after.activity.map(|seen| seen.state),
        Some(ActivityState::WaitsForSlot),
        "the resume took the place"
    );
    assert!(stalled_events(&harness).await.is_empty());
}

/// fold keeps who it told of a stall in memory, so after a daemon start the `worker.stalled` it appended before tells it that the stall was told already: nobody hears of it again, and a person sees it one window after that first notice.
#[tokio::test]
async fn a_stall_told_before_a_daemon_start_is_not_told_again() {
    let mut harness = Harness::start_without_leads().await;
    harness.lead_area("backend").await;
    harness.make_main(MAIN).await;
    let task = harness.create_task(MAIN, "Draw the page").await;
    let work = harness.start_work("backend", &task).await;
    advance(&harness, STALL);
    let told = state(&harness, &work).await.view;
    let before = told.activity.clone().expect("silent");
    assert_eq!(before.told, [MAIN]);
    assert!(!told.stalled);
    assert_eq!(stalled_events(&harness).await.len(), 1);

    advance(&harness, MINUTE);
    harness.restart().await;
    let again = state(&harness, &work).await.view;
    assert!(harness.handle.live.stalls_decided());
    let seen = again.activity.expect("still silent");
    assert_eq!(seen.state, ActivityState::Silent);
    assert_eq!(seen.since, before.since, "the same stall");
    assert_eq!(seen.told, [MAIN]);
    assert!(
        !again.stalled,
        "the person's window counts from the first notice"
    );
    assert_eq!(stalled_events(&harness).await.len(), 1, "told once");

    advance(&harness, STALL - MINUTE);
    assert!(state(&harness, &work).await.view.stalled);
    assert_eq!(stalled_events(&harness).await.len(), 1);
}

/// An owed resume that fold did not serve while a place is free, such as one whose resume failed for another reason than the limits, holds the agents' starts back only until it stalls: one window later its lead hears of it, and its restart takes the place.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_owed_resume_that_fold_does_not_serve_stalls_and_its_restart_frees_the_starts() {
    let fake = FakeClaude::new("result");
    let (harness, work) = proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    let config = std::path::Path::new(harness.root()).join(".fold/config.toml");
    std::fs::write(&config, "[workers]\ntask_worker = 1\n").unwrap();
    for title in ["Draw the menu", "Draw the footer"] {
        harness
            .call(
                MAIN,
                "create_task",
                json!({"title": title, "labels": ["ui"]}),
            )
            .await;
    }
    let other = harness
        .call(
            "web-lead",
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-2"]}),
        )
        .await;
    let other = other["worker"]["name"].as_str().unwrap().to_owned();
    until_state(&harness, &other, WorkerState::Idle).await;
    advance(&harness, Duration::from_secs(1));
    request_changes(&harness, "P-1", "Split the page.").await;
    assert_eq!(
        state(&harness, &work)
            .await
            .view
            .activity
            .map(|seen| seen.state),
        Some(ActivityState::WaitsForSlot)
    );

    // A raised limit that no event announces frees a place without serving the owed resume.
    std::fs::write(&config, "[workers]\ntask_worker = 2\n").unwrap();
    let refused = harness
        .refused(
            "web-lead",
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-3"]}),
        )
        .await;
    assert_eq!(refused["details"]["reason"], "owed_resume_waits");
    assert_eq!(refused["details"]["agent"], TASK_WORKER);

    advance(&harness, STALL);
    let seen = state(&harness, &work)
        .await
        .view
        .activity
        .expect("the owed resume stalls");
    assert_eq!(seen.state, ActivityState::Silent);
    assert_eq!(seen.told, ["web-lead"]);
    assert_eq!(stalled_events(&harness).await.len(), 1);

    harness
        .call("web-lead", "restart_worker", json!({"name": TASK_WORKER}))
        .await;
    until_runs(&fake, TASK_WORKER, 2).await;
    eventually(WAIT, async || {
        match state(&harness, &work).await.view.activity {
            None => Ok(()),
            other => Err(other),
        }
    })
    .await;
}

/// A task worker whose turn could not reach the model waits for the network, which is never a stall however long it lasts, and a turn that reaches the model again ends the wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_whose_turn_waits_for_the_network_is_never_a_stall() {
    let fake = FakeClaude::new("network_error");
    let harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    let waiting = eventually(WAIT, async || {
        let row = worker(&harness, TASK_WORKER).await;
        row.wait.clone().ok_or(row.state)
    })
    .await;

    advance(&harness, STALL * 3);
    let shown = state(&harness, &work).await.view;
    let seen = shown.activity.expect("a waiting worker");
    assert_eq!(seen.state, ActivityState::WaitsForNetwork);
    assert_eq!(seen.since, waiting.since);
    assert!(!seen.is_stall());
    assert!(!shown.stalled);
    assert!(seen.told.is_empty());
    assert!(stalled_events(&harness).await.is_empty());
}
