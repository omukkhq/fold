#![cfg(test)]

use std::path::Path;

use crate::common::invariants::assert_consistent;
use crate::reviews_support::{Fixture, human, names, p};
use fold_core::events::types;
use fold_core::reviews::Verdict;
use fold_core::{Actor, AgentName, RevisionId, TaskStatus};
use fold_daemon::services::reviews::IntegrationOutcome;
use fold_store::workspaces;

fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

/// `P-1` approved by `review`, which carries `integrator` and acts with the returned actor.
async fn approved(fx: &Fixture) -> Actor {
    fx.proposal().await;
    fx.agents
        .set_integrator(&human(), &AgentName::parse("review").unwrap(), true)
        .await
        .unwrap();
    let review = fx.agents.actor(&fx.review.ctx, None).await.unwrap();
    fx.reviews
        .submit_review(&review, p(1), Verdict::Approve, "looks right", Vec::new())
        .await
        .unwrap();
    review
}

async fn head(fx: &Fixture) -> RevisionId {
    workspaces::head(fx.project.store().read())
        .await
        .unwrap()
        .unwrap()
        .number
}

/// The number of files in the object store, so a test sees the objects a stopped command left behind.
fn object_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                object_files(&entry.path())
            } else {
                1
            }
        })
        .sum()
}

fn objects(fx: &Fixture) -> usize {
    object_files(&fx.project.paths().fold_dir().join("objects"))
}

/// Integrates `P-1` with `failpoint` set to return, checks the command reports `failpoint`, and restarts the project as a crash there would.
async fn integrate_stopped_at(fx: &mut Fixture, review: &Actor, failpoint: &str) {
    fail::cfg(failpoint, "return").unwrap();
    let result = fx.reviews.integrate_proposal(review, p(1), Some(1)).await;
    fail::remove(failpoint);
    match result {
        Err(error) => assert_eq!(error.code(), Some("failpoint"), "{error}"),
        Ok(outcome) => panic!("expected the failpoint, got {outcome:?}"),
    }
    fx.restart().await;
}

/// A crash before the commit leaves main, the proposal, the task and the workspace as they were, with no revision, integration, completion or event; a retry integrates and ends consistent. `gap` says whether the stopped attempt had already taken integration number 1.
async fn stopped_before_commit(failpoint: &str, gap: bool) {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    let review = approved(&fx).await;
    let seq = fx.last_seq().await;
    let objects_before = objects(&fx);

    integrate_stopped_at(&mut fx, &review, failpoint).await;

    assert_eq!(head(&fx).await, r(1));
    assert_eq!(fx.count("SELECT COUNT(*) FROM revisions").await, 1);
    assert_eq!(fx.count("SELECT COUNT(*) FROM integrations").await, 0);
    assert_eq!(fx.count("SELECT COUNT(*) FROM completions").await, 0);
    assert_eq!(
        fx.text("SELECT state FROM proposals WHERE number = 1")
            .await,
        Some("open".to_owned())
    );
    assert_eq!(fx.status(1).await, TaskStatus::Review);
    assert_eq!(
        fx.text("SELECT state FROM workspaces WHERE number = 1")
            .await,
        Some("active".to_owned())
    );
    assert_eq!(fx.last_seq().await, seq);
    if gap {
        assert!(
            objects(&fx) > objects_before,
            "the revision object stays behind as a harmless orphan"
        );
    }
    assert_consistent(&fx.project).await;

    let outcome = fx
        .reviews
        .integrate_proposal(&review, p(1), Some(1))
        .await
        .unwrap();
    assert_eq!(outcome, IntegrationOutcome::Succeeded { revision: r(2) });
    let number = if gap { 2 } else { 1 };
    assert_eq!(
        fx.count("SELECT number FROM integrations WHERE result = 'succeeded'")
            .await,
        number
    );
    assert_eq!(head(&fx).await, r(2));
    assert_eq!(fx.status(1).await, TaskStatus::Done);
    assert_consistent(&fx.project).await;
    scenario.teardown();
}

#[tokio::test]
async fn a_crash_after_the_merge_leaves_main_and_the_proposal_unchanged() {
    stopped_before_commit("integration_after_merge", false).await;
}

#[tokio::test]
async fn a_crash_after_the_objects_leaves_only_orphan_objects() {
    stopped_before_commit("integration_after_objects", true).await;
}

#[tokio::test]
async fn a_crash_inside_the_transaction_rolls_all_of_it_back() {
    stopped_before_commit("integration_in_transaction", true).await;
}

/// A crash after the commit, before anything is published, still leaves the whole integration: the revision, main, the completion, the archived workspace and the stored events.
#[tokio::test]
async fn a_crash_after_the_commit_keeps_the_whole_integration() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    let review = approved(&fx).await;
    let seq = fx.last_seq().await;

    integrate_stopped_at(&mut fx, &review, "integration_after_commit").await;

    assert_eq!(head(&fx).await, r(2));
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM completions c JOIN integrations i ON i.id = c.integration_id
             JOIN revisions r ON r.id = i.revision_id
             WHERE i.number = 1 AND i.result = 'succeeded' AND r.number = 2",
        )
        .await,
        1
    );
    assert_eq!(fx.status(1).await, TaskStatus::Done);
    assert_eq!(
        fx.text("SELECT state FROM proposals WHERE number = 1")
            .await,
        Some("integrated".to_owned())
    );
    assert_eq!(
        fx.text("SELECT state FROM workspaces WHERE number = 1")
            .await,
        Some("archived".to_owned())
    );
    let stored = fx.events_after(seq).await;
    let stored = names(&stored);
    for expected in [
        types::INTEGRATION_SUCCEEDED,
        types::MAIN_ADVANCED,
        types::PROPOSAL_INTEGRATED,
        types::WORKSPACE_ARCHIVED,
    ] {
        assert!(
            stored.contains(&expected.as_str()),
            "{expected:?} missing from {stored:?}"
        );
    }
    assert_consistent(&fx.project).await;

    let again = fx.reviews.integrate_proposal(&review, p(1), Some(1)).await;
    assert_eq!(again.unwrap_err().code(), Some("proposal_not_open"));
    scenario.teardown();
}
