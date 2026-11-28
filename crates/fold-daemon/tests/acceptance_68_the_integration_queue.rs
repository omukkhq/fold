#![cfg(test)]

use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::auto_integrate::Running;
use crate::common::acceptance::{HUMAN, Harness, WAIT};

async fn proposal(h: &Harness, id: &str) -> Value {
    h.call("review", "get_proposal", json!({"id": id})).await
}

/// `agent` builds a task titled `title` that writes `path` and sends it for review; returns the task and the proposal.
async fn proposed(h: &Harness, agent: &str, title: &str, path: &str) -> (String, String) {
    let task = h.create_task(agent, title).await;
    let work = h.start_work(agent, &task).await;
    work.write(&[(path, "content\n")]);
    let id = h.propose(agent, &work, title).await;
    (task, id)
}

async fn approved(h: &Harness, id: &str) {
    h.give_review("review", id).await;
    h.approve(id).await;
}

/// Acceptance 68: every approved proposal shows its place in the automatic integration queue or the named reason it waits. Three approved proposals stand at positions 1, 2 and 3, each naming the one ahead, in `get_proposal` and `list_proposals`; a proposal whose prerequisite is not Done waits for `prerequisites`, and one with the human review mark waits for `human_review`, with no position. The automatic integration job then takes the three in that order.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acceptance_68_approved_proposals_show_their_queue_place_or_why_they_wait() {
    let h = Harness::start().await;
    let (first_task, first) = proposed(&h, "backend", "Add the login endpoint", "src/a.txt").await;
    let (_, second) = proposed(&h, "frontend", "Add the page footer", "src/b.txt").await;
    let (_, third) = proposed(&h, "backend", "Add the logout endpoint", "src/c.txt").await;
    let later = h.create_task("frontend", "Show the login form").await;
    h.call(
        "frontend",
        "add_dependency",
        json!({"task": later, "needs": first_task}),
    )
    .await;
    let later_work = h.start_work("frontend", &later).await;
    later_work.write(&[("src/d.txt", "form\n")]);
    let waiting = h
        .propose("frontend", &later_work, "Show the login form")
        .await;
    let marked_task = h.create_task("backend", "Change the session cookie").await;
    h.call(
        "review",
        "require_human_review",
        json!({"task": marked_task, "reviewer": HUMAN, "reason": "touches auth"}),
    )
    .await;
    let marked_work = h.start_work("backend", &marked_task).await;
    marked_work.write(&[("src/e.txt", "cookie\n")]);
    let marked = h
        .propose("backend", &marked_work, "Change the session cookie")
        .await;
    // The approvals come in another order than the numbers: the queue follows the numbers, as the job does.
    for id in [&third, &marked, &first, &waiting, &second] {
        approved(&h, id).await;
    }

    let mut ahead = Value::Null;
    for (position, id) in [&first, &second, &third].into_iter().enumerate() {
        let read = proposal(&h, id).await;
        assert_eq!(read["queue"]["position"], position + 1, "{read}");
        assert_eq!(read["queue"]["ahead"], ahead, "{read}");
        assert_eq!(read["queue"]["integrating_now"], false, "{read}");
        assert_eq!(
            read["queue"]["estimated_start"],
            Value::Null,
            "no integration run to average yet"
        );
        assert!(read.get("waits_for").is_none(), "{read}");
        ahead = json!(id);
    }
    let read = proposal(&h, &waiting).await;
    assert!(read.get("queue").is_none(), "{read}");
    assert_eq!(read["waits_for"]["reason"], "prerequisites", "{read}");
    assert_eq!(read["waits_for"]["prerequisites"][0]["task"], first_task);
    let read = proposal(&h, &marked).await;
    assert!(read.get("queue").is_none(), "{read}");
    assert_eq!(read["waits_for"]["reason"], "human_review", "{read}");
    assert_eq!(read["waits_for"]["reviewer"], HUMAN);

    let listed = h
        .call("review", "list_proposals", json!({"state": "open"}))
        .await;
    for line in listed["proposals"].as_array().unwrap() {
        let queued = line.get("queue").is_some();
        let waits = line.get("waits_for").is_some();
        assert!(
            queued != waits,
            "exactly one of a place or a reason: {line}"
        );
    }
    let positions: Vec<(Value, Value)> = listed["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|line| line.get("queue").is_some())
        .map(|line| (line["id"].clone(), line["queue"]["position"].clone()))
        .collect();
    assert_eq!(
        positions,
        [
            (json!(first), json!(1)),
            (json!(second), json!(2)),
            (json!(third), json!(3)),
        ]
    );

    let integrate = Running::start(&h.state);
    integrate.cycles(1).await;
    eventually(WAIT, async || {
        let read = proposal(&h, &third).await;
        if read["state"] == "integrated" {
            Ok(())
        } else {
            Err(read)
        }
    })
    .await;
    let integrated: Vec<String> = h
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "proposal.integrated")
        .filter_map(|event| event.proposal)
        .collect();
    assert_eq!(
        &integrated[..3],
        [first.as_str(), second.as_str(), third.as_str()],
        "the job takes them in the order of their places"
    );
    integrate.stop().await;
}
