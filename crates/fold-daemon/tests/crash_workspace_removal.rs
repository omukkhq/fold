#![cfg(test)]

use std::sync::Arc;

use crate::common::invariants::assert_consistent;
use crate::reviews_support::{Fixture, by, t, ws};
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

    let row = ProjectRow {
        slug: "demo".to_owned(),
        name: "demo".to_owned(),
        root: fx.project.paths().root().to_string_lossy().into_owned(),
    };
    let handle = ProjectHandle::open(&row, Arc::new(fx.project.clock().clone()))
        .await
        .unwrap();
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
