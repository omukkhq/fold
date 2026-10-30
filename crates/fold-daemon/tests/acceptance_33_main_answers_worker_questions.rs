#![cfg(test)]

use axum::http::{Method, StatusCode};
use fold_core::{AgentName, HumanName, WorkerState};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, SLUG, WAIT};
use crate::workers_support::{FakeClaude, Run};

const MAIN: &str = "frontend";

/// The main agent defines area `area` over a label of the same name and starts its lead, whose name it returns.
async fn start_lead(h: &Harness, area: &str) -> String {
    h.call(MAIN, "create_label", json!({"name": area, "color": "sage"}))
        .await;
    h.call(MAIN, "define_area", json!({"name": area, "label": area}))
        .await;
    let started = h
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": area}),
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

/// The first run of `agent` once fold answered its tool call.
async fn answered_run(fake: &FakeClaude, agent: &str) -> Run {
    eventually(WAIT, async || match fake.runs_of(agent).first() {
        Some(run) if !responses(run).is_empty() => Ok(run.clone()),
        _ => Err(fake.runs_of(agent).len()),
    })
    .await
}

async fn needs_you(h: &Harness) -> Vec<Value> {
    let (status, list) = h
        .socket(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/needs-you"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{list}");
    list["items"].as_array().unwrap().clone()
}

/// Acceptance 33, the main agent answers workers' own questions: two leads ask the humans with Claude Code's AskUserQuestion, each a set of two questions, and wait inside that tool call. The user does what they wait for and tells the main agent, which finds both questions with `list_open_asks` and answers each with the user's words through `mark_answered`. Each tool call ends with those answers as its result, the leads go on with their turns, and each thread shows the main agent's reply with the stored answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_33_the_main_agent_answers_two_waiting_ask_user_questions_for_the_user() {
    let fake = FakeClaude::new("ask_questions");
    let h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main(MAIN).await;
    let ana = HumanName::parse("ana").unwrap();
    let now = h.project.now();
    h.project
        .registry()
        .write(async |tx| fold_store::projects::ensure_human(tx, &ana, now).await)
        .await
        .unwrap();
    let web = start_lead(&h, "web").await;
    let api = start_lead(&h, "api").await;

    // Both leads wait in their AskUserQuestion call; the main agent sees both, with the questions to answer.
    let asks = eventually(WAIT, async || {
        let listed = h.call(MAIN, "list_open_asks", json!({})).await;
        let asks = listed["asks"].as_array().cloned().unwrap_or_default();
        if asks.len() == 2 {
            Ok(asks)
        } else {
            Err(asks.len())
        }
    })
    .await;
    let mut agents: Vec<&str> = asks
        .iter()
        .map(|ask| ask["agent"].as_str().unwrap())
        .collect();
    agents.sort_unstable();
    let mut leads = [web.as_str(), api.as_str()];
    leads.sort_unstable();
    assert_eq!(agents, leads);
    for ask in &asks {
        assert_eq!(ask["reply_only"], true);
        assert_eq!(ask["questions"], 2);
        assert_eq!(
            ask["answer_these"],
            json!([
                {"text": "Which colour should the fold probe use?", "choices": ["Red", "Blue"]},
                {"text": "Which size should the fold probe use?", "choices": ["Small", "Large"]},
            ])
        );
    }
    let first = asks[0]["id"].as_str().unwrap().to_owned();

    // A plain close cannot end a worker's question, and no agent but the main one answers for the user.
    let plain = h
        .refused(MAIN, "mark_answered", json!({"message": first}))
        .await;
    assert_eq!(plain["code"], "ask_not_open");
    assert_eq!(plain["details"]["reason"], "reply_only");
    let other = h
        .refused(
            "backend",
            "mark_answered",
            json!({"message": first, "answers": ["Blue", "Large"]}),
        )
        .await;
    assert_eq!(other["code"], "main_agent_required");

    // The user says what they decided; the main agent answers both questions with those words.
    let mut replies = Vec::new();
    for ask in &asks {
        let id = ask["id"].as_str().unwrap();
        let answered = h
            .call(
                MAIN,
                "mark_answered",
                json!({"message": id, "answers": ["blue", "Large"]}),
            )
            .await;
        assert_eq!(answered["answered"], id);
        replies.push(answered["reply"].as_str().unwrap().to_owned());
    }

    for lead in [&web, &api] {
        let run = answered_run(&fake, lead).await;
        let allowed = responses(&run).remove(0);
        assert_eq!(allowed["behavior"], "allow", "{lead}");
        assert_eq!(
            allowed["updatedInput"]["answers"],
            json!({
                "Which colour should the fold probe use?": "Blue",
                "Which size should the fold probe use?": "Large",
            }),
            "{lead}"
        );
        eventually(WAIT, async || {
            let row = h
                .handle
                .agents()
                .get_worker(&AgentName::parse(lead).unwrap())
                .await
                .unwrap();
            if row.state == WorkerState::Idle {
                Ok(())
            } else {
                Err(row.state)
            }
        })
        .await;
    }
    assert!(needs_you(&h).await.is_empty());
    assert_eq!(
        h.call(MAIN, "list_open_asks", json!({})).await,
        json!({"asks": []})
    );

    // Each thread shows the main agent's reply with the answers it gave for the user, which reached the lead only as its tool's answer.
    for (ask, reply) in asks.iter().zip(&replies) {
        let thread = h
            .call(MAIN, "get_thread", json!({"thread": ask["id"]}))
            .await;
        let answer = &thread["messages"][1];
        assert_eq!(answer["id"], reply.as_str());
        assert_eq!(answer["from"], MAIN);
        assert_eq!(answer["answers_ask"], ask["id"]);
        assert_eq!(answer["answers"], json!(["Blue", "Large"]));
        let lead = ask["agent"].as_str().unwrap();
        let echoed = format!("message {reply} arrived");
        assert!(
            fake.runs_of(lead)
                .iter()
                .all(|run| !run.stdin.iter().any(|line| line.contains(&echoed))),
            "{lead} gets the answers once, as its tool's answer"
        );
    }
}
