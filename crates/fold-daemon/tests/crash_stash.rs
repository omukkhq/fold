#![cfg(test)]

use std::path::{Path, PathBuf};

use fold_core::EventFilter;
use fold_core::events::types;
use fold_daemon::services::projects::{
    RecordOutcome, RemoteContext, StashOutcome, UnstashOutcome, sync_folder,
};
use fold_vcs::git::Git;

use crate::common::acceptance::Harness;
use crate::folder_stash::{
    entries, refusal, stash_as_ana, stash_finishing_as_ana, unstash_as_ana, wait_for,
};
use crate::human_changes::{branch_born, integrate, record_as_ana, record_finishing_as_ana};

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

/// A crash after a conflicting unstash wrote its markers and before its row: the unstash after the restart finishes it by its journal and keeps the entry as conflicted, so no second unstash writes markers into markers, no stash takes them into a new entry, and `fld record` refuses the marker file until the human fixes it.
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
    let UnstashOutcome::Conflicted { conflicted, .. } = unstash_as_ana(&h).await.unwrap() else {
        panic!("the unstash that stopped is finished as conflicted");
    };
    assert_eq!(conflicted[0].path, "README.md");
    assert_eq!(
        refusal(unstash_as_ana(&h).await.unwrap_err()).0,
        "stash_conflicted"
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

/// A stop inside a conflicting unstash, then a record that finds the markers: the record finishes the unstash by its journal and keeps the entry as conflicted, and the record after the fix drops it, so no later unstash applies it again.
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

/// A stop inside a clean unstash, then `fld record` of the folder that holds the entry's changes: the record finishes the unstash by its journal and closes the entry as applied, so a later unstash does not apply it again.
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

/// A project with Git sync on and its branch born, or a plain one.
async fn project(with_git: bool) -> Harness {
    if with_git {
        let h = Harness::start().await;
        branch_born(&h).await;
        h
    } else {
        Harness::start_plain().await
    }
}

fn read(dir: &Path, path: &str) -> String {
    std::fs::read_to_string(dir.join(path)).unwrap()
}

/// The paths that hold the project folder where it is, after one follow.
async fn held(h: &Harness) -> Vec<String> {
    sync_folder(&RemoteContext::of(&h.handle), &Git::default())
        .await
        .unwrap()
        .followed
        .held
}

async fn stored(h: &Harness, kind: fold_core::EventType) -> usize {
    let filter = EventFilter {
        types: vec![kind],
        ..EventFilter::default()
    };
    fold_store::events::read_after(h.handle.store.read(), 0, &filter, 100)
        .await
        .unwrap()
        .len()
}

/// The main revision the checkout record says the project folder holds.
async fn folder_revision(h: &Harness) -> fold_core::RevisionId {
    fold_store::workspaces::checkout(h.handle.store.read())
        .await
        .unwrap()
        .unwrap()
        .revision
}

/// True while an unstash's journal row is open.
async fn journal_open(h: &Harness) -> bool {
    let mut conn = h.handle.store.read().acquire().await.unwrap();
    fold_store::workspaces::open_unstash(&mut conn)
        .await
        .unwrap()
        .is_some()
}

/// The stashed `README.md` and the new `plan.md` merge cleanly into the folder, which an unstash writes in that order.
async fn clean_stash(h: &Harness) -> PathBuf {
    let dir = PathBuf::from(h.root());
    std::fs::write(dir.join("README.md"), "# demo\nmine\n").unwrap();
    std::fs::write(dir.join("plan.md"), "the plan\n").unwrap();
    stash_as_ana(h).await.unwrap();
    assert_eq!(read(&dir, "README.md"), "# demo\n");
    dir
}

/// The stashed `README.md` conflicts with main and the stashed `plan.md` is new, so an unstash writes `plan.md` first and then the markers of `README.md`.
async fn conflicting_stash(h: &Harness) -> PathBuf {
    let dir = PathBuf::from(h.root());
    std::fs::write(dir.join("README.md"), "# mine\n").unwrap();
    std::fs::write(dir.join("plan.md"), "the plan\n").unwrap();
    stash_as_ana(h).await.unwrap();
    integrate(h, "frontend", &[("README.md", "# fold\n")]).await;
    wait_for(&dir, "README.md", "# fold\n").await;
    dir
}

/// Stops the unstash at `failpoint`.
async fn stop_unstash_at(h: &Harness, failpoint: &str) {
    fail::cfg(failpoint, "return").unwrap();
    failed_at(unstash_as_ana(h).await, failpoint);
    fail::remove(failpoint);
}

async fn a_stop_before_the_first_write_leaves_the_folder_and_the_entry(with_git: bool) {
    let scenario = fail::FailScenario::setup();
    let mut h = project(with_git).await;
    let dir = conflicting_stash(&h).await;
    stop_unstash_at(&h, "unstash_after_journal").await;
    assert_eq!(read(&dir, "README.md"), "# fold\n");
    assert!(!dir.join("plan.md").exists());
    integrate(&h, "frontend", &[("notes.md", "notes\n")]).await;

    h.restart().await;
    // While the journal row is open the folder waits on the paths the unstash writes.
    assert_eq!(held(&h).await, ["README.md", "plan.md"]);
    assert!(!dir.join("notes.md").exists());
    // Nothing of the unstash reached the folder, so the record finds nothing, the entry stays stashed and the folder follows main again.
    let done = record_finishing_as_ana(&h).await.unwrap();
    assert!(done.unstash.is_none());
    let RecordOutcome::UpToDate { revision } = done.outcome else {
        panic!("nothing to record: {:?}", done.outcome);
    };
    assert_eq!(folder_revision(&h).await, revision);
    assert_eq!(read(&dir, "notes.md"), "notes\n");
    assert!(!journal_open(&h).await);
    let listed = entries(&h).await;
    assert_eq!(listed.len(), 1);
    assert!(listed[0].conflicted.is_empty());
    assert!(held(&h).await.is_empty());
    let UnstashOutcome::Conflicted { conflicted, .. } = unstash_as_ana(&h).await.unwrap() else {
        panic!("README.md conflicts with main");
    };
    assert_eq!(conflicted.len(), 1);
    assert_eq!(read(&dir, "plan.md"), "the plan\n");
    assert!(read(&dir, "README.md").contains(">>>>>>> stash@{0}"));
    assert_eq!(stored(&h, types::STASH_CONFLICTED).await, 1);
    scenario.teardown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_before_the_first_write_of_an_unstash_leaves_the_folder_and_the_entry() {
    a_stop_before_the_first_write_leaves_the_folder_and_the_entry(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_git_sync_a_stop_before_the_first_write_of_an_unstash_leaves_the_folder_and_the_entry()
{
    a_stop_before_the_first_write_leaves_the_folder_and_the_entry(true).await;
}

async fn a_stop_after_some_writes_is_finished_by_the_journal(with_git: bool) {
    let scenario = fail::FailScenario::setup();
    let mut h = project(with_git).await;
    let dir = conflicting_stash(&h).await;
    stop_unstash_at(&h, "merge_after_first_write").await;
    assert_eq!(read(&dir, "plan.md"), "the plan\n");
    assert_eq!(read(&dir, "README.md"), "# fold\n");

    h.restart().await;
    // The record writes the markers the stop left out, keeps the entry as conflicted as the journal says, and refuses them.
    let (reason, paths) = refusal(record_as_ana(&h).await.unwrap_err());
    assert_eq!(reason, "stash_markers");
    assert_eq!(paths, serde_json::json!(["README.md"]));
    assert!(read(&dir, "README.md").contains(">>>>>>> stash@{0}"));
    assert_eq!(entries(&h).await[0].conflicted, ["README.md"]);
    assert_eq!(stored(&h, types::STASH_CONFLICTED).await, 1);
    std::fs::write(dir.join("README.md"), "# fold, and mine\n").unwrap();
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::Recorded { .. }
    ));
    assert!(entries(&h).await.is_empty());
    assert_eq!(read(&dir, "plan.md"), "the plan\n");
    assert_eq!(
        refusal(unstash_as_ana(&h).await.unwrap_err()).0,
        "stash_empty"
    );
    scenario.teardown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_after_some_writes_of_an_unstash_is_finished_by_its_journal() {
    a_stop_after_some_writes_is_finished_by_the_journal(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_git_sync_a_stop_after_some_writes_of_an_unstash_is_finished_by_its_journal() {
    a_stop_after_some_writes_is_finished_by_the_journal(true).await;
}

async fn markers_fixed_by_hand_after_a_stop_never_apply_the_entry_again(with_git: bool) {
    let scenario = fail::FailScenario::setup();
    let mut h = project(with_git).await;
    let dir = conflicting_stash(&h).await;
    stop_unstash_at(&h, "unstash_after_write").await;
    assert!(read(&dir, "README.md").contains(">>>>>>> stash@{0}"));

    // The human fixes the markers before fold looks at the folder again.
    std::fs::write(dir.join("README.md"), "# fold, and mine\n").unwrap();
    h.restart().await;
    let RecordOutcome::Recorded { files, .. } = record_as_ana(&h).await.unwrap() else {
        panic!("the fixed files are recorded");
    };
    assert_eq!(files, ["README.md", "plan.md"]);
    assert!(entries(&h).await.is_empty());
    assert_eq!(stored(&h, types::STASH_CONFLICTED).await, 1);
    assert_eq!(stored(&h, types::STASH_DROPPED).await, 1);
    assert_eq!(
        refusal(unstash_as_ana(&h).await.unwrap_err()).0,
        "stash_empty"
    );
    assert_eq!(read(&dir, "README.md"), "# fold, and mine\n");
    assert_eq!(read(&dir, "plan.md"), "the plan\n");
    scenario.teardown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn markers_fixed_by_hand_after_a_stop_inside_an_unstash_never_apply_the_entry_again() {
    markers_fixed_by_hand_after_a_stop_never_apply_the_entry_again(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_git_sync_markers_fixed_by_hand_after_a_stop_inside_an_unstash_never_apply_the_entry_again()
 {
    markers_fixed_by_hand_after_a_stop_never_apply_the_entry_again(true).await;
}

async fn a_stop_after_all_writes_is_finished_by_the_next_unstash(with_git: bool) {
    let scenario = fail::FailScenario::setup();
    let mut h = project(with_git).await;
    let dir = conflicting_stash(&h).await;
    stop_unstash_at(&h, "unstash_after_write").await;
    let marked = read(&dir, "README.md");

    h.restart().await;
    let UnstashOutcome::Conflicted {
        conflicted,
        entries: count,
        ..
    } = unstash_as_ana(&h).await.unwrap()
    else {
        panic!("the unstash that stopped is finished as conflicted");
    };
    assert_eq!(count, 1);
    assert_eq!(conflicted[0].path, "README.md");
    assert_eq!(read(&dir, "README.md"), marked);
    assert_eq!(held(&h).await, ["README.md"]);
    assert_eq!(
        refusal(stash_as_ana(&h).await.unwrap_err()).0,
        "stash_conflicted"
    );
    scenario.teardown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_after_all_writes_of_an_unstash_is_finished_by_the_next_unstash() {
    a_stop_after_all_writes_is_finished_by_the_next_unstash(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_git_sync_a_stop_after_all_writes_of_an_unstash_is_finished_by_the_next_unstash() {
    a_stop_after_all_writes_is_finished_by_the_next_unstash(true).await;
}

/// A file changed after a stop before the first write: nothing of the unstash reached the folder, so it is abandoned and the next unstash merges the entry with the human's change, which keeps both sides.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_changed_after_a_stop_before_the_first_write_is_merged_with_the_entry() {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start_plain().await;
    let dir = conflicting_stash(&h).await;
    stop_unstash_at(&h, "unstash_after_journal").await;
    std::fs::write(dir.join("README.md"), "# fold, edited\n").unwrap();

    h.restart().await;
    let UnstashOutcome::Conflicted { conflicted, .. } = unstash_as_ana(&h).await.unwrap() else {
        panic!("README.md conflicts with the human's change");
    };
    assert_eq!(conflicted.len(), 1);
    let marked = read(&dir, "README.md");
    assert!(
        marked.contains("# fold, edited") && marked.contains("# mine"),
        "{marked}"
    );
    assert_eq!(read(&dir, "plan.md"), "the plan\n");
    assert_eq!(stored(&h, types::STASH_CONFLICTED).await, 1);
    assert!(!journal_open(&h).await);
    scenario.teardown();
}

/// A stop inside the finish, after its writes and before the entry's row: the next finish writes nothing again, so the markers are never nested.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_inside_the_finish_of_an_unstash_finishes_it_once() {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start_plain().await;
    let dir = conflicting_stash(&h).await;
    stop_unstash_at(&h, "merge_after_first_write").await;

    h.restart().await;
    stop_unstash_at(&h, "unstash_finish_after_write").await;
    let marked = read(&dir, "README.md");
    assert_eq!(marked.matches(">>>>>>> stash@{0}").count(), 1, "{marked}");
    assert!(journal_open(&h).await);
    assert!(entries(&h).await[0].conflicted.is_empty());
    let UnstashOutcome::Conflicted { conflicted, .. } = unstash_as_ana(&h).await.unwrap() else {
        panic!("the unstash that stopped is finished as conflicted");
    };
    assert_eq!(conflicted[0].path, "README.md");
    assert_eq!(read(&dir, "README.md"), marked);
    assert_eq!(stored(&h, types::STASH_CONFLICTED).await, 1);
    scenario.teardown();
}

/// A stop part-way through a clean unstash, then `fld stash`: the stash finishes the unstash, answers with its outcome, and stashes the applied files again as one entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stash_after_a_stop_part_way_finishes_the_unstash_and_names_it() {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start_plain().await;
    let dir = clean_stash(&h).await;
    stop_unstash_at(&h, "merge_after_first_write").await;
    assert_eq!(read(&dir, "README.md"), "# demo\nmine\n");
    assert!(!dir.join("plan.md").exists());

    h.restart().await;
    let done = stash_finishing_as_ana(&h).await.unwrap();
    assert!(
        matches!(
            done.unstash,
            Some(UnstashOutcome::Applied { entries: 0, .. })
        ),
        "{:?}",
        done.unstash
    );
    let StashOutcome::Stashed {
        files,
        entries: count,
        ..
    } = done.outcome
    else {
        panic!("the applied files are stashed again: {:?}", done.outcome);
    };
    assert_eq!(
        (files.as_slice(), count),
        (&["README.md".to_owned(), "plan.md".to_owned()][..], 1)
    );
    assert_eq!(read(&dir, "README.md"), "# demo\n");
    assert!(!dir.join("plan.md").exists());
    assert!(!journal_open(&h).await);
    assert!(matches!(
        unstash_as_ana(&h).await.unwrap(),
        UnstashOutcome::Applied { entries: 0, .. }
    ));
    assert_eq!(read(&dir, "README.md"), "# demo\nmine\n");
    assert_eq!(read(&dir, "plan.md"), "the plan\n");
    scenario.teardown();
}

/// A stop part-way through a clean unstash: the unstash after the restart writes the rest and applies the entry once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_part_way_through_a_clean_unstash_applies_the_entry_once() {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start_plain().await;
    let dir = clean_stash(&h).await;
    stop_unstash_at(&h, "merge_after_first_write").await;

    h.restart().await;
    assert!(matches!(
        unstash_as_ana(&h).await.unwrap(),
        UnstashOutcome::Applied { entries: 0, .. }
    ));
    assert_eq!(read(&dir, "README.md"), "# demo\nmine\n");
    assert_eq!(read(&dir, "plan.md"), "the plan\n");
    assert_eq!(stored(&h, types::STASH_APPLIED).await, 1);
    assert_eq!(
        refusal(unstash_as_ana(&h).await.unwrap_err()).0,
        "stash_empty"
    );
    scenario.teardown();
}

/// A file that changes after the journal row and before the first write: the check before the writes refuses, nothing is written, and the row is abandoned, so the next unstash merges the entry with the changed file.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_changed_before_the_first_write_abandons_the_unstash() {
    let scenario = fail::FailScenario::setup();
    let h = Harness::start_plain().await;
    let dir = conflicting_stash(&h).await;
    let readme = dir.join("README.md");
    fail::cfg_callback("unstash_after_journal", move || {
        std::fs::write(&readme, "# changed\n").unwrap();
    })
    .unwrap();
    let refused = unstash_as_ana(&h).await.unwrap_err();
    fail::remove("unstash_after_journal");
    assert_eq!(refusal(refused).0, "changed_during_unstash");
    assert!(!journal_open(&h).await);
    assert!(entries(&h).await[0].conflicted.is_empty());
    assert_eq!(read(&dir, "README.md"), "# changed\n");
    assert!(!dir.join("plan.md").exists());

    let UnstashOutcome::Conflicted { .. } = unstash_as_ana(&h).await.unwrap() else {
        panic!("README.md conflicts with the changed file");
    };
    let marked = read(&dir, "README.md");
    assert!(
        marked.contains("# changed") && marked.contains("# mine"),
        "{marked}"
    );
    scenario.teardown();
}

/// `fld status` names an unstash that stopped part-way and its entry, and leaves the journal for the command that finishes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_folder_status_names_an_unstash_that_stopped_part_way_and_finishes_nothing() {
    let scenario = fail::FailScenario::setup();
    let h = project(false).await;
    conflicting_stash(&h).await;
    stop_unstash_at(&h, "unstash_after_journal").await;
    let (status, body) = h
        .socket(
            axum::http::Method::GET,
            "/api/v1/projects/demo/folder",
            None,
        )
        .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(
        body["blockers"],
        serde_json::json!([{"kind": "unstash_open", "paths": ["README.md", "plan.md"], "next": "run `fld unstash` to finish it"}])
    );
    assert_eq!(body["stash"][0]["state"], "unstashing");
    assert_eq!(body["stash"][0]["next"], true);
    assert!(journal_open(&h).await);
    scenario.teardown();
}
