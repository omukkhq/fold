#![cfg(test)]

use std::time::Duration;

use axum::http::Method;
use fold_core::{ReviewBatchId, codes};
use fold_daemon::jobs::{JobProgress, spawn_plan_deadlines};
use fold_store::reviews::plans as stored;
use fold_store::testing::eventually;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::common::acceptance::{HUMAN, Harness, StoredEvent, WAIT, Work};
use crate::workers_support::FakeClaude;

/// `frontend` is the main agent and `review-lead` the review lead that it started, connected.
async fn with_review_lead() -> (Harness, FakeClaude) {
    let fake = FakeClaude::new("result");
    let mut h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main("frontend").await;
    h.call("frontend", "start_worker", json!({"role": "review_lead"}))
        .await;
    h.connect("review-lead").await;
    (h, fake)
}

/// A new task of the main agent; it is To do until someone works on it.
async fn task(h: &Harness, title: &str) -> String {
    h.create_task("frontend", title).await
}

/// The main agent works on `task` and opens the first version of its proposal with one new file; returns the proposal and the work.
async fn open(h: &Harness, task: &str, path: &str) -> (String, Work) {
    let work = h.start_work("frontend", task).await;
    work.write(&[(path, "pub fn a() {}\n")]);
    let proposal = h.propose("frontend", &work, "Add a file").await;
    (proposal, work)
}

fn kinds<'a>(events: &'a [StoredEvent], kind: &str) -> Vec<&'a StoredEvent> {
    events.iter().filter(|event| event.kind == kind).collect()
}

async fn plan(h: &Harness, id: &str) -> Value {
    h.call("review-lead", "get_review_plan", json!({"id": id}))
        .await
}

/// fold refuses what a plan may never hold, each with a stable code and a hint: more than five tasks, a task after a member's task, a blocked task, a task with an open proposal, a proposal with a verdict, which also takes a member out while an unreviewed new version stays, an actor other than the review lead, and a proposal of a plan given to a reviewer alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plan_refuses_what_could_never_be_one_batch() {
    let (h, _fake) = with_review_lead().await;
    let first = task(&h, "First").await;
    let (p1, _) = open(&h, &first, "src/first.rs").await;
    let mut others = Vec::new();
    for title in ["Two", "Three", "Four", "Five", "Six"] {
        others.push(task(&h, title).await);
    }
    let refused = h
        .refused(
            "review-lead",
            "create_review_plan",
            json!({"proposals": [p1], "tasks": others}),
        )
        .await;
    assert_eq!(refused["code"], codes::REVIEW_PLAN_TOO_LARGE, "{refused}");

    let after = task(&h, "After the first").await;
    h.call(
        "frontend",
        "add_dependency",
        json!({"task": after, "needs": first}),
    )
    .await;
    let refused = h
        .refused(
            "review-lead",
            "create_review_plan",
            json!({"proposals": [p1], "tasks": [after]}),
        )
        .await;
    assert_eq!(refused["code"], codes::TASK_NOT_AWAITABLE, "{refused}");
    assert_eq!(refused["details"]["reason"], "depends_on_member");

    h.call(
        "frontend",
        "set_blocker",
        json!({"id": others[0], "reason": "Wait for the provider"}),
    )
    .await;
    let refused = h
        .refused(
            "review-lead",
            "create_review_plan",
            json!({"proposals": [p1], "tasks": [others[0]]}),
        )
        .await;
    assert_eq!(refused["details"]["reason"], "blocker", "{refused}");

    let (p2, work) = open(&h, &others[1], "src/two.rs").await;
    let refused = h
        .refused(
            "review-lead",
            "create_review_plan",
            json!({"proposals": [p1], "tasks": [others[1]]}),
        )
        .await;
    assert_eq!(refused["details"]["reason"], "proposal_open", "{refused}");
    assert!(refused["hint"].as_str().unwrap().contains(&p2));

    // A version that the author makes before any review still waits, also after the plan took the proposal.
    let revise = async |body: &str| {
        work.write(&[("src/two.rs", body)]);
        h.call(
            "frontend",
            "record_changes",
            json!({"ws": work.workspace, "message": "Rename"}),
        )
        .await;
        h.call("frontend", "update_proposal", json!({"id": p2}))
            .await;
    };
    revise("pub fn b() {}\n").await;
    let planned = h
        .call(
            "review-lead",
            "create_review_plan",
            json!({"proposals": [p2], "tasks": [others[4]]}),
        )
        .await;
    assert_eq!(planned["id"], "rp-1");
    revise("pub fn c() {}\n").await;
    assert_eq!(plan(&h, "rp-1").await["proposals"], json!([p2]));

    // A verdict takes the proposal out, and a proposal with a verdict never waits again.
    let path = format!("/api/v1/projects/demo/proposals/{p2}/reviews");
    let (status, answer) = h
        .http(
            Method::POST,
            &path,
            Some(&json!({"version": 3, "verdict": "request_changes", "body": "rename back"})),
            HUMAN,
        )
        .await;
    assert!(status.is_success(), "{status}: {answer}");
    let read = plan(&h, "rp-1").await;
    assert_eq!(read["proposals"], json!([]), "{read}");
    let events = h.events().await;
    let left = kinds(&events, "review.plan_left");
    assert_eq!(
        (&left[0].data["proposal"], &left[0].data["reason"]),
        (&json!(p2), &json!("reviewed"))
    );
    let refused = h
        .refused(
            "review-lead",
            "create_review_plan",
            json!({"proposals": [p2], "tasks": [others[2]]}),
        )
        .await;
    assert_eq!(refused["code"], codes::PROPOSAL_NOT_PLANNABLE, "{refused}");
    assert_eq!(refused["details"]["reason"], "reviewed");

    let refused = h
        .refused(
            "frontend",
            "create_review_plan",
            json!({"proposals": [p1], "tasks": [others[2]]}),
        )
        .await;
    assert_eq!(refused["code"], codes::REVIEW_PLAN_DENIED, "{refused}");

    let created = h
        .call(
            "review-lead",
            "create_review_plan",
            json!({"proposals": [p1], "tasks": [others[2]]}),
        )
        .await;
    assert_eq!(created["id"], "rp-2");
    let refused = h
        .refused(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "proposals": [p1]}),
        )
        .await;
    assert_eq!(refused["code"], codes::ALREADY_IN_PLAN, "{refused}");
    assert!(refused["hint"].as_str().unwrap().contains("plan rp-2"));
    let refused = h
        .refused(
            "review-lead",
            "create_review_plan",
            json!({"proposals": [p1], "tasks": [others[3]]}),
        )
        .await;
    assert_eq!(refused["code"], codes::ALREADY_IN_PLAN, "{refused}");
    let events = h.events().await;
    assert_eq!(kinds(&events, "review.plan_created").len(), 2);
}

/// The proposal of an awaited task joins the plan when it opens, the review lead's start gives every open proposal to one reviewer as one batch, and the batch finds its plan.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_awaited_proposal_joins_and_the_plan_starts_as_one_batch() {
    let (h, _fake) = with_review_lead().await;
    let first = task(&h, "First").await;
    let second = task(&h, "Second").await;
    let (p1, _) = open(&h, &first, "src/first.rs").await;
    let created = h
        .call(
            "review-lead",
            "create_review_plan",
            json!({"proposals": [p1], "tasks": [second]}),
        )
        .await;
    assert_eq!(created["awaited"][0]["task"], second, "{created}");
    assert_eq!(created["awaited"][0]["worker"], Value::Null);
    assert!(
        created["next"].as_str().unwrap().contains("send_message"),
        "{created}"
    );
    let view = h
        .call("review-lead", "get_proposal", json!({"id": p1}))
        .await;
    assert_eq!(view["review_plan"]["id"], "rp-1", "{view}");
    assert_eq!(view["review_plan"]["awaited"], json!([second]));

    let (p2, _) = open(&h, &second, "src/second.rs").await;
    let read = plan(&h, "rp-1").await;
    assert_eq!(read["proposals"], json!([p1, p2]), "{read}");
    assert_eq!(read["awaited"], json!([]));
    assert!(read["next"].as_str().unwrap().contains("plan rp-1"));
    let events = h.events().await;
    let joined = kinds(&events, "review.plan_joined");
    assert_eq!(joined.len(), 1);
    assert_eq!(joined[0].actor, "fold");
    assert_eq!(joined[0].data["proposal"], p2);
    assert_eq!(joined[0].data["awaited"], json!([]));

    let (status, board) = h
        .http(Method::GET, "/api/v1/projects/demo/board", None, HUMAN)
        .await;
    assert!(status.is_success(), "{board}");
    let cards: Vec<&Value> = board["columns"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|column| column["cards"].as_array().unwrap())
        .filter(|card| card["proposal"]["review_plan"]["plan"] == "rp-1")
        .collect();
    assert_eq!(cards.len(), 2, "{board}");
    let (status, page) = h
        .http(
            Method::GET,
            &format!("/api/v1/projects/demo/proposals/{p2}"),
            None,
            HUMAN,
        )
        .await;
    assert!(status.is_success(), "{page}");
    assert_eq!(
        page["review_plan"],
        json!({"plan": "rp-1", "deadline_at": created["deadline_at"], "wait_ended": false, "awaited": [], "proposals": [p1, p2]})
    );

    let started = h
        .call(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "plan": "rp-1"}),
        )
        .await;
    let reviewer = started["worker"]["name"].as_str().unwrap().to_owned();
    let read = plan(&h, "rp-1").await;
    assert_eq!(read["state"], "started", "{read}");
    assert_eq!(read["reviewer"], reviewer);
    assert_eq!(read["batch"], "rb-1");
    assert!(
        read["members"]
            .as_array()
            .unwrap()
            .iter()
            .all(|member| member["left"] == "started")
    );
    let events = h.events().await;
    let batch = kinds(&events, "review.batch_created");
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].data["members"].as_array().unwrap().len(), 2);
    let plan_started = kinds(&events, "review.plan_started");
    assert_eq!(plan_started[0].data["batch"], "rb-1");
    let worker_started = kinds(&events, "worker.started");
    assert_eq!(
        worker_started.last().unwrap().data["proposals"],
        json!([p1, p2])
    );
    let found = stored::plan_of_batch(
        h.handle.store.read(),
        ReviewBatchId::from_number(1).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(found.map(|plan| plan.to_string()), Some("rp-1".to_owned()));
    let view = h
        .call("review-lead", "get_proposal", json!({"id": p1}))
        .await;
    assert_eq!(view["review_plan"], Value::Null);
    let refused = h
        .refused(
            "review-lead",
            "start_worker",
            json!({"role": "reviewer", "plan": "rp-1"}),
        )
        .await;
    assert_eq!(refused["code"], codes::REVIEW_PLAN_NOT_OPEN, "{refused}");
}

/// An hour after the plan was made its wait ends: the tasks it still waits for leave it, and the review lead starts the open proposals, as one batch when there are two or more, else alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_deadline_ends_the_wait_and_the_open_members_start_as_a_batch_or_alone() {
    let (h, _fake) = with_review_lead().await;
    let mut ids = Vec::new();
    for title in ["One", "Two", "Three", "Four", "Five"] {
        ids.push(task(&h, title).await);
    }
    let (p1, _) = open(&h, &ids[0], "src/one.rs").await;
    h.call(
        "review-lead",
        "create_review_plan",
        json!({"proposals": [p1], "tasks": [ids[1], ids[2]]}),
    )
    .await;
    let (p2, _) = open(&h, &ids[1], "src/two.rs").await;
    let (p4, _) = open(&h, &ids[3], "src/four.rs").await;
    h.call(
        "review-lead",
        "create_review_plan",
        json!({"proposals": [p4], "tasks": [ids[4]]}),
    )
    .await;
    let reviews = h.handle.reviews(&h.state.registry);
    h.project
        .clock()
        .advance(Duration::from_secs(59 * 60))
        .unwrap();
    assert_eq!(reviews.end_due_plan_waits().await.unwrap(), 0);
    h.project.clock().advance(Duration::from_secs(60)).unwrap();
    assert_eq!(reviews.end_due_plan_waits().await.unwrap(), 2);
    assert_eq!(reviews.end_due_plan_waits().await.unwrap(), 0, "once");

    let events = h.events().await;
    let ended = kinds(&events, "review.plan_wait_ended");
    assert_eq!(ended.len(), 2);
    assert_eq!(ended[0].actor, "fold");
    assert_eq!(ended[0].data["proposals"], json!([p1, p2]));
    assert_eq!(ended[0].data["left"], json!([ids[2]]));
    let read = plan(&h, "rp-1").await;
    assert_eq!(read["wait_ended"], true);
    assert_eq!(read["awaited"], json!([]));
    let refused = h
        .refused(
            "review-lead",
            "change_review_plan",
            json!({"id": "rp-1", "add_tasks": [ids[2]]}),
        )
        .await;
    assert_eq!(refused["code"], codes::REVIEW_PLAN_NOT_OPEN, "{refused}");

    h.call(
        "review-lead",
        "start_worker",
        json!({"role": "reviewer", "plan": "rp-1"}),
    )
    .await;
    assert_eq!(plan(&h, "rp-1").await["batch"], "rb-1");
    h.call(
        "review-lead",
        "start_worker",
        json!({"role": "reviewer", "plan": "rp-2"}),
    )
    .await;
    let alone = plan(&h, "rp-2").await;
    assert_eq!(alone["state"], "started", "{alone}");
    assert_eq!(alone["batch"], Value::Null);
    let view = h
        .call("review-lead", "get_proposal", json!({"id": p4}))
        .await;
    assert_eq!(view["claim"]["holder"], alone["reviewer"], "{view}");

    // A task that left the plan opens its proposal later and is reviewed on its own rules.
    let (p3, _) = open(&h, &ids[2], "src/three.rs").await;
    let view = h
        .call("review-lead", "get_proposal", json!({"id": p3}))
        .await;
    assert_eq!(view["review_plan"], Value::Null);
}

/// The deadline job ends the wait by the project's clock, not by a fixed sleep.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_deadline_job_ends_a_wait_by_the_projects_clock() {
    let (h, _fake) = with_review_lead().await;
    let first = task(&h, "First").await;
    let second = task(&h, "Second").await;
    let (p1, _) = open(&h, &first, "src/first.rs").await;
    h.call(
        "review-lead",
        "create_review_plan",
        json!({"proposals": [p1], "tasks": [second]}),
    )
    .await;
    h.project
        .clock()
        .advance(Duration::from_secs(60 * 60))
        .unwrap();
    let (stop, stopped) = watch::channel(false);
    let job = spawn_plan_deadlines(h.state.clone(), Duration::from_millis(5), stopped);
    let mut progress = job.progress();
    tokio::time::timeout(
        WAIT,
        progress.wait_for(|seen: &JobProgress| seen.cycles >= 1),
    )
    .await
    .expect("the job ends the wait within the timeout")
    .unwrap();
    stop.send(true).unwrap();
    job.stopped(WAIT).await;
    assert_eq!(plan(&h, "rp-1").await["wait_ended"], true);
}

/// An awaited task that gets a blocker or is archived leaves the plan with that reason, a proposal whose task gets the human review mark leaves too, and a plan with no member left ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_task_that_stops_being_eligible_leaves_and_an_empty_plan_ends() {
    let (h, _fake) = with_review_lead().await;
    let first = task(&h, "First").await;
    let blocked = task(&h, "Blocked").await;
    let archived = task(&h, "Archived").await;
    let (p1, _) = open(&h, &first, "src/first.rs").await;
    h.call(
        "review-lead",
        "create_review_plan",
        json!({"proposals": [p1], "tasks": [blocked, archived]}),
    )
    .await;
    h.call(
        "frontend",
        "set_blocker",
        json!({"id": blocked, "reason": "Wait for the provider"}),
    )
    .await;
    h.call("frontend", "archive_task", json!({"id": archived}))
        .await;
    let events = h.events().await;
    let left: Vec<(Value, Value, Value)> = kinds(&events, "review.plan_left")
        .into_iter()
        .map(|event| {
            (
                event.data["task"].clone(),
                event.data["reason"].clone(),
                event.data["awaited"].clone(),
            )
        })
        .collect();
    assert_eq!(
        left,
        [
            (json!(blocked), json!("blocker"), json!([archived])),
            (json!(archived), json!("archived"), json!([])),
        ]
    );
    let read = plan(&h, "rp-1").await;
    assert_eq!(read["state"], "open");
    assert_eq!(read["proposals"], json!([p1]));

    let path = format!("/api/v1/projects/demo/tasks/{first}/human-review");
    let (status, answer) = h
        .http(
            Method::PUT,
            &path,
            Some(&json!({"reason": "The user reads this one"})),
            HUMAN,
        )
        .await;
    assert!(status.is_success(), "{status}: {answer}");
    let read = plan(&h, "rp-1").await;
    assert_eq!(read["state"], "ended", "{read}");
    let events = h.events().await;
    assert_eq!(
        kinds(&events, "review.plan_left").last().unwrap().data["reason"],
        "human_review"
    );
    assert_eq!(kinds(&events, "review.plan_ended").len(), 1);
    let deleted = h
        .refused("frontend", "delete_task", json!({"id": archived}))
        .await;
    assert!(
        deleted["details"]["history"]
            .as_array()
            .is_some_and(|kept| kept.contains(&json!("review_plan"))),
        "{deleted}"
    );
}

/// The review lead changes an open plan: it takes a task out, adds another, and the tasks of its proposals count toward the five.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_review_lead_changes_an_open_plan_within_five_tasks() {
    let (h, _fake) = with_review_lead().await;
    let mut ids = Vec::new();
    for title in ["One", "Two", "Three", "Four", "Five", "Six"] {
        ids.push(task(&h, title).await);
    }
    let (p1, _) = open(&h, &ids[0], "src/one.rs").await;
    h.call(
        "review-lead",
        "create_review_plan",
        json!({"proposals": [p1], "tasks": [ids[1], ids[2], ids[3], ids[4]]}),
    )
    .await;
    let refused = h
        .refused(
            "review-lead",
            "change_review_plan",
            json!({"id": "rp-1", "add_tasks": [ids[5]]}),
        )
        .await;
    assert_eq!(refused["code"], codes::REVIEW_PLAN_TOO_LARGE, "{refused}");
    let changed = h
        .call(
            "review-lead",
            "change_review_plan",
            json!({"id": "rp-1", "add_tasks": [ids[5]], "remove": [ids[1]]}),
        )
        .await;
    let awaited: Vec<Value> = changed["awaited"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["task"].clone())
        .collect();
    assert_eq!(
        awaited,
        [&ids[2], &ids[3], &ids[4], &ids[5]].map(|id| json!(id))
    );
    let refused = h
        .refused(
            "review-lead",
            "change_review_plan",
            json!({"id": "rp-1", "remove": [ids[1]]}),
        )
        .await;
    assert_eq!(refused["code"], codes::INVALID_REQUEST, "{refused}");
    let removed = h
        .call(
            "review-lead",
            "change_review_plan",
            json!({"id": "rp-1", "remove": [ids[0], ids[2], ids[3], ids[4], ids[5]]}),
        )
        .await;
    assert_eq!(removed["state"], "ended", "{removed}");
    eventually(WAIT, async || {
        let events = h.events().await;
        if kinds(&events, "review.plan_ended").len() == 1 {
            Ok(())
        } else {
            Err(events.len())
        }
    })
    .await;
}

/// The main agent claims or batches no proposal of an open plan and never adds to a plan, but on the user's word takes a member out or closes the plan, whose proposals a reviewer can then get alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_main_agent_takes_members_out_of_a_plan_or_closes_it() {
    let (h, _fake) = with_review_lead().await;
    let first = task(&h, "First").await;
    let second = task(&h, "Second").await;
    let third = task(&h, "Third").await;
    let fourth = task(&h, "Fourth").await;
    let (p1, _) = open(&h, &first, "src/first.rs").await;
    let (p2, _) = open(&h, &second, "src/second.rs").await;
    h.call(
        "review-lead",
        "create_review_plan",
        json!({"proposals": [p1, p2], "tasks": [third]}),
    )
    .await;
    for (tool, arguments) in [
        ("claim_review", json!({"id": p1})),
        ("create_review_batch", json!({"proposals": [p1, p2]})),
    ] {
        let refused = h.refused("frontend", tool, arguments).await;
        assert_eq!(refused["code"], codes::ALREADY_IN_PLAN, "{tool}: {refused}");
    }
    let refused = h
        .refused(
            "frontend",
            "change_review_plan",
            json!({"id": "rp-1", "add_tasks": [fourth]}),
        )
        .await;
    assert_eq!(refused["code"], codes::REVIEW_PLAN_DENIED, "{refused}");

    let changed = h
        .call(
            "frontend",
            "change_review_plan",
            json!({"id": "rp-1", "remove": [second]}),
        )
        .await;
    assert_eq!(changed["proposals"], json!([p1]), "{changed}");
    let closed = h
        .call(
            "frontend",
            "change_review_plan",
            json!({"id": "rp-1", "close": true}),
        )
        .await;
    assert_eq!(closed["state"], "ended", "{closed}");
    let events = h.events().await;
    let changes = kinds(&events, "review.plan_changed");
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[1].data["closed"], true);
    assert_eq!(changes[1].data["removed"], json!([first, third]));
    let ended = kinds(&events, "review.plan_ended");
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].data["reason"], "closed");
    let refused = h
        .refused(
            "frontend",
            "change_review_plan",
            json!({"id": "rp-1", "remove": [first]}),
        )
        .await;
    assert_eq!(refused["code"], codes::REVIEW_PLAN_NOT_OPEN, "{refused}");
    h.call(
        "review-lead",
        "start_worker",
        json!({"role": "reviewer", "proposals": [p1, p2]}),
    )
    .await;
}
