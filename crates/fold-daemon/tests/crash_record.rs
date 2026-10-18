#![cfg(test)]

use fold_daemon::services::projects::RecordOutcome;

use crate::common::acceptance::Harness;
use crate::human_changes::{branch_born, file_of, record_as_ana};

async fn rows(h: &Harness, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(h.handle.store.read())
        .await
        .unwrap()
}

/// Runs `fld record` with `failpoint` set to return, checks that the command reports it and that main, the human changes and the folder stay as they were, then restarts the project as a crash there would and records again: the same changes reach main once.
async fn stopped_at(failpoint: &str) {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start().await;
    branch_born(&h).await;
    let root = std::path::PathBuf::from(h.root());
    std::fs::write(root.join("README.md"), "# mine\n").unwrap();
    std::fs::write(root.join("plan.md"), "the plan\n").unwrap();

    fail::cfg(failpoint, "return").unwrap();
    let result = record_as_ana(&h).await;
    fail::remove(failpoint);
    match result {
        Err(error) => assert_eq!(error.code(), Some("failpoint"), "{failpoint}: {error}"),
        Ok(outcome) => panic!("{failpoint}: expected the failpoint, got {outcome:?}"),
    }
    assert_eq!(h.head().await.number.to_string(), "r1", "{failpoint}");
    assert_eq!(
        rows(&h, "SELECT COUNT(*) FROM human_changes").await,
        0,
        "{failpoint}"
    );
    assert_eq!(
        rows(&h, "SELECT COUNT(*) FROM integrations").await,
        0,
        "{failpoint}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        "# mine\n"
    );

    h.restart().await;
    branch_born(&h).await;
    let outcome = record_as_ana(&h).await.unwrap();
    let RecordOutcome::Recorded {
        revision, files, ..
    } = outcome
    else {
        panic!("{failpoint}: a clean record after the restart: {outcome:?}");
    };
    assert_eq!(revision.to_string(), "r2", "{failpoint}");
    assert_eq!(files, ["README.md", "plan.md"], "{failpoint}");
    assert_eq!(
        file_of(&h, "r2", "plan.md").await.as_deref(),
        Some("the plan\n")
    );
    assert_eq!(
        rows(&h, "SELECT COUNT(*) FROM human_changes").await,
        1,
        "{failpoint}"
    );
    assert_eq!(
        rows(&h, "SELECT COUNT(*) FROM revisions").await,
        2,
        "{failpoint}"
    );
    scenario.teardown();
}

/// A crash after the folder is read and its objects are written but before anything refers to them, or inside the integration's transaction, leaves no trace on main, and the next `fld record` brings in the same files.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crash_inside_fld_record_loses_nothing_and_records_nothing_twice() {
    for failpoint in [
        "record_after_snapshot",
        "integration_after_objects",
        "integration_in_transaction",
    ] {
        stopped_at(failpoint).await;
    }
}
