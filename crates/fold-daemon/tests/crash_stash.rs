#![cfg(test)]

use std::path::PathBuf;

use fold_daemon::services::projects::{StashOutcome, UnstashOutcome};

use crate::common::acceptance::Harness;
use crate::folder_stash::{entries, refusal, stash_as_ana, unstash_as_ana, wait_for};
use crate::human_changes::{integrate, record_as_ana};

fn failed_at<T: std::fmt::Debug>(
    result: Result<T, fold_daemon::services::projects::ProjectError>,
    failpoint: &str,
) {
    match result {
        Err(error) => assert_eq!(error.code(), Some("failpoint"), "{failpoint}: {error}"),
        Ok(outcome) => panic!("{failpoint}: expected the failpoint, got {outcome:?}"),
    }
}

/// A crash after the entry's row and before its paths are put back leaves the changes both in the entry and in the folder: the next `fld stash` puts them back with no second entry, and `fld unstash` brings them back once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crash_inside_fld_stash_keeps_the_changes_and_stashes_them_once() {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    std::fs::write(dir.join("README.md"), "# mine\n").unwrap();
    fail::cfg("stash_after_row", "return").unwrap();
    failed_at(stash_as_ana(&h).await, "stash_after_row");
    fail::remove("stash_after_row");
    assert_eq!(entries(&h).await.len(), 1);
    assert_eq!(
        std::fs::read_to_string(dir.join("README.md")).unwrap(),
        "# mine\n"
    );

    h.restart().await;
    let StashOutcome::Stashed { entries: count, .. } = stash_as_ana(&h).await.unwrap() else {
        panic!("the stash again");
    };
    assert_eq!(count, 1);
    assert_eq!(entries(&h).await.len(), 1);
    assert_eq!(
        std::fs::read_to_string(dir.join("README.md")).unwrap(),
        "# demo\n"
    );
    assert!(matches!(
        unstash_as_ana(&h).await.unwrap(),
        UnstashOutcome::Applied { entries: 0, .. }
    ));
    assert_eq!(
        std::fs::read_to_string(dir.join("README.md")).unwrap(),
        "# mine\n"
    );
    scenario.teardown();
}

/// A crash after the unstash wrote the files and before its row leaves the entry on the stack and its change in the folder; the unstash after the restart finds the change there, merges nothing twice and closes the entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crash_inside_a_clean_fld_unstash_applies_the_entry_once_after_the_restart() {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    std::fs::write(dir.join("README.md"), "# demo\nmine\n").unwrap();
    stash_as_ana(&h).await.unwrap();
    fail::cfg("unstash_after_write", "return").unwrap();
    failed_at(unstash_as_ana(&h).await, "unstash_after_write");
    fail::remove("unstash_after_write");
    assert_eq!(entries(&h).await.len(), 1);
    assert_eq!(
        std::fs::read_to_string(dir.join("README.md")).unwrap(),
        "# demo\nmine\n"
    );

    h.restart().await;
    assert!(matches!(
        unstash_as_ana(&h).await.unwrap(),
        UnstashOutcome::Applied { entries: 0, .. }
    ));
    assert_eq!(
        std::fs::read_to_string(dir.join("README.md")).unwrap(),
        "# demo\nmine\n"
    );
    assert!(entries(&h).await.is_empty());
    scenario.teardown();
}

/// A crash after a conflicting unstash wrote its markers and before its row: no second unstash writes markers into markers, no stash takes them into a new entry, and `fld record` refuses the marker file until the human fixes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crash_inside_a_conflicting_fld_unstash_never_records_or_nests_its_markers() {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    std::fs::write(dir.join("README.md"), "# mine\n").unwrap();
    stash_as_ana(&h).await.unwrap();
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    wait_for(&dir, "README.md", "# fold\n").await;
    fail::cfg("unstash_after_write", "return").unwrap();
    failed_at(unstash_as_ana(&h).await, "unstash_after_write");
    fail::remove("unstash_after_write");
    let marked = std::fs::read_to_string(dir.join("README.md")).unwrap();
    assert!(marked.contains(">>>>>>> stash@{0}"), "{marked}");

    h.restart().await;
    // The unstash finds its markers and keeps the entry as conflicted, so the folder waits for the fix.
    assert_eq!(
        refusal(unstash_as_ana(&h).await.unwrap_err()).0,
        "stash_markers"
    );
    let listed = entries(&h).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].conflicted, ["README.md"]);
    assert_eq!(
        refusal(stash_as_ana(&h).await.unwrap_err()).0,
        "stash_conflicted"
    );
    assert_eq!(
        refusal(record_as_ana(&h).await.unwrap_err()).0,
        "stash_markers"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("README.md")).unwrap(),
        marked
    );
    std::fs::write(dir.join("README.md"), "# fold, and mine\n").unwrap();
    assert!(record_as_ana(&h).await.is_ok());
    assert!(entries(&h).await.is_empty());
    scenario.teardown();
}

/// A stop inside a conflicting unstash, then a record that finds the markers: the entry is kept as conflicted, and the record after the fix drops it, so no later unstash applies it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_after_a_crash_inside_a_conflicting_unstash_keeps_the_entry_until_the_fix() {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    std::fs::write(dir.join("README.md"), "# mine\n").unwrap();
    stash_as_ana(&h).await.unwrap();
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    wait_for(&dir, "README.md", "# fold\n").await;
    fail::cfg("unstash_after_write", "return").unwrap();
    failed_at(unstash_as_ana(&h).await, "unstash_after_write");
    fail::remove("unstash_after_write");

    h.restart().await;
    assert_eq!(
        refusal(record_as_ana(&h).await.unwrap_err()).0,
        "stash_markers"
    );
    assert_eq!(entries(&h).await[0].conflicted, ["README.md"]);
    std::fs::write(dir.join("README.md"), "# fold, and mine\n").unwrap();
    assert!(record_as_ana(&h).await.is_ok());
    assert!(entries(&h).await.is_empty());
    assert_eq!(
        refusal(unstash_as_ana(&h).await.unwrap_err()).0,
        "stash_empty"
    );
    scenario.teardown();
}

/// A stop inside a clean unstash, then `fld record` of the folder that holds the entry's changes: the record closes the entry as applied, so a later unstash does not apply it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_after_a_crash_inside_a_clean_unstash_applies_the_entry() {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    std::fs::write(dir.join("README.md"), "# demo\nmine\n").unwrap();
    stash_as_ana(&h).await.unwrap();
    fail::cfg("unstash_after_write", "return").unwrap();
    failed_at(unstash_as_ana(&h).await, "unstash_after_write");
    fail::remove("unstash_after_write");

    h.restart().await;
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        fold_daemon::services::projects::RecordOutcome::Recorded { .. }
    ));
    assert!(entries(&h).await.is_empty());
    assert_eq!(
        refusal(unstash_as_ana(&h).await.unwrap_err()).0,
        "stash_empty"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("README.md")).unwrap(),
        "# demo\nmine\n"
    );
    scenario.teardown();
}
