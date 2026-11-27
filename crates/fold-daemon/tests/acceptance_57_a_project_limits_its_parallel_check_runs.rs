#![cfg(test)]

use std::time::Duration;

use crate::common::acceptance::Harness;
use fold_store::testing::eventually;
use serde_json::{Value, json};

/// Long enough for shells to start and runs to end on a slow machine.
const RUN_WAIT: Duration = Duration::from_secs(60);

/// The newest `checks.started` of `task`.
async fn started_event(harness: &Harness, agent: &str, task: &str) -> Value {
    let events = harness
        .call(
            agent,
            "get_events",
            json!({"after": 0, "types": ["checks.started"], "task": task}),
        )
        .await;
    events["events"].as_array().unwrap().last().unwrap().clone()
}

/// Acceptance 57: the checks file of four workspaces sets `[runs] max_parallel_runs = 2`. backend asks for a run in each, in order: the first two run, and the last two show `waiting_for: "run_slot"` with their positions in `checks.started` and in `get_check_run`, until the first two end and free their places; then every run passes.
#[tokio::test]
async fn acceptance_57_runs_over_the_limit_wait_for_a_place() {
    let harness = Harness::start().await;
    let marks = tempfile::TempDir::new().unwrap();
    let release = marks.path().join("release");
    let checks = format!(
        "[runs]\nmax_parallel_runs = 2\n\n[[check]]\nname = \"slow\"\nrun = \"touch '{dir}'/$$; until [ -e '{release}' ]; do sleep 0.05; done\"\n",
        dir = marks.path().display(),
        release = release.display(),
    );
    let mut runs = Vec::new();
    let mut tasks = Vec::new();
    for number in 1..=4 {
        let task = harness
            .create_task("backend", &format!("Task {number}"))
            .await;
        let work = harness.start_work("backend", &task).await;
        work.write(&[("fold-checks.toml", &checks), ("src/lib.txt", "x\n")]);
        let started = harness
            .call("backend", "run_checks", json!({"task": task}))
            .await;
        assert_eq!(started["checks"], "running", "{started}");
        let event = started_event(&harness, "backend", &task).await;
        if number <= 2 {
            assert!(event["data"].get("waiting_for").is_none(), "{event}");
        } else {
            assert_eq!(event["data"]["waiting_for"], "run_slot", "{event}");
            assert_eq!(event["data"]["position"], number - 2, "{event}");
        }
        runs.push(started["run"].as_str().unwrap().to_owned());
        tasks.push(task);
    }

    eventually(RUN_WAIT, async || {
        let commands = std::fs::read_dir(marks.path()).unwrap().count();
        if commands == 2 { Ok(()) } else { Err(commands) }
    })
    .await;
    for (index, run) in runs.iter().enumerate() {
        let view = harness
            .call("backend", "get_check_run", json!({"run": run}))
            .await;
        assert_eq!(view["state"], "running", "{view}");
        if index < 2 {
            assert!(view.get("waiting_for").is_none(), "{view}");
        } else {
            assert_eq!(view["waiting_for"], "run_slot", "{view}");
            assert_eq!(view["position"], index - 1, "{view}");
        }
    }
    assert_eq!(std::fs::read_dir(marks.path()).unwrap().count(), 2);

    std::fs::write(&release, "").unwrap();
    for run in &runs {
        eventually(RUN_WAIT, async || {
            let view = harness
                .call("backend", "get_check_run", json!({"run": run}))
                .await;
            if view["state"] == "passed" {
                Ok(())
            } else {
                Err(view)
            }
        })
        .await;
    }
    assert_eq!(std::fs::read_dir(marks.path()).unwrap().count(), 4 + 1);
}
