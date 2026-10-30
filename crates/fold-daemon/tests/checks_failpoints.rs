#![cfg(test)]

use crate::common::intents::Covering as _;
use fold_core::reviews::checks::{RunReason, RunState};
use fold_daemon::services::reviews::Proposed;
use fold_daemon::services::reviews::checks::{CheckPermits, CheckRunner, recover_check_runs};

use crate::checks::{PASSING, workspace_with_checks};
use crate::reviews_support::{Fixture, by, t};

/// A stop between the passed run's results and the version it makes leaves neither: the run is still running with no result, and no proposal exists. The next start ends the run as stopped and asks the worker to call create_proposal again.
#[tokio::test]
async fn a_stop_before_the_version_leaves_no_result_and_no_proposal() {
    let scenario = fail::FailScenario::setup();
    let mut fx = Fixture::new().await;
    workspace_with_checks(&fx, PASSING).await;
    fail::cfg("checks_before_version", "return").unwrap();
    let runner = CheckRunner::new(CheckPermits::new(1));
    let reviews = fx.service().with_checks(runner.clone());
    let proposed = reviews
        .create_proposal_covering(&by(&fx.frontend.worker), t(1), "Add checks", None)
        .await
        .unwrap();
    let Proposed::Checking { run, .. } = proposed else {
        panic!("expected a run");
    };
    runner.wait_idle().await;
    let view = reviews.get_check_run(run, false).await.unwrap();
    assert_eq!(
        view.run.state,
        RunState::Running,
        "the stop left the run without an end"
    );
    assert!(view.results.is_empty());
    fail::remove("checks_before_version");
    assert_eq!(fx.count("SELECT COUNT(*) FROM proposals").await, 0);
    assert_eq!(fx.count("SELECT COUNT(*) FROM check_results").await, 0);

    fx.restart().await;
    let clock = fx.project.clock().clone();
    assert_eq!(
        recover_check_runs(fx.project.store(), &fx.bus, &clock)
            .await
            .unwrap(),
        1
    );
    let view = fx.reviews.get_check_run(run, false).await.unwrap();
    assert_eq!(view.run.state, RunState::Stopped);
    assert_eq!(view.run.reason, Some(RunReason::DaemonStopped));
    assert_eq!(fx.count("SELECT COUNT(*) FROM proposals").await, 0);
    let message = fx
        .text("SELECT body FROM messages WHERE sender_kind = 'fold' ORDER BY number DESC LIMIT 1")
        .await
        .unwrap();
    assert!(message.contains("Call create_proposal again"), "{message}");
    scenario.teardown();
}
