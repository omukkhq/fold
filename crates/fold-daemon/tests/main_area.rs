#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_core::codes;
use fold_core::policy::MAIN_AREA_REASON;
use fold_core::roles::Role;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, SLUG, TOKEN};

const MAIN: &str = "frontend";
const LEAD: &str = "backend";

/// `frontend` is the main agent, `backend` leads its area of the work and `review` is the review lead.
async fn harness() -> Harness {
    let mut h = Harness::start_without_leads().await;
    h.make_main(MAIN).await;
    h.lead_area(LEAD).await;
    h
}

fn path(rest: &str) -> String {
    format!("/api/v1/projects/{SLUG}{rest}")
}

async fn create_main_task(h: &Harness, title: &str) -> String {
    let created = h
        .call(
            MAIN,
            "create_task",
            json!({"title": title, "labels": ["main"]}),
        )
        .await;
    created["id"].as_str().unwrap().to_owned()
}

async fn mark_of(h: &Harness, task: &str) -> Value {
    h.call(MAIN, "get_task", json!({"id": task})).await["human_review"].clone()
}

/// The main agent builds `task` and opens a proposal for it; returns the proposal id.
async fn build(h: &Harness, task: &str) -> String {
    let work = h.start_work(MAIN, task).await;
    work.write(&[("AGENTS.md", "# Notes\n")]);
    h.propose(MAIN, &work, "Write the agent notes").await
}

async fn human_approve(h: &Harness, proposal: &str) -> (StatusCode, Value) {
    let version = h.version_of(MAIN, proposal).await;
    h.http(
        Method::POST,
        &path(&format!("/proposals/{proposal}/reviews")),
        Some(&json!({"version": version, "verdict": "approve", "body": "Fine."})),
        HUMAN,
    )
    .await
}

#[tokio::test]
async fn a_task_of_main_belongs_to_the_main_agent_and_to_no_lead() {
    let h = harness().await;
    let task = create_main_task(&h, "Set up the checks").await;
    let mark = mark_of(&h, &task).await;
    assert_eq!(mark["reason"], MAIN_AREA_REASON, "{mark}");
    assert_eq!(mark["set_by"], "fold", "{mark}");

    let error = h.refused(LEAD, "claim_task", json!({"id": task})).await;
    assert_eq!(error["code"], codes::TASK_OUTSIDE_AREA, "{error}");
    let error = h
        .refused(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "main"}),
        )
        .await;
    assert_eq!(error["code"], codes::MAIN_AREA, "{error}");
    let error = h
        .refused(
            MAIN,
            "update_area",
            json!({"name": "main", "model": "opus"}),
        )
        .await;
    assert_eq!(error["code"], codes::MAIN_AREA, "{error}");
    h.call(
        MAIN,
        "create_label",
        json!({"name": "docs", "color": "sage"}),
    )
    .await;
    for define in [
        json!({"name": "main", "label": "docs"}),
        json!({"name": "notes", "label": "main"}),
    ] {
        let error = h.refused(MAIN, "define_area", define).await;
        assert_eq!(error["code"], codes::AREA_EXISTS, "{error}");
    }
    let areas = h.call(MAIN, "list_workers", json!({})).await["areas"].clone();
    assert!(
        areas
            .as_array()
            .unwrap()
            .iter()
            .all(|area| area["name"] != "main"),
        "{areas}"
    );
    let (status, listed) = h.http(Method::GET, &path("/workers"), None, HUMAN).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["main_label"], "main", "{listed}");

    let claimed = h.call(MAIN, "claim_task", json!({"id": task})).await;
    assert_eq!(claimed["holder"], MAIN, "{claimed}");
}

#[tokio::test]
async fn a_task_that_comes_into_main_gets_the_mark_and_a_planned_word_leaves_it_off() {
    let h = harness().await;
    let task = h.create_task_for(MAIN, LEAD, "Write the notes").await;
    assert_eq!(mark_of(&h, &task).await, Value::Null);
    h.call(MAIN, "update_task", json!({"id": task, "labels": ["main"]}))
        .await;
    assert_eq!(mark_of(&h, &task).await["reason"], MAIN_AREA_REASON);

    let planned = h
        .call(
            MAIN,
            "create_task_plan",
            json!({"tasks": [
                {"key": "own", "title": "Ignore build output", "description": "Acceptance: .gitignore lists target", "labels": ["main"], "main_approves": true},
                {"key": "marked", "title": "Write AGENTS.md", "description": "Acceptance: the notes exist", "labels": ["main"]},
                {"key": "named", "title": "Add the checks", "description": "Acceptance: fold-checks.toml exists", "labels": ["main"], "human_review": {"reason": "the user reads the checks"}},
            ]}),
        )
        .await;
    let id = |key: &str| {
        planned["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|task| task["key"] == key)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(mark_of(&h, &id("own")).await, Value::Null);
    assert_eq!(mark_of(&h, &id("marked")).await["reason"], MAIN_AREA_REASON);
    assert_eq!(
        mark_of(&h, &id("named")).await["reason"],
        "the user reads the checks"
    );
    let error = h
        .refused(
            MAIN,
            "create_task_plan",
            json!({"tasks": [{"key": "x", "title": "X", "description": "Acceptance: x", "main_approves": true}]}),
        )
        .await;
    assert_eq!(error["details"]["reason"], "main_approves", "{error}");
}

#[tokio::test]
async fn a_ready_task_of_main_reaches_the_main_agent() {
    let h = harness().await;
    let mut inbox = h.register_inbox(MAIN).await;
    let task = create_main_task(&h, "Set up the checks").await;
    let text = format!("fold: {task} is ready in area main, for you to build. Call claim_task.");
    let posted = inbox.next().await;
    assert!(posted.contains(&text), "{posted}");
    assert!(posted.contains(TOKEN), "{posted}");
}

#[tokio::test]
async fn no_review_agent_reviews_a_task_of_main_and_a_human_approves_it_alone() {
    let h = harness().await;
    let task = create_main_task(&h, "Write the agent notes").await;
    let proposal = build(&h, &task).await;
    let version = h.version_of(MAIN, &proposal).await;

    let error = h
        .refused("review", "claim_review", json!({"id": proposal}))
        .await;
    assert_eq!(error["code"], codes::WORKER_CLAIM_DENIED, "{error}");
    for (agent, code) in [
        ("review", codes::MAIN_AREA),
        (MAIN, codes::AUTHOR_CANNOT_REVIEW),
    ] {
        let error = h
            .refused(
                agent,
                "submit_review",
                json!({"id": proposal, "version": version, "verdict": "approve", "body": "Looks right."}),
            )
            .await;
        assert_eq!(error["code"], code, "{agent}: {error}");
    }
    let waiting = fold_daemon::services::reviews::awaiting_human_review(&h.handle.store)
        .await
        .unwrap();
    assert_eq!(
        waiting
            .iter()
            .map(|item| item.proposal.to_string())
            .collect::<Vec<_>>(),
        std::slice::from_ref(&proposal)
    );

    let (status, detail) = h
        .http(
            Method::GET,
            &path(&format!("/proposals/{proposal}")),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["main_task"], true, "{detail}");
    let (status, reviewed) = human_approve(&h, &proposal).await;
    assert_eq!(status, StatusCode::CREATED, "{reviewed}");
    let (status, integrated) = h
        .http(
            Method::POST,
            &path(&format!("/proposals/{proposal}/integrate")),
            Some(&json!({"expected_version": version})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{integrated}");
    assert_eq!(h.completions(&task).await, 1);
}

#[tokio::test]
async fn the_main_agent_approves_its_own_proposal_only_once_a_human_cleared_the_mark() {
    let h = harness().await;
    let task = create_main_task(&h, "Write the agent notes").await;
    let proposal = build(&h, &task).await;
    let version = h.version_of(MAIN, &proposal).await;
    let approve =
        json!({"id": proposal, "version": version, "verdict": "approve", "body": "Checked."});
    let error = h.refused(MAIN, "submit_review", approve.clone()).await;
    assert_eq!(error["code"], codes::AUTHOR_CANNOT_REVIEW, "{error}");

    let (status, cleared) = h
        .http(
            Method::DELETE,
            &path(&format!("/tasks/{task}/human-review")),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    let submitted = h.call(MAIN, "submit_review", approve).await;
    assert_eq!(submitted["verdict"], "approve", "{submitted}");
    let error = h
        .refused(
            MAIN,
            "integrate_proposal",
            json!({"id": proposal, "expected_version": version}),
        )
        .await;
    assert_eq!(error["code"], codes::AUTHOR_CANNOT_INTEGRATE, "{error}");

    let (status, detail) = h
        .http(
            Method::GET,
            &path(&format!("/proposals/{proposal}")),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    let approval = detail["gates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|gate| gate["gate"] == "approval")
        .unwrap()
        .clone();
    assert_eq!(approval["status"], "met", "{detail}");
}

#[tokio::test]
async fn the_review_lead_starts_no_reviewer_and_makes_no_batch_for_a_task_of_main() {
    let mut h = harness().await;
    h.connect("review-lead").await;
    crate::common::roles::give_role(&h.handle.store, "review-lead", Role::ReviewLead, None).await;
    let task = create_main_task(&h, "Write the agent notes").await;
    let main_proposal = build(&h, &task).await;
    let mut others = Vec::new();
    for title in ["Fix the parser", "Fix the lexer"] {
        let task = h.call(MAIN, "create_task", json!({"title": title})).await["id"]
            .as_str()
            .unwrap()
            .to_owned();
        others.push((task.clone(), build(&h, &task).await));
    }

    let error = h
        .refused(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": [main_proposal]}),
        )
        .await;
    assert_eq!(error["code"], codes::MAIN_AREA, "{error}");
    let error = h
        .refused(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": [main_proposal, others[0].1]}),
        )
        .await;
    assert_eq!(error["code"], codes::MAIN_AREA, "{error}");

    let batch = h.give_batch("review", &[&others[0].1, &others[1].1]).await;
    h.call(
        MAIN,
        "update_task",
        json!({"id": others[0].0, "labels": ["main"]}),
    )
    .await;
    let error = h
        .refused(
            "review",
            "submit_batch_review",
            json!({"id": batch, "verdicts": [{"proposal": others[0].1, "verdict": "approve", "body": "Fine."}]}),
        )
        .await;
    assert_eq!(error["code"], codes::MAIN_AREA, "{error}");

    let mut both = Harness::work_labels_of(LEAD);
    both.as_array_mut().unwrap().push(json!("main"));
    let error = h
        .refused(MAIN, "update_task", json!({"id": task, "labels": both}))
        .await;
    assert_eq!(error["details"]["reason"], "two_areas", "{error}");
}

#[tokio::test]
async fn a_task_that_leaves_main_loses_folds_mark_and_keeps_a_humans() {
    let h = harness().await;
    let task = create_main_task(&h, "Write the agent notes").await;
    assert_eq!(mark_of(&h, &task).await["reason"], MAIN_AREA_REASON);
    h.call(
        MAIN,
        "update_task",
        json!({"id": task, "labels": Harness::work_labels_of(LEAD)}),
    )
    .await;
    assert_eq!(mark_of(&h, &task).await, Value::Null);
    let events = h
        .call(MAIN, "get_events", json!({"task": task}))
        .await
        .to_string();
    assert!(events.contains("task.human_review_cleared"), "{events}");

    let kept = h.create_task_for(MAIN, LEAD, "Change the billing").await;
    let (status, marked) = h
        .http(
            Method::PUT,
            &path(&format!("/tasks/{kept}/human-review")),
            Some(&json!({"reason": "billing"})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{marked}");
    for labels in [json!(["main"]), Harness::work_labels_of(LEAD)] {
        h.call(MAIN, "update_task", json!({"id": kept, "labels": labels}))
            .await;
        assert_eq!(mark_of(&h, &kept).await["reason"], "billing");
    }
}
