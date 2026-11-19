#![cfg(test)]

use std::fs;
use std::sync::Arc;

use crate::common::invariants::assert_consistent;
use crate::common::roles::give_role;
use crate::reviews_support::{Fixture, by, t, ws};
use fold_core::roles::Role;
use fold_daemon::projects::ProjectHandle;
use fold_daemon::services::projects::gc;
use fold_store::projects::ProjectRow;

const CHANGED: &str = "pub fn changed() {}\n";

/// A stop between the final checkpoint and the delete leaves the checkpoint, the start of the removal and the files; the next gc deletes the directory without a second checkpoint.
#[tokio::test]
async fn a_stop_after_the_final_checkpoint_leaves_a_removal_that_gc_completes() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    let author = by(&fx.frontend.worker);
    fx.workspaces.create_workspace(&author, t(1)).await.unwrap();
    fx.write("src/lib.rs", CHANGED);
    fx.workspaces
        .abandon_workspace(&author, ws(1), "the plan changed")
        .await
        .unwrap();
    fx.agents.end_session(&fx.frontend.ctx).await.unwrap();

    fail::cfg("workspace_removal_after_checkpoint", "return").unwrap();
    let result = fx.workspaces.remove_closed_directory(ws(1)).await;
    fail::remove("workspace_removal_after_checkpoint");
    assert_eq!(result.unwrap_err().code(), Some("failpoint"));
    fx.restart().await;

    let finals = "SELECT COUNT(*) FROM checkpoints WHERE reason = 'before_removal'";
    assert_eq!(fx.count(finals).await, 1);
    assert_eq!(
        std::fs::read_to_string(fx.task_dir().join("src/lib.rs")).unwrap(),
        CHANGED
    );
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspaces WHERE number = 1 AND removal_started_at IS NOT NULL AND directory_removed_at IS NULL")
            .await,
        1
    );
    assert_consistent(&fx.project).await;

    let handle = open(&fx).await;
    let report = gc(&handle, false).await.unwrap();
    assert_eq!(report.workspaces_removed, 1);
    assert!(!fx.task_dir().exists());
    assert_eq!(fx.count(finals).await, 1);
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM workspaces WHERE number = 1 AND directory_removed_at IS NOT NULL"
        )
        .await,
        1
    );
    scenario.teardown();
}

/// A stop between the delete and the write that records it leaves the row's removal started and the directory gone; the next gc records the removal without a second checkpoint.
#[tokio::test]
async fn a_stop_after_the_delete_leaves_a_started_removal_that_gc_records() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    let author = by(&fx.frontend.worker);
    fx.workspaces.create_workspace(&author, t(1)).await.unwrap();
    fx.write("src/lib.rs", CHANGED);
    fx.workspaces
        .abandon_workspace(&author, ws(1), "the plan changed")
        .await
        .unwrap();
    fx.agents.end_session(&fx.frontend.ctx).await.unwrap();

    fail::cfg("workspace_removal_after_delete", "return").unwrap();
    let result = fx.workspaces.remove_closed_directory(ws(1)).await;
    fail::remove("workspace_removal_after_delete");
    assert_eq!(result.unwrap_err().code(), Some("failpoint"));
    fx.restart().await;

    let finals = "SELECT COUNT(*) FROM checkpoints WHERE reason = 'before_removal'";
    assert_eq!(fx.count(finals).await, 1);
    assert!(!fx.task_dir().exists());
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspaces WHERE number = 1 AND removal_started_at IS NOT NULL AND directory_removed_at IS NULL")
            .await,
        1
    );
    assert_consistent(&fx.project).await;

    let handle = open(&fx).await;
    let planned = gc(&handle, true).await.unwrap();
    assert_eq!(planned.workspaces_removed, 1);
    let report = gc(&handle, false).await.unwrap();
    assert_eq!(report.workspaces_removed, 1);
    assert_eq!(fx.count(finals).await, 1);
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM workspaces WHERE number = 1 AND directory_removed_at IS NOT NULL"
        )
        .await,
        1
    );
    scenario.teardown();
}

/// A stop between moving a finished worker's folder aside and deleting it leaves the moved folder and no folder under the worker's name; the next gc deletes what was moved.
#[tokio::test]
async fn a_stop_after_a_worker_folder_moved_aside_leaves_it_for_the_next_gc() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    give_role(fx.project.store(), "reviewer-1", Role::Reviewer, None).await;
    let workers = fx.project.paths().workers_dir();
    let folder = workers.join("reviewer-1");
    fs::create_dir_all(folder.join("start/.claude")).unwrap();
    fs::write(folder.join("prompt.md"), "prompt\n").unwrap();
    let handle = open(&fx).await;

    fail::cfg("worker_folder_after_move", "return").unwrap();
    let report = gc(&handle, false).await.unwrap();
    fail::remove("worker_folder_after_move");
    assert_eq!(report.workers_removed, 0, "{:?}", report.workers);
    assert!(!folder.exists());
    let moved = workers.join(".removing-reviewer-1");
    assert!(moved.join("prompt.md").is_file());
    drop(handle);
    fx.restart().await;

    let handle = open(&fx).await;
    let report = gc(&handle, false).await.unwrap();
    assert!(report.workers.is_empty());
    assert!(!moved.exists());
    assert!(!folder.exists());
    scenario.teardown();
}

async fn open(fx: &Fixture) -> ProjectHandle {
    let row = ProjectRow {
        slug: "demo".to_owned(),
        name: "demo".to_owned(),
        root: fx.project.paths().root().to_string_lossy().into_owned(),
    };
    ProjectHandle::open(&row, Arc::new(fx.project.clock().clone()))
        .await
        .unwrap()
}
