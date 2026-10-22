#![cfg(test)]

use std::sync::Arc;
use std::time::Duration;

use crate::common::roles::give_role;
use fold_core::object::ObjectKind;
use fold_core::requests::{DraftCard, DraftEdge, RequestKind, RequestState, Requester};
use fold_core::roles::Role;
use fold_core::{
    Actor, AgentActor, HumanName, RequestId, TaskId, TaskStatus, Timestamp, WorkerName, codes,
};
use fold_daemon::services::events::EventBus;
use fold_daemon::services::tasks::{
    NewRequestAttachment, RequestFilter, SubmitDraft, TaskError, TaskService,
};
use fold_store::testing::{TestProject, execute};
use fold_vcs::objects::ObjectStore;
use serde_json::{Value, json};
use sqlx::Row;

const LEASE_MINUTES: u64 = 30;
const LEASE: Duration = Duration::from_secs(LEASE_MINUTES * 60);

fn agent(worker: &str) -> Actor {
    let worker = WorkerName::parse(worker).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::new(),
    })
}

fn human(name: &str) -> Actor {
    Actor::human(HumanName::parse(name).unwrap())
}

fn rq(number: u32) -> RequestId {
    RequestId::from_number(number).unwrap()
}

fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

fn code(error: &TaskError) -> &'static str {
    error.code().unwrap()
}

fn details(error: &TaskError) -> Value {
    match error {
        TaskError::Fold(error) => Value::Object(error.details().cloned().unwrap_or_default()),
        other => panic!("not a refusal: {other}"),
    }
}

fn draft(cards: &[&str], edges: &[(&str, &str)]) -> SubmitDraft {
    SubmitDraft {
        cards: cards
            .iter()
            .map(|key| DraftCard {
                key: (*key).to_owned(),
                title: format!("Build {key}"),
                description: format!("**Scope** the {key} part"),
                label: None,
            })
            .collect(),
        edges: edges
            .iter()
            .map(|(prerequisite, dependent)| DraftEdge {
                prerequisite: (*prerequisite).to_owned(),
                dependent: (*dependent).to_owned(),
            })
            .collect(),
        note: None,
    }
}

fn attachment(name: &str, bytes: &[u8]) -> NewRequestAttachment {
    NewRequestAttachment {
        name: name.to_owned(),
        media_type: "text/plain".to_owned(),
        bytes: bytes.to_vec(),
    }
}

struct Fixture {
    project: TestProject,
    tasks: TaskService,
}

/// Agents `frontend`, `backend` and `review`, each with its worker. `frontend`, `backend` and `review` lead the areas `front`, `work` and `checks`, so each may claim a request. Nobody has a session.
impl Fixture {
    async fn new() -> Self {
        let project = TestProject::new().await;
        let tasks = TaskService::new(
            project.store().clone(),
            EventBus::new(),
            Arc::new(project.clock().clone()),
            LEASE_MINUTES,
        )
        .with_objects(project.paths(), Arc::new(tokio::sync::RwLock::new(())));
        execute(
            project.store(),
            "INSERT INTO agents (id, name, created_at) VALUES (1, 'frontend', 0), (2, 'backend', 0), (3, 'review', 0);
             INSERT INTO workers (id, agent_id, name, created_at) VALUES
                 (1, 1, 'frontend', 0), (3, 2, 'backend', 0), (4, 3, 'review', 0);",
        )
        .await;
        give_role(project.store(), "frontend", Role::AreaLead, Some("front")).await;
        give_role(project.store(), "backend", Role::AreaLead, None).await;
        give_role(project.store(), "review", Role::AreaLead, Some("checks")).await;
        Self { project, tasks }
    }

    fn advance(&self, by: Duration) -> Timestamp {
        self.project.clock().advance(by).unwrap()
    }

    async fn create(&self, by: &Actor, body: &str) -> RequestId {
        self.tasks
            .create_request(by, body, Vec::new())
            .await
            .unwrap()
            .id
    }

    async fn claimed(&self, by: &Actor) -> RequestId {
        let id = self.create(by, "The export button does nothing").await;
        self.tasks
            .claim_request(
                &agent("frontend"),
                id,
                RequestKind::Bug,
                "Export does nothing",
            )
            .await
            .unwrap();
        id
    }

    /// `(seq, type, data, created_at)` of every event after `after`, in order.
    async fn events_after(&self, after: i64) -> Vec<(i64, String, Value, i64)> {
        sqlx::query("SELECT seq, type, data, created_at FROM events WHERE seq > ? ORDER BY seq")
            .bind(after)
            .fetch_all(self.project.store().read())
            .await
            .unwrap()
            .into_iter()
            .map(|row| {
                let data: String = row.get(2);
                (
                    row.get(0),
                    row.get(1),
                    serde_json::from_str(&data).unwrap(),
                    row.get(3),
                )
            })
            .collect()
    }

    async fn last_seq(&self) -> i64 {
        self.count("SELECT COALESCE(MAX(seq), 0) FROM events").await
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .fetch_one(self.project.store().read())
            .await
            .unwrap()
    }
}

fn types(events: &[(i64, String, Value, i64)]) -> Vec<&str> {
    events.iter().map(|event| event.1.as_str()).collect()
}

#[tokio::test]
async fn a_human_creates_a_request_with_an_attachment_and_a_mention() {
    let fx = Fixture::new().await;
    let before = fx.last_seq().await;
    let created = fx
        .tasks
        .create_request(
            &human("tanim"),
            "Export fails on big boards, see #T-12 and @web/src/board.ts.\nThe log is attached.",
            vec![attachment("export.log", b"line one\nline two\n")],
        )
        .await
        .unwrap();
    assert_eq!(created.id, rq(1));
    assert_eq!(created.state, RequestState::Open);
    assert_eq!((created.kind, created.title.as_deref()), (None, None));
    assert_eq!(created.requester.label(), "human:tanim");
    assert_eq!(created.drafter, None);
    assert_eq!(created.latest_version, None);
    assert_eq!(
        created.body,
        "Export fails on big boards, see #T-12 and @web/src/board.ts.\nThe log is attached.",
        "mentions stay text and name nothing that must exist"
    );

    let detail = fx.tasks.get_request(rq(1)).await.unwrap();
    assert_eq!(detail.attachments.len(), 1);
    let stored = &detail.attachments[0];
    assert_eq!(
        (
            stored.position,
            stored.name.as_str(),
            stored.media_type.as_str(),
            stored.size
        ),
        (1, "export.log", "text/plain", 18)
    );
    let objects = ObjectStore::open(&fx.project.paths().fold_dir()).unwrap();
    let blob = objects.read(&stored.object).unwrap();
    assert_eq!(blob.kind, ObjectKind::Blob);
    assert_eq!(blob.payload, b"line one\nline two\n");
    assert!(detail.drafts.is_empty() && detail.comments.is_empty() && detail.tasks.is_empty());

    let events = fx.events_after(before).await;
    assert_eq!(types(&events), ["request.created"]);
    assert_eq!(events[0].2, json!({"request": "RQ-1", "attachments": 1}));

    let by_agent = fx
        .tasks
        .create_request(&agent("backend"), "Is retry safe?", Vec::new())
        .await
        .unwrap();
    assert_eq!(by_agent.id, rq(2));
    assert_eq!(by_agent.requester, Requester::parse("backend").unwrap());
}

#[tokio::test]
async fn creation_checks_the_message_and_the_attachments() {
    let fx = Fixture::new().await;
    let refusal = async |body: &str, attachments: Vec<NewRequestAttachment>| {
        fx.tasks
            .create_request(&human("tanim"), body, attachments)
            .await
            .unwrap_err()
    };
    let reason = |error: &TaskError| {
        assert_eq!(code(error), codes::INVALID_REQUEST);
        details(error)["reason"].clone()
    };
    assert_eq!(reason(&refusal("  ", Vec::new()).await), json!("empty"));
    let eleven = (0..11)
        .map(|n| attachment(&format!("{n}.txt"), b"x"))
        .collect();
    assert_eq!(
        reason(&refusal("hi", eleven).await),
        json!("too_many_attachments")
    );
    let big = vec![attachment("big.bin", &vec![0; 10 * 1024 * 1024 + 1])];
    assert_eq!(
        reason(&refusal("hi", big).await),
        json!("attachment_too_large")
    );
    let named = vec![attachment("../etc/passwd", b"x")];
    let error = refusal("hi", named).await;
    assert_eq!(reason(&error), json!("invalid_attachment_name"));
    assert!(!error.to_string().contains("passwd"));

    let system = fx
        .tasks
        .create_request(&Actor::System, "hi", Vec::new())
        .await
        .unwrap_err();
    assert_eq!(code(&system), codes::HUMAN_REQUIRED);
    assert_eq!(fx.count("SELECT COUNT(*) FROM requests").await, 0);
    assert_eq!(fx.count("SELECT COUNT(*) FROM events").await, 0);

    let largest = vec![attachment("max.bin", &vec![7; 10 * 1024 * 1024])];
    fx.tasks
        .create_request(&human("tanim"), "hi", largest)
        .await
        .unwrap();
}

#[tokio::test]
async fn only_an_orchestrator_claims_and_the_claim_sorts_and_names_the_request() {
    let fx = Fixture::new().await;
    let id = fx
        .create(&human("tanim"), "The export button does nothing")
        .await;
    execute(
        fx.project.store(),
        "INSERT INTO agents (id, name, created_at) VALUES (4, 'docs', 0);
         INSERT INTO workers (id, agent_id, name, created_at) VALUES (5, 4, 'docs', 0);",
    )
    .await;
    // `docs` has neither the `main` tag nor a lead's role, as a session opened by hand.
    for actor in [agent("docs"), human("tanim")] {
        let error = fx
            .tasks
            .claim_request(&actor, id, RequestKind::Bug, "Export")
            .await
            .unwrap_err();
        assert_eq!(code(&error), codes::ORCHESTRATOR_REQUIRED);
    }
    let untitled = fx
        .tasks
        .claim_request(&agent("frontend"), id, RequestKind::Bug, " ")
        .await
        .unwrap_err();
    assert_eq!(code(&untitled), codes::INVALID_REQUEST);

    let before = fx.last_seq().await;
    let claimed = fx
        .tasks
        .claim_request(
            &agent("frontend"),
            id,
            RequestKind::Bug,
            " Export does nothing ",
        )
        .await
        .unwrap();
    assert_eq!(claimed.state, RequestState::Drafting);
    assert_eq!(claimed.kind, Some(RequestKind::Bug));
    assert_eq!(claimed.title.as_deref(), Some("Export does nothing"));
    assert_eq!(
        claimed.drafter.as_ref().map(WorkerName::as_str),
        Some("frontend")
    );
    assert_eq!(claimed.drafter_since, Some(fx.project.now()));
    let events = fx.events_after(before).await;
    assert_eq!(types(&events), ["request.claimed"]);
    assert_eq!(
        events[0].2,
        json!({"request": "RQ-1", "worker": "frontend", "kind": "bug", "taken_over": false})
    );

    let second = fx
        .tasks
        .claim_request(&agent("backend"), id, RequestKind::Feature, "Other")
        .await
        .unwrap_err();
    assert_eq!(code(&second), codes::ALREADY_CLAIMED);
    assert_eq!(
        details(&second),
        json!({"request": "RQ-1", "holder": "frontend"})
    );

    let again = fx
        .tasks
        .claim_request(&agent("frontend"), id, RequestKind::Question, "Renamed")
        .await
        .unwrap();
    assert_eq!(
        again, claimed,
        "a repeat claim by the drafter changes nothing"
    );
    assert_eq!(fx.events_after(before).await.len(), 1);

    let missing = fx
        .tasks
        .claim_request(&agent("frontend"), rq(9), RequestKind::Bug, "x")
        .await
        .unwrap_err();
    assert_eq!(code(&missing), codes::REQUEST_NOT_FOUND);
}

#[tokio::test]
async fn a_drafter_whose_agent_has_no_session_for_the_lease_is_taken_over() {
    let fx = Fixture::new().await;
    let start = fx.project.now().as_millis();
    execute(
        fx.project.store(),
        &format!("INSERT INTO sessions (id, agent_id, pid, started_at, last_call_at) VALUES (1, 1, 100, {start}, {start})"),
    )
    .await;
    let id = fx.claimed(&human("tanim")).await;

    fx.advance(LEASE * 4);
    let busy = fx
        .tasks
        .claim_request(&agent("backend"), id, RequestKind::Feature, "Mine")
        .await
        .unwrap_err();
    assert_eq!(
        code(&busy),
        codes::ALREADY_CLAIMED,
        "an open session keeps the drafter"
    );

    let ended = fx.project.now().as_millis();
    execute(
        fx.project.store(),
        &format!("UPDATE sessions SET ended_at = {ended}, last_call_at = {ended} WHERE id = 1"),
    )
    .await;
    fx.advance(LEASE - Duration::from_secs(1));
    let early = fx
        .tasks
        .claim_request(&agent("backend"), id, RequestKind::Feature, "Mine")
        .await
        .unwrap_err();
    assert_eq!(code(&early), codes::ALREADY_CLAIMED);

    let at = fx.advance(Duration::from_secs(1));
    let before = fx.last_seq().await;
    let taken = fx
        .tasks
        .claim_request(&agent("backend"), id, RequestKind::Feature, "Mine")
        .await
        .unwrap();
    assert_eq!(
        taken.drafter.as_ref().map(WorkerName::as_str),
        Some("backend")
    );
    assert_eq!(taken.drafter_since, Some(at));
    assert_eq!(taken.state, RequestState::Drafting);
    assert_eq!(taken.kind, Some(RequestKind::Feature));
    let events = fx.events_after(before).await;
    assert_eq!(
        events[0].2,
        json!({"request": "RQ-1", "worker": "backend", "kind": "feature", "taken_over": true, "from": "frontend"})
    );
}

#[tokio::test]
async fn drafts_changes_edits_and_approval_follow_the_requester() {
    let fx = Fixture::new().await;
    let id = fx.claimed(&human("tanim")).await;

    let v1 = fx
        .tasks
        .submit_request_draft(&agent("frontend"), id, draft(&["c1"], &[]), None, None)
        .await
        .unwrap();
    assert_eq!(
        (v1.state, v1.latest_version),
        (RequestState::DraftReady, Some(1))
    );

    for actor in [agent("backend"), human("ana"), agent("frontend")] {
        let error = fx
            .tasks
            .request_draft_changes(&actor, id, 1, "split it")
            .await
            .unwrap_err();
        assert_eq!(code(&error), codes::REQUESTER_REQUIRED);
    }
    let not_drafter = fx
        .tasks
        .submit_request_draft(&agent("backend"), id, draft(&["c1"], &[]), None, None)
        .await
        .unwrap_err();
    assert_eq!(code(&not_drafter), codes::DRAFTER_REQUIRED);

    let before = fx.last_seq().await;
    let changes = fx
        .tasks
        .request_draft_changes(&human("tanim"), id, 1, "Split the API from the screen")
        .await
        .unwrap();
    assert_eq!(changes.state, RequestState::ChangesRequested);
    assert_eq!(
        fx.events_after(before).await[0].2,
        json!({"request": "RQ-1", "version": 1})
    );

    let v2 = fx
        .tasks
        .submit_request_draft(
            &agent("frontend"),
            id,
            draft(&["api", "screen"], &[("api", "screen")]),
            Some(RequestKind::Feature),
            Some("Export large boards"),
        )
        .await
        .unwrap();
    assert_eq!(
        (v2.state, v2.latest_version),
        (RequestState::DraftReady, Some(2))
    );
    assert_eq!(v2.kind, Some(RequestKind::Feature));
    assert_eq!(v2.title.as_deref(), Some("Export large boards"));

    let edit = fx
        .tasks
        .edit_request_draft(&agent("backend"), id, draft(&["api"], &[]))
        .await
        .unwrap_err();
    assert_eq!(code(&edit), codes::REQUESTER_REQUIRED);
    let before = fx.last_seq().await;
    let mut edited = draft(&["api", "screen"], &[("api", "screen")]);
    edited.cards[1].title = "Build the export screen".to_owned();
    edited.note = Some("renamed the screen card".to_owned());
    let v3 = fx
        .tasks
        .edit_request_draft(&human("tanim"), id, edited)
        .await
        .unwrap();
    assert_eq!(
        (v3.state, v3.latest_version),
        (RequestState::DraftReady, Some(3))
    );
    let events = fx.events_after(before).await;
    assert_eq!(
        events[0].2,
        json!({"request": "RQ-1", "version": 3, "role": "requester", "cards": 2, "edges": 1})
    );

    let stale = fx
        .tasks
        .approve_request_draft(&human("tanim"), id, 2)
        .await
        .unwrap_err();
    assert_eq!(code(&stale), codes::STALE_VERSION);
    assert_eq!(details(&stale), json!({"expected": 2, "latest": 3}));
    let not_requester = fx
        .tasks
        .approve_request_draft(&agent("frontend"), id, 3)
        .await
        .unwrap_err();
    assert_eq!(code(&not_requester), codes::REQUESTER_REQUIRED);

    let before = fx.last_seq().await;
    let approved = fx
        .tasks
        .approve_request_draft(&human("tanim"), id, 3)
        .await
        .unwrap();
    assert_eq!(approved.request.state, RequestState::Approved);
    assert_eq!(approved.tasks, vec![t(1), t(2)]);
    let api = fx.tasks.get(t(1)).await.unwrap();
    let screen = fx.tasks.get(t(2)).await.unwrap();
    assert_eq!(
        (api.title.as_str(), api.status),
        ("Build api", TaskStatus::Todo)
    );
    assert_eq!(
        (screen.title.as_str(), screen.status),
        ("Build the export screen", TaskStatus::Backlog)
    );
    assert_eq!(api.created_by, "human:tanim");
    let edges = fx.tasks.edges().await.unwrap();
    assert_eq!(
        (edges.len(), edges[0].task, edges[0].needs),
        (1, t(2), t(1))
    );
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM tasks WHERE request_id = (SELECT id FROM requests WHERE number = 1)").await,
        2
    );

    let events = fx.events_after(before).await;
    let names = types(&events);
    assert_eq!(names.first(), Some(&"task.created"));
    assert_eq!(names.last(), Some(&"request.approved"));
    assert_eq!(
        names.iter().filter(|name| **name == "task.created").count(),
        2
    );
    assert!(names.contains(&"dependency.added"));
    assert!(names.contains(&"task.status_changed"));
    assert!(
        events
            .windows(2)
            .all(|pair| pair[1].0 == pair[0].0 + 1 && pair[1].3 == pair[0].3)
    );
    assert_eq!(
        events.last().unwrap().2,
        json!({"request": "RQ-1", "version": 3, "tasks": ["T-1", "T-2"], "drafter": "frontend"})
    );

    let detail = fx.tasks.get_request(id).await.unwrap();
    assert_eq!(detail.drafts.len(), 3);
    assert_eq!(detail.drafts[0].author, "frontend");
    assert_eq!(detail.drafts[2].author, "human:tanim");
    assert_eq!(
        detail.drafts[2].note.as_deref(),
        Some("renamed the screen card")
    );
    assert_eq!(detail.drafts[1].edges[0].prerequisite, "api");
    assert_eq!(detail.comments.len(), 1);
    assert_eq!(detail.comments[0].body, "Split the API from the screen");
    assert_eq!(detail.tasks, vec![t(1), t(2)]);

    let after = [
        fx.tasks
            .close_request(&human("tanim"), id, "done")
            .await
            .unwrap_err(),
        fx.tasks
            .approve_request_draft(&human("tanim"), id, 3)
            .await
            .unwrap_err(),
        fx.tasks
            .release_request(&agent("frontend"), id)
            .await
            .unwrap_err(),
        fx.tasks
            .claim_request(&agent("backend"), id, RequestKind::Bug, "x")
            .await
            .unwrap_err(),
        fx.tasks
            .edit_request_draft(&human("tanim"), id, draft(&["a"], &[]))
            .await
            .unwrap_err(),
    ];
    for error in after {
        assert_eq!(code(&error), codes::REQUEST_NOT_OPEN);
    }
}

#[tokio::test]
async fn an_approval_that_fails_leaves_nothing_behind() {
    let fx = Fixture::new().await;
    let id = fx.claimed(&agent("backend")).await;
    fx.tasks
        .submit_request_draft(
            &agent("frontend"),
            id,
            draft(&["a", "b"], &[("a", "b")]),
            None,
            None,
        )
        .await
        .unwrap();
    let before = fx.last_seq().await;
    execute(
        fx.project.store(),
        "CREATE TRIGGER refuse_approval BEFORE UPDATE OF state ON requests WHEN NEW.state = 'approved'
         BEGIN SELECT RAISE(ABORT, 'refused'); END",
    )
    .await;
    fx.tasks
        .approve_request_draft(&agent("backend"), id, 1)
        .await
        .unwrap_err();
    assert_eq!(fx.count("SELECT COUNT(*) FROM tasks").await, 0);
    assert_eq!(fx.count("SELECT COUNT(*) FROM dependencies").await, 0);
    assert!(fx.events_after(before).await.is_empty());
    execute(fx.project.store(), "DROP TRIGGER refuse_approval").await;
    let approved = fx
        .tasks
        .approve_request_draft(&agent("backend"), id, 1)
        .await
        .unwrap();
    assert_eq!(approved.tasks.len(), 2);
}

#[tokio::test]
async fn release_close_and_invalid_drafts() {
    let fx = Fixture::new().await;
    let id = fx.claimed(&human("tanim")).await;
    let foreign = fx
        .tasks
        .release_request(&agent("backend"), id)
        .await
        .unwrap_err();
    assert_eq!(code(&foreign), codes::DRAFTER_REQUIRED);
    let released = fx
        .tasks
        .release_request(&agent("frontend"), id)
        .await
        .unwrap();
    assert_eq!(
        (released.state, released.drafter.clone()),
        (RequestState::Open, None)
    );
    assert_eq!(released.kind, Some(RequestKind::Bug), "the sorting stays");
    let unheld = fx
        .tasks
        .release_request(&agent("frontend"), id)
        .await
        .unwrap_err();
    assert_eq!(code(&unheld), codes::DRAFTER_REQUIRED);

    fx.tasks
        .claim_request(
            &agent("review"),
            id,
            RequestKind::Bug,
            "Export does nothing",
        )
        .await
        .unwrap();
    let invalid = fx
        .tasks
        .submit_request_draft(&agent("review"), id, draft(&[], &[]), None, None)
        .await
        .unwrap_err();
    assert_eq!(code(&invalid), codes::INVALID_DRAFT);
    assert_eq!(details(&invalid), json!({"reason": "empty"}));
    let cycle = fx
        .tasks
        .submit_request_draft(
            &agent("review"),
            id,
            draft(&["a", "b"], &[("a", "b"), ("b", "a")]),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(code(&cycle), codes::DEPENDENCY_CYCLE);
    let early = fx
        .tasks
        .approve_request_draft(&human("tanim"), id, 1)
        .await
        .unwrap_err();
    assert_eq!(details(&early)["reason"], json!("state"));
    fx.tasks
        .submit_request_draft(&agent("review"), id, draft(&["a"], &[]), None, None)
        .await
        .unwrap();
    fx.tasks
        .request_draft_changes(&human("tanim"), id, 1, "more detail")
        .await
        .unwrap();
    let kept = fx
        .tasks
        .release_request(&agent("review"), id)
        .await
        .unwrap();
    assert_eq!(
        (kept.state, kept.drafter),
        (RequestState::ChangesRequested, None)
    );
    let next = fx
        .tasks
        .claim_request(
            &agent("backend"),
            id,
            RequestKind::Bug,
            "Export does nothing",
        )
        .await
        .unwrap();
    assert_eq!(
        (next.state, next.drafter.as_ref().map(WorkerName::as_str)),
        (RequestState::ChangesRequested, Some("backend")),
        "the next drafter owes the requester the revision"
    );
    let revised = fx
        .tasks
        .submit_request_draft(&agent("backend"), id, draft(&["a", "b"], &[]), None, None)
        .await
        .unwrap();
    assert_eq!(
        (revised.state, revised.latest_version),
        (RequestState::DraftReady, Some(2))
    );

    let closer = fx
        .tasks
        .close_request(&agent("review"), id, "dup")
        .await
        .unwrap_err();
    assert_eq!(code(&closer), codes::REQUESTER_REQUIRED);
    let before = fx.last_seq().await;
    let closed = fx
        .tasks
        .close_request(&human("tanim"), id, "Fixed elsewhere")
        .await
        .unwrap();
    assert_eq!(closed.state, RequestState::Closed);
    assert_eq!(closed.close_reason.as_deref(), Some("Fixed elsewhere"));
    assert_eq!(
        fx.events_after(before).await[0].2,
        json!({"request": "RQ-1"})
    );
    let reopened = fx
        .tasks
        .claim_request(&agent("review"), id, RequestKind::Bug, "x")
        .await
        .unwrap_err();
    assert_eq!(code(&reopened), codes::REQUEST_NOT_OPEN);
}

#[tokio::test]
async fn requests_list_by_state_kind_and_requester() {
    let fx = Fixture::new().await;
    fx.claimed(&human("tanim")).await;
    fx.create(&agent("backend"), "Why is sync slow?").await;
    let all = fx
        .tasks
        .list_requests(&RequestFilter::default())
        .await
        .unwrap();
    let ids: Vec<RequestId> = all.iter().map(|request| request.id).collect();
    assert_eq!(ids, vec![rq(2), rq(1)]);
    let open = RequestFilter {
        state: Some(RequestState::Open),
        ..RequestFilter::default()
    };
    assert_eq!(fx.tasks.list_requests(&open).await.unwrap()[0].id, rq(2));
    let bugs = RequestFilter {
        kind: Some(RequestKind::Bug),
        requester: Requester::parse("human:tanim"),
        ..RequestFilter::default()
    };
    assert_eq!(fx.tasks.list_requests(&bugs).await.unwrap().len(), 1);
    let mine = RequestFilter {
        requester: Requester::parse("review"),
        ..RequestFilter::default()
    };
    assert!(fx.tasks.list_requests(&mine).await.unwrap().is_empty());
    let missing = fx.tasks.get_request(rq(7)).await.unwrap_err();
    assert_eq!(code(&missing), codes::REQUEST_NOT_FOUND);
}
