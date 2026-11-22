#![cfg(test)]

use std::time::Duration;

use fold_core::{ProjectConfig, WorkerState, WorkspaceId};
use fold_store::testing::eventually;
use serde_json::json;

use crate::common::acceptance::{Harness, WAIT};
use crate::worker_resume::{TASK_WORKER, task_worker_for, until_runs, until_state};
use crate::workers_support::FakeClaude;

/// Longer than the watcher's debounce and maximum wait together, so an edit a watch saw would show by then.
const QUIET: Duration = Duration::from_millis(1_500);

async fn until_watch(harness: &Harness, workspace: WorkspaceId, watched: bool) {
    let live = harness.handle.live.clone();
    eventually(WAIT, async || {
        if live.has_watch(workspace).await == watched {
            Ok(())
        } else {
            Err(!watched)
        }
    })
    .await;
}

async fn until_file(harness: &Harness, workspace: WorkspaceId, path: &str) {
    let live = harness.handle.live.clone();
    eventually(WAIT, async || match live.get(workspace) {
        Some(state) if state.view.files.iter().any(|file| file.path == path) => Ok(()),
        other => Err(other.map(|state| state.view.files.len())),
    })
    .await;
}

/// Acceptance 47, the watch follows the worker: a task worker idle in its open session has a live watch on its workspace. Its lead parks it, its process stops and its session ends, so the workspace loses the watch and rests on the timer scan: an edit shows only once `rescan_seconds` of the project's clock have passed. The lead's go resumes the worker, its session opens again, the workspace gets its watch back at once, and the next edit shows through the watch with no tick.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_47_a_workspace_gets_and_loses_its_watch_as_its_worker_starts_stops_and_resumes()
{
    let fake = FakeClaude::new("result");
    let mut harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    let workspace: WorkspaceId = work.workspace.parse().unwrap();
    let live = harness.handle.live.clone();
    assert!(
        live.has_watch(workspace).await,
        "the started worker's workspace is watched"
    );

    harness
        .call(
            "web-lead",
            "stop_worker",
            json!({"name": TASK_WORKER, "wait_for": "my go after the review"}),
        )
        .await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    harness.disconnect(TASK_WORKER).await;
    until_watch(&harness, workspace, false).await;
    assert_eq!(live.resting(), 1);
    // The scan that the rest asks for has run once the read answers, so the edit below comes after it.
    live.rescan(workspace).await.unwrap();

    work.write(&[("rested.md", "on the timer\n")]);
    let early = tokio::time::timeout(QUIET, until_file(&harness, workspace, "rested.md")).await;
    assert!(
        early.is_err(),
        "no watch reports the edit of a resting workspace"
    );
    live.tick();
    assert!(
        live.get(workspace)
            .unwrap()
            .view
            .files
            .iter()
            .all(|file| file.path != "rested.md"),
        "a tick before the rescan is due scans nothing"
    );
    harness
        .project
        .clock()
        .advance(Duration::from_secs(ProjectConfig::default().rescan_seconds))
        .unwrap();
    live.tick();
    until_file(&harness, workspace, "rested.md").await;

    harness
        .send("web-lead", &[TASK_WORKER], "Go: the review is done")
        .await;
    until_runs(&fake, TASK_WORKER, 2).await;
    harness.connect(TASK_WORKER).await;
    until_watch(&harness, workspace, true).await;
    assert_eq!(live.resting(), 0);

    work.write(&[("watched.md", "through the watch\n")]);
    until_file(&harness, workspace, "watched.md").await;
}
