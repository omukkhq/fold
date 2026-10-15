#![cfg(test)]

use crate::common::invariants::assert_consistent;
use crate::reviews_support::{Fixture, by, names, t, ws};
use fold_core::events::types;
use fold_core::object::{ObjectKind, hash_object};
use fold_vcs::objects::ObjectStore;

const CHANGED: &str = "pub fn changed() {}\n";

/// `frontend` takes `T-1` and changes `src/lib.rs` in its workspace `ws-1`.
async fn changed_workspace(fx: &Fixture) {
    fx.workspaces
        .create_workspace(&by(&fx.frontend.worker), t(1))
        .await
        .unwrap();
    fx.write("src/lib.rs", CHANGED);
}

async fn checkpoints(fx: &Fixture) -> i64 {
    fx.count("SELECT COUNT(*) FROM checkpoints").await
}

/// A crash before the checkpoint row commits leaves no row and no `checkpoint.recorded`, the workspace files as they were and its objects as harmless orphans; a retry records the checkpoint.
async fn stopped_before_commit(failpoint: &str) {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    changed_workspace(&fx).await;
    let author = by(&fx.frontend.worker);
    let rows = checkpoints(&fx).await;
    let seq = fx.last_seq().await;

    fail::cfg(failpoint, "return").unwrap();
    let result = fx
        .workspaces
        .record_checkpoint(&author, ws(1), Some("before the crash"))
        .await;
    fail::remove(failpoint);
    assert_eq!(result.unwrap_err().code(), Some("failpoint"));
    fx.restart().await;

    assert_eq!(checkpoints(&fx).await, rows);
    assert_eq!(fx.last_seq().await, seq);
    assert_eq!(
        std::fs::read_to_string(fx.task_dir().join("src/lib.rs")).unwrap(),
        CHANGED
    );
    let objects = ObjectStore::open(&fx.project.paths().fold_dir()).unwrap();
    assert!(
        objects
            .contains(&hash_object(ObjectKind::Blob, CHANGED.as_bytes()))
            .unwrap(),
        "the snapshot's blob stays behind as a harmless orphan"
    );
    assert_consistent(&fx.project).await;

    let checkpoint = fx
        .workspaces
        .record_checkpoint(&author, ws(1), Some("after the restart"))
        .await
        .unwrap();
    assert_eq!(checkpoint.label.as_deref(), Some("after the restart"));
    assert_eq!(checkpoints(&fx).await, rows + 1);
    assert_eq!(
        names(&fx.events_after(seq).await),
        [types::CHECKPOINT_RECORDED.as_str()]
    );
    assert_consistent(&fx.project).await;
    scenario.teardown();
}

#[tokio::test]
async fn a_crash_after_the_checkpoint_objects_leaves_no_row() {
    stopped_before_commit("checkpoint_after_objects").await;
}

#[tokio::test]
async fn a_crash_inside_the_checkpoint_transaction_leaves_no_row() {
    stopped_before_commit("checkpoint_in_transaction").await;
}
