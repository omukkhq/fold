#![cfg(test)]

use fold_core::roles::Role;
use fold_core::{Clock as _, TaskId};
use fold_daemon::services::tasks::TaskError;
use fold_store::tasks::completions::{self, CompletionKind};
use serde_json::{Value, json};

use crate::common::acceptance::{FakeInbox, Harness};
use crate::common::roles::give_role;

const LEAD: &str = "lead";

/// The project with `lead` as the review lead, connected, and its session inbox.
async fn with_review_lead() -> (Harness, FakeInbox) {
    let mut h = Harness::start().await;
    give_role(&h.handle.store, LEAD, Role::ReviewLead, None).await;
    h.connect(LEAD).await;
    let inbox = h.register_inbox(LEAD).await;
    (h, inbox)
}

/// `frontend` asks for a feature and approves its draft of `cards` (key and title), with `edges` of keys (prerequisite, dependent); returns the new task ids in card order.
async fn approved_request(
    h: &Harness,
    cards: &[(&str, &str)],
    edges: &[(&str, &str)],
) -> Vec<String> {
    let created = h
        .call(
            "frontend",
            "create_request",
            json!({"body": "Show a report of the week's work."}),
        )
        .await;
    let request = created["id"].as_str().unwrap().to_owned();
    h.call(
        "frontend",
        "claim_request",
        json!({"id": request, "kind": "feature", "title": "Weekly report"}),
    )
    .await;
    let label = Harness::work_labels_of("frontend")[0].clone();
    let cards: Vec<Value> = cards
        .iter()
        .map(|(key, title)| {
            json!({"key": key, "title": title, "description": "As the request asks.", "label": label})
        })
        .collect();
    let edges: Vec<Value> = edges
        .iter()
        .map(|(prerequisite, dependent)| json!({"prerequisite": prerequisite, "dependent": dependent}))
        .collect();
    h.call(
        "frontend",
        "submit_request_draft",
        json!({"id": request, "cards": cards, "edges": edges}),
    )
    .await;
    let approved = h
        .call(
            "frontend",
            "approve_request_draft",
            json!({"id": request, "expected_version": 1}),
        )
        .await;
    approved["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task.as_str().unwrap().to_owned())
        .collect()
}

/// Marks `task` Done with a completion record, as a finished task has one.
async fn complete(h: &Harness, task: &str) {
    let task: TaskId = task.parse().unwrap();
    let now = h.project.clock().now();
    h.handle
        .store
        .write(async |tx| {
            completions::insert(tx, task, CompletionKind::Children, None, now).await?;
            fold_daemon::services::tasks::recompute(tx, &[task], now).await?;
            Ok::<_, TaskError>(())
        })
        .await
        .unwrap();
}

/// The notice text of the next post that contains `needle`. The post carries it as a JSON string, so it ends at the escaped line break.
async fn notice(inbox: &mut FakeInbox, needle: &str) -> String {
    loop {
        let post = inbox.next().await;
        if let Some(at) = post.find("fold: P-") {
            let line = &post[at..];
            let end = line.find(['"', '\\']).unwrap_or(line.len());
            if line[..end].contains(needle) {
                return line[..end].to_owned();
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_review_lead_hears_which_related_tasks_a_plan_can_wait_for() {
    let (h, mut inbox) = with_review_lead().await;
    let request = approved_request(
        &h,
        &[
            ("api", "Add the report API"),
            ("screen", "Add the report screen"),
            ("tests", "Test the report"),
            ("docs", "Document the report"),
            ("seed", "Seed the report data"),
        ],
        &[("api", "docs")],
    )
    .await;
    let [api, screen, tests, docs, seed] = <[String; 5]>::try_from(request).unwrap();
    let schema = h.create_task("frontend", "Add the report schema").await;
    let parent = h.create_task("frontend", "Ship the weekly report").await;
    let export = h.create_task("frontend", "Add the report export").await;
    let unrelated = h.create_task("frontend", "Fix the login page").await;

    // The screen is under way, the tests wait for a person and the seed data is Done; the docs need the API.
    h.start_work("frontend", &screen).await;
    h.call(
        "frontend",
        "set_blocker",
        json!({"id": tests, "reason": "Wait for the test data"}),
    )
    .await;
    complete(&h, &seed).await;
    let work = h.start_work("frontend", &api).await;
    // The API also needs the schema, an edge added after its work started so that the API can still open its proposal, and one parent links the API and the export.
    h.call(
        "frontend",
        "add_dependency",
        json!({"task": api, "needs": schema}),
    )
    .await;
    for child in [&api, &export] {
        h.call(
            "frontend",
            "add_checklist_item",
            json!({"task": parent, "linked_task": child}),
        )
        .await;
    }
    work.write(&[("src/report_api.txt", "GET /report\n")]);
    let proposal = h.propose("frontend", &work, "Add the report API").await;

    let expected = format!(
        "fold: {proposal} opened for {api}; related {schema} (todo), {export} (todo), {screen} (in_progress) can still open proposals: create_review_plan(proposals[{proposal}], tasks[{schema}, {export}, {screen}]) waits for them. Call create_review_plan."
    );
    assert_eq!(notice(&mut inbox, " opened for ").await, expected);

    let read = h.call(LEAD, "get_proposal", json!({"id": proposal})).await;
    assert_eq!(
        read["related_tasks"],
        json!([
            {"id": schema, "status": "todo", "title": "Add the report schema", "relation": "dependency"},
            {"id": export, "status": "todo", "title": "Add the report export", "relation": "same_parent"},
            {"id": screen, "status": "in_progress", "title": "Add the report screen", "relation": "same_request"},
        ]),
        "{read}"
    );
    assert_eq!(
        read["plan_hint"],
        format!(
            "create_review_plan(proposals[{proposal}], tasks[{schema}, {export}, {screen}]) waits for them"
        )
    );
    for left_out in [&tests, &docs, &seed, &parent, &unrelated] {
        assert!(
            !read.to_string().contains(&format!("\"{left_out}\"")),
            "{left_out}: {read}"
        );
    }

    // fold names only tasks the plan accepts, and once the proposal waits in a plan the list is gone.
    let plan = h
        .call(
            LEAD,
            "create_review_plan",
            json!({"proposals": [proposal], "tasks": [schema, export, screen]}),
        )
        .await;
    assert_eq!(plan["awaited"].as_array().unwrap().len(), 3, "{plan}");
    let planned = h.call(LEAD, "get_proposal", json!({"id": proposal})).await;
    assert!(planned.get("related_tasks").is_none(), "{planned}");
    assert!(planned.get("plan_hint").is_none(), "{planned}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proposal_with_no_related_task_to_come_is_started_at_once() {
    let (h, mut inbox) = with_review_lead().await;
    let request = approved_request(
        &h,
        &[
            ("api", "Add the report API"),
            ("docs", "Document the report"),
        ],
        &[("api", "docs")],
    )
    .await;
    let work = h.start_work("frontend", &request[0]).await;
    work.write(&[("src/report_api.txt", "GET /report\n")]);
    let proposal = h.propose("frontend", &work, "Add the report API").await;
    assert_eq!(
        notice(&mut inbox, " opened for ").await,
        format!(
            "fold: {proposal} opened for {}. Call start_worker.",
            request[0]
        )
    );
    let read = h.call(LEAD, "get_proposal", json!({"id": proposal})).await;
    assert!(read.get("related_tasks").is_none(), "{read}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_version_before_any_review_still_names_the_plan() {
    let (h, mut inbox) = with_review_lead().await;
    let request = approved_request(
        &h,
        &[
            ("api", "Add the report API"),
            ("docs", "Document the report"),
        ],
        &[],
    )
    .await;
    let work = h.start_work("frontend", &request[0]).await;
    work.write(&[("src/report_api.txt", "GET /report\n")]);
    let proposal = h.propose("frontend", &work, "Add the report API").await;
    notice(&mut inbox, " opened for ").await;
    work.write(&[("src/report_api.txt", "GET /report?week=1\n")]);
    h.call(
        "frontend",
        "record_changes",
        json!({"ws": work.workspace, "message": "Take the week"}),
    )
    .await;
    h.call("frontend", "update_proposal", json!({"id": proposal}))
        .await;
    assert_eq!(
        notice(&mut inbox, " has version 2").await,
        format!(
            "fold: {proposal} has version 2; related {} (todo) can still open a proposal: create_review_plan(proposals[{proposal}], tasks[{}]) waits for it. Call create_review_plan.",
            request[1], request[1]
        )
    );
}
