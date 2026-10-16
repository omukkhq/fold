#![cfg(test)]

use fold_core::{Actor, AgentName, HumanName, Role, WorkerState};
use fold_daemon::services::agents::StartWorker;
use fold_store::testing::eventually;

use crate::common::acceptance::{Harness, WAIT};
use crate::workers_support::FakeClaude;

const BEFORE_SPAWN: &str = "worker_launch_before_spawn";

/// A stop that lands after the start committed but before its process is known finds nothing to end; the launch that follows must end the process itself and give it no turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_before_the_process_starts_leaves_no_process_running() {
    let scenario = fail::FailScenario::setup();
    let fake = FakeClaude::new("init");
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let human = Actor::human(HumanName::parse("ana").unwrap());
    let lead = AgentName::parse("review-lead").unwrap();
    fail::cfg(BEFORE_SPAWN, "pause").unwrap();
    let agents = harness.handle.agents();
    let starter = human.clone();
    let start = tokio::spawn(async move {
        agents
            .start_worker(
                &starter,
                StartWorker {
                    role: Some(Role::ReviewLead),
                    ..StartWorker::default()
                },
            )
            .await
    });
    eventually(WAIT, async || {
        match harness.handle.agents().get_worker(&lead).await {
            Ok(row) if row.state == WorkerState::Starting => Ok(()),
            other => Err(other.map(|row| row.state).ok()),
        }
    })
    .await;
    let stopped = harness
        .handle
        .agents()
        .stop_worker(&human, &lead)
        .await
        .unwrap();
    assert_eq!(stopped.state, WorkerState::Stopped);
    fail::remove(BEFORE_SPAWN);
    let started = start.await.unwrap().unwrap();
    assert_eq!(started.state, WorkerState::Stopped);
    eventually(WAIT, async || {
        if harness.handle.workers.is_running(&lead) {
            Err(())
        } else {
            Ok(())
        }
    })
    .await;
    let row = harness.handle.agents().get_worker(&lead).await.unwrap();
    assert_eq!(row.state, WorkerState::Stopped);
    assert_eq!(row.pid, None);
    let runs = fake.runs_of(lead.as_str());
    assert_eq!(runs.len(), 1, "the launch started its process and ended it");
    assert!(
        runs[0]
            .stdin
            .iter()
            .all(|line| !line.contains("\"type\":\"user\"")),
        "{:?}",
        runs[0].stdin
    );
    let stops = harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "worker.stopped")
        .count();
    assert_eq!(stops, 1);
    scenario.teardown();
}
