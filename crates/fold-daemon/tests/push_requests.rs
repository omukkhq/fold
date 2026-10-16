use fold_core::{Actor, HumanName};
use serde_json::{Value, json};

use crate::common::acceptance::{FakeInbox, Harness, TOKEN, message_notice, wire};

#[cfg(test)]
async fn expect(inbox: &mut FakeInbox, text: &str) {
    assert_eq!(inbox.next().await, wire(TOKEN, text));
}

/// Proves that nothing else reached `agent` before: the next post it receives is the notice of a fresh message.
#[cfg(test)]
async fn nothing_before_a_message(h: &Harness, inbox: &mut FakeInbox, agent: &str) {
    let from = if agent == "review" {
        "frontend"
    } else {
        "review"
    };
    let message = h.send(from, &[agent], "ping").await;
    expect(inbox, &message_notice(&message)).await;
}

/// Makes `agent` the lead of an area, as if the main agent had started it as one.
#[cfg(test)]
async fn lead_an_area(h: &Harness, agent: &str) {
    crate::common::roles::give_role(
        &h.handle.store,
        agent,
        fold_core::roles::Role::AreaLead,
        None,
    )
    .await;
}

#[cfg(test)]
fn cards() -> Value {
    json!([
        {"key": "c1", "title": "Index the board query", "description": "Add the index."},
        {"key": "c2", "title": "Show the faster board", "description": "Use it."},
    ])
}

#[cfg(test)]
fn draft(id: &str) -> Value {
    json!({"id": id, "cards": cards(), "edges": [{"prerequisite": "c1", "dependent": "c2"}]})
}

#[tokio::test]
async fn each_request_step_pushes_the_agent_that_acts_next() {
    let h = Harness::start().await;
    lead_an_area(&h, "backend").await;
    let mut frontend = h.register_inbox("frontend").await;
    let mut backend = h.register_inbox("backend").await;
    let mut review = h.register_inbox("review").await;

    h.call(
        "frontend",
        "create_request",
        json!({"body": "The board is slow"}),
    )
    .await;
    // The area lead `backend` hears of it; the review lead `review` drafts no request.
    let created = "fold: RQ-1 created. Call claim_request.";
    expect(&mut backend, created).await;

    h.call(
        "backend",
        "claim_request",
        json!({"id": "RQ-1", "kind": "bug", "title": "Slow board"}),
    )
    .await;
    let asked = h
        .send_with(
            "backend",
            json!({"to": ["frontend"], "body": "Which board?", "request": "RQ-1"}),
        )
        .await;
    expect(&mut frontend, &message_notice(&asked)).await;

    h.call("backend", "submit_request_draft", draft("RQ-1"))
        .await;
    expect(
        &mut frontend,
        "fold: RQ-1 draft v1 ready. Call get_request.",
    )
    .await;

    h.call(
        "frontend",
        "request_draft_changes",
        json!({"id": "RQ-1", "version": 1, "comment": "Split c1."}),
    )
    .await;
    expect(
        &mut backend,
        "fold: RQ-1 changes requested. Call get_request.",
    )
    .await;

    h.call("frontend", "edit_request_draft", draft("RQ-1"))
        .await;
    h.call(
        "frontend",
        "approve_request_draft",
        json!({"id": "RQ-1", "expected_version": 2}),
    )
    .await;
    expect(
        &mut backend,
        "fold: RQ-1 approved as T-1, T-2. Call get_ready_tasks.",
    )
    .await;

    nothing_before_a_message(&h, &mut frontend, "frontend").await;
    nothing_before_a_message(&h, &mut review, "review").await;
}

#[tokio::test]
async fn a_human_request_reaches_the_main_agent_and_every_area_lead_and_own_steps_push_nothing() {
    let h = Harness::start().await;
    h.make_main("frontend").await;
    lead_an_area(&h, "backend").await;
    let mut frontend = h.register_inbox("frontend").await;
    let mut backend = h.register_inbox("backend").await;
    let mut review = h.register_inbox("review").await;
    let ana = Actor::human(HumanName::parse("ana").unwrap());

    h.handle
        .tasks
        .create_request(&ana, "Why two boards?", Vec::new())
        .await
        .unwrap();
    let created = "fold: RQ-1 created. Call claim_request.";
    expect(&mut frontend, created).await;
    expect(&mut backend, created).await;

    h.call(
        "frontend",
        "claim_request",
        json!({"id": "RQ-1", "kind": "question", "title": "Two boards"}),
    )
    .await;
    h.call("frontend", "submit_request_draft", draft("RQ-1"))
        .await;
    h.handle
        .tasks
        .approve_request_draft(&ana, "RQ-1".parse().unwrap(), 1)
        .await
        .unwrap();
    // As the main agent, `frontend` first hears that the card without an area is ready.
    expect(
        &mut frontend,
        "fold: T-1 is ready and has no area. Call update_task.",
    )
    .await;
    expect(
        &mut frontend,
        "fold: RQ-1 approved as T-1, T-2. Call get_ready_tasks.",
    )
    .await;

    h.call(
        "backend",
        "create_request",
        json!({"body": "Rename the board"}),
    )
    .await;
    let created = "fold: RQ-2 created. Call claim_request.";
    expect(&mut frontend, created).await;
    h.call(
        "backend",
        "claim_request",
        json!({"id": "RQ-2", "kind": "feature", "title": "Rename"}),
    )
    .await;
    h.call("backend", "submit_request_draft", draft("RQ-2"))
        .await;

    nothing_before_a_message(&h, &mut backend, "backend").await;
    nothing_before_a_message(&h, &mut review, "review").await;
}
