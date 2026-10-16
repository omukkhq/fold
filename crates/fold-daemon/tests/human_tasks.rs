#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_core::{AgentName, WorkerState, codes};
use fold_store::agents::WorkerProcessRow;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, SLUG, WAIT};
use crate::workers_support::FakeClaude;

const MAIN: &str = "frontend";
const LEAD: &str = "web-lead";
const CODER: &str = "web-coder";
/// An agent that fold does not supervise and that lacks the `main` tag.
const SOLO: &str = "solo";

async fn until_state(harness: &Harness, agent: &str, state: WorkerState) -> WorkerProcessRow {
    eventually(WAIT, async || {
        let row = harness
            .handle
            .agents()
            .get_worker(&AgentName::parse(agent).unwrap())
            .await
            .unwrap();
        if row.state == state {
            Ok(row)
        } else {
            Err(row.state)
        }
    })
    .await
}

/// Every stdin line of `agent`'s runs once one holds `text`.
async fn until_stdin(fake: &FakeClaude, agent: &str, text: &str) -> Vec<String> {
    eventually(WAIT, async || {
        let lines: Vec<String> = fake
            .runs_of(agent)
            .into_iter()
            .flat_map(|run| run.stdin)
            .collect();
        if lines.iter().any(|line| line.contains(text)) {
            Ok(lines)
        } else {
            Err(lines.len())
        }
    })
    .await
}

async fn task(harness: &Harness, id: &str) -> Value {
    harness.call(MAIN, "get_task", json!({"id": id})).await
}

async fn ready(harness: &Harness) -> Vec<String> {
    harness.call(LEAD, "get_ready_tasks", json!({})).await["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["id"].as_str().unwrap().to_owned())
        .collect()
}

async fn http(
    harness: &Harness,
    method: Method,
    path: &str,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    harness
        .http(
            method,
            &format!("/api/v1/projects/{SLUG}/{path}"),
            body,
            HUMAN,
        )
        .await
}

/// Area `web` over label `ui`; T-1 "Verify the app on Meta", a human task; T-2 "Wire the Meta login", which needs T-1; T-3, a task with a proposal; the area lead `web-lead`, and the unsupervised agent `solo`, both connected.
async fn planned(fake: &FakeClaude) -> Harness {
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(MAIN).await;
    harness
        .call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    harness
        .call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    let human = harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Verify the app on Meta", "labels": ["ui"], "kind": "human"}),
        )
        .await;
    assert_eq!(human, json!({"id": "T-1", "status": "todo"}));
    let waiting = harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Wire the Meta login", "labels": ["ui"], "needs": ["T-1"]}),
        )
        .await;
    assert_eq!(waiting, json!({"id": "T-2", "status": "backlog"}));
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Build the settings screen", "labels": ["ui"]}),
        )
        .await;
    harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    harness.connect(LEAD).await;
    harness.connect(SOLO).await;
    harness
}

#[tokio::test]
async fn a_human_task_blocks_its_dependents_until_a_human_marks_it_done_and_no_agent_claims_it() {
    let fake = FakeClaude::new("result");
    let harness = planned(&fake).await;
    let human = task(&harness, "T-1").await;
    assert_eq!(human["kind"], "human");
    assert_eq!(human["status"], "todo");
    assert_eq!(
        human["status_reason"],
        "Calculated: every prerequisite is Done and a person marks the task Done"
    );
    assert_eq!(ready(&harness).await, ["T-3"]);
    let graph = harness.call(LEAD, "get_task_graph", json!({})).await;
    assert_eq!(graph["tasks"][0]["id"], "T-1");
    assert_eq!(graph["tasks"][0]["kind"], "human");
    assert!(graph["tasks"][0].get("blocker").is_none(), "{graph}");

    for (agent, tool, arguments) in [
        (SOLO, "claim_task", json!({"id": "T-1"})),
        (SOLO, "takeover_claim", json!({"id": "T-1"})),
        (SOLO, "create_workspace", json!({"task": "T-1"})),
        (
            SOLO,
            "create_proposal",
            json!({"task": "T-1", "title": "Verify it"}),
        ),
        (SOLO, "report_task", json!({"id": "T-1", "note": "Done."})),
        (
            LEAD,
            "start_worker",
            json!({"role": "task_coder", "tasks": ["T-1"]}),
        ),
    ] {
        let refused = harness.refused(agent, tool, arguments).await;
        assert_eq!(
            refused["code"],
            codes::TASK_NEEDS_HUMAN,
            "{tool}: {refused}"
        );
        assert!(
            refused["hint"]
                .as_str()
                .unwrap()
                .contains("get_ready_tasks"),
            "{tool}: {refused}"
        );
    }
    assert_eq!(
        harness
            .refused(LEAD, "send_back_task", json!({"id": "T-1", "note": "No."}))
            .await["code"],
        codes::TASK_KIND_MISMATCH
    );
    assert_eq!(task(&harness, "T-1").await["claim"], Value::Null);
    assert_eq!(task(&harness, "T-2").await["status"], "backlog");

    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Verify the app on Google", "labels": ["ui"], "kind": "human"}),
        )
        .await;
    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Build the profile screen", "labels": ["ui"]}),
        )
        .await;
    let lines = until_stdin(&fake, LEAD, "fold: T-5 is ready in your area").await;
    assert!(
        !lines.iter().any(|line| line.contains("T-4 is ready")),
        "a human task is offered to no lead: {lines:?}"
    );

    let (status, done) = http(&harness, Method::POST, "tasks/T-1/complete", None).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["status"], "done");
    assert_eq!(harness.completions("T-1").await, 1);
    assert_eq!(task(&harness, "T-2").await["status"], "todo");
    let completion = harness
        .events()
        .await
        .into_iter()
        .find(|event| event.kind == "task.completed")
        .unwrap();
    assert_eq!(completion.data, json!({"kind": "human"}));
    assert_eq!(completion.actor, "human:ana");
    until_stdin(&fake, LEAD, "fold: T-2 is ready in your area").await;
    let (status, again) = http(&harness, Method::POST, "tasks/T-1/complete", None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(again["error"]["code"], codes::TASK_CLOSED);
}

#[tokio::test]
async fn a_task_with_a_blocker_is_not_ready_and_is_neither_claimed_nor_given_to_a_coder_until_cleared()
 {
    let fake = FakeClaude::new("result");
    let harness = planned(&fake).await;
    assert_eq!(ready(&harness).await, ["T-3"]);
    let set = harness
        .call(
            LEAD,
            "set_blocker",
            json!({"id": "T-3", "reason": "  Wait until the design is signed off  "}),
        )
        .await;
    assert_eq!(set, json!({"id": "T-3", "blocker": true}));
    let held = task(&harness, "T-3").await;
    assert_eq!(held["status"], "todo");
    assert_eq!(
        held["blocker"]["reason"],
        "Wait until the design is signed off"
    );
    assert_eq!(held["blocker"]["set_by"], LEAD);
    assert!(ready(&harness).await.is_empty());
    let graph = harness.call(LEAD, "get_task_graph", json!({})).await;
    assert_eq!(
        graph["tasks"][2],
        json!({
            "id": "T-3",
            "title": "Build the settings screen",
            "status": "todo",
            "blocked": false,
            "kind": "proposal",
            "labels": ["ui"],
            "needs": [],
            "holder": null,
            "blocker": {"reason": "Wait until the design is signed off"},
        })
    );

    for (agent, tool, arguments) in [
        (SOLO, "claim_task", json!({"id": "T-3"})),
        (SOLO, "takeover_claim", json!({"id": "T-3"})),
        (SOLO, "create_workspace", json!({"task": "T-3"})),
        (
            LEAD,
            "start_worker",
            json!({"role": "task_coder", "tasks": ["T-3"]}),
        ),
    ] {
        let refused = harness.refused(agent, tool, arguments).await;
        assert_eq!(refused["code"], codes::TASK_ON_HOLD, "{tool}: {refused}");
        assert!(
            refused["hint"].as_str().unwrap().contains("clear_blocker"),
            "{tool}: {refused}"
        );
        assert_eq!(refused["details"]["set_by"], LEAD, "{tool}");
    }
    let (_, board) = http(&harness, Method::GET, "board", None).await;
    let card = board["columns"][1]["cards"]
        .as_array()
        .unwrap()
        .iter()
        .find(|card| card["task"] == "T-3")
        .unwrap()
        .clone();
    assert_eq!(
        card["blocker"]["reason"],
        "Wait until the design is signed off"
    );
    assert_eq!(card["blocker"]["set_by"], LEAD);

    let (status, cleared) = http(&harness, Method::DELETE, "tasks/T-3/blocker", None).await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert_eq!(cleared["blocker"], Value::Null);
    assert_eq!(cleared["can_set_blocker"], true);
    until_stdin(&fake, LEAD, "fold: T-3 is ready in your area").await;
    assert_eq!(ready(&harness).await, ["T-3"]);
    let started = harness
        .call(
            LEAD,
            "start_worker",
            json!({"role": "task_coder", "tasks": ["T-3"]}),
        )
        .await;
    assert_eq!(started["worker"]["name"], CODER);
    let kinds: Vec<String> = harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind.starts_with("task.hold_"))
        .map(|event| format!("{} {}", event.kind, event.data["reason"]))
        .collect();
    assert_eq!(
        kinds,
        [
            "task.hold_set \"Wait until the design is signed off\"",
            "task.hold_cleared \"Wait until the design is signed off\"",
        ]
    );
}

#[tokio::test]
async fn only_a_human_or_the_main_agent_clears_a_blocker_and_completes_a_human_task() {
    let fake = FakeClaude::new("result");
    let mut harness = planned(&fake).await;
    harness
        .call(
            LEAD,
            "start_worker",
            json!({"role": "task_coder", "tasks": ["T-3"]}),
        )
        .await;
    until_state(&harness, CODER, WorkerState::Idle).await;
    harness.connect(CODER).await;
    harness
        .call(CODER, "claim_task", json!({"id": "T-3"}))
        .await;
    let claimed = harness
        .refused(
            LEAD,
            "set_blocker",
            json!({"id": "T-3", "reason": "Wait for the copy"}),
        )
        .await;
    assert_eq!(claimed["code"], codes::TASK_HOLD_DENIED, "{claimed}");
    assert_eq!(claimed["details"]["reason"], "started");
    assert!(claimed["hint"].as_str().unwrap().contains("get_task"));
    harness
        .call(CODER, "create_workspace", json!({"task": "T-3"}))
        .await;
    harness
        .call(CODER, "release_task", json!({"id": "T-3"}))
        .await;
    let (status, with_workspace) = http(
        &harness,
        Method::PUT,
        "tasks/T-3/blocker",
        Some(&json!({"reason": "Wait for the copy"})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{with_workspace}");
    assert_eq!(with_workspace["error"]["code"], codes::TASK_HOLD_DENIED);
    assert_eq!(with_workspace["error"]["details"]["reason"], "started");
    let (_, view) = http(&harness, Method::GET, "tasks/T-3", None).await;
    assert_eq!(view["can_set_blocker"], false, "{view}");
    assert_eq!(view["blocker"], Value::Null);

    let by_coder = harness
        .refused(
            CODER,
            "set_blocker",
            json!({"id": "T-2", "reason": "Wait."}),
        )
        .await;
    assert_eq!(by_coder["code"], codes::TASK_HOLD_DENIED);
    let by_solo = harness
        .refused(SOLO, "set_blocker", json!({"id": "T-2", "reason": "Wait."}))
        .await;
    assert_eq!(by_solo["code"], codes::ORCHESTRATOR_REQUIRED);
    let blank = harness
        .refused(MAIN, "set_blocker", json!({"id": "T-2", "reason": " "}))
        .await;
    assert_eq!(blank["code"], codes::INVALID_HOLD_REASON);
    harness
        .call(
            MAIN,
            "set_blocker",
            json!({"id": "T-2", "reason": "Wait for the app id"}),
        )
        .await;
    let set_at = task(&harness, "T-2").await["blocker"]["set_at"].clone();
    harness
        .project
        .clock()
        .advance(std::time::Duration::from_secs(1))
        .unwrap();
    harness
        .call(
            LEAD,
            "set_blocker",
            json!({"id": "T-2", "reason": "Wait for the app id"}),
        )
        .await;
    let same = task(&harness, "T-2").await;
    assert_eq!(
        same["blocker"]["set_by"], MAIN,
        "the same reason changes nothing"
    );
    assert_eq!(same["blocker"]["set_at"], set_at);

    for (agent, code) in [
        (LEAD, codes::MAIN_AGENT_REQUIRED),
        (CODER, codes::MAIN_AGENT_REQUIRED),
        (SOLO, codes::ORCHESTRATOR_REQUIRED),
    ] {
        let clear = harness
            .refused(agent, "clear_blocker", json!({"id": "T-2"}))
            .await;
        assert_eq!(clear["code"], code, "{agent} clears");
        let complete = harness
            .refused(agent, "complete_task", json!({"id": "T-1"}))
            .await;
        assert_eq!(complete["code"], code, "{agent} completes");
    }
    assert_eq!(
        task(&harness, "T-2").await["blocker"]["reason"],
        "Wait for the app id"
    );
    assert_eq!(task(&harness, "T-1").await["status"], "todo");

    let cleared = harness
        .call(MAIN, "clear_blocker", json!({"id": "T-2"}))
        .await;
    assert_eq!(cleared, json!({"id": "T-2", "blocker": false}));
    let again = harness
        .call(MAIN, "clear_blocker", json!({"id": "T-2"}))
        .await;
    assert_eq!(again, json!({"id": "T-2", "blocker": false}));

    harness
        .call(
            MAIN,
            "create_task",
            json!({"title": "Pick the provider", "labels": ["ui"]}),
        )
        .await;
    harness
        .call(
            MAIN,
            "add_dependency",
            json!({"task": "T-1", "needs": "T-4"}),
        )
        .await;
    let early = harness
        .refused(MAIN, "complete_task", json!({"id": "T-1"}))
        .await;
    assert_eq!(early["code"], codes::PREREQUISITES_NOT_DONE);
    harness
        .call(
            MAIN,
            "remove_dependency",
            json!({"task": "T-1", "needs": "T-4"}),
        )
        .await;
    let completed = harness
        .call(MAIN, "complete_task", json!({"id": "T-1"}))
        .await;
    assert_eq!(completed, json!({"id": "T-1", "status": "done"}));
    let closed = harness
        .refused(MAIN, "set_blocker", json!({"id": "T-1", "reason": "Late."}))
        .await;
    assert_eq!(closed["code"], codes::TASK_CLOSED);
}

#[tokio::test]
async fn a_human_task_is_created_by_whoever_plans_the_work_and_the_web_sets_its_blocker() {
    let fake = FakeClaude::new("result");
    let mut harness = planned(&fake).await;
    harness
        .call(
            LEAD,
            "start_worker",
            json!({"role": "task_coder", "tasks": ["T-3"]}),
        )
        .await;
    until_state(&harness, CODER, WorkerState::Idle).await;
    harness.connect(CODER).await;
    let by_coder = harness
        .refused(
            CODER,
            "create_task",
            json!({"title": "Call the bank", "labels": ["ui"], "kind": "human"}),
        )
        .await;
    assert_eq!(by_coder["code"], codes::TASK_KIND_DENIED);
    let by_lead = harness
        .call(
            LEAD,
            "create_task",
            json!({"title": "Call the bank", "labels": ["ui"], "kind": "human"}),
        )
        .await;
    assert_eq!(by_lead, json!({"id": "T-4", "status": "todo"}));

    let (status, made) = http(
        &harness,
        Method::POST,
        "tasks",
        Some(&json!({"title": "Buy the domain", "kind": "human"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["kind"], "human");
    assert_eq!(made["blocker"], Value::Null);
    let created = harness
        .events()
        .await
        .into_iter()
        .find(|event| event.kind == "task.created" && event.task.as_deref() == Some("T-5"))
        .unwrap();
    assert_eq!(created.data["kind"], "human");

    let (status, blank) = http(
        &harness,
        Method::PUT,
        "tasks/T-5/blocker",
        Some(&json!({"reason": ""})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{blank}");
    assert_eq!(blank["error"]["code"], codes::INVALID_HOLD_REASON);
    let (status, held) = http(
        &harness,
        Method::PUT,
        "tasks/T-2/blocker",
        Some(&json!({"reason": "Wait for the legal review"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{held}");
    assert_eq!(held["blocker"]["reason"], "Wait for the legal review");
    assert_eq!(held["blocker"]["set_by"], "human:ana");
    let (_, board) = http(&harness, Method::GET, "board", None).await;
    let cards: Vec<Value> = board["columns"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|column| column["cards"].as_array().unwrap().clone())
        .collect();
    let card = |id: &str| {
        cards
            .iter()
            .find(|card| card["task"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(card("T-5")["kind"], "human");
    assert_eq!(
        card("T-2")["blocker"]["reason"],
        "Wait for the legal review"
    );
    assert_eq!(card("T-3")["blocker"], Value::Null);
}

#[tokio::test]
async fn needs_you_lists_each_ready_human_task_and_each_blocker_with_its_reason() {
    let fake = FakeClaude::new("result");
    let harness = planned(&fake).await;
    harness
        .call(
            MAIN,
            "set_blocker",
            json!({"id": "T-2", "reason": "Wait for the app id"}),
        )
        .await;
    let (status, list) = http(&harness, Method::GET, "needs-you", None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let rows: Vec<(String, String, String, Value, Value)> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["kind"].as_str().unwrap().to_owned(),
                item["screen"].as_str().unwrap().to_owned(),
                item["task"].as_str().unwrap().to_owned(),
                item["title"].clone(),
                item["reason"].clone(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (
                "human_task".to_owned(),
                "task".to_owned(),
                "T-1".to_owned(),
                json!("Verify the app on Meta"),
                Value::Null
            ),
            (
                "blocker".to_owned(),
                "task".to_owned(),
                "T-2".to_owned(),
                json!("Wire the Meta login"),
                json!("Wait for the app id")
            ),
        ]
    );

    harness
        .call(MAIN, "clear_blocker", json!({"id": "T-2"}))
        .await;
    let (status, done) = http(&harness, Method::POST, "tasks/T-1/complete", None).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let (_, list) = http(&harness, Method::GET, "needs-you", None).await;
    assert_eq!(list["items"], json!([]), "{list}");
}
