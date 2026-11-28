#![cfg(test)]

use fold_core::{Clock as _, ProposalId};
use fold_daemon::services::reviews::{QueueStanding, WaitReason};

use crate::auto_integrate::{approve, reviewer};
use crate::common::intents::Covering as _;
use crate::common::roles::give_batch;
use crate::proposal_tools_support::{Fixture, t};

fn p(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
}

async fn standing(fx: &Fixture, proposal: u32) -> Option<QueueStanding> {
    fx.reviews()
        .integration_queue()
        .await
        .unwrap()
        .standing(p(proposal))
        .cloned()
}

async fn propose(fx: &Fixture, author: &str, task: u32, path: &str, content: &str) {
    let author = fx.agent(author).await;
    fx.workspaces()
        .create_workspace(&author, t(task))
        .await
        .unwrap();
    fx.write(task, path, content);
    fx.reviews()
        .create_proposal_covering(&author, t(task), "Change it", None)
        .await
        .unwrap()
        .detail()
        .unwrap();
}

#[tokio::test]
async fn a_conflicted_proposal_waits_for_its_conflict_and_has_no_place() {
    let fx = Fixture::new().await;
    let review = reviewer(&fx, "review").await;
    propose(&fx, "frontend", 1, "src/lib.rs", "pub fn y() {}\n").await;
    propose(&fx, "backend", 2, "src/lib.rs", "pub fn z() {}\n").await;
    approve(&fx, &review, 2).await;
    approve(&fx, &review, 1).await;
    let QueueStanding::Queued { place, batch } = standing(&fx, 2).await.unwrap() else {
        panic!("P-2 is queued");
    };
    assert_eq!(
        (place.position, batch),
        (2, None),
        "number order, not approval order"
    );

    let done = fx.reviews().integrate_ready().await.unwrap();
    assert_eq!(done.len(), 2, "P-1 integrates and P-2 conflicts");
    assert_eq!(
        standing(&fx, 2).await,
        Some(QueueStanding::Waits(WaitReason::Conflict))
    );
    assert_eq!(
        standing(&fx, 1).await,
        None,
        "an integrated proposal has no standing"
    );
}

#[tokio::test]
async fn a_member_of_an_open_review_batch_waits_for_the_batch() {
    let fx = Fixture::new().await;
    let review = reviewer(&fx, "review").await;
    propose(&fx, "frontend", 1, "src/lib.rs", "pub fn y() {}\n").await;
    propose(&fx, "backend", 2, "README.md", "# demo, exported\n").await;
    approve(&fx, &review, 1).await;
    let batch = give_batch(
        &fx.handle.store,
        "review",
        &[p(1), p(2)],
        fx.project.clock().now(),
    )
    .await;
    assert_eq!(
        standing(&fx, 1).await,
        Some(QueueStanding::Waits(WaitReason::Batch { batch }))
    );
    assert_eq!(standing(&fx, 2).await, None, "no approval, no standing");
}

#[tokio::test]
async fn an_approved_version_waits_while_its_next_version_runs_its_checks() {
    let fx = Fixture::new().await;
    let author = fx.agent("frontend").await;
    let review = reviewer(&fx, "review").await;
    let dir = tempfile::tempdir().unwrap();
    let go = dir.path().join("go");
    fx.gated_proposal(&author, &go).await;
    approve(&fx, &review, 1).await;
    // A version that changes `fold-checks.toml` gets the human review mark.
    assert_eq!(
        standing(&fx, 1).await,
        Some(QueueStanding::Waits(WaitReason::HumanReview {
            reviewer: None
        }))
    );
    fx.write(1, "src/lib.rs", "pub fn z() {}\n");
    fx.reviews()
        .update_proposal_covering(&author, p(1), None)
        .await
        .unwrap();
    assert_eq!(
        standing(&fx, 1).await,
        Some(QueueStanding::Waits(WaitReason::NextVersionRun))
    );
    std::fs::write(&go, "").unwrap();
}
