#![cfg(test)]

use std::time::Duration;

use fold_core::WorkerState;
use fold_store::testing::eventually;
use serde_json::json;

use crate::common::acceptance::WAIT;
use crate::worker_resume::{TASK_WORKER, task_worker_for, until_runs, until_state, worker};
use crate::workers_support::FakeClaude;

const WAIT_FOR: &str = "my go after the T-2 migration";

/// Acceptance 64, a parked worker stays parked when its checks end: the task worker sends `create_proposal`, and fold runs the proposal checks before it makes the version. The web lead parks the worker with stop_worker and wait_for while the run waits, and the run ends red. fold makes no proposal and tells the worker in a message, but the worker gets no new turn: it still waits for the lead's go. The lead's go resumes it in its own session, and its turn holds the run's result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_64_a_parked_worker_stays_parked_when_its_check_run_fails_and_its_go_names_the_result()
 {
    let fake = FakeClaude::new("result").exiting_on_eof_with(1);
    let mut harness = task_worker_for(&fake, &["Draw the page"], WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    let outside = tempfile::tempdir().unwrap();
    let gate = outside.path().join("go");
    work.write(&[
        ("page.html", "<p>page</p>\n"),
        (
            "fold-checks.toml",
            &format!(
                "[[check]]\nname = \"render\"\nrun = \"until [ -e '{}' ]; do sleep 0.05; done; exit 1\"\n",
                gate.display()
            ),
        ),
    ]);
    // A park cancels the runs that run at that time, so the worker is parked before its session starts the run.
    harness
        .call(
            "web-lead",
            "stop_worker",
            json!({"name": TASK_WORKER, "wait_for": WAIT_FOR}),
        )
        .await;
    let stopped = until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    let park = stopped.park.expect("the row records the park");
    let created = harness
        .call(
            TASK_WORKER,
            "create_proposal",
            json!({"intent": "Make the change the task asks for.", "task": "T-1", "title": "Draw the page"}),
        )
        .await;
    assert_eq!(created["checks"], "running", "{created}");
    harness.disconnect(TASK_WORKER).await;
    harness
        .project
        .clock()
        .advance(Duration::from_secs(60))
        .unwrap();
    std::fs::write(&gate, "").unwrap();

    let finished = eventually(WAIT, async || {
        harness
            .events()
            .await
            .into_iter()
            .find(|event| event.kind == "checks.finished")
            .map(|event| event.data)
            .ok_or(())
    })
    .await;
    assert_eq!(finished["state"], "failed", "{finished}");
    eventually(WAIT, async || {
        if harness.fold_messages(TASK_WORKER).await.is_empty() {
            Err("fold has not told the worker the result")
        } else {
            Ok(())
        }
    })
    .await;
    // A daemon start serves every owed resume before it returns.
    harness.restart().await;
    harness.connect("web-lead").await;
    let row = worker(&harness, TASK_WORKER).await;
    assert_eq!(row.state, WorkerState::Stopped, "no new turn while parked");
    assert_eq!(row.park.map(|park| park.at), Some(park.at));
    assert_eq!(
        fake.runs()
            .iter()
            .filter(|run| run.agent() == TASK_WORKER)
            .count(),
        1,
        "the red run resumes nobody"
    );

    harness
        .send(
            "web-lead",
            &[TASK_WORKER],
            "Go: the T-2 migration is in main",
        )
        .await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert_eq!(
        runs[1].flag("--resume"),
        Some(stopped.session_id.as_str()),
        "the go resumes it in its own session"
    );
    let turn = runs[1].stdin.join("\n");
    for wanted in [
        "The checks of T-1 (cr-1) did not pass, so fold made no proposal: render failed.",
        "call create_proposal again",
    ] {
        assert!(turn.contains(wanted), "{wanted} in {turn}");
    }
}
