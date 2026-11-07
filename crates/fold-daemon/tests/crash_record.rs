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

/// A crash inside the integration that records a Git commit of the human leaves main as it was, and the restarted Git job records the commit once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crash_while_a_git_commit_is_recorded_records_it_once_after_the_restart() {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start().await;
    branch_born(&h).await;
    fail::cfg("integration_in_transaction", "return").unwrap();
    let root = std::path::PathBuf::from(h.root());
    std::fs::write(root.join("plan.md"), "the plan\n").unwrap();
    crate::human_changes::git(&root, &["add", "plan.md"]);
    crate::human_changes::git(&root, &["commit", "--quiet", "-m", "plan"]);
    // Each run of the Git job stops inside the transaction; main stays on r1.
    let context = fold_daemon::services::projects::RemoteContext::of(&h.handle);
    let stopped =
        fold_daemon::services::projects::sync_folder(&context, &fold_vcs::git::Git::default())
            .await
            .unwrap_err();
    assert_eq!(stopped.code(), Some("failpoint"));
    assert_eq!(h.head().await.number.to_string(), "r1");
    assert_eq!(rows(&h, "SELECT COUNT(*) FROM human_changes").await, 0);
    fail::remove("integration_in_transaction");

    h.restart().await;
    fold_store::testing::eventually(crate::common::acceptance::WAIT, async || {
        let head = h.head().await.number.to_string();
        if head == "r2" { Ok(()) } else { Err(head) }
    })
    .await;
    assert_eq!(
        file_of(&h, "r2", "plan.md").await.as_deref(),
        Some("the plan\n")
    );
    assert_eq!(rows(&h, "SELECT COUNT(*) FROM human_changes").await, 1);
    scenario.teardown();
}

/// A confirmed record of a worker's change that stops inside its integration leaves the change marked as the worker's, so the next record asks again; the record that reaches main ends the mark as recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_record_that_fails_keeps_a_workers_change_marked() {
    use fold_core::{Actor, AgentName, HumanName, Timestamp};
    use fold_daemon::services::projects::{ProjectError, record};
    use fold_vcs::git::Git;

    let scenario = fail::FailScenario::setup();
    let h = Harness::start().await;
    branch_born(&h).await;
    let root = std::path::PathBuf::from(h.root());
    std::fs::write(root.join("notes.txt"), "from the worker\n").unwrap();
    let lead = AgentName::parse("web-lead").unwrap();
    let at = Timestamp::from_millis(1).unwrap();
    h.handle
        .store
        .write(async |tx| {
            fold_store::agents::insert_agent(tx, &lead, at).await?;
            fold_store::workspaces::insert_agent_change(tx, b"notes.txt", &lead, at, at).await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    let ana = Actor::human(HumanName::parse("ana").unwrap());
    let asked = |error: ProjectError| match error {
        ProjectError::Fold(error) => {
            assert_eq!(error.code(), "record_needs_confirmation");
            let tree = error.details().unwrap()["tree"].as_str().unwrap();
            tree.parse::<fold_core::object::ObjectHash>().unwrap()
        }
        ProjectError::Store(error) => panic!("{error}"),
    };
    let tree = asked(
        record(&h.handle, &ana, &Git::default(), None)
            .await
            .unwrap_err(),
    );

    fail::cfg("integration_in_transaction", "return").unwrap();
    let stopped = record(&h.handle, &ana, &Git::default(), Some(tree)).await;
    fail::remove("integration_in_transaction");
    assert_eq!(stopped.unwrap_err().code(), Some("failpoint"));
    assert_eq!(h.head().await.number.to_string(), "r1");
    assert_eq!(
        rows(
            &h,
            "SELECT COUNT(*) FROM agent_changes WHERE cleared_at IS NULL"
        )
        .await,
        1
    );
    let tree = asked(
        record(&h.handle, &ana, &Git::default(), None)
            .await
            .unwrap_err(),
    );

    let done = record(&h.handle, &ana, &Git::default(), Some(tree))
        .await
        .unwrap();
    let RecordOutcome::Recorded { files, .. } = done.outcome else {
        panic!("{:?}", done.outcome);
    };
    assert_eq!(files, ["notes.txt"]);
    assert_eq!(
        rows(
            &h,
            "SELECT COUNT(*) FROM agent_changes WHERE cleared_how = 'recorded'"
        )
        .await,
        1
    );
    scenario.teardown();
}
