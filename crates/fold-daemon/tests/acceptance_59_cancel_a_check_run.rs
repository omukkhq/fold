#![cfg(test)]

use std::path::Path;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, body};

/// Long enough for a shell to start on a slow machine, and much shorter than the checks, so only a stopped run ends in it.
const STOP_WAIT: Duration = Duration::from_secs(30);

/// A checks file whose first check writes its shell's pid and its background child's pid into `marks`, then waits; the second check marks that it started.
fn checks(marks: &Path) -> String {
    let mark = |name: &str| marks.join(name).display().to_string();
    format!(
        "[[check]]\nname = \"slow\"\nrun = \"sleep 600 & echo $! > '{child}'; echo $$ > '{shell}'; wait\"\n\n[[check]]\nname = \"later\"\nrun = \"touch '{later}'\"\n",
        child = mark("child"),
        shell = mark("shell"),
        later = mark("later"),
    )
}

/// The pid that the first check wrote into `marks/name`, once it has written it.
async fn pid(marks: &Path, name: &str) -> i32 {
    let path = marks.join(name);
    eventually(STOP_WAIT, async || match std::fs::read_to_string(&path) {
        Ok(text) if text.ends_with('\n') => Ok(()),
        _ => Err("the check has not written its pid"),
    })
    .await;
    std::fs::read_to_string(&path)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

/// Whether the process `pid` still runs; a zombie runs no more.
fn alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        !stat
            .rsplit(')')
            .next()
            .is_some_and(|rest| rest.trim_start().starts_with('Z'))
    })
}

/// Waits until the process `pid` has ended.
async fn ended(pid: i32) {
    eventually(STOP_WAIT, async || {
        if alive(pid) {
            Err("the process still runs")
        } else {
            Ok(())
        }
    })
    .await;
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

/// `agent`'s refusal of `tool` with `arguments`.
async fn refusal(harness: &Harness, agent: &str, tool: &str, arguments: Value) -> Value {
    let result = harness.call_raw(agent, tool, arguments).await;
    assert_eq!(result.is_error, Some(true), "{result:?}");
    body(&result)["error"].clone()
}

/// Backend's task with the checks of [`checks`] and a running run of them; returns the task, the run and the folder of the marks.
async fn running(harness: &Harness) -> (String, String, tempfile::TempDir) {
    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let work = harness.start_work("backend", &task).await;
    let marks = tempfile::TempDir::new().unwrap();
    work.write(&[
        ("fold-checks.toml", &checks(marks.path())),
        ("src/login.txt", "GET /login\n"),
    ]);
    let started = harness
        .call("backend", "run_checks", json!({"task": task}))
        .await;
    assert_eq!(started["checks"], "running", "{started}");
    let run = started["run"].as_str().unwrap().to_owned();
    (task, run, marks)
}

/// Acceptance 59: while the first check of backend's run goes, a reviewer and the lead of another area cannot cancel it and it goes on; backend, whose run it is, cancels it. The process group of the running check ends, the later check never starts, the run fails with reason `cancelled` and backend as `by` in `checks.finished`, and fold's message to backend says who cancelled it.
#[tokio::test]
async fn acceptance_59_a_worker_cancels_its_own_run_and_others_cannot() {
    let harness = Harness::start().await;
    let (task, run, marks) = running(&harness).await;
    let shell = pid(marks.path(), "shell").await;
    let child = pid(marks.path(), "child").await;

    for agent in ["frontend", "review"] {
        let error = refusal(&harness, agent, "cancel_check_run", json!({"run": run})).await;
        assert_eq!(error["code"], "check_cancel_denied", "{agent}: {error}");
        assert_eq!(error["details"]["run"], run.as_str(), "{error}");
    }
    let still = harness
        .call("backend", "get_check_run", json!({"run": run}))
        .await;
    assert_eq!(still["state"], "running", "{still}");
    assert!(alive(child), "a refused cancel leaves the check running");

    let cancelled = harness
        .call("backend", "cancel_check_run", json!({"run": run}))
        .await;
    assert_eq!(cancelled["reason"], "cancelled", "{cancelled}");
    ended(shell).await;
    ended(child).await;

    let event = tokio::time::timeout(STOP_WAIT, finished(&harness, "backend", &task))
        .await
        .expect("the cancel did not end the run");
    assert_eq!(event["data"]["state"], "failed", "{event}");
    assert_eq!(event["data"]["reason"], "cancelled", "{event}");
    assert_eq!(event["data"]["by"], "backend", "{event}");
    let read = harness
        .call("backend", "get_check_run", json!({"run": run}))
        .await;
    assert_eq!(read["state"], "failed", "{read}");
    assert_eq!(read["reason"], "cancelled", "{read}");
    assert!(
        !marks.path().join("later").exists(),
        "no later check starts"
    );
    let messages = harness.fold_messages("backend").await;
    assert!(
        messages
            .iter()
            .any(|message| message.contains(&format!("({run}) were cancelled by backend"))),
        "{messages:?}"
    );

    let again = refusal(&harness, "backend", "cancel_check_run", json!({"run": run})).await;
    assert_eq!(again["code"], "check_run_ended", "{again}");
}

/// Acceptance 59, a command killed from outside: a SIGTERM that fold did not send ends the first check's shell. No later check of the run starts, and the run fails with reason `killed`, whose message names the signal.
#[tokio::test]
async fn acceptance_59_an_outside_signal_stops_the_run() {
    let harness = Harness::start().await;
    let (task, run, marks) = running(&harness).await;
    let shell = pid(marks.path(), "shell").await;
    let child = pid(marks.path(), "child").await;
    let killed = std::process::Command::new("kill")
        .args(["-TERM", &shell.to_string()])
        .status()
        .unwrap();
    assert!(killed.success());

    let event = tokio::time::timeout(STOP_WAIT, finished(&harness, "backend", &task))
        .await
        .expect("the run did not end");
    assert_eq!(event["data"]["state"], "failed", "{event}");
    assert_eq!(event["data"]["reason"], "killed", "{event}");
    ended(child).await;
    let read = harness
        .call(
            "backend",
            "get_check_run",
            json!({"run": run, "detail": "full"}),
        )
        .await;
    let outcomes: Vec<&str> = read["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, ["failed", "not_run"], "{read}");
    assert!(
        !marks.path().join("later").exists(),
        "no later check starts"
    );
    let messages = harness.fold_messages("backend").await;
    assert!(
        messages
            .iter()
            .any(|message| message.contains("slow (signal 15) killed")),
        "{messages:?}"
    );
}

/// Acceptance 59, a human over HTTP: the route takes exactly one of a run, a task and all. The cancel of backend's task ends its running run as `cancelled` by the human; a second cancel of that run is `check_run_ended`, and the cancel of all finds no running run.
#[tokio::test]
async fn acceptance_59_a_human_cancels_the_runs_of_a_task_over_http() {
    let harness = Harness::start().await;
    let (task, run, marks) = running(&harness).await;
    let child = pid(marks.path(), "child").await;
    let path = "/api/v1/projects/demo/check-runs/cancel";

    let (status, both) = harness
        .http(
            Method::POST,
            path,
            Some(&json!({"run": run, "task": task})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{both}");
    assert_eq!(both["error"]["code"], "invalid_request", "{both}");

    let (status, cancelled) = harness
        .http(Method::POST, path, Some(&json!({"task": task})), HUMAN)
        .await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(cancelled["cancelled"], json!([run]), "{cancelled}");
    ended(child).await;
    let event = tokio::time::timeout(STOP_WAIT, finished(&harness, "backend", &task))
        .await
        .expect("the cancel did not end the run");
    assert_eq!(event["data"]["reason"], "cancelled", "{event}");
    assert_eq!(event["data"]["by"], format!("human:{HUMAN}"), "{event}");

    let (status, ended_run) = harness
        .http(Method::POST, path, Some(&json!({"run": run})), HUMAN)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{ended_run}");
    assert_eq!(ended_run["error"]["code"], "check_run_ended", "{ended_run}");

    let (status, none) = harness
        .http(Method::POST, path, Some(&json!({"all": true})), HUMAN)
        .await;
    assert_eq!(status, StatusCode::OK, "{none}");
    assert_eq!(none["cancelled"], json!([]), "{none}");
    assert!(
        !marks.path().join("later").exists(),
        "no later check starts"
    );
}

/// Acceptance 59, a run that waits for a run slot: under `[runs] max_parallel_runs = 1` the first run holds the place and the next two wait. backend cancels the second while it waits: the cancel answers at once, the run ends as `cancelled` without a command, and once the first run ends the third gets the place and runs.
#[tokio::test]
async fn acceptance_59_a_run_that_waits_for_a_run_slot_is_cancelled_without_a_start() {
    let harness = Harness::start().await;
    let marks = tempfile::TempDir::new().unwrap();
    let release = marks.path().join("release");
    let checks = format!(
        "[runs]\nmax_parallel_runs = 1\n\n[[check]]\nname = \"slow\"\nrun = \"touch '{dir}'/$$; until [ -e '{release}' ]; do sleep 0.05; done\"\n",
        dir = marks.path().display(),
        release = release.display(),
    );
    let mut runs = Vec::new();
    for number in 1..=3 {
        let task = harness
            .create_task("backend", &format!("Task {number}"))
            .await;
        let work = harness.start_work("backend", &task).await;
        work.write(&[("fold-checks.toml", &checks), ("src/lib.txt", "x\n")]);
        let started = harness
            .call("backend", "run_checks", json!({"task": task}))
            .await;
        assert_eq!(started["checks"], "running", "{started}");
        runs.push((task, started["run"].as_str().unwrap().to_owned()));
    }
    let commands = || std::fs::read_dir(marks.path()).unwrap().count();
    eventually(STOP_WAIT, async || {
        if commands() == 1 {
            Ok(())
        } else {
            Err(commands())
        }
    })
    .await;
    let (waiting_task, waiting) = &runs[1];
    let view = harness
        .call("backend", "get_check_run", json!({"run": waiting}))
        .await;
    assert_eq!(view["waiting_for"], "run_slot", "{view}");

    let asked = std::time::Instant::now();
    let cancelled = harness
        .call("backend", "cancel_check_run", json!({"run": waiting}))
        .await;
    assert_eq!(cancelled["reason"], "cancelled", "{cancelled}");
    assert!(
        asked.elapsed() < Duration::from_secs(10),
        "the cancel waited for the slot"
    );
    let event = tokio::time::timeout(STOP_WAIT, finished(&harness, "backend", waiting_task))
        .await
        .expect("the cancel did not end the waiting run");
    assert_eq!(event["data"]["reason"], "cancelled", "{event}");
    assert_eq!(commands(), 1, "the cancelled run started no command");

    std::fs::write(&release, "").unwrap();
    for (index, (_, run)) in runs.iter().enumerate() {
        let want = if index == 1 { "failed" } else { "passed" };
        eventually(STOP_WAIT, async || {
            let view = harness
                .call("backend", "get_check_run", json!({"run": run}))
                .await;
            if view["state"] == want {
                Ok(())
            } else {
                Err(view)
            }
        })
        .await;
    }
    assert_eq!(commands(), 2 + 1, "the third run got the place and ran");
}
