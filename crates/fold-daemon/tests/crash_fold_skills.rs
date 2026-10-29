#![cfg(test)]

use std::path::PathBuf;

use fold_core::Actor;
use fold_daemon::services::projects::{FOLD_SKILLS_TITLE, FoldSkills, keep_fold_skills_out};
use fold_vcs::git::Git;

use crate::common::acceptance::Harness;
use crate::fold_skills::{OLD_FILES, STOPPED, as_an_older_fold_left_it};
use crate::human_changes::file_of;

async fn fold_revisions(h: &Harness) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM integrations WHERE actor = 'fold' AND kind = 'record'")
        .fetch_one(h.handle.store.read())
        .await
        .unwrap()
}

/// fold's revision stops at `failpoint`; the restarted project ends with main free of fold's skills through one revision of fold, and the files in the folder as they were.
async fn stopped_at(failpoint: &str) {
    let scenario = fail::FailScenario::setup();
    let mut h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    as_an_older_fold_left_it(&h, "").await;

    fail::cfg(failpoint, "return").unwrap();
    let stopped = keep_fold_skills_out(&h.handle, &Actor::System, &Git::default()).await;
    fail::remove(failpoint);
    assert_eq!(
        stopped.unwrap_err().code(),
        Some("failpoint"),
        "{failpoint}"
    );

    h.restart().await;
    let head = h.head().await;
    assert_eq!(
        head.integration.unwrap().message.as_deref(),
        Some(FOLD_SKILLS_TITLE),
        "{failpoint}"
    );
    assert_eq!(fold_revisions(&h).await, 1, "{failpoint}");
    for path in STOPPED {
        assert_eq!(
            file_of(&h, &head.number.to_string(), path).await,
            None,
            "{failpoint}: {path}"
        );
    }
    for (path, text) in OLD_FILES {
        assert_eq!(
            std::fs::read_to_string(dir.join(path)).ok().as_deref(),
            Some(text),
            "{failpoint}: {path}"
        );
    }
    assert_eq!(
        keep_fold_skills_out(&h.handle, &Actor::System, &Git::default())
            .await
            .unwrap(),
        FoldSkills::Untracked
    );
    scenario.teardown();
}

/// A crash inside the transaction of fold's revision leaves main as it was for the next open, and one after its commit leaves the checkout record without the files, so the next follow keeps them in the folder.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crash_inside_folds_own_revision_keeps_the_files_and_makes_the_revision_once() {
    for failpoint in ["integration_in_transaction", "integration_after_commit"] {
        stopped_at(failpoint).await;
    }
}
