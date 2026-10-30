#![cfg(test)]

use std::path::Path;
use std::process::Command;

use axum::http::{Method, StatusCode};
use fold_core::{AgentName, Role, WorkerState, codes};
use fold_store::agents::WorkerProcessRow;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::auto_integrate::Running;
use crate::common::acceptance::{HUMAN, Harness, WAIT, Work, assert_in_order};
use crate::workers_support::{FakeClaude, Run};

const BASE: &str = "/api/v1/projects/demo";
const MAIN: &str = "main";
const LEAD: &str = "backend-lead";
const REVIEW_LEAD: &str = "review-lead";

async fn worker(h: &Harness, agent: &str) -> WorkerProcessRow {
    h.handle
        .agents()
        .get_worker(&AgentName::parse(agent).unwrap())
        .await
        .unwrap()
}

async fn until_state(h: &Harness, agent: &str, state: WorkerState) -> WorkerProcessRow {
    eventually(WAIT, async || {
        let row = worker(h, agent).await;
        if row.state == state {
            Ok(row)
        } else {
            Err(row.state)
        }
    })
    .await
}

/// The runs of `agent` once there are `count` and the newest has read its first turn.
async fn until_runs(fake: &FakeClaude, agent: &str, count: usize) -> Vec<Run> {
    eventually(WAIT, async || {
        let runs = fake.runs_of(agent);
        match runs.last() {
            Some(last) if runs.len() == count && last.stdin.len() >= 2 => Ok(runs),
            _ => Err(runs.len()),
        }
    })
    .await
}

/// How many integrations of the project have their Git commit.
async fn committed(h: &Harness) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM revision_commits c JOIN revisions r ON r.id = c.revision_id WHERE r.number > 1",
    )
    .fetch_one(h.handle.store.read())
    .await
    .unwrap()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8(output.stdout).unwrap()
}

/// The lead starts a task worker for `task`; the task worker connects once its first turn has ended.
async fn start_task_worker(h: &mut Harness, task: &str) -> String {
    let started = h
        .call(
            LEAD,
            "start_worker",
            json!({"role": "task_worker", "tasks": [task], "model": "sonnet", "instructions": "Keep the endpoint small."}),
        )
        .await;
    let task_worker = started["worker"]["name"].as_str().unwrap().to_owned();
    let view = h.call(LEAD, "get_task", json!({"id": task})).await;
    assert_eq!(
        view["claim"]["holder"], task_worker,
        "the start gives the new worker the claim"
    );
    until_state(h, &task_worker, WorkerState::Idle).await;
    h.connect(&task_worker).await;
    let refused = h
        .refused(&task_worker, "claim_task", json!({"id": task}))
        .await;
    assert_eq!(refused["code"], codes::WORKER_CLAIM_DENIED);
    task_worker
}

/// The task worker builds `task` in the workspace of the claim its lead gave it and proposes it under `title`; fold then stops it.
async fn build(
    h: &Harness,
    task_worker: &str,
    task: &str,
    file: &str,
    title: &str,
) -> (Work, String) {
    let work = h.start_work(task_worker, task).await;
    work.write(&[(file, "pub fn handler() {}\n")]);
    let proposal = h.propose(task_worker, &work, title).await;
    until_state(h, task_worker, WorkerState::Stopped).await;
    (work, proposal)
}

/// Acceptance 11, the team: the main agent that `fld claude` starts defines an area, turns a human's request into three cards and starts the leads; the area lead starts task workers and fold gives each the claim on its task, the review lead reviews a batch itself and gives the last proposal to a reviewer it starts with its review claim, fold resumes the task worker whose proposal needs changes, integrates every approved proposal in dependency order with one Git commit each, sends the dependent task to the lead once it is ready, and stops every short-lived worker on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_11_the_main_agent_runs_the_team_from_a_request_to_three_commits() {
    let mut fake = FakeClaude::new("result");
    // The daemon's own environment, as when the main agent's session started it.
    fake.launcher
        .env
        .push(("FOLD_AGENT_TAGS".to_owned(), "main".to_owned()));
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    let integrate = Running::start(&h.state);

    h.connect_tagged(MAIN, &["main"]).await;
    let agents = h.call(MAIN, "list_agents", json!({})).await;
    let main = agents["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["name"] == MAIN)
        .unwrap()
        .clone();
    assert_eq!(main["tags"], json!(["main"]), "{agents}");
    h.call(
        MAIN,
        "create_label",
        json!({"name": "backend", "color": "sage"}),
    )
    .await;
    let other = h
        .refused(
            "backend",
            "define_area",
            json!({"name": "backend", "label": "backend"}),
        )
        .await;
    assert_eq!(other["code"], codes::ORCHESTRATOR_REQUIRED, "{other}");
    let area = h
        .call(
            MAIN,
            "define_area",
            json!({"name": "backend", "label": "backend"}),
        )
        .await;
    assert_eq!(area["area"]["name"], "backend", "{area}");

    let (status, created) = h
        .create_request_form(
            &[(
                "body",
                None,
                None,
                "The API needs a list endpoint, an item endpoint and a search over both."
                    .as_bytes(),
            )],
            Some(HUMAN),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let refused = h
        .refused(
            "frontend",
            "claim_request",
            json!({"id": "RQ-1", "kind": "feature", "title": "Endpoints"}),
        )
        .await;
    assert_eq!(refused["code"], codes::ORCHESTRATOR_REQUIRED, "{refused}");
    h.call(
        MAIN,
        "claim_request",
        json!({"id": "RQ-1", "kind": "feature", "title": "Add the item API"}),
    )
    .await;
    let card = |key: &str, title: &str| json!({"key": key, "title": title, "label": "backend", "description": format!("{title}.\n\n**Acceptance**: its test passes.")});
    h.call(
        MAIN,
        "submit_request_draft",
        json!({
            "id": "RQ-1",
            "cards": [
                card("list", "Add the list endpoint"),
                card("item", "Add the item endpoint"),
                card("search", "Add the search endpoint"),
            ],
            "edges": [
                {"prerequisite": "list", "dependent": "search"},
                {"prerequisite": "item", "dependent": "search"},
            ],
        }),
    )
    .await;
    let (status, approved) = h
        .http(
            Method::POST,
            &format!("{BASE}/requests/RQ-1/approve"),
            Some(&json!({"expected_version": 1})),
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["tasks"], json!(["T-1", "T-2", "T-3"]));
    let search = h.call(MAIN, "get_task", json!({"id": "T-3"})).await;
    assert_eq!(search["status"], "backlog", "{search}");

    let lead = h
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "backend", "instructions": "Start a task worker for each ready task."}),
        )
        .await;
    assert_eq!(lead["worker"]["name"], LEAD, "{lead}");
    let review_lead = h
        .call(MAIN, "start_worker", json!({"role": "review_lead"}))
        .await;
    assert_eq!(review_lead["worker"]["name"], REVIEW_LEAD, "{review_lead}");
    let lead_run = &until_runs(&fake, LEAD, 1).await[0];
    assert!(
        lead_run.stdin[1].contains("area lead of area backend"),
        "{}",
        lead_run.stdin[1]
    );
    assert_eq!(lead_run.env["CLAUDE_CODE_PROMPT_CACHE_TTL"], "1h");
    assert!(
        !lead_run.env.contains_key("FOLD_AGENT_TAGS"),
        "a worker never asks for the main tag"
    );
    let review_run = &until_runs(&fake, REVIEW_LEAD, 1).await[0];
    assert_eq!(review_run.env["CLAUDE_CODE_PROMPT_CACHE_TTL"], "1h");
    until_state(&h, LEAD, WorkerState::Idle).await;
    until_state(&h, REVIEW_LEAD, WorkerState::Idle).await;
    h.connect(LEAD).await;
    h.connect(REVIEW_LEAD).await;

    let list_task_worker = start_task_worker(&mut h, "T-1").await;
    let item_task_worker = start_task_worker(&mut h, "T-2").await;
    assert_eq!(
        [list_task_worker.as_str(), item_task_worker.as_str()],
        ["backend-worker", "backend-worker-2"]
    );
    let task_worker_run = &until_runs(&fake, &item_task_worker, 1).await[0];
    assert_eq!(task_worker_run.flag("--model"), Some("sonnet"));
    assert_eq!(task_worker_run.env["CLAUDE_CODE_PROMPT_CACHE_TTL"], "5m");
    assert!(task_worker_run.stdin[1].contains("Keep the endpoint small."));
    let (_, list) = build(
        &h,
        &list_task_worker,
        "T-1",
        "src/list.rs",
        "Add the list endpoint",
    )
    .await;
    let (item_work, item) = build(
        &h,
        &item_task_worker,
        "T-2",
        "src/item.rs",
        "Add the item endpoint",
    )
    .await;
    assert_eq!([list.as_str(), item.as_str()], ["P-1", "P-2"]);

    // The review lead never reviews: it gives the group to a reviewer, which gets the batch from the start.
    let refused = h
        .refused(
            REVIEW_LEAD,
            "create_review_batch",
            json!({"proposals": ["P-1", "P-2"]}),
        )
        .await;
    assert_eq!(refused["code"], codes::REVIEW_LEAD_DELEGATES, "{refused}");
    let batch_reviewer = h
        .call(
            REVIEW_LEAD,
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-1", "P-2"]}),
        )
        .await;
    assert_eq!(
        batch_reviewer["worker"]["name"], "reviewer",
        "{batch_reviewer}"
    );
    until_state(&h, "reviewer", WorkerState::Idle).await;
    h.connect("reviewer").await;
    let checkout = h
        .call("reviewer", "create_batch_checkout", json!({"id": "rb-1"}))
        .await;
    assert_eq!(
        checkout["members"],
        json!([{"proposal": "P-1", "version": "v1"}, {"proposal": "P-2", "version": "v1"}])
    );
    h.call(
        "reviewer",
        "submit_batch_review",
        json!({
            "id": "rb-1",
            "verdicts": [
                {"proposal": "P-1", "verdict": "approve"},
                {"proposal": "P-2", "verdict": "request_changes", "body": "Answer 404 for a missing item."},
            ],
        }),
    )
    .await;

    let resumed = until_runs(&fake, &item_task_worker, 2).await;
    assert!(resumed[1].has("--resume"));
    assert!(resumed[1].stdin[1].contains("Answer 404 for a missing item."));
    until_state(&h, &item_task_worker, WorkerState::Idle).await;
    eventually(WAIT, async || {
        let view = h.call(MAIN, "get_task", json!({"id": "T-1"})).await;
        if view["status"] == "done" {
            Ok(())
        } else {
            Err(view["status"].clone())
        }
    })
    .await;
    // fold stopped the reviewer once each of its proposals had a verdict; it keeps the review claims.
    until_state(&h, "reviewer", WorkerState::Stopped).await;
    item_work.write(&[("src/item.rs", "pub fn handler() -> u16 { 404 }\n")]);
    h.call(
        &item_task_worker,
        "record_changes",
        json!({"ws": item_work.workspace, "message": "Answer 404 for a missing item"}),
    )
    .await;
    let updated = h
        .call(
            &item_task_worker,
            "update_proposal",
            json!({"intent": "Make the change the task asks for.", "id": "P-2"}),
        )
        .await;
    assert_eq!(updated["version"], "v2", "{updated}");
    until_state(&h, &item_task_worker, WorkerState::Stopped).await;
    // The new version goes back to the reviewer that holds its claim, in its own session.
    let reviewer_runs = until_runs(&fake, "reviewer", 2).await;
    assert!(reviewer_runs[1].has("--resume"));
    assert!(
        reviewer_runs[1].stdin[1].contains("P-2 v2"),
        "{}",
        reviewer_runs[1].stdin[1]
    );
    until_state(&h, "reviewer", WorkerState::Idle).await;
    h.call("reviewer", "create_review_checkout", json!({"id": "P-2"}))
        .await;
    h.call(
        "reviewer",
        "submit_review",
        json!({"id": "P-2", "version": 2, "verdict": "approve", "body": "The 404 is right."}),
    )
    .await;

    let notified = eventually(WAIT, async || {
        let runs = fake.runs_of(LEAD);
        let stdin = runs.iter().flat_map(|run| run.stdin.iter());
        match stdin
            .filter(|line| line.contains("T-3 is ready in your area"))
            .count()
        {
            0 => Err(()),
            count => Ok(count),
        }
    })
    .await;
    assert_eq!(notified, 1);
    let search_task_worker = start_task_worker(&mut h, "T-3").await;
    assert_eq!(search_task_worker, "backend-worker-3");
    let (_, search) = build(
        &h,
        &search_task_worker,
        "T-3",
        "src/search.rs",
        "Add the search endpoint",
    )
    .await;
    assert_eq!(search, "P-3");
    let reviewer = h
        .call(
            REVIEW_LEAD,
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-3"]}),
        )
        .await;
    assert_eq!(reviewer["worker"]["name"], "reviewer-2", "{reviewer}");
    let proposal = h
        .call(REVIEW_LEAD, "get_proposal", json!({"id": "P-3"}))
        .await;
    assert_eq!(proposal["claim"]["holder"], "reviewer-2", "{proposal}");
    until_state(&h, "reviewer-2", WorkerState::Idle).await;
    h.connect("reviewer-2").await;
    let refused = h
        .refused("reviewer-2", "claim_review", json!({"id": "P-3"}))
        .await;
    assert_eq!(refused["code"], codes::WORKER_CLAIM_DENIED);
    let checkout = h
        .call("reviewer-2", "create_review_checkout", json!({"id": "P-3"}))
        .await;
    assert_eq!(checkout["version"], "v1", "{checkout}");
    h.call(
        "reviewer-2",
        "submit_review",
        json!({"id": "P-3", "version": 1, "verdict": "approve", "body": "Searches both."}),
    )
    .await;
    until_state(&h, "reviewer-2", WorkerState::Stopped).await;

    eventually(WAIT, async || match committed(&h).await {
        3 => Ok(()),
        count => Err(count),
    })
    .await;
    let subjects = git(
        &h.project.paths().git_dir(),
        &["log", "--format=%s", "-3", "refs/fold/head"],
    );
    assert_eq!(
        subjects,
        "Add the search endpoint\nAdd the item endpoint\nAdd the list endpoint\n"
    );
    for task in ["T-1", "T-2", "T-3"] {
        assert_eq!(h.completions(task).await, 1, "{task} is Done");
    }
    let events = h.events().await;
    let integrated: Vec<Option<String>> = events
        .iter()
        .filter(|event| event.kind == "integration.succeeded")
        .map(|event| event.proposal.clone())
        .collect();
    assert_eq!(
        integrated,
        [Some("P-1".into()), Some("P-2".into()), Some("P-3".into())]
    );
    assert_in_order(
        &events,
        &[
            "area.defined",
            "request.approved",
            "worker.started",
            "review.batch_submitted",
            "worker.resumed",
            "proposal.version_added",
            "integration.succeeded",
        ],
    );

    let workers = h.handle.agents().list_workers().await.unwrap();
    for row in &workers {
        match row.role {
            Role::TaskWorker | Role::Reviewer => {
                assert_eq!(row.state, WorkerState::Stopped, "{}", row.agent);
                assert_eq!(row.exit_code, Some(0), "{}", row.agent);
            }
            Role::AreaLead | Role::ReviewLead | Role::Main => {
                assert!(row.state.is_running(), "{} keeps running", row.agent);
            }
        }
    }
    assert_eq!(
        workers
            .iter()
            .filter(|row| row.role == Role::TaskWorker)
            .count(),
        3
    );
    let started_by: Vec<(String, Value)> = events
        .iter()
        .filter(|event| event.kind == "worker.started" && event.data["resumed"].is_null())
        .map(|event| (event.actor.clone(), event.data["agent"].clone()))
        .collect();
    assert_eq!(
        started_by,
        [
            ("main".to_owned(), json!(LEAD)),
            ("main".to_owned(), json!(REVIEW_LEAD)),
            (LEAD.to_owned(), json!("backend-worker")),
            (LEAD.to_owned(), json!("backend-worker-2")),
            (REVIEW_LEAD.to_owned(), json!("reviewer")),
            (LEAD.to_owned(), json!("backend-worker-3")),
            (REVIEW_LEAD.to_owned(), json!("reviewer-2")),
        ]
    );
    integrate.stop().await;
}
