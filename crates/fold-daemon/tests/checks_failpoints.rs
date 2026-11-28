#![cfg(test)]

use crate::common::intents::Covering as _;
use fold_core::reviews::checks::{RunReason, RunState};
use fold_daemon::services::reviews::Proposed;
use fold_daemon::services::reviews::checks::{CheckPermits, CheckRunner, recover_check_runs};

use std::sync::Mutex;
use std::sync::mpsc;
use std::time::Duration;

use fold_core::{FoldError, codes};
use fold_daemon::services::reviews::checks::Started;

use crate::checks::{PASSING, workspace_with_checks};
use crate::reviews_support::{Fixture, by, refusal, t, ws};

const WAIT: Duration = Duration::from_secs(30);

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

/// Runs `run_checks` of the frontend's task until it stops before its write, runs `meanwhile` there, then lets the write go and returns its refusal.
async fn run_checks_with(fx: &Fixture, meanwhile: impl AsyncFnOnce(&Fixture)) -> FoldError {
    let (reached, at_point) = mpsc::sync_channel::<()>(1);
    let (go, wait_go) = mpsc::sync_channel::<()>(1);
    let (reached, wait_go) = (Mutex::new(reached), Mutex::new(wait_go));
    fail::cfg_callback("checks_run_before_write", move || {
        reached.lock().unwrap().send(()).unwrap();
        wait_go.lock().unwrap().recv_timeout(WAIT).unwrap();
    })
    .unwrap();
    let reviews = fx.reviews.clone();
    let author = by(&fx.frontend.worker);
    let started = tokio::spawn(async move { reviews.run_checks(&author, t(1)).await });
    tokio::task::spawn_blocking(move || at_point.recv_timeout(WAIT))
        .await
        .unwrap()
        .expect("run_checks reaches its write");
    meanwhile(fx).await;
    fail::remove("checks_run_before_write");
    go.send(()).unwrap();
    let result: Result<Started, _> = started.await.unwrap();
    refusal(result)
}

/// A task workspace that closes after the snapshot of `run_checks` and before its write is a missing workspace, not a refusal of the owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workspace_closed_before_the_run_is_written_has_no_task_workspace() {
    let scenario = fail::FailScenario::setup();
    let fx = Fixture::new().await;
    workspace_with_checks(&fx, PASSING).await;
    let refused = run_checks_with(&fx, async |fx| {
        fx.workspaces
            .abandon_workspace(&by(&fx.frontend.worker), ws(1), "another plan")
            .await
            .unwrap();
    })
    .await;
    assert_eq!(refused.code(), codes::WORKSPACE_NOT_ACTIVE);
    assert_eq!(refused.message(), "T-1 has no active task workspace");
    assert_eq!(fx.count("SELECT COUNT(*) FROM check_runs").await, 0);
    scenario.teardown();
}

/// A task workspace that another one replaces after the snapshot of `run_checks` and before its write refuses the run, which names the new workspace.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workspace_replaced_before_the_run_is_written_refuses_the_run() {
    let scenario = fail::FailScenario::setup();
    let fx = Fixture::new().await;
    workspace_with_checks(&fx, PASSING).await;
    let refused = run_checks_with(&fx, async |fx| {
        let author = by(&fx.frontend.worker);
        fx.workspaces
            .abandon_workspace(&author, ws(1), "another plan")
            .await
            .unwrap();
        fx.workspaces.create_workspace(&author, t(1)).await.unwrap();
    })
    .await;
    assert_eq!(refused.code(), codes::INVALID_REQUEST);
    assert_eq!(refused.message(), "ws-1 changed while it was recorded");
    let details = refused.details().expect("the refusal has details");
    assert_eq!(details["reason"], "workspace_changed");
    assert_eq!(details["workspace"], "ws-2");
    assert_eq!(fx.count("SELECT COUNT(*) FROM check_runs").await, 0);
    scenario.teardown();
}
