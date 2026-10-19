#![cfg(test)]

use std::time::Duration;

use crate::git_support::{
    approved, assert_map_matches_rows, commit_of, git, start_job, wait_for_rows,
};
use crate::reviews_support::{Fixture, p};
use fold_core::events::types;
use fold_daemon::services::events::BusMessage;
use fold_vcs::git::Git;
use fold_vcs::git_remote::FOLD_HEAD_REF;
use fold_vcs::git_sync_map::SYNC_MAP_FILE;

/// `P-1` integrated as `r2`, with no Git commit written yet.
async fn integrated(fx: &Fixture) {
    let review = approved(fx).await;
    fx.reviews
        .integrate_proposal(&review, p(1), Some(1))
        .await
        .unwrap();
}

async fn commit(fx: &Fixture) -> Result<usize, Option<&'static str>> {
    crate::git_support::sync(fx)
        .await
        .map(|committed| committed.added.len())
        .map_err(|error| error.code())
}

/// The `git.committed` events that name a `revision_commits` row's revision and commit.
const COMMITTED_EVENTS_OF_ROWS: &str = r#"SELECT COUNT(*) FROM events e JOIN revision_commits rc ON rc.revision_id = e.revision_id AND rc."commit" = json_extract(e.data, '$.commit') WHERE e.type = 'git.committed'"#;

fn map(fx: &Fixture) -> String {
    std::fs::read_to_string(fx.project.paths().fold_dir().join(SYNC_MAP_FILE)).unwrap_or_default()
}

/// The daemon stops after the integration commits and before Git runs; the restarted daemon writes the missing commits once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_before_the_git_write_is_caught_up_after_the_restart() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    fail::cfg("git_commit_before_write", "return").unwrap();
    let mut bus = fx.bus.subscribe();
    let job = start_job(&fx, Git::default());
    integrated(&fx).await;
    let mut failed = 0;
    tokio::time::timeout(Duration::from_secs(30), async {
        // One failure at the start and one after the integration.
        while failed < 2 {
            if let Ok(BusMessage::Durable(event)) = bus.recv().await
                && event.event_type == types::GIT_COMMIT_FAILED
            {
                assert_eq!(event.data["code"], "failpoint");
                failed += 1;
            }
        }
    })
    .await
    .unwrap();
    drop(job);
    fail::remove("git_commit_before_write");
    assert_eq!(fx.count("SELECT COUNT(*) FROM revision_commits").await, 0);
    assert_eq!(map(&fx), "");

    fx.restart().await;
    let _job = start_job(&fx, Git::default());
    wait_for_rows(&fx, 2).await;
    assert_map_matches_rows(&fx, 2).await;
    let mirror = fx.project.paths().git_dir();
    assert_eq!(git(&mirror, &["rev-list", "--count", "main"]).trim(), "2");
    assert_eq!(commit(&fx).await, Ok(0));
    assert_map_matches_rows(&fx, 2).await;
    scenario.teardown();
}

/// fast-import moved fold's ref and the map never got the lines: the next run writes the same commits again.
#[tokio::test]
async fn a_stop_before_the_map_writes_the_same_commits_on_the_next_run() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    integrated(&fx).await;
    fail::cfg("git_commit_before_map", "return").unwrap();
    assert_eq!(commit(&fx).await, Err(Some("failpoint")));
    fail::remove("git_commit_before_map");
    let mirror = fx.project.paths().git_dir();
    let written = git(&mirror, &["rev-parse", FOLD_HEAD_REF])
        .trim()
        .to_owned();
    assert_eq!(map(&fx), "");
    assert_eq!(fx.count("SELECT COUNT(*) FROM revision_commits").await, 0);

    fx.restart().await;
    assert_eq!(commit(&fx).await, Ok(2));
    assert_eq!(commit_of(&fx, 2).await, written);
    assert_map_matches_rows(&fx, 2).await;
    scenario.teardown();
}

/// The map has the lines and the rows were cut off: the next run fills the rows from the map and writes no commit.
#[tokio::test]
async fn a_stop_before_the_rows_fills_them_from_the_map() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    integrated(&fx).await;
    fail::cfg("git_commit_before_rows", "return").unwrap();
    assert_eq!(commit(&fx).await, Err(Some("failpoint")));
    fail::remove("git_commit_before_rows");
    assert_eq!(map(&fx).lines().count(), 2);
    assert_eq!(fx.count("SELECT COUNT(*) FROM revision_commits").await, 0);

    fx.restart().await;
    assert_eq!(commit(&fx).await, Ok(0));
    assert_map_matches_rows(&fx, 2).await;
    assert_eq!(fx.count(COMMITTED_EVENTS_OF_ROWS).await, 2);
    scenario.teardown();
}

/// The map has the commit and the branch never moved, while fold's checkout wrote the folder already: the next run moves the branch and the index to it.
#[tokio::test]
async fn a_stop_before_the_branch_move_is_finished_by_the_next_run() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    assert_eq!(commit(&fx).await, Ok(1));
    let root = fx.project.paths().root().to_owned();
    let first = git(&root, &["rev-parse", "main"]).trim().to_owned();
    assert_eq!(first, commit_of(&fx, 1).await);
    integrated(&fx).await;
    fail::cfg("git_commit_before_branch", "return").unwrap();
    assert_eq!(commit(&fx).await, Err(Some("failpoint")));
    fail::remove("git_commit_before_branch");
    assert_eq!(map(&fx).lines().count(), 2);
    assert_eq!(git(&root, &["rev-parse", "main"]).trim(), first);
    assert!(root.join("src/new.rs").is_file());

    fx.restart().await;
    assert_eq!(commit(&fx).await, Ok(0));
    assert_map_matches_rows(&fx, 2).await;
    assert_eq!(
        git(&root, &["rev-parse", "main"]).trim(),
        commit_of(&fx, 2).await
    );
    assert!(root.join("src/new.rs").is_file());
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
    scenario.teardown();
}
