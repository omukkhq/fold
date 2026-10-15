#![cfg(test)]

pub mod daemon_support;

use std::fs;

use daemon_support::{Opened, agent};
use fold_daemon::jobs::reconcile_workspaces;
use fold_daemon::services::workspaces::Reconciled;
use fold_store::workspaces;

/// The failpoint stops a sync after it wrote the merged files and before its rows commit, as a crash there would. The restarted daemon reports the unfinished write with the workspace and its checkpoint, writes nothing into the workspace, and the agent's next sync finishes it.
#[tokio::test]
async fn a_sync_stopped_after_its_writes_is_reported_at_start() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Opened::new().await;
    let (id, dir) = fx.create(1).await;
    let lib = dir.join("src/lib.rs");
    fs::write(&lib, "pub fn mine() {}\n").unwrap();
    fx.move_main(&[("src/lib.rs", b"pub fn theirs() {}\n")])
        .await;
    fail::cfg("workspace_sync_after_write", "return").unwrap();
    let stopped = fx
        .workspaces()
        .sync_workspace(&agent("frontend"), id)
        .await
        .unwrap_err();
    fail::remove("workspace_sync_after_write");
    assert_eq!(stopped.code(), Some("failpoint"));
    let half = fs::read(&lib).unwrap();
    let mark = workspaces::sync_checkpoint(fx.handle.store.read(), id)
        .await
        .unwrap()
        .unwrap();

    fx.restart(|| {}).await;
    assert_eq!(fs::read(&lib).unwrap(), half, "the daemon wrote nothing");
    let done = reconcile_workspaces(&fx.handle).await;
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].workspace, id);
    assert_eq!(done[0].outcome, Reconciled::Attached);
    assert_eq!(done[0].unfinished, Some(mark));
    let (_, changes) = fx.get(&format!("/workspaces/{id}/changes")).await;
    assert_eq!(changes["unfinished_write"], mark.to_string());

    fx.workspaces()
        .sync_workspace(&agent("frontend"), id)
        .await
        .unwrap();
    let done = reconcile_workspaces(&fx.handle).await;
    assert_eq!(done[0].unfinished, None);
    let (_, changes) = fx.get(&format!("/workspaces/{id}/changes")).await;
    assert!(changes["unfinished_write"].is_null());
    scenario.teardown();
}
