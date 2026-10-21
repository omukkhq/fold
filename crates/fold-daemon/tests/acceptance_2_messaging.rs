#![cfg(test)]

use crate::common::acceptance::{
    AGENTS, Harness, TOKEN, ids, message_notice, proposal_notice, ready_notice, wire,
};
use fold_core::AgentName;
use serde_json::json;

/// Acceptance 2, fold messages: a reply joins its parent's thread, and an acknowledged message leaves the inbox.
#[tokio::test]
async fn acceptance_2_a_reply_lands_in_the_senders_inbox_and_in_one_thread() {
    let harness = Harness::start().await;

    let question = harness
        .send("frontend", &["backend"], "Is the login endpoint ready?")
        .await;
    let inbox = harness.call("backend", "get_inbox", json!({})).await;
    assert_eq!(ids(&inbox), [question.as_str()]);
    assert_eq!(inbox["messages"][0]["from"], "frontend");
    assert_eq!(inbox["messages"][0]["body"], "Is the login endpoint ready?");

    let answer = harness
        .send_with(
            "backend",
            json!({"to": ["frontend"], "body": "Yes, POST /login is up.", "reply_to": question}),
        )
        .await;
    let acknowledged = harness
        .call("backend", "acknowledge_message", json!({"id": question}))
        .await;
    assert_eq!(acknowledged["acknowledged"], json!([question]));
    assert!(harness.inbox("backend").await.is_empty());

    let inbox = harness.call("frontend", "get_inbox", json!({})).await;
    assert_eq!(ids(&inbox), [answer.as_str()]);
    assert_eq!(inbox["messages"][0]["reply_to"], question.as_str());
    assert_eq!(inbox["messages"][0]["thread"], question.as_str());

    let thread = harness
        .call("frontend", "get_thread", json!({"thread": question}))
        .await;
    assert_eq!(ids(&thread), [question, answer]);
}

/// Acceptance 2, reconnect and restart: what arrived while a session was away waits in its inbox, an acknowledged message never comes back, and an unacknowledged one outlives a daemon restart.
#[tokio::test]
async fn acceptance_2_messages_wait_through_a_reconnect_and_a_daemon_restart() {
    let mut harness = Harness::start().await;
    let first = harness.send("frontend", &["backend"], "first").await;
    assert_eq!(harness.inbox("backend").await, [first.as_str()]);
    harness
        .call("backend", "acknowledge_message", json!({"id": first}))
        .await;

    harness.disconnect("backend").await;
    let second = harness
        .send("frontend", &["backend"], "sent while backend was away")
        .await;
    harness.connect("backend").await;
    assert_eq!(harness.inbox("backend").await, [second.as_str()]);
    harness
        .call("backend", "acknowledge_message", json!({"id": second}))
        .await;

    let third = harness
        .send("frontend", &["backend"], "left unacknowledged")
        .await;
    harness.restart().await;
    assert_eq!(harness.inbox("backend").await, [third]);
    let thread = harness
        .call("backend", "get_thread", json!({"thread": first}))
        .await;
    assert_eq!(ids(&thread), [first]);
}

/// Acceptance 2, waiting: a session blocked in `wait_for_events` returns when a message for it arrives.
#[tokio::test]
async fn acceptance_2_wait_for_events_returns_when_a_message_for_the_waiting_agent_arrives() {
    let harness = Harness::start().await;
    harness.drain_events("review").await;

    let (woken, sent) = tokio::join!(
        harness.call(
            "review",
            "wait_for_events",
            json!({"types": ["message.sent"], "timeout_s": 20}),
        ),
        async {
            harness.until_waiting(1).await;
            harness
                .send("frontend", &["review"], "P-1 is ready for review")
                .await
        },
    );

    let events = woken["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{woken}");
    assert_eq!(events[0]["type"], "message.sent");
    assert_eq!(events[0]["actor"], "frontend");
    assert_eq!(events[0]["data"]["message"], sent.as_str());
    assert_eq!(events[0]["data"]["recipients"], json!(["review"]));
    assert_eq!(harness.inbox("review").await, [sent]);
}

/// Acceptance 2, session push: a message wakes an idle session through the inbox socket its session-start hook registered; an agent without a registered socket gets nothing pushed and still finds the message in its inbox.
#[tokio::test]
async fn acceptance_2_the_push_wakes_an_idle_session_and_skips_an_agent_without_an_inbox() {
    let harness = Harness::start().await;
    let mut review = harness.register_inbox("review").await;

    let first = harness
        .send("frontend", &["review"], "P-1 is ready for review")
        .await;
    let posted = review.next().await;
    assert_eq!(posted, wire(TOKEN, &message_notice(&first)));
    let lines: Vec<&str> = posted.lines().collect();
    assert_eq!(lines.len(), 2, "the auth line, then one notice line");
    assert!(lines[1].contains("get_inbox"), "{posted}");

    let backend = AgentName::parse("backend").unwrap();
    assert!(harness.handle.push.sockets(&backend).is_empty());
    let unpushed = harness
        .send("frontend", &["backend"], "no socket for this one")
        .await;
    let sentinel = harness.send("frontend", &["review"], "second").await;
    assert_eq!(review.next().await, wire(TOKEN, &message_notice(&sentinel)));
    assert_eq!(harness.inbox("backend").await, [unpushed]);
    assert_eq!(harness.inbox("review").await, [first, sentinel]);
}

/// Acceptance 2, session push: a proposal opened while `review` is idle wakes it through its inbox socket with a notice naming `start_worker`, because `review` is the review lead, which gives the review to a reviewer. The author's own agent hears nothing about its proposal; as the lead of the task's area it heard that the task was ready.
#[tokio::test]
async fn acceptance_2_a_proposal_opened_while_review_is_idle_wakes_it_through_the_push() {
    let mut harness = Harness::start_without_roles().await;
    crate::common::roles::give_role(
        &harness.handle.store,
        "review",
        fold_core::roles::Role::ReviewLead,
        None,
    )
    .await;
    harness.lead_the_work_area().await;
    let mut review = harness.register_inbox("review").await;
    let mut backend = harness.register_inbox("backend").await;

    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let work = harness.start_work("backend", &task).await;
    work.write(&[("src/login.txt", "POST /login\n")]);
    let proposal = harness
        .propose("backend", &work, "Add the login endpoint")
        .await;

    assert_eq!(
        review.next().await,
        wire(TOKEN, &proposal_notice(&proposal, &task))
    );
    let sentinel = harness
        .send("frontend", &["backend"], "after the proposal")
        .await;
    // `backend` leads the area of the task, so it heard that the task was ready before it claimed it.
    assert_eq!(backend.next().await, wire(TOKEN, &ready_notice(&task)));
    assert_eq!(
        backend.next().await,
        wire(TOKEN, &message_notice(&sentinel))
    );
}

/// Acceptance 2, native messaging: Claude Code's own `SendMessage` never reaches fold, so a conversation that never called `send_message` leaves no `messages` row. The native exchange itself runs by hand.
#[tokio::test]
async fn acceptance_2_sessions_that_never_call_send_message_leave_no_messages_row() {
    let harness = Harness::start().await;
    for agent in AGENTS {
        harness.call(agent, "list_agents", json!({})).await;
        assert!(harness.inbox(agent).await.is_empty());
    }

    assert_eq!(harness.message_rows().await, 0);

    harness.send("frontend", &["backend"], "through fold").await;
    assert_eq!(harness.message_rows().await, 1);
}
