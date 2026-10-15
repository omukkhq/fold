#![cfg(test)]

use fold_core::{RevisionId, TaskStatus};

use crate::auto_integrate::{Running, approve, head, integrator, state_of, status};
use crate::common::invariants::assert_consistent;
use crate::proposal_tools_support::Fixture;

fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

async fn succeeded(fx: &Fixture) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM integrations i JOIN completions c ON c.integration_id = i.id
         WHERE i.result = 'succeeded' AND i.actor = 'fold'",
    )
    .fetch_one(fx.handle.store.read())
    .await
    .unwrap()
}

/// The job stops at `failpoint` inside the automatic integration of the approved `P-1`, and the daemon restarts. Main shows all of the integration (`kept`) or none of it, and the restarted job's first run finishes what is left.
async fn stopped_inside(failpoint: &str, kept: bool) {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    let author = fx.agent("backend").await;
    let review = integrator(&fx, "review").await;
    fx.proposal(&author, 1, "pub fn y() {}\n").await;
    let job = Running::start(&fx.state);
    job.runs(1).await;

    fail::cfg(failpoint, "return").unwrap();
    approve(&fx, &review, 1).await;
    job.runs(2).await;
    fail::remove(failpoint);
    job.stop().await;

    if kept {
        assert_eq!(head(&fx).await, r(2));
        assert_eq!(state_of(&fx, 1).await, "integrated");
        assert_eq!(succeeded(&fx).await, 1);
    } else {
        assert_eq!(head(&fx).await, r(1));
        assert_eq!(state_of(&fx, 1).await, "open");
        assert_eq!(succeeded(&fx).await, 0);
        assert_eq!(status(&fx, 1).await, TaskStatus::Review);
    }
    assert_consistent(&fx.project).await;

    fx.restart().await;
    let job = Running::start(&fx.state);
    job.runs(1).await;
    assert_eq!(head(&fx).await, r(2));
    assert_eq!(state_of(&fx, 1).await, "integrated");
    assert_eq!(status(&fx, 1).await, TaskStatus::Done);
    assert_eq!(succeeded(&fx).await, 1, "the integration happened once");
    assert_consistent(&fx.project).await;
    job.stop().await;
    scenario.teardown();
}

#[tokio::test]
async fn a_stop_inside_the_transaction_leaves_none_and_the_job_retries() {
    stopped_inside("integration_in_transaction", false).await;
}

#[tokio::test]
async fn a_stop_after_the_merge_leaves_none_and_the_job_retries() {
    stopped_inside("integration_after_merge", false).await;
}

#[tokio::test]
async fn a_stop_after_the_commit_keeps_all_and_the_job_does_not_repeat_it() {
    stopped_inside("integration_after_commit", true).await;
}
