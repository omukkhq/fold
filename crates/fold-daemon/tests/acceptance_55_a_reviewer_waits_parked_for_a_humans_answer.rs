#![cfg(test)]

use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::WorkerState;
use fold_daemon::services::tasks::sweep_leases;
use fold_store::testing::eventually;
use serde_json::json;

use crate::common::acceptance::{HUMAN, SLUG, WAIT};
use crate::worker_resume::{
    MAIN, REVIEWER, TASK_WORKER, task_worker_for, until_runs, until_state, worker,
};
use crate::workers_support::FakeClaude;

const LEAD: &str = "review-lead";

/// Waits until the running review lead has read a stdin line that contains `needle`, and returns every line it read.
async fn until_lead_hears(fake: &FakeClaude, needle: &str) -> Vec<String> {
    eventually(WAIT, async || {
        let runs = fake.runs_of(LEAD);
        match runs.last() {
            Some(run) if run.stdin.iter().any(|line| line.contains(needle)) => {
                Ok(run.stdin.clone())
            }
            _ => Err(runs.len()),
        }
    })
    .await
}

/// Acceptance 55: a reviewer asks the humans about its proposal and ends its turn. fold parks it in its own name: no notice of the version it leaves and no stale claim reach the review lead while a person thinks, and the human's answer in the ask's thread resumes the same reviewer in its own session with that answer.
#[tokio::test]
async fn acceptance_55_a_reviewer_that_asks_a_human_waits_parked_until_the_answer_resumes_it() {
    let fake = FakeClaude::new("result");
    let mut harness = task_worker_for(&fake, &["Draw the login page"], WorkerState::Idle).await;
    fold_daemon::services::projects::ensure_human(
        harness.project.registry(),
        harness.project.clock(),
        &fold_core::HumanName::parse(HUMAN).unwrap(),
    )
    .await
    .unwrap();
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    work.write(&[("login.html", "<form>login</form>\n")]);
    harness
        .propose(TASK_WORKER, &work, "Draw the login page")
        .await;
    until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    harness
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    until_state(&harness, LEAD, WorkerState::Idle).await;
    harness.connect(LEAD).await;
    harness
        .call(
            LEAD,
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-1"]}),
        )
        .await;
    until_state(&harness, REVIEWER, WorkerState::Idle).await;
    harness.connect(REVIEWER).await;
    harness
        .call(REVIEWER, "create_review_checkout", json!({"id": "P-1"}))
        .await;

    let ask = harness
        .send_with(
            REVIEWER,
            json!({"to": ["humans"], "body": "Must the form name the password rules?", "proposal": "P-1", "asks_human": true}),
        )
        .await;
    // The reviewer's next turn ends with the ask open and no other work, so fold parks it.
    harness
        .send(LEAD, &[REVIEWER], "Tell me when the human answers.")
        .await;
    let parked = until_state(&harness, REVIEWER, WorkerState::Stopped).await;
    let park = parked.park.expect("the reviewer is parked");
    assert_eq!(
        park.by.as_str(),
        REVIEWER,
        "it waits for a person, not an agent"
    );
    assert!(park.wait_for.contains("P-1"), "{}", park.wait_for);
    harness.disconnect(REVIEWER).await;

    // Past the reviewer's idle time and the stall time, but inside the lease that the park renewed.
    harness
        .project
        .clock()
        .advance(Duration::from_secs(20 * 60))
        .unwrap();
    assert_eq!(
        sweep_leases(&harness.handle, harness.project.now())
            .await
            .unwrap()
            .len(),
        0
    );
    harness.send(MAIN, &[LEAD], "after the park").await;
    let heard = until_lead_hears(&fake, "after the park").await;
    assert!(
        heard
            .iter()
            .all(|line| !line.contains("no verdict") && !line.contains("went stale")),
        "the review lead heard of the parked reviewer: {heard:?}"
    );
    assert_eq!(fake.runs_of(REVIEWER).len(), 1);

    let (status, reply) = harness
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/feedback"),
            Some(&json!({"reply_to": ask, "body": "Yes, list them under the field."})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{reply}");
    let runs = until_runs(&fake, REVIEWER, 2).await;
    assert_eq!(runs[1].flag("--resume"), Some(parked.session_id.as_str()));
    let answer = reply["id"].as_str().unwrap();
    assert!(
        runs[1]
            .stdin
            .iter()
            .any(|line| line.contains(&format!("{answer} from human:{HUMAN}"))),
        "the turn names the answer: {:?}",
        runs[1].stdin
    );
    harness.connect(REVIEWER).await;
    let inbox = harness.call(REVIEWER, "get_inbox", json!({})).await;
    assert!(
        inbox["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["body"] == "Yes, list them under the field."),
        "{inbox}"
    );
    let events = harness.events().await;
    assert!(
        events.iter().all(|event| event.kind != "claim.stale"),
        "no claim went stale"
    );
    assert_eq!(worker(&harness, REVIEWER).await.park, None);
}
