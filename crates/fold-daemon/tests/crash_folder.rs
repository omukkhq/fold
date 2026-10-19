#![cfg(test)]

use std::path::PathBuf;

use fold_daemon::services::projects::RecordOutcome;
use fold_store::testing::eventually;

use crate::common::acceptance::{Harness, WAIT};
use crate::human_changes::{integrate, record_as_ana};

/// The daemon stops after fold's checkout wrote main's files into the project folder and before its record names them: the restarted daemon takes the files as written, so no record turns fold's own writes into a change of the human's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_between_the_folder_writes_and_the_record_loses_nothing_and_records_nothing() {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    fail::cfg("folder_after_write", "return").unwrap();
    let revision = integrate(&h, "frontend", &[("src.txt", "agent\n")]).await;
    eventually(WAIT, async || {
        if dir.join("src.txt").is_file() {
            Ok(())
        } else {
            Err("the folder is written before the stop")
        }
    })
    .await;
    let stale = fold_store::workspaces::checkout(h.handle.store.read())
        .await
        .unwrap()
        .unwrap()
        .revision;
    assert_eq!(stale.to_string(), "r1");
    fail::remove("folder_after_write");

    h.restart().await;
    eventually(WAIT, async || {
        let at = fold_store::workspaces::checkout(h.handle.store.read())
            .await
            .unwrap()
            .unwrap()
            .revision
            .to_string();
        if at == revision { Ok(()) } else { Err(at) }
    })
    .await;
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::UpToDate { .. }
    ));
    let human_changes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM human_changes")
        .fetch_one(h.handle.store.read())
        .await
        .unwrap();
    assert_eq!(human_changes, 0);
    scenario.teardown();
}

/// The daemon stops after the folder writes, and the human then changes a file main changed: the restarted daemon holds the folder on that file and takes fold's other files as written, so the record holds the human's file alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_folder_after_a_stop_takes_the_written_files_as_fold_s() {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    let first = integrate(&h, "frontend", &[("plan.md", "a\nb\nc\nd\ne\n")]).await;
    eventually(WAIT, async || {
        let at = checkout_revision(&h).await;
        if at == first { Ok(()) } else { Err(at) }
    })
    .await;
    fail::cfg("folder_after_write", "return").unwrap();
    integrate(
        &h,
        "frontend",
        &[("plan.md", "a\nb\nc\nd\nE\n"), ("src.txt", "agent\n")],
    )
    .await;
    eventually(WAIT, async || {
        if dir.join("src.txt").is_file() {
            Ok(())
        } else {
            Err("the folder is written before the stop")
        }
    })
    .await;
    h.stop().await;
    fail::remove("folder_after_write");
    std::fs::write(dir.join("plan.md"), "A\nb\nc\nd\nE\n").unwrap();

    h.start_again().await;
    eventually(WAIT, async || match h.handle.git_attention.folder_held() {
        Some((_, paths)) if paths == ["plan.md"] => Ok(()),
        other => Err(other),
    })
    .await;
    assert_eq!(checkout_revision(&h).await, first);
    let RecordOutcome::Recorded { files, .. } = record_as_ana(&h).await.unwrap() else {
        panic!("a clean record");
    };
    assert_eq!(files, ["plan.md"]);
    scenario.teardown();
}

async fn checkout_revision(h: &Harness) -> String {
    fold_store::workspaces::checkout(h.handle.store.read())
        .await
        .unwrap()
        .unwrap()
        .revision
        .to_string()
}
