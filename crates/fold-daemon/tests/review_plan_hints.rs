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
    h.call(
        "frontend",
        "update_proposal",
        json!({"intent": "Make the change the task asks for.", "id": proposal}),
    )
    .await;
    assert_eq!(
        notice(&mut inbox, " has version 2").await,
        format!(
            "fold: {proposal} has version 2; related {} (todo) can still open a proposal: create_review_plan(proposals[{proposal}], tasks[{}]) waits for it. Call create_review_plan.",
            request[1], request[1]
        )
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sibling_proposal_that_opened_first_goes_into_the_plan_of_the_next() {
    let (h, mut inbox) = with_review_lead().await;
    let request = approved_request(
        &h,
        &[
            ("api", "Add the report API"),
            ("docs", "Document the report"),
            ("tests", "Test the report"),
        ],
        &[],
    )
    .await;
    let [api, docs, tests] = <[String; 3]>::try_from(request).unwrap();
    let api_work = h.start_work("frontend", &api).await;
    let docs_work = h.start_work("frontend", &docs).await;
    api_work.write(&[("src/report_api.txt", "GET /report\n")]);
    let first = h.propose("frontend", &api_work, "Add the report API").await;
    // The review lead misses this notice, so the API proposal waits for a review in no plan when the docs proposal opens.
    notice(&mut inbox, &format!("{first} opened for ")).await;
    docs_work.write(&[("docs/report.md", "# The report\n")]);
    let second = h
        .propose("frontend", &docs_work, "Document the report")
        .await;
    assert_eq!(
        notice(&mut inbox, &format!("{second} opened for ")).await,
        format!(
            "fold: {second} opened for {docs}; related {first} ({api}) waits for a review, and {tests} (todo) can still open a proposal: create_review_plan(proposals[{second}, {first}], tasks[{tests}]) waits for it. Call create_review_plan."
        )
    );

    // After a restart the lead reads the first proposal and finds the second there too.
    let read = h.call(LEAD, "get_proposal", json!({"id": first})).await;
    assert_eq!(
        read["related_tasks"],
        json!([
            {"id": docs, "status": "review", "title": "Document the report", "relation": "same_request", "proposal": second},
            {"id": tests, "status": "todo", "title": "Test the report", "relation": "same_request"},
        ]),
        "{read}"
    );
    assert_eq!(
        read["plan_hint"],
        format!("create_review_plan(proposals[{first}, {second}], tasks[{tests}]) waits for it")
    );
    let plan = h
        .call(
            LEAD,
            "create_review_plan",
            json!({"proposals": [first, second], "tasks": [tests]}),
        )
        .await;
    assert_eq!(plan["awaited"].as_array().unwrap().len(), 1, "{plan}");
    for proposal in [&first, &second] {
        let planned = h.call(LEAD, "get_proposal", json!({"id": proposal})).await;
        assert_eq!(planned["review_plan"]["id"], plan["id"], "{planned}");
        assert!(planned.get("related_tasks").is_none(), "{planned}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_open_proposal_of_the_same_area_joins_the_review_but_no_plan_waits_for_the_area() {
    let (h, mut inbox) = with_review_lead().await;
    let login = h.create_task("frontend", "Fix the login page").await;
    let header = h.create_task("frontend", "Add the header").await;
    let logo = h.create_task("frontend", "Add the logo").await;
    let login_work = h.start_work("frontend", &login).await;
    login_work.write(&[("src/login.txt", "login\n")]);
    let first = h
        .propose("frontend", &login_work, "Fix the login page")
        .await;
    // The header shares the area and is In progress, but the area alone never makes a plan wait.
    h.start_work("frontend", &header).await;
    assert_eq!(
        notice(&mut inbox, &format!("{first} opened for ")).await,
        format!("fold: {first} opened for {login}. Call start_worker.")
    );
    let logo_work = h.start_work("frontend", &logo).await;
    logo_work.write(&[("src/logo.txt", "logo\n")]);
    let second = h.propose("frontend", &logo_work, "Add the logo").await;
    assert_eq!(
        notice(&mut inbox, &format!("{second} opened for ")).await,
        format!(
            "fold: {second} opened for {logo}; related {first} ({login}) waits for a review: start_worker(role reviewer, proposals[{second}, {first}]) gives them to one reviewer. Call start_worker."
        )
    );
    let read = h.call(LEAD, "get_proposal", json!({"id": second})).await;
    assert_eq!(
        read["related_tasks"],
        json!([
            {"id": login, "status": "review", "title": "Fix the login page", "relation": "same_area", "proposal": first},
        ]),
        "{read}"
    );
    assert!(
        !read.to_string().contains(&format!("\"{header}\"")),
        "{read}"
    );
    // A human's read of the same proposal has no grouping hint: only the review lead groups reviews.
    let human = h
        .handle
        .reviews(h.project.registry())
        .view_proposal(
            &fold_core::Actor::human(fold_core::HumanName::parse("ana").unwrap()),
            second.parse().unwrap(),
            None,
        )
        .await
        .unwrap();
    assert!(human.related_tasks.is_empty());
}
