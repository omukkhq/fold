#![cfg(test)]

use std::time::Duration;

use fold_core::WorkerState;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::auto_integrate::Running;
use crate::common::acceptance::{Harness, WAIT, assert_in_order};
use crate::workers_support::FakeClaude;

const LEAD: &str = "review-lead";

/// The first line of the review lead's session input that contains `needle`, once fold has sent it.
async fn lead_hears(fake: &FakeClaude, needle: &str) -> String {
    eventually(WAIT, async || {
        let line = fake
            .runs_of(LEAD)
            .into_iter()
            .flat_map(|run| run.stdin)
            .find(|line| line.contains(needle));
        line.ok_or(needle.to_owned())
    })
    .await
}

/// The proposals and tasks of the `create_review_plan(proposals[...], tasks[...])` call that a notice names.
fn named_plan(notice: &str) -> (Vec<String>, Vec<String>) {
    let list = |key: &str| -> Vec<String> {
        let start = notice.find(key).expect("the notice names the plan call") + key.len();
        let end = start + notice[start..].find(']').unwrap();
        notice[start..end].split(", ").map(str::to_owned).collect()
    };
    (list("create_review_plan(proposals["), list(", tasks["))
}

async fn state_of(h: &Harness, proposal: &str) -> Value {
    h.call(LEAD, "get_proposal", json!({"id": proposal})).await
}

/// Acceptance 27: the code and the docs of one request are reviewed together while fold integrates automatically. The request's approval makes two tasks whose files differ. The API proposal opens first, and fold's notice tells the review lead that the docs task of the same request can still open its proposal and which plan waits for it; the lead makes that plan at once, as its notice says, before any reviewer starts. Minutes pass and the integration job runs, but the API proposal waits in the plan instead of integrating alone. When the docs proposal opens it joins the plan, one reviewer gets both as a batch and approves them, and fold integrates them together.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acceptance_27_the_review_lead_plans_on_the_notice_and_one_reviewer_reviews_both_tasks_of_a_request()
 {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main("frontend").await;
    h.call("frontend", "start_worker", json!({"role": "review_lead"}))
        .await;
    h.connect(LEAD).await;
    let integrate = Running::start(&h.state);

    let request = h
        .call(
            "frontend",
            "create_request",
            json!({"body": "Let a client read the weekly report over the API."}),
        )
        .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    h.call(
        "frontend",
        "claim_request",
        json!({"id": request, "kind": "feature", "title": "Weekly report API"}),
    )
    .await;
    h.call(
        "frontend",
        "submit_request_draft",
        json!({"id": request, "cards": [
            {"key": "api", "title": "Add the report API", "description": "GET /report returns the week."},
            {"key": "docs", "title": "Document the report API", "description": "The API guide names GET /report."},
        ]}),
    )
    .await;
    let approved = h
        .call(
            "frontend",
            "approve_request_draft",
            json!({"id": request, "expected_version": 1}),
        )
        .await;
    let (api, docs) = (
        approved["tasks"][0].as_str().unwrap().to_owned(),
        approved["tasks"][1].as_str().unwrap().to_owned(),
    );

    let api_work = h.start_work("frontend", &api).await;
    let docs_work = h.start_work("frontend", &docs).await;
    api_work.write(&[("src/report_api.txt", "GET /report\n")]);
    let runs = integrate.checks();
    let first = h.propose("frontend", &api_work, "Add the report API").await;

    // The lead follows its notice: it makes the plan the notice names, before any reviewer starts.
    let notice = lead_hears(&fake, &format!("{first} opened for {api}")).await;
    assert!(
        notice.contains("Call create_review_plan."),
        "the notice of {first}: {notice}"
    );
    let (proposals, tasks) = named_plan(&notice);
    assert_eq!(
        (&proposals, &tasks),
        (&vec![first.clone()], &vec![docs.clone()])
    );
    let plan = h
        .call(
            LEAD,
            "create_review_plan",
            json!({"proposals": proposals, "tasks": tasks}),
        )
        .await;
    let plan = plan["id"].as_str().unwrap().to_owned();
    let alone = h
        .refused(
            LEAD,
            "start_worker",
            json!({"role": "reviewer", "proposals": [first]}),
        )
        .await;
    assert_eq!(alone["code"], "already_in_plan", "{alone}");

    // Minutes pass and the job has run since the API proposal opened; the proposal waits in the plan.
    h.project
        .clock()
        .advance(Duration::from_secs(4 * 60))
        .unwrap();
    integrate.runs(runs + 1).await;
    let waiting = state_of(&h, &first).await;
    assert_eq!(waiting["state"], "open", "{waiting}");
    assert_eq!(waiting["review_plan"]["id"], plan.as_str(), "{waiting}");

    docs_work.write(&[("docs/report_api.md", "# GET /report\n")]);
    let second = h
        .propose("frontend", &docs_work, "Document the report API")
        .await;
    lead_hears(&fake, &format!("of {docs} joined {plan}")).await;
    let started = h
        .call(
            LEAD,
            "start_worker",
            json!({"role": "reviewer", "plan": plan}),
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
    let batch = batches["batches"][0]["id"].as_str().unwrap().to_owned();
    let members: Vec<Value> = batches["batches"][0]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| member["proposal"].clone())
        .collect();
    assert_eq!(members, [json!(first), json!(second)], "{batches}");
    h.call(&reviewer, "create_batch_checkout", json!({"id": batch}))
        .await;
    h.call(
        &reviewer,
        "submit_batch_review",
        json!({"id": batch, "verdicts": [
            {"proposal": first, "verdict": "approve"},
            {"proposal": second, "verdict": "approve"},
        ]}),
    )
    .await;

    for proposal in [&first, &second] {
        let integrated = eventually(WAIT, async || {
            let read = state_of(&h, proposal).await;
            if read["state"] == "integrated" {
                Ok(read)
            } else {
                Err(read["state"].clone())
            }
        })
        .await;
        assert_eq!(
            integrated["integrated"]["batch"],
            batch.as_str(),
            "{integrated}"
        );
    }
    integrate.stop().await;
    assert_in_order(
        &h.events().await,
        &[
            "request.approved",
            "proposal.opened",
            "review.plan_created",
            "proposal.opened",
            "review.plan_joined",
            "review.batch_created",
            "review.plan_started",
            "review.batch_submitted",
            "integration.succeeded",
            "integration.succeeded",
        ],
    );
}

/// Acceptance 27, the missed notice: the review lead acts on neither notice of the API proposal, so it still waits for a review in no plan when the docs proposal of the same request opens. The docs notice names the API proposal and the start_worker call that gives both to one reviewer, since no task is left to wait for. One reviewer approves both as a batch and fold integrates them together.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acceptance_27_a_lead_that_missed_the_first_notice_still_gives_both_proposals_to_one_reviewer()
 {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main("frontend").await;
    h.call("frontend", "start_worker", json!({"role": "review_lead"}))
        .await;
    h.connect(LEAD).await;
    let integrate = Running::start(&h.state);

    let request = h
        .call(
            "frontend",
            "create_request",
            json!({"body": "Let a client read the weekly report over the API."}),
        )
        .await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    h.call(
        "frontend",
        "claim_request",
        json!({"id": request, "kind": "feature", "title": "Weekly report API"}),
    )
    .await;
    h.call(
        "frontend",
        "submit_request_draft",
        json!({"id": request, "cards": [
            {"key": "api", "title": "Add the report API", "description": "GET /report returns the week."},
            {"key": "docs", "title": "Document the report API", "description": "The API guide names GET /report."},
        ]}),
    )
    .await;
    let approved = h
        .call(
            "frontend",
            "approve_request_draft",
            json!({"id": request, "expected_version": 1}),
        )
        .await;
    let (api, docs) = (
        approved["tasks"][0].as_str().unwrap().to_owned(),
        approved["tasks"][1].as_str().unwrap().to_owned(),
    );

    let api_work = h.start_work("frontend", &api).await;
    let docs_work = h.start_work("frontend", &docs).await;
    api_work.write(&[("src/report_api.txt", "GET /report\n")]);
    let first = h.propose("frontend", &api_work, "Add the report API").await;
    lead_hears(&fake, &format!("{first} opened for {api}")).await;
    docs_work.write(&[("docs/report_api.md", "# GET /report\n")]);
    let second = h
        .propose("frontend", &docs_work, "Document the report API")
        .await;

    // The lead acts on the second notice, which names the first proposal too.
    let notice = lead_hears(&fake, &format!("{second} opened for {docs}")).await;
    assert!(
        notice.contains(&format!(
            "related {first} ({api}) waits for a review: start_worker(role reviewer, proposals[{second}, {first}]) gives them to one reviewer. Call start_worker."
        )),
        "the notice of {second}: {notice}"
    );
    let started = h
        .call(
            LEAD,
            "start_worker",
            json!({"role": "reviewer", "proposals": [second, first]}),
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
    let batch = batches["batches"][0]["id"].as_str().unwrap().to_owned();
    h.call(&reviewer, "create_batch_checkout", json!({"id": batch}))
        .await;
    h.call(
        &reviewer,
        "submit_batch_review",
        json!({"id": batch, "verdicts": [
            {"proposal": first, "verdict": "approve"},
            {"proposal": second, "verdict": "approve"},
        ]}),
    )
    .await;
    for proposal in [&first, &second] {
        let integrated = eventually(WAIT, async || {
            let read = state_of(&h, proposal).await;
            if read["state"] == "integrated" {
                Ok(read)
            } else {
                Err(read["state"].clone())
            }
        })
        .await;
        assert_eq!(
            integrated["integrated"]["batch"],
            batch.as_str(),
            "{integrated}"
        );
    }
    integrate.stop().await;
}
