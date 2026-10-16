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

/// The lead starts a task coder for `task`; the coder connects once its first turn has ended.
async fn start_coder(h: &mut Harness, task: &str) -> String {
    let started = h
        .call(
            LEAD,
            "start_worker",
            json!({"role": "task_coder", "tasks": [task], "model": "sonnet", "instructions": "Keep the endpoint small."}),
        )
        .await;
    let coder = started["worker"]["name"].as_str().unwrap().to_owned();
    until_state(h, &coder, WorkerState::Idle).await;
    h.connect(&coder).await;
    coder
}

/// The coder builds `task` in its workspace and proposes it under `title`; fold then stops it.
async fn build(h: &Harness, coder: &str, task: &str, file: &str, title: &str) -> (Work, String) {
    let work = h.start_work(coder, task).await;
    work.write(&[(file, "pub fn handler() {}\n")]);
    let proposal = h.propose(coder, &work, title).await;
    until_state(h, coder, WorkerState::Stopped).await;
    (work, proposal)
}

/// Acceptance 11, the team: the main agent that `fld claude` starts defines an area, turns a human's request into three cards and starts the leads; the area lead starts coders, the review lead reviews a batch, fold resumes the coder whose proposal needs changes, integrates every approved proposal in dependency order with one Git commit each, sends the dependent task to the lead once it is ready, and stops every short-lived worker on its own.
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
            json!({"role": "area_lead", "area": "backend", "instructions": "Start a coder for each ready task."}),
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

    let list_coder = start_coder(&mut h, "T-1").await;
    let item_coder = start_coder(&mut h, "T-2").await;
    assert_eq!(
        [list_coder.as_str(), item_coder.as_str()],
        ["backend-coder", "backend-coder-2"]
    );
    let coder_run = &until_runs(&fake, &item_coder, 1).await[0];
    assert_eq!(coder_run.flag("--model"), Some("sonnet"));
    assert_eq!(coder_run.env["CLAUDE_CODE_PROMPT_CACHE_TTL"], "5m");
    assert!(coder_run.stdin[1].contains("Keep the endpoint small."));
    let (_, list) = build(
        &h,
        &list_coder,
        "T-1",
        "src/list.rs",
        "Add the list endpoint",
    )
    .await;
    let (item_work, item) = build(
        &h,
        &item_coder,
        "T-2",
        "src/item.rs",
        "Add the item endpoint",
    )
    .await;
    assert_eq!([list.as_str(), item.as_str()], ["P-1", "P-2"]);

    let batch = h
        .call(
            REVIEW_LEAD,
            "create_review_batch",
            json!({"proposals": ["P-1", "P-2"]}),
        )
        .await;
    assert_eq!(batch["id"], "rb-1", "{batch}");
    let checkout = h
        .call(REVIEW_LEAD, "create_batch_checkout", json!({"id": "rb-1"}))
        .await;
    assert_eq!(
        checkout["members"],
        json!([{"proposal": "P-1", "version": "v1"}, {"proposal": "P-2", "version": "v1"}])
    );
    h.call(
        REVIEW_LEAD,
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

    let resumed = until_runs(&fake, &item_coder, 2).await;
    assert!(resumed[1].has("--resume"));
    assert!(resumed[1].stdin[1].contains("Answer 404 for a missing item."));
    until_state(&h, &item_coder, WorkerState::Idle).await;
    eventually(WAIT, async || {
        let view = h.call(MAIN, "get_task", json!({"id": "T-1"})).await;
        if view["status"] == "done" {
            Ok(())
        } else {
            Err(view["status"].clone())
        }
    })
    .await;
    item_work.write(&[("src/item.rs", "pub fn handler() -> u16 { 404 }\n")]);
    h.call(
        &item_coder,
        "record_changes",
        json!({"ws": item_work.workspace, "message": "Answer 404 for a missing item"}),
    )
    .await;
    let updated = h
        .call(&item_coder, "update_proposal", json!({"id": "P-2"}))
        .await;
    assert_eq!(updated["version"], "v2", "{updated}");
    until_state(&h, &item_coder, WorkerState::Stopped).await;
    h.call(
        REVIEW_LEAD,
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
    let search_coder = start_coder(&mut h, "T-3").await;
    assert_eq!(search_coder, "backend-coder-3");
    let (_, search) = build(
        &h,
        &search_coder,
        "T-3",
        "src/search.rs",
        "Add the search endpoint",
    )
    .await;
    assert_eq!(search, "P-3");
    h.call(
        REVIEW_LEAD,
        "submit_review",
        json!({"id": "P-3", "version": 1, "verdict": "approve", "body": "Searches both."}),
    )
    .await;

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
            Role::TaskCoder | Role::BatchReviewer => {
                assert_eq!(row.state, WorkerState::Stopped, "{}", row.agent);
                assert_eq!(row.exit_code, Some(0), "{}", row.agent);
            }
            Role::AreaLead | Role::ReviewLead => {
                assert!(row.state.is_running(), "{} keeps running", row.agent);
            }
        }
    }
    assert_eq!(
        workers
            .iter()
            .filter(|row| row.role == Role::TaskCoder)
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
            (LEAD.to_owned(), json!("backend-coder")),
            (LEAD.to_owned(), json!("backend-coder-2")),
            (LEAD.to_owned(), json!("backend-coder-3")),
        ]
    );
    integrate.stop().await;
}
