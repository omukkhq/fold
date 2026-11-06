#![cfg(test)]

use std::path::PathBuf;

use fold_core::events::types;
use fold_daemon::services::projects::RecordOutcome;
use serde_json::json;

use crate::common::acceptance::Harness;
use crate::folder_reset::{human_change_states, main_workspace_state, reset_as_ana, stored};
use crate::folder_stash::entries;
use crate::human_changes::{branch_born, integrate, record_as_ana};

fn failed_at<T: std::fmt::Debug>(
    result: Result<T, fold_daemon::services::projects::ProjectError>,
    failpoint: &str,
) {
    match result {
        Err(error) => assert_eq!(error.code(), Some("failpoint"), "{failpoint}: {error}"),
        Ok(outcome) => panic!("{failpoint}: expected the failpoint, got {outcome:?}"),
    }
}

/// A crash after the safety entry's row leaves the changes both in the entry and in the folder, and one after the files leaves the human change waiting with its workspace open: after a restart, `fld reset` finishes each step once, so the stack holds one entry, the change is withdrawn once and its workspace is abandoned once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crash_inside_fld_reset_is_finished_by_the_next_reset() {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged("main", &["main"]).await;
    let dir = PathBuf::from(h.root());
    std::fs::write(dir.join("README.md"), "# mine\n").unwrap();
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::Conflicted { .. }
    ));
    h.call("main", "open_main_workspace", json!({})).await;

    fail::cfg("reset_after_row", "return").unwrap();
    failed_at(reset_as_ana(&h).await, "reset_after_row");
    fail::remove("reset_after_row");
    assert_eq!(entries(&h).await.len(), 1);
    assert_eq!(
        std::fs::read_to_string(dir.join("README.md")).unwrap(),
        "# mine\n"
    );
    assert_eq!(human_change_states(&h).await, ["conflicted"]);

    h.restart().await;
    fail::cfg("reset_after_files", "return").unwrap();
    failed_at(reset_as_ana(&h).await, "reset_after_files");
    fail::remove("reset_after_files");
    assert_eq!(entries(&h).await.len(), 1);
    assert_ne!(
        std::fs::read_to_string(dir.join("README.md")).unwrap(),
        "# mine\n"
    );
    assert_eq!(human_change_states(&h).await, ["conflicted"]);
    assert_eq!(main_workspace_state(&h).await, "active");

    h.restart().await;
    let done = reset_as_ana(&h).await.unwrap();
    assert!(!done.stashed);
    assert!(done.at_head);
    assert!(done.dropped_change.is_some());
    assert_eq!(entries(&h).await.len(), 1);
    assert_eq!(
        std::fs::read_to_string(dir.join("README.md")).unwrap(),
        "# fold\n"
    );
    assert_eq!(human_change_states(&h).await, ["superseded"]);
    assert_eq!(main_workspace_state(&h).await, "abandoned");
    assert_eq!(stored(&h, types::STASH_PUSHED).await.len(), 1);
    assert_eq!(stored(&h, types::HUMAN_CHANGE_WITHDRAWN).await.len(), 1);
    assert_eq!(stored(&h, types::WORKSPACE_ABANDONED).await.len(), 1);
    scenario.teardown();
}
