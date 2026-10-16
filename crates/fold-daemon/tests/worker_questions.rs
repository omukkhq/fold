#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_core::roles::PermissionBehavior;
use fold_core::{
    Actor, AgentActor, AgentName, PermissionQuestionId, WorkerName, WorkerState, codes,
};
use fold_daemon::services::agents::AgentError;
use fold_store::agents::WorkerProcessRow;
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, SLUG, WAIT};
use crate::workers_support::{FakeClaude, Run};

const MAIN: &str = "frontend";

fn name(text: &str) -> AgentName {
    AgentName::parse(text).unwrap()
}

async fn worker(harness: &Harness, agent: &str) -> WorkerProcessRow {
    harness
        .handle
        .agents()
        .get_worker(&name(agent))
        .await
        .unwrap()
}

async fn until_state(harness: &Harness, agent: &str, state: WorkerState) -> WorkerProcessRow {
    eventually(WAIT, async || {
        let row = worker(harness, agent).await;
        if row.state == state {
            Ok(row)
        } else {
            Err(row.state)
        }
    })
    .await
}

/// The main agent defines area `web` over label `ui` and starts its lead `web-lead`.
async fn start_lead(harness: &Harness) -> String {
    harness
        .call(MAIN, "create_label", json!({"name": "ui", "color": "sage"}))
        .await;
    harness
        .call(MAIN, "define_area", json!({"name": "web", "label": "ui"}))
        .await;
    let started = harness
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    started["worker"]["name"].as_str().unwrap().to_owned()
}

/// The answers fold wrote on a run's stdin.
fn responses(run: &Run) -> Vec<Value> {
    run.stdin
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["type"] == "control_response")
        .map(|line| line["response"]["response"].clone())
        .collect()
}

/// The runs of `agent` once there are `count` of them and the newest has `answers` answers on its stdin.
async fn until_answered(fake: &FakeClaude, agent: &str, count: usize, answers: usize) -> Vec<Run> {
    eventually(WAIT, async || {
        let runs = fake.runs_of(agent);
        match runs.last() {
            Some(last) if runs.len() == count && responses(last).len() >= answers => Ok(runs),
            _ => Err(runs.len()),
        }
    })
    .await
}

async fn open_questions(harness: &Harness) -> Vec<Value> {
    let (status, list) = harness
        .socket(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/worker-questions"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["items"].as_array().unwrap().clone()
}

async fn until_questions(harness: &Harness, count: usize) -> Vec<Value> {
    eventually(WAIT, async || {
        let items = open_questions(harness).await;
        if items.len() == count {
            Ok(items)
        } else {
            Err(items.len())
        }
    })
    .await
}

async fn needs_you(harness: &Harness) -> Vec<Value> {
    let (status, list) = harness
        .socket(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/needs-you"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["items"].as_array().unwrap().clone()
}

async fn answer(harness: &Harness, question: &str, body: Value) -> (StatusCode, Value) {
    harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/worker-questions/{question}/answer"),
            Some(&body),
        )
        .await
}

/// Sends the lead a message from the main agent, which reaches its stdin as a new turn, and returns the message id.
async fn nudge(harness: &Harness, lead: &str) -> String {
    let sent = harness
        .call(
            MAIN,
            "send_message",
            json!({"to": [lead], "body": "carry on"}),
        )
        .await;
    sent["id"].as_str().unwrap().to_owned()
}

async fn stored(harness: &Harness, kind: &str) -> Vec<Value> {
    harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == kind)
        .map(|event| event.data)
        .collect()
}

#[tokio::test]
async fn an_ask_rule_call_waits_for_one_question_and_the_humans_allow() {
    let fake = FakeClaude::new("permission_request");
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let lead = start_lead(&harness).await;
    let questions = until_questions(&harness, 1).await;
    assert_eq!(questions[0]["id"], "pq-1");
    assert_eq!(questions[0]["worker"], lead);
    assert_eq!(questions[0]["tool"], "Bash");
    assert_eq!(questions[0]["input"]["command"], "echo fold-probe-allow");
    assert_eq!(questions[0]["answer"], Value::Null);
    let waiting = needs_you(&harness).await;
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    assert_eq!(waiting[0]["kind"], "permission");
    assert_eq!(waiting[0]["screen"], "worker");
    assert_eq!(waiting[0]["question"], "pq-1");
    assert_eq!(waiting[0]["agent"], lead);
    assert_eq!(worker(&harness, &lead).await.state, WorkerState::Working);
    assert!(
        responses(&fake.runs_of(&lead)[0]).is_empty(),
        "the request waits for the human"
    );

    let agent_worker = WorkerName::main(&name(MAIN));
    let agent = Actor::Agent(AgentActor {
        agent: agent_worker.agent(),
        worker: agent_worker,
        tags: Vec::new(),
    });
    let by_agent = harness
        .handle
        .agents()
        .answer_worker_permission(
            &agent,
            PermissionQuestionId::from_number(1).unwrap(),
            PermissionBehavior::Allow,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(by_agent, AgentError::Fold(error) if error.code() == codes::HUMAN_REQUIRED));

    let (status, answered) = answer(&harness, "pq-1", json!({"behavior": "allow"})).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    assert_eq!(answered["answer"]["behavior"], "allow");
    assert_eq!(answered["answer"]["answered_by"], "human:ana");
    let run = until_answered(&fake, &lead, 1, 1).await.remove(0);
    let allowed = responses(&run).remove(0);
    assert_eq!(allowed["behavior"], "allow");
    assert_eq!(allowed["updatedInput"]["command"], "echo fold-probe-allow");
    let idle = until_state(&harness, &lead, WorkerState::Idle).await;
    assert_eq!(idle.turns, 1, "the turn ended after the tool ran");

    let (status, again) = answer(&harness, "pq-1", json!({"behavior": "deny"})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");
    assert_eq!(again["error"]["code"], codes::PERMISSION_QUESTION_CLOSED);
    let (status, missing) = answer(&harness, "pq-9", json!({"behavior": "deny"})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{missing}");
    assert!(open_questions(&harness).await.is_empty());
    assert!(needs_you(&harness).await.is_empty());
    let asked = stored(&harness, "worker.permission_asked").await;
    assert_eq!(
        asked,
        [json!({"question": "pq-1", "agent": lead, "tool": "Bash"})]
    );
    let answered = stored(&harness, "worker.permission_answered").await;
    assert_eq!(
        answered,
        [json!({"question": "pq-1", "agent": lead, "tool": "Bash", "behavior": "allow"})]
    );
    let events = serde_json::to_string(
        &harness
            .events()
            .await
            .into_iter()
            .map(|e| e.data)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(
        !events.contains("fold-probe-allow"),
        "an event holds the tool input"
    );
}

#[tokio::test]
async fn an_answer_never_decides_a_later_call_of_a_live_worker() {
    let fake = FakeClaude::new("permission_request");
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let lead = start_lead(&harness).await;
    until_questions(&harness, 1).await;
    let (status, answered) = answer(&harness, "pq-1", json!({"behavior": "allow"})).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    until_answered(&fake, &lead, 1, 1).await;
    until_state(&harness, &lead, WorkerState::Idle).await;
    nudge(&harness, &lead).await;
    let questions = until_questions(&harness, 1).await;
    assert_eq!(
        questions[0]["id"], "pq-2",
        "the same call asks the human again"
    );
    assert_eq!(responses(&fake.runs_of(&lead)[0]).len(), 1);
}

#[tokio::test]
async fn a_denied_call_reaches_the_worker_with_the_humans_note() {
    let fake = FakeClaude::new("permission_deny");
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let lead = start_lead(&harness).await;
    until_questions(&harness, 1).await;
    let (status, answered) = answer(
        &harness,
        "pq-1",
        json!({"behavior": "deny", "message": "use the staging target"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    let run = until_answered(&fake, &lead, 1, 1).await.remove(0);
    assert_eq!(
        responses(&run)[0],
        json!({"behavior": "deny", "message": "use the staging target"})
    );
    until_state(&harness, &lead, WorkerState::Idle).await;
}

#[tokio::test]
async fn a_stop_closes_the_open_question() {
    let fake = FakeClaude::new("permission_request");
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let lead = start_lead(&harness).await;
    until_questions(&harness, 1).await;
    harness
        .call(MAIN, "stop_worker", json!({"name": lead}))
        .await;
    assert!(open_questions(&harness).await.is_empty());
    let (status, closed) = answer(&harness, "pq-1", json!({"behavior": "allow"})).await;
    assert_eq!(status, StatusCode::CONFLICT, "{closed}");
}

#[tokio::test]
async fn a_restart_keeps_one_question_and_the_resumed_worker_gets_the_answer_first() {
    let fake = FakeClaude::new("permission_request");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let lead = start_lead(&harness).await;
    until_questions(&harness, 1).await;
    let session = worker(&harness, &lead).await.session_id;
    harness.stop().await;
    harness.start_again().await;
    let questions = open_questions(&harness).await;
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0]["id"], "pq-1");
    assert_eq!(
        fake.runs_of(&lead).len(),
        1,
        "a worker that waits for an answer is not resumed yet"
    );
    let waiting = worker(&harness, &lead).await;
    assert_eq!(waiting.state, WorkerState::Working);
    assert_eq!(waiting.pid, None);

    let (status, answered) = answer(&harness, "pq-1", json!({"behavior": "allow"})).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    let runs = until_answered(&fake, &lead, 2, 1).await;
    let resumed = &runs[1];
    assert_eq!(resumed.flag("--resume"), Some(session.as_str()));
    assert!(resumed.stdin[1].contains("\"type\":\"user\""));
    assert!(resumed.stdin[1].contains("The human allowed it. Run it again now."));
    assert!(resumed.stdin[1].contains("echo fold-probe-allow"));
    let again = responses(resumed).remove(0);
    assert_eq!(
        again["behavior"], "allow",
        "the repeated call takes the stored answer"
    );
    assert_eq!(again["updatedInput"]["command"], "echo fold-probe-allow");
    until_state(&harness, &lead, WorkerState::Idle).await;
    assert_eq!(stored(&harness, "worker.permission_asked").await.len(), 1);
    assert!(open_questions(&harness).await.is_empty());
    harness.connect(MAIN).await;
    nudge(&harness, &lead).await;
    let questions = until_questions(&harness, 1).await;
    assert_eq!(
        questions[0]["id"], "pq-2",
        "the stored answer took only the repeat of the lost request"
    );
}

#[tokio::test]
async fn an_ask_user_question_becomes_a_question_with_choices_and_the_reply_answers_it() {
    let fake = FakeClaude::new("ask_question");
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let ana = fold_core::HumanName::parse("ana").unwrap();
    let now = harness.project.now();
    harness
        .project
        .registry()
        .write(async |tx| fold_store::projects::ensure_human(tx, &ana, now).await)
        .await
        .unwrap();
    assert!(needs_you(&harness).await.is_empty());
    let lead = start_lead(&harness).await;
    let waiting = eventually(WAIT, async || {
        let items = needs_you(&harness).await;
        if items.len() == 1 {
            Ok(items)
        } else {
            Err(items.len())
        }
    })
    .await;
    assert_eq!(waiting[0]["kind"], "question");
    assert_eq!(waiting[0]["screen"], "messages");
    assert_eq!(waiting[0]["agent"], lead);
    assert_eq!(waiting[0]["choices"], json!(["Red", "Blue"]));
    let message = waiting[0]["message"].as_str().unwrap().to_owned();
    assert!(
        open_questions(&harness).await.is_empty(),
        "a question with choices is a message"
    );
    let (status, thread) = harness
        .socket(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/threads/{message}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    assert!(thread["awaiting_human"].as_bool().unwrap());
    assert_eq!(thread["messages"][0]["sender"], lead);
    assert_eq!(
        thread["messages"][0]["body"],
        "Which colour should the fold probe use?"
    );
    assert_eq!(thread["messages"][0]["choices"], json!(["Red", "Blue"]));
    assert!(stored(&harness, "worker.permission_asked").await.is_empty());

    let (status, reply) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/feedback"),
            Some(&json!({"reply_to": message, "body": " blue "})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{reply}");
    let run = until_answered(&fake, &lead, 1, 1).await.remove(0);
    let allowed = responses(&run).remove(0);
    assert_eq!(allowed["behavior"], "allow");
    assert_eq!(
        allowed["updatedInput"]["answers"],
        json!({"Which colour should the fold probe use?": "Blue"})
    );
    assert_eq!(
        allowed["updatedInput"]["questions"][0]["options"][1]["label"],
        "Blue"
    );
    assert!(needs_you(&harness).await.is_empty());
    let reply = reply["id"].as_str().unwrap().to_owned();
    let later = nudge(&harness, &lead).await;
    let run = eventually(WAIT, async || {
        let run = fake.runs_of(&lead).remove(0);
        let arrived = format!("message {later} arrived");
        if run.stdin.iter().any(|line| line.contains(&arrived)) {
            Ok(run)
        } else {
            Err(run.stdin.len())
        }
    })
    .await;
    let echoed = format!("message {reply} arrived");
    assert!(
        !run.stdin.iter().any(|line| line.contains(&echoed)),
        "the reply reaches the worker once, as the tool's answer"
    );
}

#[tokio::test]
async fn an_agent_asks_humans_a_question_with_choices() {
    let harness = Harness::start().await;
    let sent = harness
        .call(
            MAIN,
            "send_message",
            json!({"to": ["humans"], "body": "Ship the page today?", "choices": ["Yes", "No, tomorrow"]}),
        )
        .await;
    let id = sent["id"].as_str().unwrap().to_owned();
    let thread = harness
        .call(MAIN, "get_thread", json!({"thread": id}))
        .await;
    assert_eq!(
        thread["messages"][0]["choices"],
        json!(["Yes", "No, tomorrow"])
    );
    let plain = harness
        .call(
            MAIN,
            "send_message",
            json!({"to": ["backend"], "body": "plain"}),
        )
        .await;
    let inbox = harness.call("backend", "get_inbox", json!({})).await;
    let item = inbox["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == plain["id"])
        .unwrap();
    assert!(item.get("choices").is_none());
    let refused = harness
        .refused(
            MAIN,
            "send_message",
            json!({"to": ["humans"], "body": "Pick", "choices": ["only one"]}),
        )
        .await;
    assert_eq!(refused["code"], codes::INVALID_REQUEST);
    assert_eq!(refused["details"]["reason"], "choices");
    harness
        .call(
            MAIN,
            "send_message",
            json!({"to": ["humans"], "body": "The page is up."}),
        )
        .await;
    let waiting = needs_you(&harness).await;
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0]["message"], id);
    assert_eq!(waiting[0]["agent"], MAIN);
    assert_eq!(waiting[0]["choices"], json!(["Yes", "No, tomorrow"]));
}

#[tokio::test]
async fn a_question_without_a_human_to_ask_is_denied_so_the_turn_goes_on() {
    let fake = FakeClaude::new("ask_question");
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let lead = start_lead(&harness).await;
    let run = until_answered(&fake, &lead, 1, 1).await.remove(0);
    let denied = responses(&run).remove(0);
    assert_eq!(denied["behavior"], "deny");
    assert!(
        denied["message"]
            .as_str()
            .unwrap()
            .contains("could not put this tool call to a human")
    );
    assert!(needs_you(&harness).await.is_empty());
}

#[tokio::test]
async fn a_restart_while_a_question_with_choices_waits_resumes_the_worker_with_the_reply() {
    let fake = FakeClaude::new("ask_question");
    let mut harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    let ana = fold_core::HumanName::parse("ana").unwrap();
    let now = harness.project.now();
    harness
        .project
        .registry()
        .write(async |tx| fold_store::projects::ensure_human(tx, &ana, now).await)
        .await
        .unwrap();
    let lead = start_lead(&harness).await;
    let waiting = eventually(WAIT, async || {
        let items = needs_you(&harness).await;
        if items.len() == 1 {
            Ok(items)
        } else {
            Err(items.len())
        }
    })
    .await;
    let message = waiting[0]["message"].as_str().unwrap().to_owned();
    harness.stop().await;
    harness.start_again().await;
    assert_eq!(fake.runs_of(&lead).len(), 1);
    let (status, reply) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/feedback"),
            Some(&json!({"reply_to": message, "body": "a darker red"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{reply}");
    let runs = until_answered(&fake, &lead, 2, 1).await;
    assert!(runs[1].stdin[1].contains(
        r#"The human answered: \"Which colour should the fold probe use?\": \"a darker red\""#
    ));
    let again = responses(&runs[1]).remove(0);
    assert_eq!(
        again["updatedInput"]["answers"],
        json!({"Which colour should the fold probe use?": "a darker red"}),
        "the repeated question takes the stored reply"
    );
}
