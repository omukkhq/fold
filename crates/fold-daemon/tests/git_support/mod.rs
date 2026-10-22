use crate::common::roles::give_role;
use fold_core::roles::Role;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use crate::reviews_support::{Fixture, p};
use fold_core::Actor;
use fold_core::reviews::Verdict;
use fold_daemon::jobs::{AutoCommit, auto_commit};
use fold_daemon::services::projects::RemoteContext;
use fold_vcs::git::Git;
use fold_vcs::git_sync_map::SYNC_MAP_FILE;

/// `git` in `dir` without the user's configuration.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Starts the project's Git job over the fixture's bus, as an open project does.
pub fn start_job(fx: &Fixture, git: Git) -> AutoCommit {
    start_watched_job(fx, git).0
}

/// [`start_job`], with the job's context, whose attention a test reads.
pub fn start_watched_job(fx: &Fixture, git: Git) -> (AutoCommit, RemoteContext) {
    let context = RemoteContext::new(
        "app".to_owned(),
        fx.project.paths().clone(),
        fx.project.store().clone(),
        Arc::new(fx.project.clock().clone()),
        fx.bus.clone(),
    );
    (auto_commit(context.clone(), git), context)
}

/// `P-1` approved by `review`, a reviewer, which acts with the returned actor.
pub async fn approved(fx: &Fixture) -> Actor {
    fx.proposal().await;
    give_role(fx.project.store(), "review", Role::Reviewer, None).await;
    let review = fx.agents.actor(&fx.review.ctx).await.unwrap();
    fx.reviews
        .submit_review(
            &review,
            p(1),
            1,
            Verdict::Approve,
            "looks right",
            Vec::new(),
        )
        .await
        .unwrap();
    review
}

/// Waits until `revision_commits` holds `rows` rows.
pub async fn wait_for_rows(fx: &Fixture, rows: i64) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while fx.count("SELECT COUNT(*) FROM revision_commits").await != rows {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

/// The commit `revision_commits` records for the revision.
pub async fn commit_of(fx: &Fixture, revision: u32) -> String {
    fx.text(&format!(
        r#"SELECT c."commit" FROM revision_commits c JOIN revisions r ON r.id = c.revision_id
           WHERE r.number = {revision}"#
    ))
    .await
    .unwrap()
}

/// The map's lines and the rows agree, one per revision.
pub async fn assert_map_matches_rows(fx: &Fixture, revisions: u32) {
    let map = std::fs::read_to_string(fx.project.paths().fold_dir().join(SYNC_MAP_FILE)).unwrap();
    let mut expected = String::new();
    for number in 1..=revisions {
        expected.push_str(&format!("r{number} {}\n", commit_of(fx, number).await));
    }
    assert_eq!(map, expected);
}

/// One run of the Git job on the fixture: the project folder follows main, and the commits and the branch follow it.
pub async fn sync(
    fx: &Fixture,
) -> Result<fold_daemon::services::projects::Committed, fold_daemon::services::projects::ProjectError>
{
    let context = RemoteContext::new(
        "app".to_owned(),
        fx.project.paths().clone(),
        fx.project.store().clone(),
        Arc::new(fx.project.clock().clone()),
        fx.bus.clone(),
    );
    fold_daemon::services::projects::sync_folder(&context, &Git::default())
        .await?
        .committed
}
