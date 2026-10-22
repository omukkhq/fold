#![cfg(test)]

use fold_core::{Actor, AgentName, HumanName, LabelName, RequestId, TaskId, codes};
use fold_daemon::services::tasks::TaskPatch;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{FakeInbox, Harness, TOKEN, WAIT, message_notice, wire};
use crate::workers_support::FakeClaude;

const MAIN: &str = "frontend";

fn ana() -> Actor {
    Actor::human(HumanName::parse("ana").unwrap())
}

async fn expect(inbox: &mut FakeInbox, text: &str) {
    assert_eq!(inbox.next().await, wire(TOKEN, text));
}

/// Proves that nothing else reached `agent` before: the next post it receives is the notice of a fresh message.
async fn nothing_before_a_message(h: &Harness, inbox: &mut FakeInbox, agent: &str) {
    let from = if agent == MAIN { "backend" } else { MAIN };
    let message = h.send(from, &[agent], "ping").await;
    expect(inbox, &message_notice(&message, from, "ping")).await;
}

/// `agent` gets the `main` tag from a handshake, as the session that `fld claude` starts does.
async fn make_main(h: &Harness, agent: &str) {
    h.make_main(agent).await;
    let registered = h
        .call(agent, "register_agent", json!({"name": agent}))
        .await;
    assert!(registered.to_string().contains("\"main\""), "{registered}");
}

async fn human_request(h: &Harness, body: &str) -> RequestId {
    h.handle
        .tasks
        .create_request(&ana(), body, Vec::new())
        .await
        .unwrap()
        .id
}

fn card(key: &str, label: Option<&str>) -> Value {
    let mut card = json!({"key": key, "title": format!("Build {key}"), "description": format!("**Scope** the {key} part")});
    if let Some(label) = label {
        card["label"] = json!(label);
    }
    card
}

/// The stdin lines the worker process of `agent` has read so far, once one holds `text`.
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

#[tokio::test]
async fn only_the_main_agent_claims_once_there_is_one_and_card_labels_must_exist() {
    let h = Harness::start_without_leads().await;
    let request = human_request(&h, "The board is slow").await;
    let claim = json!({"id": request.to_string(), "kind": "bug", "title": "Slow board"});

    make_main(&h, MAIN).await;
    let refused = h.refused("backend", "claim_request", claim.clone()).await;
    assert_eq!(refused["code"], codes::ORCHESTRATOR_REQUIRED);
    assert!(
        refused["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("main agent"),
        "{refused}"
    );
    h.call(MAIN, "claim_request", claim).await;

    let unknown = h
        .refused(
            MAIN,
            "submit_request_draft",
            json!({"id": request.to_string(), "cards": [card("c1", Some("server"))]}),
        )
        .await;
    assert_eq!(unknown["code"], codes::LABEL_NOT_FOUND);
    h.call(
        MAIN,
        "create_label",
        json!({"name": "server", "color": "sage"}),
    )
    .await;
    h.call(
        MAIN,
        "submit_request_draft",
        json!({"id": request.to_string(), "cards": [card("c1", Some("server")), card("c2", None)]}),
    )
    .await;
    let read = h
        .call(MAIN, "get_request", json!({"id": request.to_string()}))
        .await;
    assert_eq!(read["draft"]["cards"][0]["label"], "server");
    assert!(read["draft"]["cards"][1].get("label").is_none(), "{read}");

    let approved = h
        .handle
        .tasks
        .approve_request_draft(&ana(), request, 1)
        .await
        .unwrap();
    let labels = h
        .call(
            MAIN,
            "get_task",
            json!({"id": approved.tasks[0].to_string()}),
        )
        .await["labels"]
        .clone();
    assert_eq!(labels, json!(["server"]));
}

#[tokio::test]
async fn an_agents_request_reaches_only_the_main_agent() {
    let h = Harness::start_without_leads().await;
    make_main(&h, MAIN).await;
    let mut main = h.register_inbox(MAIN).await;
    let mut backend = h.register_inbox("backend").await;
    let mut review = h.register_inbox("review").await;

    h.call(
        "review",
        "create_request",
        json!({"body": "The board query skips archived tasks"}),
    )
    .await;
    expect(&mut main, "fold: RQ-1 created. Call claim_request.").await;
    human_request(&h, "Rename the board").await;
    expect(&mut main, "fold: RQ-2 created. Call claim_request.").await;

    nothing_before_a_message(&h, &mut backend, "backend").await;
    nothing_before_a_message(&h, &mut review, "review").await;
}

#[tokio::test]
async fn an_area_lead_hears_of_a_new_request_and_claims_it_when_it_is_relevant() {
    let h = Harness::start_without_leads().await;
    make_main(&h, MAIN).await;
    crate::common::roles::give_role(
        &h.handle.store,
        "backend",
        fold_core::roles::Role::AreaLead,
        None,
    )
    .await;
    let mut main = h.register_inbox(MAIN).await;
    let mut lead = h.register_inbox("backend").await;
    let mut review = h.register_inbox("review").await;

    let request = human_request(&h, "The board query skips archived tasks").await;
    expect(&mut main, "fold: RQ-1 created. Call claim_request.").await;
    expect(&mut lead, "fold: RQ-1 created. Call claim_request.").await;
    nothing_before_a_message(&h, &mut review, "review").await;

    let claim = json!({"id": request.to_string(), "kind": "bug", "title": "Archived tasks"});
    let refused = h.refused("review", "claim_request", claim.clone()).await;
    assert_eq!(
        refused["code"],
        codes::ORCHESTRATOR_REQUIRED,
        "the review lead drafts no request"
    );
    let claimed = h.call("backend", "claim_request", claim).await;
    assert_eq!(claimed["drafter"], "backend", "{claimed}");
}

#[tokio::test]
async fn a_hand_opened_session_neither_claims_a_request_nor_defines_an_area_without_a_main_agent() {
    let h = Harness::start_without_leads().await;
    let request = human_request(&h, "Rename the board").await;
    let claim = json!({"id": request.to_string(), "kind": "feature", "title": "Rename"});
    for agent in ["frontend", "backend"] {
        let refused = h.refused(agent, "claim_request", claim.clone()).await;
        assert_eq!(refused["code"], codes::ORCHESTRATOR_REQUIRED, "{agent}");
    }
    h.call(
        "frontend",
        "create_label",
        json!({"name": "ui", "color": "sage"}),
    )
    .await;
    let refused = h
        .refused(
            "frontend",
            "define_area",
            json!({"name": "web", "label": "ui"}),
        )
        .await;
    assert_eq!(refused["code"], codes::ORCHESTRATOR_REQUIRED);
    assert!(
        refused["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("main agent"),
        "{refused}"
    );
    let read = h
        .call(
            "frontend",
            "get_request",
            json!({"id": request.to_string()}),
        )
        .await;
    assert_eq!(read["state"], "open", "{read}");
}

#[tokio::test]
async fn approved_cards_go_to_the_lead_of_their_area_and_the_rest_to_the_main_agent() {
    let fake = FakeClaude::new("result");
    let h = Harness::start_with_launcher(fake.launcher.clone()).await;
    make_main(&h, MAIN).await;
    let mut main = h.register_inbox(MAIN).await;
    for label in ["ui", "server"] {
        h.call(
            MAIN,
            "create_label",
            json!({"name": label, "color": "sage"}),
        )
        .await;
    }
    for (area, label) in [("web", "ui"), ("api", "server")] {
        h.call(MAIN, "define_area", json!({"name": area, "label": label}))
            .await;
        h.call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": area}),
        )
        .await;
        until_stdin(&fake, &format!("{area}-lead"), "fold started you").await;
    }

    let request = human_request(&h, "Make the page faster").await;
    expect(&mut main, "fold: RQ-1 created. Call claim_request.").await;
    h.call(
        MAIN,
        "claim_request",
        json!({"id": request.to_string(), "kind": "feature", "title": "Faster page"}),
    )
    .await;
    h.call(
        MAIN,
        "submit_request_draft",
        json!({
            "id": request.to_string(),
            "cards": [card("c1", Some("server")), card("c2", None), card("c3", Some("ui"))],
            "edges": [{"prerequisite": "c1", "dependent": "c3"}],
        }),
    )
    .await;
    h.handle
        .tasks
        .approve_request_draft(&ana(), request, 1)
        .await
        .unwrap();

    expect(
        &mut main,
        "fold: T-2 is ready and has no area. Call update_task.",
    )
    .await;
    expect(
        &mut main,
        "fold: RQ-1 approved as T-1, T-2, T-3. Call get_ready_tasks.",
    )
    .await;
    until_stdin(
        &fake,
        "api-lead",
        "fold: T-1 is ready in your area. Call start_worker.",
    )
    .await;

    h.call(
        MAIN,
        "remove_dependency",
        json!({"task": "T-3", "needs": "T-1"}),
    )
    .await;
    let web = until_stdin(
        &fake,
        "web-lead",
        "fold: T-3 is ready in your area. Call start_worker.",
    )
    .await;
    assert!(
        !web.iter()
            .any(|line| line.contains("T-1 is ready") || line.contains("T-2 is ready")),
        "{web:?}"
    );
    let ping = h.send(MAIN, &["api-lead"], "ping").await;
    let api = until_stdin(&fake, "api-lead", &format!("fold: message {ping} arrived.")).await;
    assert!(
        !api.iter().any(|line| line.contains("T-3 is ready")),
        "{api:?}"
    );
    nothing_before_a_message(&h, &mut main, MAIN).await;
}

#[tokio::test]
async fn an_area_label_added_to_a_ready_task_offers_it_to_the_lead_of_that_area() {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    make_main(&h, MAIN).await;
    let mut main = h.register_inbox(MAIN).await;
    h.call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    h.call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    h.call(
        MAIN,
        "start_worker",
        json!({"role": "area_lead", "area": "web"}),
    )
    .await;
    until_stdin(&fake, "web-lead", "fold started you").await;
    h.connect("web-lead").await;

    h.call(MAIN, "create_task", json!({"title": "Polish the board"}))
        .await;
    expect(
        &mut main,
        "fold: T-1 is ready and has no area. Call update_task.",
    )
    .await;
    h.call(
        MAIN,
        "update_task",
        json!({"id": "T-1", "title": "Polish the board header"}),
    )
    .await;
    h.call(MAIN, "update_task", json!({"id": "T-1", "labels": ["ui"]}))
        .await;
    let lines = until_stdin(
        &fake,
        "web-lead",
        "fold: T-1 is ready in your area. Call start_worker.",
    )
    .await;
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("T-1 is ready"))
            .count(),
        1,
        "{lines:?}"
    );
    let started = h
        .call(
            "web-lead",
            "start_worker",
            json!({"role": "task_worker", "tasks": ["T-1"]}),
        )
        .await;
    assert!(started["worker"]["name"].is_string(), "{started}");
}

#[tokio::test]
async fn a_label_added_to_a_claimed_task_offers_it_to_nobody() {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    make_main(&h, MAIN).await;
    let mut main = h.register_inbox(MAIN).await;
    h.call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    h.call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    h.call(
        MAIN,
        "start_worker",
        json!({"role": "area_lead", "area": "web"}),
    )
    .await;
    until_stdin(&fake, "web-lead", "fold started you").await;
    h.connect("web-lead").await;

    h.call(MAIN, "create_task", json!({"title": "Polish the board"}))
        .await;
    expect(
        &mut main,
        "fold: T-1 is ready and has no area. Call update_task.",
    )
    .await;
    h.call(MAIN, "claim_task", json!({"id": "T-1"})).await;
    h.call(MAIN, "update_task", json!({"id": "T-1", "labels": ["ui"]}))
        .await;
    let message = h.send(MAIN, &["web-lead"], "ping").await;
    let lines = until_stdin(
        &fake,
        "web-lead",
        &format!("fold: message {message} arrived."),
    )
    .await;
    assert!(
        !lines.iter().any(|line| line.contains("T-1 is ready")),
        "{lines:?}"
    );
}

#[tokio::test]
async fn a_human_who_labels_a_ready_task_into_an_area_without_a_lead_tells_the_main_agent() {
    let h = Harness::start_without_leads().await;
    make_main(&h, MAIN).await;
    let mut main = h.register_inbox(MAIN).await;
    for label in ["server", "chore"] {
        h.call(
            MAIN,
            "create_label",
            json!({"name": label, "color": "sage"}),
        )
        .await;
    }
    h.call(
        MAIN,
        "define_area",
        json!({"name": "api", "label": "server"}),
    )
    .await;
    h.call(
        MAIN,
        "create_task",
        json!({"title": "Speed up the board query"}),
    )
    .await;
    expect(
        &mut main,
        "fold: T-1 is ready and has no area. Call update_task.",
    )
    .await;

    let task = TaskId::from_number(1).unwrap();
    let relabel = async |labels: &[&str]| {
        h.handle
            .tasks
            .update_task(
                &ana(),
                task,
                TaskPatch {
                    labels: Some(
                        labels
                            .iter()
                            .map(|label| LabelName::parse(label).unwrap())
                            .collect(),
                    ),
                    ..TaskPatch::default()
                },
            )
            .await
            .unwrap();
    };
    relabel(&["chore"]).await;
    relabel(&["chore", "server"]).await;
    expect(
        &mut main,
        "fold: T-1 is ready in area api, which has no lead. Call start_worker.",
    )
    .await;
    nothing_before_a_message(&h, &mut main, MAIN).await;
}

#[tokio::test]
async fn register_agent_never_gives_the_main_tag_and_only_a_handshake_does() {
    let h = Harness::start_without_roles().await;
    let refused = h
        .refused(
            "backend",
            "register_agent",
            json!({"name": "backend", "tags": ["main"]}),
        )
        .await;
    assert_eq!(refused["code"], codes::MAIN_TAG_DENIED, "{refused}");
    assert!(
        refused["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("fld claude"),
        "{refused}"
    );
    let agents = h.call("backend", "list_agents", json!({})).await;
    assert!(!agents.to_string().contains("\"main\""), "{agents}");

    h.make_main(MAIN).await;
    let repeated = h
        .call(
            MAIN,
            "register_agent",
            json!({"name": MAIN, "tags": ["main"]}),
        )
        .await;
    assert!(repeated.to_string().contains("\"main\""), "{repeated}");
}

#[tokio::test]
async fn one_agent_that_fold_did_not_start_holds_the_main_tag() {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    make_main(&h, MAIN).await;
    make_main(&h, MAIN).await;
    let taken = h
        .refused(
            "backend",
            "register_agent",
            json!({"name": "backend", "tags": ["main"]}),
        )
        .await;
    assert_eq!(taken["code"], codes::MAIN_TAG_DENIED);
    assert_eq!(
        taken["message"],
        "only the session that fld claude starts takes the main tag"
    );
    assert!(
        taken["hint"]
            .as_str()
            .unwrap_or_default()
            .contains("register_agent"),
        "{taken}"
    );

    h.call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    h.call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    h.call(
        MAIN,
        "start_worker",
        json!({"role": "area_lead", "area": "web"}),
    )
    .await;
    h.connect("web-lead").await;
    let worker = h
        .refused(
            "web-lead",
            "register_agent",
            json!({"name": "web-lead", "tags": ["main"]}),
        )
        .await;
    assert_eq!(worker["code"], codes::MAIN_TAG_DENIED);
    let read = h
        .call("backend", "register_agent", json!({"name": "backend"}))
        .await;
    assert!(!read.to_string().contains("\"main\""), "{read}");
}

#[tokio::test]
async fn the_lead_decides_on_the_request_of_its_stopped_task_worker() {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    make_main(&h, MAIN).await;
    h.call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    h.call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    h.call(
        MAIN,
        "create_task",
        json!({"title": "Draw the page", "labels": ["ui"]}),
    )
    .await;
    h.call(
        MAIN,
        "start_worker",
        json!({"role": "area_lead", "area": "web"}),
    )
    .await;
    h.connect("web-lead").await;
    h.call(
        "web-lead",
        "start_worker",
        json!({"role": "task_worker", "tasks": ["T-1"]}),
    )
    .await;
    h.connect("web-worker").await;
    h.call(
        "web-worker",
        "create_request",
        json!({"body": "The page header overlaps the menu"}),
    )
    .await;
    h.call("web-lead", "stop_worker", json!({"name": "web-worker"}))
        .await;
    let task_worker = AgentName::parse("web-worker").unwrap();
    eventually(WAIT, async || {
        let state = h
            .handle
            .agents()
            .get_worker(&task_worker)
            .await
            .unwrap()
            .state;
        if state.is_running() {
            Err(state)
        } else {
            Ok(state)
        }
    })
    .await;

    h.call(
        MAIN,
        "claim_request",
        json!({"id": "RQ-1", "kind": "bug", "title": "Header overlap"}),
    )
    .await;
    h.call(
        MAIN,
        "submit_request_draft",
        json!({"id": "RQ-1", "cards": [card("c1", Some("ui"))]}),
    )
    .await;
    until_stdin(
        &fake,
        "web-lead",
        "fold: RQ-1 draft v1 ready. Call get_request.",
    )
    .await;

    let refused = h
        .refused(
            "backend",
            "approve_request_draft",
            json!({"id": "RQ-1", "expected_version": 1}),
        )
        .await;
    assert_eq!(refused["code"], codes::REQUESTER_REQUIRED);
    let approved = h
        .call(
            "web-lead",
            "approve_request_draft",
            json!({"id": "RQ-1", "expected_version": 1}),
        )
        .await;
    assert!(approved.to_string().contains("T-2"), "{approved}");
}
