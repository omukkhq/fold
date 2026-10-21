#![cfg(test)]

use fold_core::WorkerState;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, WAIT, assert_in_order};
use crate::workers_support::FakeClaude;

/// Acceptance 24: two related tasks are reviewed in one batch. The login form opens its proposal first; the review lead sees that the login route will touch the same flow and makes a review plan that waits for it, instead of starting a reviewer at once. When the route's proposal opens, fold adds it to the plan, and the review lead's start gives both proposals to one reviewer as one batch, which it checks out once and approves, and both integrate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acceptance_24_the_review_lead_waits_for_a_related_task_and_one_reviewer_reviews_both() {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main("frontend").await;
    h.call("frontend", "start_worker", json!({"role": "review_lead"}))
        .await;
    h.connect("review-lead").await;
    let form = h.create_task("frontend", "Add the login form").await;
    let route = h.create_task("frontend", "Add the login route").await;

    let form_work = h.start_work("frontend", &form).await;
    form_work.write(&[("src/login_form.txt", "<form action=/login>\n")]);
    let first = h
        .propose("frontend", &form_work, "Add the login form")
        .await;
    let plan = h
        .call(
            "review-lead",
            "create_review_plan",
            json!({"proposals": [first], "tasks": [route]}),
        )
        .await;
    assert_eq!(plan["id"], "rp-1", "{plan}");
    assert_eq!(plan["awaited"][0]["task"], route);
    let early = h
        .refused(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": [first]}),
        )
        .await;
    assert_eq!(early["code"], "already_in_plan");

    let route_work = h.start_work("frontend", &route).await;
    route_work.write(&[("src/login_route.txt", "POST /login\n")]);
    let second = h
        .propose("frontend", &route_work, "Add the login route")
        .await;
    let ready = h
        .call("review-lead", "get_review_plan", json!({"id": "rp-1"}))
        .await;
    assert_eq!(ready["proposals"], json!([first, second]), "{ready}");
    assert_eq!(ready["awaited"], json!([]));

    let started = h
        .call(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "plan": "rp-1"}),
        )
        .await;
    let reviewer = started["worker"]["name"].as_str().unwrap().to_owned();
    eventually(WAIT, async || {
        let row = h
            .handle
            .agents()
            .get_worker(&fold_core::AgentName::parse(&reviewer).unwrap())
            .await
            .unwrap();
        if row.state == WorkerState::Idle {
            Ok(())
        } else {
            Err(row.state)
        }
    })
    .await;
    h.connect(&reviewer).await;
    let batches = h
        .call(&reviewer, "list_review_batches", json!({"open": true}))
        .await;
    let batch = &batches["batches"][0];
    assert_eq!(batch["holder"], reviewer.as_str(), "{batches}");
    let members: Vec<Value> = batch["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| member["proposal"].clone())
        .collect();
    assert_eq!(members, [json!(first), json!(second)]);
    h.call(&reviewer, "create_batch_checkout", json!({"id": "rb-1"}))
        .await;
    let submitted = h
        .call(
            &reviewer,
            "submit_batch_review",
            json!({"id": "rb-1", "verdicts": [
                {"proposal": first, "verdict": "approve"},
                {"proposal": second, "verdict": "approve"},
            ]}),
        )
        .await;
    assert_eq!(
        submitted["reviewed"].as_array().unwrap().len(),
        2,
        "{submitted}"
    );
    for proposal in [&first, &second] {
        let integrated = h
            .call(
                "review-lead",
                "integrate_proposal",
                json!({"id": proposal, "expected_version": 1}),
            )
            .await;
        assert_eq!(integrated["result"], "succeeded", "{integrated}");
    }
    let plan = h
        .call("review-lead", "get_review_plan", json!({"id": "rp-1"}))
        .await;
    assert_eq!(plan["state"], "started");
    assert_eq!(plan["batch"], "rb-1");
    assert_in_order(
        &h.events().await,
        &[
            "review.plan_created",
            "review.plan_joined",
            "review.batch_created",
            "review.plan_started",
            "review.batch_submitted",
            "integration.succeeded",
            "integration.succeeded",
        ],
    );
}
