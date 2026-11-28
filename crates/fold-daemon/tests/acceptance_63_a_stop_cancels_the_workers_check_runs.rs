#![cfg(test)]

use std::path::Path;
use std::time::Duration;

use fold_core::WorkerState;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, Work};
use crate::worker_resume::{MAIN, TASK_WORKER, task_worker_for, until_state};
use crate::workers_support::FakeClaude;

/// Long enough for a shell to start on a slow machine, and much shorter than the checks, so only a cancelled run ends in it.
const WAIT: Duration = Duration::from_secs(30);

/// A checks file whose first check writes its shell's pid into `marks` and waits; the second check marks that it started.
fn checks(marks: &Path) -> String {
    let mark = |name: &str| marks.join(name).display().to_string();
    format!(
        "[[check]]\nname = \"slow\"\nrun = \"echo $$ > '{shell}'; sleep 600\"\n\n[[check]]\nname = \"later\"\nrun = \"touch '{later}'\"\n",
        shell = mark("shell"),
        later = mark("later"),
    )
}

/// The web lead's task worker with its workspace of `T-1`, where the checks of [`checks`] run; returns the harness, the work and the run id once the first check runs.
async fn running(fake: &FakeClaude, marks: &Path) -> (Harness, Work, String) {
    let harness = task_worker_for(fake, &["Add the report table"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    work.write(&[
        ("fold-checks.toml", &checks(marks)),
        ("report.html", "<table></table>\n"),
    ]);
    let started = harness
        .call(TASK_WORKER, "run_checks", json!({"task": "T-1"}))
        .await;
    assert_eq!(started["checks"], "running", "{started}");
    let run = started["run"].as_str().unwrap().to_owned();
    let shell = marks.join("shell");
    eventually(WAIT, async || match std::fs::read_to_string(&shell) {
        Ok(text) if text.ends_with('\n') => Ok(()),
        _ => Err("the first check has not started"),
    })
    .await;
    (harness, work, run)
}

/// The stored `checks.finished` events of `T-1`.
async fn finished(harness: &Harness) -> Vec<Value> {
    harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "checks.finished")
        .map(|event| event.data)
        .collect()
}

/// The data of the `checks.finished` of `run`, once it is stored.
async fn finished_of(harness: &Harness, run: &str) -> Value {
    eventually(WAIT, async || {
        finished(harness)
            .await
            .into_iter()
            .find(|data| data["run"] == run)
            .ok_or("the run has not finished")
    })
    .await
}

/// Asserts that `run` ended as `cancelled` by `by`, that its later check never started and that fold told the task worker who cancelled it.
async fn cancelled_by(harness: &Harness, run: &str, by: &str, marks: &Path) {
    let data = finished_of(harness, run).await;
    assert_eq!(data["state"], "failed", "{data}");
    assert_eq!(data["reason"], "cancelled", "{data}");
    assert_eq!(data["by"], by, "{data}");
    let read = harness
        .call(MAIN, "get_check_run", json!({"run": run}))
        .await;
    assert_eq!(read["reason"], "cancelled", "{read}");
    assert!(!marks.join("later").exists(), "no later check starts");
    let messages = harness.fold_messages(TASK_WORKER).await;
    assert!(
        messages
            .iter()
            .any(|message| message.contains(&format!("({run}) were cancelled by {by}"))),
        "{messages:?}"
    );
}

/// Acceptance 63, a park cancels the worker's run: while the first check of the task worker's run goes, the web lead parks the worker with stop_worker and wait_for. The run ends as `cancelled` with the lead as `by`, its later check never starts, and fold's message, which the worker reads when it resumes, says who cancelled it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_63_a_park_cancels_the_running_check_run() {
    let fake = FakeClaude::new("result");
    let marks = tempfile::TempDir::new().unwrap();
    let (harness, _work, run) = running(&fake, marks.path()).await;

    let parked = harness
        .call(
            "web-lead",
            "stop_worker",
            json!({"name": TASK_WORKER, "wait_for": "my go after the T-288 migration"}),
        )
        .await;
    assert_eq!(
        parked["worker"]["parked"]["wait_for"],
        "my go after the T-288 migration"
    );
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    cancelled_by(&harness, &run, "web-lead", marks.path()).await;
}

/// Acceptance 63, a stop without a running run: the web lead stops a task worker that never ran checks. The stop works as before, and no check run starts or ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_63_a_stop_without_a_running_run_changes_no_run() {
    let fake = FakeClaude::new("result");
    let harness = task_worker_for(&fake, &["Add the report table"], WorkerState::Idle).await;
    harness.start_work(TASK_WORKER, "T-1").await;

    harness
        .call("web-lead", "stop_worker", json!({"name": TASK_WORKER}))
        .await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    assert!(finished(&harness).await.is_empty());
    let messages = harness.fold_messages(TASK_WORKER).await;
    assert!(
        !messages.iter().any(|message| message.contains("cancelled")),
        "{messages:?}"
    );
}

/// Acceptance 63, a pause cancels every run: while the task worker's run goes, the main agent pauses the project. The run ends as `cancelled` with the main agent as `by`, and its later check never starts.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_63_a_pause_cancels_every_running_check_run() {
    let fake = FakeClaude::new("result");
    let marks = tempfile::TempDir::new().unwrap();
    let (harness, _work, run) = running(&fake, marks.path()).await;

    let paused = harness.call(MAIN, "pause_project", json!({})).await;
    assert_eq!(paused["paused"], true, "{paused}");
    cancelled_by(&harness, &run, MAIN, marks.path()).await;
}

/// Acceptance 63, a stop of a worker that has ended keeps its run: the task worker has stopped, as fold stops it once its proposal is sent, and its run goes on for the result fold resumes it with. The web lead's stop and park of the ended worker stop nothing, so the run still runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_63_a_stop_of_an_ended_worker_keeps_its_run() {
    let fake = FakeClaude::new("result");
    let marks = tempfile::TempDir::new().unwrap();
    let harness = task_worker_for(&fake, &["Add the report table"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    work.write(&[
        ("fold-checks.toml", &checks(marks.path())),
        ("report.html", "<table></table>\n"),
    ]);
    harness
        .call("web-lead", "stop_worker", json!({"name": TASK_WORKER}))
        .await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    let started = harness
        .call(TASK_WORKER, "run_checks", json!({"task": "T-1"}))
        .await;
    assert_eq!(started["checks"], "running", "{started}");
    let run = started["run"].as_str().unwrap().to_owned();
    let shell = marks.path().join("shell");
    eventually(WAIT, async || match std::fs::read_to_string(&shell) {
        Ok(text) if text.ends_with('\n') => Ok(()),
        _ => Err("the first check has not started"),
    })
    .await;

    harness
        .call("web-lead", "stop_worker", json!({"name": TASK_WORKER}))
        .await;
    harness
        .call(
            "web-lead",
            "stop_worker",
            json!({"name": TASK_WORKER, "wait_for": "my go after the T-288 migration"}),
        )
        .await;
    let read = harness
        .call(MAIN, "get_check_run", json!({"run": run}))
        .await;
    assert_eq!(read["state"], "running", "{read}");
    assert!(finished(&harness).await.is_empty());

    harness
        .call(MAIN, "cancel_check_run", json!({"run": run}))
        .await;
    finished_of(&harness, &run).await;
}
