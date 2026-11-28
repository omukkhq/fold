#![cfg(test)]

use std::path::Path;
use std::time::Duration;

use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::Harness;

/// Long enough for a shell to start on a slow machine, and much longer than the checks' limits, so only a run that waits for nothing ends in it.
const WAIT: Duration = Duration::from_secs(60);

/// A fake build gate in `dir`: it writes the gate's wait line on stderr, waits until the file `go` exists, then for `extra`, writes the slot marker and runs its arguments. Without `go`, it never takes a slot.
fn gate(dir: &Path, extra: &str) -> String {
    let script = dir.join("gate.sh");
    let go = dir.join("go");
    std::fs::write(
        &script,
        format!(
            "echo 'build-gate: all 1 slots are in use; waiting for one' >&2\nuntil [ -e '{go}' ]; do sleep 0.05; done\nsleep {extra}\necho 'build-gate: slot taken' >&2\nexec \"$@\"\n",
            go = go.display()
        ),
    )
    .unwrap();
    script.display().to_string()
}

/// `agent` waits for the end of a check run of `task` and returns that `checks.finished` event.
async fn finished(harness: &Harness, agent: &str, task: &str) -> Value {
    loop {
        let woke = harness
            .call(
                agent,
                "wait_for_events",
                json!({"types": ["checks.finished"], "task": task, "timeout_s": 60}),
            )
            .await;
        if let Some(event) = woke["events"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|event| event["type"] == "checks.finished")
        {
            return event.clone();
        }
    }
}

/// Backend's task with `checks` as its checks file and a running run of them; returns the task and the run.
async fn running(harness: &Harness, checks: &str) -> (String, String) {
    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let work = harness.start_work("backend", &task).await;
    work.write(&[
        ("fold-checks.toml", checks),
        ("src/login.txt", "GET /login\n"),
    ]);
    let started = harness
        .call("backend", "run_checks", json!({"task": task}))
        .await;
    assert_eq!(started["checks"], "running", "{started}");
    (task, started["run"].as_str().unwrap().to_owned())
}

/// The run once it shows that it waits for a build slot.
async fn waiting(harness: &Harness, run: &str) -> Value {
    eventually(WAIT, async || {
        let read = harness
            .call("backend", "get_check_run", json!({"run": run}))
            .await;
        if read["waiting_for"] == "build_slot" {
            Ok(())
        } else {
            Err("the run does not wait for a build slot yet")
        }
    })
    .await;
    harness
        .call("backend", "get_check_run", json!({"run": run}))
        .await
}

/// Acceptance 66: backend's check waits behind a full build gate for longer than its timeout of 1 s. While it waits, `get_check_run` shows `waiting_for` `build_slot`; once the gate writes its slot marker the command runs at once, and the run passes, because the timeout counts only from the marker.
#[tokio::test]
async fn acceptance_66_a_check_that_waits_for_a_slot_longer_than_its_timeout_passes() {
    let harness = Harness::start().await;
    let dir = tempfile::TempDir::new().unwrap();
    let gate = gate(dir.path(), "1.5");
    let checks = format!(
        "[runs]\nbuild_slot_wait = \"60s\"\n\n[[check]]\nname = \"unit\"\nrun = \"sh '{gate}' true\"\ntimeout = \"1s\"\n"
    );
    let (task, run) = running(&harness, &checks).await;

    let read = waiting(&harness, &run).await;
    assert_eq!(read["state"], "running", "{read}");
    assert!(read["waiting_since"].is_i64(), "{read}");
    std::fs::write(dir.path().join("go"), "").unwrap();

    let event = tokio::time::timeout(WAIT, finished(&harness, "backend", &task))
        .await
        .expect("the run did not end");
    assert_eq!(event["data"]["state"], "passed", "{event}");
    let read = harness
        .call("backend", "get_check_run", json!({"run": run}))
        .await;
    assert_eq!(read["state"], "passed", "{read}");
    assert!(read.get("waiting_for").is_none(), "{read}");
}

/// Acceptance 66: a check whose gate never takes a slot fails after `[runs] build_slot_wait`, with reason `no_build_slot` and not `timeout`, and no later check starts.
#[tokio::test]
async fn acceptance_66_a_check_that_never_gets_a_slot_fails_with_no_build_slot() {
    let harness = Harness::start().await;
    let dir = tempfile::TempDir::new().unwrap();
    let gate = gate(dir.path(), "0");
    let later = dir.path().join("later");
    let checks = format!(
        "[runs]\nbuild_slot_wait = \"2s\"\n\n[[check]]\nname = \"unit\"\nrun = \"sh '{gate}' true\"\ntimeout = \"1s\"\n\n[[check]]\nname = \"later\"\nrun = \"touch '{later}'\"\n",
        later = later.display()
    );
    let (task, run) = running(&harness, &checks).await;
    waiting(&harness, &run).await;

    let event = tokio::time::timeout(WAIT, finished(&harness, "backend", &task))
        .await
        .expect("the run did not end");
    assert_eq!(event["data"]["state"], "failed", "{event}");
    assert_eq!(event["data"]["reason"], "no_build_slot", "{event}");
    let read = harness
        .call(
            "backend",
            "get_check_run",
            json!({"run": run, "detail": "full"}),
        )
        .await;
    assert_eq!(read["reason"], "no_build_slot", "{read}");
    assert!(read.get("waiting_for").is_none(), "{read}");
    assert!(!later.exists(), "no later check starts");
}

/// Acceptance 66: a check that writes no slot marker keeps the timeout from the start of its command.
#[tokio::test]
async fn acceptance_66_a_check_without_the_marker_times_out_from_its_start() {
    let harness = Harness::start().await;
    let checks = "[[check]]\nname = \"unit\"\nrun = \"sleep 30\"\ntimeout = \"1s\"\n";
    let (task, run) = running(&harness, checks).await;

    let event = tokio::time::timeout(WAIT, finished(&harness, "backend", &task))
        .await
        .expect("the run did not end");
    assert_eq!(event["data"]["state"], "failed", "{event}");
    assert_eq!(event["data"]["reason"], "check_failed", "{event}");
    let read = harness
        .call("backend", "get_check_run", json!({"run": run}))
        .await;
    assert_eq!(read["reason"], "check_failed", "{read}");
    assert_eq!(read["results"][0]["outcome"], "timed_out", "{read}");
}
