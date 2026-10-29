#![cfg(test)]

use std::path::Path;
use std::time::Duration;

use fold_core::policy::SessionPeer;
use fold_core::reviews::checks::{Outcome, RunReason, RunState};
use fold_core::{AgentName, CheckRunId, codes};
use fold_daemon::services::agents::peers;
use fold_daemon::services::reviews::ReviewService;
use fold_daemon::services::reviews::checks::{
    CheckPermits, CheckRunView, CheckRunner, Started, recover_check_runs,
};
use fold_store::testing::eventually;

use crate::reviews_support::{Fixture, by, names, refusal, t};

/// Long enough for a slow machine to start a shell and scan a workspace.
const WAIT: Duration = Duration::from_secs(30);

pub const PASSING: &str = r#"
[[check]]
name = "test"
run  = "test -f src/lib.rs && echo all good"
"#;

/// `frontend` creates the workspace of `T-1`, writes `fold-checks.toml` with `checks` and changes `src/lib.rs`.
pub async fn workspace_with_checks(fx: &Fixture, checks: &str) {
    fx.workspaces
        .create_workspace(&by(&fx.frontend.worker), t(1))
        .await
        .unwrap();
    fx.write("fold-checks.toml", checks);
    fx.write("src/lib.rs", "pub fn y() {}\n");
}

/// The run once it has ended.
pub async fn ended(reviews: &ReviewService, run: CheckRunId) -> CheckRunView {
    eventually(WAIT, async || {
        let view = reviews.get_check_run(run, true).await.unwrap();
        if view.run.state == RunState::Running {
            Err(view.run.state)
        } else {
            Ok(view)
        }
    })
    .await
}

pub fn running(started: Started) -> CheckRunId {
    match started {
        Started::Running { run, .. } => run,
        Started::NoChecks => panic!("expected a run"),
    }
}

/// A command that waits until `gate` exists, outside the workspace so it changes no recorded file.
fn waiting_check(gate: &Path) -> String {
    format!(
        "[[check]]\nname = \"slow\"\nrun = \"until [ -e '{}' ]; do sleep 0.05; done\"\n",
        gate.display()
    )
}

#[cfg(test)]
async fn fold_messages(fx: &Fixture) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT m.body FROM messages m
         JOIN message_recipients r ON r.message_id = m.id
         JOIN workers w ON w.id = r.worker_id
         WHERE m.sender_kind = 'fold' AND w.name = 'frontend' AND r.addressed_as = 'worker'
         ORDER BY m.number",
    )
    .fetch_all(fx.project.store().read())
    .await
    .unwrap()
}

#[tokio::test]
async fn a_passing_run_is_stored_and_told_to_the_worker() {
    let fx = Fixture::new().await;
    workspace_with_checks(&fx, PASSING).await;
    let before = fx.last_seq().await;
    let run = running(
        fx.reviews
            .run_checks(&by(&fx.frontend.worker), t(1))
            .await
            .unwrap(),
    );
    let view = ended(&fx.reviews, run).await;
    assert_eq!(view.run.state, RunState::Passed);
    assert_eq!(view.run.reason, None);
    assert_eq!(view.results.len(), 1);
    assert_eq!(view.results[0].check.as_deref(), Some("test"));
    assert_eq!(view.results[0].outcome, Outcome::Passed);
    assert_eq!(view.results[0].log, b"all good\n");
    let events = fx.events_after(before).await;
    let kinds = names(&events);
    assert!(kinds.contains(&"checks.started") && kinds.contains(&"checks.finished"));
    let finished = events
        .iter()
        .find(|event| event.event_type.as_str() == "checks.finished")
        .unwrap();
    assert_eq!(finished.data["state"], "passed");
    assert_eq!(finished.task, Some(t(1)));
    let messages = fold_messages(&fx).await;
    assert_eq!(messages, ["The checks of T-1 (cr-1) passed: test."]);
}

#[tokio::test]
async fn a_failing_check_fails_the_run_with_its_log() {
    let fx = Fixture::new().await;
    workspace_with_checks(
        &fx,
        "[setup]\nrun = \"echo setting up\"\n[[check]]\nname = \"unit\"\nrun = \"echo first; echo broken >&2; exit 4\"\n[[check]]\nname = \"style\"\nrun = \"exit 1\"\nrequired = false\n",
    )
    .await;
    let run = running(
        fx.reviews
            .run_checks(&by(&fx.frontend.worker), t(1))
            .await
            .unwrap(),
    );
    let view = ended(&fx.reviews, run).await;
    assert_eq!(view.run.state, RunState::Failed);
    assert_eq!(view.run.reason, Some(RunReason::CheckFailed));
    let outcomes: Vec<_> = view.results.iter().map(|r| r.outcome).collect();
    assert_eq!(
        outcomes,
        [Outcome::Passed, Outcome::Failed, Outcome::Failed]
    );
    assert_eq!(view.results[1].exit_code, Some(4));
    let message = fold_messages(&fx).await.pop().unwrap();
    assert!(
        message.starts_with("The checks of T-1 (cr-1) failed: unit failed (exit 4)."),
        "{message}"
    );
    assert!(
        message.contains("The end of the log of unit:\nfirst\nbroken"),
        "{message}"
    );
}

#[tokio::test]
async fn a_check_that_changes_recorded_files_fails_the_run() {
    let fx = Fixture::new().await;
    workspace_with_checks(
        &fx,
        "[[check]]\nname = \"gen\"\nrun = \"echo out > generated.txt\"\n",
    )
    .await;
    let run = running(
        fx.reviews
            .run_checks(&by(&fx.frontend.worker), t(1))
            .await
            .unwrap(),
    );
    let view = ended(&fx.reviews, run).await;
    assert_eq!(view.run.state, RunState::Failed);
    assert_eq!(view.run.reason, Some(RunReason::FilesChanged));
    assert_eq!(view.results[0].outcome, Outcome::Passed);
    let message = fold_messages(&fx).await.pop().unwrap();
    assert!(message.contains("recorded files"), "{message}");
}

#[tokio::test]
async fn an_ignored_output_never_fails_the_run() {
    let fx = Fixture::new().await;
    workspace_with_checks(
        &fx,
        "[setup]\nrun = \"mkdir -p node_modules && echo x > node_modules/dep\"\n[[check]]\nname = \"t\"\nrun = \"test -f node_modules/dep\"\n",
    )
    .await;
    fx.write(".gitignore", "node_modules/\n");
    let run = running(
        fx.reviews
            .run_checks(&by(&fx.frontend.worker), t(1))
            .await
            .unwrap(),
    );
    assert_eq!(ended(&fx.reviews, run).await.run.state, RunState::Passed);
}

#[tokio::test]
async fn a_tree_without_checks_or_without_a_matching_check_starts_nothing() {
    let fx = Fixture::new().await;
    fx.workspaces
        .create_workspace(&by(&fx.frontend.worker), t(1))
        .await
        .unwrap();
    fx.write("src/lib.rs", "pub fn y() {}\n");
    let actor = by(&fx.frontend.worker);
    assert_eq!(
        fx.reviews.run_checks(&actor, t(1)).await.unwrap(),
        Started::NoChecks
    );
    fx.write(
        "fold-checks.toml",
        "[[check]]\nname = \"py\"\nrun = \"pytest\"\nfiles = \"\\\\.py$\"\n",
    );
    assert_eq!(
        fx.reviews.run_checks(&actor, t(1)).await.unwrap(),
        Started::NoChecks
    );
    assert_eq!(fx.count("SELECT COUNT(*) FROM check_runs").await, 0);
}

#[tokio::test]
async fn an_invalid_file_and_another_worker_are_refused_at_once() {
    let fx = Fixture::new().await;
    workspace_with_checks(
        &fx,
        "[[check]]\nname = \"t\"\nrun = \"true\"\nrequire = false\n",
    )
    .await;
    let invalid = refusal(fx.reviews.run_checks(&by(&fx.frontend.worker), t(1)).await);
    assert_eq!(invalid.code(), codes::CHECKS_FILE_INVALID);
    let other = refusal(fx.reviews.run_checks(&by(&fx.backend.worker), t(1)).await);
    assert_eq!(other.code(), codes::NOT_WORKSPACE_OWNER);
    let none = refusal(fx.reviews.run_checks(&by(&fx.frontend.worker), t(2)).await);
    assert_eq!(none.code(), codes::WORKSPACE_NOT_ACTIVE);
    assert_eq!(fx.count("SELECT COUNT(*) FROM check_runs").await, 0);
}

#[tokio::test]
async fn one_run_at_a_time_goes_in_a_workspace() {
    let fx = Fixture::new().await;
    let gate = tempfile::tempdir().unwrap();
    let gate = gate.path().join("go");
    workspace_with_checks(&fx, &waiting_check(&gate)).await;
    let actor = by(&fx.frontend.worker);
    let run = running(fx.reviews.run_checks(&actor, t(1)).await.unwrap());
    let second = refusal(fx.reviews.run_checks(&actor, t(1)).await);
    assert_eq!(second.code(), codes::CHECKS_RUNNING);
    assert_eq!(second.details().unwrap()["run"], "cr-1");
    std::fs::write(&gate, "").unwrap();
    assert_eq!(ended(&fx.reviews, run).await.run.state, RunState::Passed);
    let next = running(fx.reviews.run_checks(&actor, t(1)).await.unwrap());
    assert_eq!(next.to_string(), "cr-2");
    ended(&fx.reviews, next).await;
}

#[tokio::test]
async fn a_stop_during_a_run_ends_it_as_stopped_at_the_next_start() {
    let fx = Fixture::new().await;
    let outside = tempfile::tempdir().unwrap();
    let pid_file = outside.path().join("pid");
    workspace_with_checks(
        &fx,
        &format!(
            "[[check]]\nname = \"slow\"\nrun = \"sleep 60 & echo $! > '{}'; wait\"\n",
            pid_file.display()
        ),
    )
    .await;
    let runner = CheckRunner::new(CheckPermits::new(1));
    let reviews = fx.service().with_checks(runner.clone());
    let run = running(
        reviews
            .run_checks(&by(&fx.frontend.worker), t(1))
            .await
            .unwrap(),
    );
    let pid = eventually(WAIT, async || {
        std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .ok_or("no pid yet")
    })
    .await;
    runner.shutdown().await;
    eventually(WAIT, async || {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        // A killed child that nobody reaped yet is a zombie, state Z.
        if stat.is_empty() || stat.split(' ').nth(2) == Some("Z") {
            Ok(())
        } else {
            Err(stat)
        }
    })
    .await;
    let view = reviews.get_check_run(run, false).await.unwrap();
    assert_eq!(
        view.run.state,
        RunState::Running,
        "a stop leaves the run without an end"
    );
    let clock = fx.project.clock().clone();
    let ended = recover_check_runs(fx.project.store(), &fx.bus, &clock)
        .await
        .unwrap();
    assert_eq!(ended, 1);
    let view = reviews.get_check_run(run, false).await.unwrap();
    assert_eq!(view.run.state, RunState::Stopped);
    assert_eq!(view.run.reason, Some(RunReason::DaemonStopped));
    assert!(view.results.is_empty());
    let message = fold_messages(&fx).await.pop().unwrap();
    assert!(message.contains("Call run_checks again"), "{message}");
    assert_eq!(
        recover_check_runs(fx.project.store(), &fx.bus, &clock)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn a_proposal_shows_the_runs_of_exactly_its_files() {
    let fx = Fixture::new().await;
    workspace_with_checks(&fx, PASSING).await;
    let actor = by(&fx.frontend.worker);
    let run = running(fx.reviews.run_checks(&actor, t(1)).await.unwrap());
    ended(&fx.reviews, run).await;
    fx.reviews
        .create_proposal(
            &actor,
            t(1),
            "Add the checks",
            "Make the change the task asks for.",
            None,
        )
        .await
        .unwrap()
        .detail()
        .unwrap();
    let proposal = fx
        .text("SELECT 'P-' || number FROM proposals")
        .await
        .unwrap()
        .parse()
        .unwrap();
    let view = fx
        .reviews
        .view_proposal(&by(&fx.review.worker), proposal, None)
        .await
        .unwrap();
    let runs: Vec<_> = view.checks.iter().map(|view| view.run.id).collect();
    assert_eq!(runs, [run]);
    assert!(
        view.checks[0].results[0].log.is_empty(),
        "a view leaves the logs out"
    );
}

#[tokio::test]
async fn a_process_of_a_workers_check_acts_as_that_worker() {
    let fx = Fixture::new().await;
    let gate = tempfile::tempdir().unwrap();
    let (pids, go) = (gate.path().join("pids"), gate.path().join("go"));
    workspace_with_checks(
        &fx,
        &format!(
            "[[check]]\nname = \"slow\"\nrun = \"sleep 60 & echo $$ $! > '{}'; until [ -e '{}' ]; do sleep 0.05; done\"\n",
            pids.display(),
            go.display()
        ),
    )
    .await;
    let reviews = fx
        .reviews
        .clone()
        .with_checks(CheckRunner::default().with_project("app"));
    let run = running(
        reviews
            .run_checks(&by(&fx.frontend.worker), t(1))
            .await
            .unwrap(),
    );
    let line = eventually(WAIT, async || match std::fs::read_to_string(&pids) {
        Ok(line) if line.ends_with('\n') => Ok(line),
        other => Err(format!("{other:?}")),
    })
    .await;
    let found: Vec<u32> = line
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect();
    let chain = peers::process_chain(found[1]).unwrap();
    assert_eq!(
        peers::session_peer(peers::check_peer(&chain), "app"),
        SessionPeer::Worker(AgentName::parse("frontend").unwrap())
    );
    std::fs::write(&go, "").unwrap();
    assert_eq!(ended(&reviews, run).await.run.state, RunState::Passed);
    assert_eq!(
        peers::check_peer(&[found[0], std::process::id()]),
        None,
        "an ended check leaves the registry"
    );
}
