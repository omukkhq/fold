#![cfg(test)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fold_core::reviews::Verdict;
use fold_core::roles::Role;
use fold_core::{Actor, ProposalId, codes};
use fold_daemon::services::reviews::{FixOutcome, IntegrationOutcome, ReviewError, ReviewFix};
use fold_store::testing::eventually;

use crate::common::roles::give_role;
use crate::proposal_tools_support::{Fixture, human};

const FAILPOINT: &str = "integration_after_plan";
const WAIT: Duration = Duration::from_secs(30);

async fn reviewer(fx: &Fixture) -> Actor {
    let review = fx.agent("review").await;
    give_role(&fx.handle.store, "review", Role::Reviewer, None).await;
    review
}

fn p1() -> ProposalId {
    ProposalId::from_number(1).unwrap()
}

/// Holds the first integration attempt at [`FAILPOINT`], after its gate step read that no run of the next version goes, until [`Paused::open`].
struct Paused {
    reached: Arc<AtomicBool>,
    open: Arc<AtomicBool>,
}

impl Paused {
    fn at_the_plan() -> Self {
        let reached = Arc::new(AtomicBool::new(false));
        let open = Arc::new(AtomicBool::new(false));
        let (held_reached, held_open) = (reached.clone(), open.clone());
        fail::cfg_callback(FAILPOINT, move || {
            held_reached.store(true, Ordering::SeqCst);
            while !held_open.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        })
        .unwrap();
        Self { reached, open }
    }

    async fn reached(&self) {
        tokio::time::timeout(WAIT, async {
            while !self.reached.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
    }
}

/// `review`, a reviewer, approves v1 of `P-1`, whose checks file gave its task the human review mark. A human integrates it; the attempt stops after its gate step until `start` has started a run of the next version. The integration then refuses with `update_checks_running` naming that run's purpose, main does not move, and the run makes v2 once `go` exists.
async fn a_run_after_the_plan_holds_the_integration(
    fx: &Fixture,
    reviewer: &Actor,
    go: &std::path::Path,
    purpose: &str,
    start: impl AsyncFnOnce() -> fold_core::CheckRunId,
) {
    let reviews = fx.reviews();
    reviews
        .submit_review(reviewer, p1(), 1, Verdict::Approve, "ok", Vec::new())
        .await
        .unwrap();
    let scenario = fail::FailScenario::setup();
    let paused = Paused::at_the_plan();
    let integrate = tokio::spawn({
        let reviews = reviews.clone();
        async move {
            reviews
                .integrate_proposal(&human("tanim"), p1(), Some(1))
                .await
        }
    });
    paused.reached().await;
    let run = start().await;
    paused.open();
    let result: Result<IntegrationOutcome, ReviewError> = integrate.await.unwrap();
    fail::remove(FAILPOINT);
    scenario.teardown();

    let error = crate::reviews_support::refusal(result);
    assert_eq!(error.code(), codes::UPDATE_CHECKS_RUNNING);
    let details = error.details().unwrap();
    assert_eq!(details["run"], run.to_string());
    assert_eq!(details["purpose"], purpose);
    let detail = reviews.get_proposal(p1(), None).await.unwrap();
    assert_eq!(detail.summary.current_version, 1);
    assert_eq!(
        fold_store::workspaces::head(fx.handle.store.read())
            .await
            .unwrap()
            .unwrap()
            .number
            .to_string(),
        "r1"
    );

    std::fs::write(go, "").unwrap();
    eventually(WAIT, async || {
        let detail = reviews.get_proposal(p1(), None).await.unwrap();
        if detail.summary.current_version == 2 {
            Ok(())
        } else {
            Err(detail.summary.current_version)
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_that_starts_after_the_integration_plan_keeps_the_older_version_off_main() {
    let fx = Fixture::new().await;
    let author = fx.agent("frontend").await;
    let review = reviewer(&fx).await;
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    fx.gated_proposal(&author, &go).await;
    fx.write(1, "src/lib.rs", "pub fn z() {}\n");
    a_run_after_the_plan_holds_the_integration(&fx, &review, &go, "update_proposal", async || {
        match fx
            .reviews()
            .update_proposal(&author, p1(), "Make the change the task asks for.", None)
            .await
            .unwrap()
        {
            fold_daemon::services::reviews::Proposed::Checking { run, .. } => run,
            fold_daemon::services::reviews::Proposed::Made(_) => panic!("expected a run"),
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fix_that_starts_after_the_integration_plan_keeps_the_older_version_off_main() {
    let fx = Fixture::new().await;
    let author = fx.agent("frontend").await;
    let review = reviewer(&fx).await;
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    fx.gated_proposal(&author, &go).await;
    fx.fix_p1_checkout(&review).await;
    let reviewer: &Actor = &review;
    a_run_after_the_plan_holds_the_integration(&fx, reviewer, &go, "review_fix", async || {
        let fix = ReviewFix {
            path: "src/lib.rs".to_owned(),
            reason: "y returned nothing".to_owned(),
        };
        match fx
            .reviews()
            .submit_review_fix(
                reviewer,
                p1(),
                1,
                &[fix],
                "cargo test: ok",
                "Make the change the task asks for.",
            )
            .await
            .unwrap()
        {
            FixOutcome::Checking { run, .. } => run,
            FixOutcome::Fixed(_) => panic!("expected a run"),
        }
    })
    .await;
}
