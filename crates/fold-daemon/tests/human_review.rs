use std::sync::Arc;
use std::time::Duration;

use fold_core::object::ObjectKind;
use fold_core::reviews::{HumanReviewMark, Verdict};
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentActor, AgentName, Event, EventActor, EventFilter, FoldError, HumanName, ProposalId,
    Tag, TaskDescription, TaskId, TaskStatus, TaskTitle, WorkerName, codes,
};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::events::EventBus;
use fold_daemon::services::projects::ensure_human;
use fold_daemon::services::reviews::{AwaitingHumanReview, ReviewError, ReviewService};
use fold_daemon::services::tasks::{self, AttentionKind, CreateTask, TaskError, TaskService};
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::events;
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::testing::TestProject;
use fold_store::workspaces::{create_initial_revision, initial_revision};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use serde_json::json;

const LEASE_MINUTES: u64 = 5;
const REASON: &str = "touches auth";

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn p(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
}

#[cfg(test)]
fn name(text: &str) -> HumanName {
    HumanName::parse(text).unwrap()
}

#[cfg(test)]
fn agent(worker: &str, tags: &[&str]) -> Actor {
    let worker = WorkerName::parse(worker).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: tags.iter().map(|tag| Tag::parse(tag).unwrap()).collect(),
    })
}

#[cfg(test)]
fn planner() -> Actor {
    agent("planner", &[])
}

#[cfg(test)]
fn human(text: &str) -> Actor {
    Actor::human(name(text))
}

#[cfg(test)]
fn refusal<T: std::fmt::Debug>(result: Result<T, ReviewError>) -> FoldError {
    match result {
        Err(ReviewError::Fold(error) | ReviewError::Task(TaskError::Fold(error))) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[cfg(test)]
struct Fixture {
    project: TestProject,
    tasks: TaskService,
    reviews: ReviewService,
}

#[cfg(test)]
impl Fixture {
    /// Tasks 1 and 2 by `planner`; `frontend` holds task 1 and has opened `P-1` on it. The registry knows `tanim` and `ana`.
    async fn new() -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let mut tree = FlatTree::new();
        let hash = objects.write(ObjectKind::Blob, b"pub fn x() {}\n").unwrap();
        tree.insert(
            b"src/lib.rs".to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash,
            },
        );
        let tree = write_initial_tree(&objects, tree).unwrap();
        let revision = initial_revision(tree, project.now()).unwrap();
        let stored = write_revision(&objects, &revision).unwrap();
        create_initial_revision(project.store(), &stored, None)
            .await
            .unwrap();
        for text in ["tanim", "ana"] {
            ensure_human(project.registry(), project.clock(), &name(text))
                .await
                .unwrap();
        }
        let agents = AgentService::new(project.store().clone(), bus.clone(), clock.clone());
        let tasks = TaskService::new(
            project.store().clone(),
            bus.clone(),
            clock.clone(),
            LEASE_MINUTES,
        );
        let workspaces = WorkspaceService::new(
            project.store().clone(),
            bus.clone(),
            clock.clone(),
            project.paths().clone(),
            LEASE_MINUTES,
        );
        let reviews = ReviewService::new(
            project.store().clone(),
            bus,
            clock,
            workspaces.clone(),
            Arc::default(),
        )
        .with_registry(project.registry().clone());
        for title in ["auth", "docs"] {
            tasks
                .create_task(
                    &planner(),
                    CreateTask {
                        title: TaskTitle::parse(title).unwrap(),
                        description: TaskDescription::new(""),
                        labels: Vec::new(),
                    },
                )
                .await
                .unwrap();
        }
        for name in ["frontend", "review", "planner"] {
            let mut ctx = agents.start_session(1).await.unwrap();
            agents
                .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
                .await
                .unwrap();
        }
        agents
            .set_integrator(&human("tanim"), &AgentName::parse("review").unwrap(), true)
            .await
            .unwrap();
        let author = agent("frontend", &[]);
        workspaces.create_workspace(&author, t(1)).await.unwrap();
        let file = project.paths().root().join(".fold/ws/t-1/src/lib.rs");
        std::fs::write(file, "pub fn y() {}\n").unwrap();
        reviews
            .create_proposal(&author, t(1), "Add auth", None)
            .await
            .unwrap();
        Self {
            project,
            tasks,
            reviews,
        }
    }

    async fn mark(&self, actor: &Actor, task: TaskId, reviewer: Option<&str>) -> HumanReviewMark {
        self.reviews
            .require_human_review(actor, task, reviewer.map(name), REASON)
            .await
            .unwrap()
    }

    async fn review(&self, actor: &Actor, verdict: Verdict) {
        self.project
            .clock()
            .advance(Duration::from_secs(1))
            .unwrap();
        self.reviews
            .submit_review(actor, p(1), verdict, "looked", Vec::new())
            .await
            .unwrap();
    }

    async fn events_after(&self, seq: u64) -> Vec<Event> {
        events::read_after(
            self.project.store().read(),
            seq,
            &EventFilter::default(),
            1000,
        )
        .await
        .unwrap()
    }

    async fn head(&self) -> u64 {
        events::head_seq(self.project.store().read()).await.unwrap()
    }

    async fn awaiting(&self) -> Vec<AwaitingHumanReview> {
        self.reviews.awaiting_human_review().await.unwrap()
    }

    async fn card_attention(&self, task: TaskId) -> Vec<AttentionKind> {
        let board = self.tasks.board().await.unwrap();
        let card = board
            .columns
            .iter()
            .flat_map(|column| &column.cards)
            .find(|card| card.id == task)
            .unwrap();
        card.attention.iter().map(|item| item.kind).collect()
    }
}

#[tokio::test]
async fn agent_sets_the_mark_and_every_view_shows_it() {
    let fx = Fixture::new().await;
    let before = fx.head().await;
    let mark = fx.mark(&planner(), t(1), Some("tanim")).await;
    let expected = HumanReviewMark {
        reviewer: Some(name("tanim")),
        reason: REASON.to_owned(),
        set_by: EventActor::parse("planner").unwrap(),
    };
    assert_eq!(mark, expected);

    let task = fx.tasks.get(t(1)).await.unwrap();
    assert_eq!(task.human_review.as_ref(), Some(&expected));
    assert_eq!(task.updated_at, fx.project.now());
    let detail = fx.tasks.get_view(t(1)).await.unwrap();
    assert_eq!(detail.task.human_review.as_ref(), Some(&expected));
    let listed = fx.tasks.list(false).await.unwrap();
    assert_eq!(listed[0].human_review.as_ref(), Some(&expected));
    assert_eq!(listed[1].human_review, None);
    let board = fx.tasks.board().await.unwrap();
    let card = board
        .columns
        .iter()
        .flat_map(|column| &column.cards)
        .find(|card| card.id == t(1))
        .unwrap();
    assert_eq!(card.human_review.as_ref(), Some(&expected));

    let written = fx.events_after(before).await;
    assert_eq!(written.len(), 1, "{written:?}");
    let event = &written[0];
    assert_eq!(event.event_type.as_str(), "task.human_review_set");
    assert_eq!(event.actor.as_str(), "planner");
    assert_eq!(event.task, Some(t(1)));
    assert_eq!(
        event.data,
        json!({"reviewer": "tanim", "reason": REASON, "set_by": "planner"})
    );
    assert_eq!(event.created_at, task.updated_at);

    let any = fx
        .reviews
        .require_human_review(&planner(), t(2), None, "  needs a look  ")
        .await
        .unwrap();
    assert_eq!(any.reviewer, None);
    assert_eq!(any.reason, "needs a look");
}

#[tokio::test]
async fn agent_cannot_clear_the_mark() {
    let fx = Fixture::new().await;
    fx.mark(&planner(), t(1), None).await;
    let before = fx.head().await;
    for actor in [planner(), agent("review", &["integrator"]), Actor::System] {
        let error = refusal(fx.reviews.clear_human_review(&actor, t(1)).await);
        assert_eq!(error.code(), codes::HUMAN_REQUIRED, "{actor:?}");
        assert_eq!(error.details().unwrap()["task"], json!("T-1"));
    }
    assert!(fx.tasks.get(t(1)).await.unwrap().human_review.is_some());
    assert_eq!(fx.head().await, before);
}

#[tokio::test]
async fn agent_cannot_change_an_existing_mark() {
    let fx = Fixture::new().await;
    let first = fx.mark(&planner(), t(1), Some("tanim")).await;
    let before = fx.head().await;

    let same = fx
        .reviews
        .require_human_review(&agent("review", &[]), t(1), Some(name("tanim")), REASON)
        .await
        .unwrap();
    assert_eq!(same, first, "a repeat keeps the first setter");
    assert_eq!(fx.head().await, before, "a repeat appends no event");

    for (reviewer, reason) in [
        (Some(name("ana")), REASON),
        (None, REASON),
        (Some(name("tanim")), "another reason"),
    ] {
        let error = refusal(
            fx.reviews
                .require_human_review(&planner(), t(1), reviewer, reason)
                .await,
        );
        assert_eq!(error.code(), codes::HUMAN_REVIEW_ALREADY_SET);
    }
    assert_eq!(fx.tasks.get(t(1)).await.unwrap().human_review, Some(first));
    assert_eq!(fx.head().await, before);

    let changed = fx
        .reviews
        .require_human_review(&human("ana"), t(1), Some(name("ana")), "ana knows auth")
        .await
        .unwrap();
    assert_eq!(changed.reviewer, Some(name("ana")));
    assert_eq!(changed.set_by.as_str(), "human:ana");
    let written = fx.events_after(before).await;
    assert_eq!(written.len(), 1);
    assert_eq!(
        written[0].data,
        json!({"reviewer": "ana", "reason": "ana knows auth", "set_by": "human:ana"})
    );
}

#[tokio::test]
async fn human_clears_and_the_event_keeps_the_old_values() {
    let fx = Fixture::new().await;
    fx.mark(&planner(), t(1), Some("tanim")).await;
    let before = fx.head().await;
    let previous = fx
        .reviews
        .clear_human_review(&human("ana"), t(1))
        .await
        .unwrap();
    assert_eq!(previous.unwrap().reviewer, Some(name("tanim")));
    let task = fx.tasks.get(t(1)).await.unwrap();
    assert_eq!(task.human_review, None);
    let written = fx.events_after(before).await;
    assert_eq!(written.len(), 1, "{written:?}");
    assert_eq!(written[0].event_type.as_str(), "task.human_review_cleared");
    assert_eq!(written[0].actor.as_str(), "human:ana");
    assert_eq!(written[0].task, Some(t(1)));
    assert_eq!(
        written[0].data,
        json!({"reviewer": "tanim", "reason": REASON})
    );
    assert_eq!(written[0].created_at, task.updated_at);
    let raw: (i64, Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT human_review, human_reviewer, human_review_reason, human_review_set_by FROM tasks WHERE number = 1",
    )
    .fetch_one(fx.project.store().read())
    .await
    .unwrap();
    assert_eq!(raw, (0, None, None, None));

    let again = fx
        .reviews
        .clear_human_review(&human("ana"), t(1))
        .await
        .unwrap();
    assert_eq!(again, None);
    assert_eq!(fx.events_after(before).await.len(), 1);
}

#[tokio::test]
async fn unknown_reviewer_is_refused() {
    let fx = Fixture::new().await;
    let before = fx.head().await;
    let error = refusal(
        fx.reviews
            .require_human_review(&planner(), t(1), Some(name("nobody")), REASON)
            .await,
    );
    assert_eq!(error.code(), codes::UNKNOWN_HUMAN);
    assert_eq!(error.details().unwrap()["reviewer"], json!("nobody"));
    assert_eq!(fx.tasks.get(t(1)).await.unwrap().human_review, None);
    assert_eq!(fx.head().await, before);

    for reason in ["", "   ", &"x".repeat(501)] {
        let error = refusal(
            fx.reviews
                .require_human_review(&planner(), t(1), None, reason)
                .await,
        );
        assert_eq!(error.code(), codes::INVALID_REQUEST);
        assert_eq!(error.details().unwrap()["field"], json!("reason"));
    }

    let missing = refusal(
        fx.reviews
            .require_human_review(&planner(), t(9), None, REASON)
            .await,
    );
    assert_eq!(missing.code(), codes::TASK_NOT_FOUND);
    let system = refusal(
        fx.reviews
            .require_human_review(&Actor::System, t(1), None, REASON)
            .await,
    );
    assert_eq!(system.code(), codes::HUMAN_REQUIRED);

    let bus = EventBus::new();
    let clock = Arc::new(fx.project.clock().clone());
    let workspaces = WorkspaceService::new(
        fx.project.store().clone(),
        bus.clone(),
        clock.clone(),
        fx.project.paths().clone(),
        LEASE_MINUTES,
    );
    let without_registry = ReviewService::new(
        fx.project.store().clone(),
        bus,
        clock,
        workspaces,
        Arc::default(),
    );
    let error = refusal(
        without_registry
            .require_human_review(&planner(), t(1), Some(name("tanim")), REASON)
            .await,
    );
    assert_eq!(error.code(), codes::UNKNOWN_HUMAN);
}

#[tokio::test]
async fn closed_task_is_refused_with_task_closed() {
    let fx = Fixture::new().await;
    fx.mark(&planner(), t(2), None).await;
    let now = fx.project.now();
    fx.project
        .store()
        .write(async |tx| {
            completions::insert(tx, t(2), CompletionKind::Children, None, now).await?;
            tasks::recompute(tx, &[t(2)], now).await?;
            Ok::<_, TaskError>(())
        })
        .await
        .unwrap();
    assert_eq!(fx.tasks.get(t(2)).await.unwrap().status, TaskStatus::Done);
    let before = fx.head().await;
    let done = refusal(
        fx.reviews
            .require_human_review(&planner(), t(2), None, "again")
            .await,
    );
    assert_eq!(done.code(), codes::TASK_CLOSED);
    assert_eq!(done.details().unwrap()["reason"], json!("done"));
    let cleared = refusal(fx.reviews.clear_human_review(&human("ana"), t(2)).await);
    assert_eq!(cleared.code(), codes::TASK_CLOSED);
    assert_eq!(fx.head().await, before);

    fx.tasks
        .create_task(
            &planner(),
            CreateTask {
                title: TaskTitle::parse("old").unwrap(),
                description: TaskDescription::new(""),
                labels: Vec::new(),
            },
        )
        .await
        .unwrap();
    fx.tasks.archive_task(&planner(), t(3)).await.unwrap();
    let before_archived = fx.head().await;
    let archived = refusal(
        fx.reviews
            .require_human_review(&planner(), t(3), None, REASON)
            .await,
    );
    assert_eq!(archived.code(), codes::TASK_CLOSED);
    assert_eq!(archived.details().unwrap()["reason"], json!("archived"));
    assert_eq!(fx.head().await, before_archived);
}

#[tokio::test]
async fn mark_does_not_change_task_status() {
    let fx = Fixture::new().await;
    let review = fx.tasks.get(t(1)).await.unwrap();
    let todo = fx.tasks.get(t(2)).await.unwrap();
    assert_eq!(review.status, TaskStatus::Review);
    assert_eq!(todo.status, TaskStatus::Todo);
    let before = fx.head().await;
    fx.mark(&planner(), t(1), Some("tanim")).await;
    fx.mark(&planner(), t(2), None).await;
    for (task, expected) in [(t(1), &review), (t(2), &todo)] {
        let now = fx.tasks.get(task).await.unwrap();
        assert_eq!(now.status, expected.status);
        assert_eq!(now.blocked, expected.blocked);
    }
    fx.reviews
        .clear_human_review(&human("tanim"), t(1))
        .await
        .unwrap();
    assert_eq!(fx.tasks.get(t(1)).await.unwrap().status, TaskStatus::Review);
    let types: Vec<&str> = fx
        .events_after(before)
        .await
        .iter()
        .map(|event| event.event_type.as_str())
        .collect();
    assert_eq!(
        types,
        [
            "task.human_review_set",
            "task.human_review_set",
            "task.human_review_cleared"
        ]
    );
}

#[tokio::test]
async fn awaiting_human_review_lists_and_clears_after_a_human_approval() {
    let fx = Fixture::new().await;
    assert!(fx.awaiting().await.is_empty(), "no mark, nothing waits");
    let version_at = fx.project.now();
    fx.project.clock().advance(Duration::from_secs(5)).unwrap();
    fx.mark(&planner(), t(1), Some("tanim")).await;
    fx.mark(&planner(), t(2), None).await;
    let marked_at = fx.project.now();
    assert!(marked_at > version_at);
    assert!(
        fx.awaiting().await.is_empty(),
        "the human sees P-1 only once an agent approved it"
    );
    fx.review(&agent("planner", &["integrator"]), Verdict::Approve)
        .await;
    assert!(
        fx.awaiting().await.is_empty(),
        "an agent that no longer holds integrator does not count"
    );

    fx.review(&agent("review", &["integrator"]), Verdict::Approve)
        .await;
    let waiting = fx.awaiting().await;
    assert_eq!(
        waiting,
        [AwaitingHumanReview {
            task: t(1),
            proposal: p(1),
            reviewer: Some(name("tanim")),
            since: marked_at,
        }],
        "an agent approval never counts for the human, and T-2 has no proposal"
    );
    fx.review(&human("ana"), Verdict::Approve).await;
    assert_eq!(
        fx.awaiting().await.len(),
        1,
        "ana is not the named reviewer"
    );

    fx.review(&agent("review", &[]), Verdict::RequestChanges)
        .await;
    assert!(
        fx.awaiting().await.is_empty(),
        "the proposal is with its author"
    );

    fx.review(&human("tanim"), Verdict::Approve).await;
    assert!(
        fx.awaiting().await.is_empty(),
        "no agent approved after the request_changes"
    );

    fx.review(&agent("review", &["integrator"]), Verdict::Approve)
        .await;
    let back = fx.awaiting().await;
    assert_eq!(back.len(), 1, "tanim approved before the agent did");
    assert_eq!(back[0].since, marked_at);
    fx.review(&human("tanim"), Verdict::Approve).await;
    assert!(
        fx.awaiting().await.is_empty(),
        "tanim approved after the agent"
    );

    fx.review(&agent("review", &[]), Verdict::RequestChanges)
        .await;
    fx.review(&agent("review", &["integrator"]), Verdict::Approve)
        .await;
    assert_eq!(
        fx.awaiting().await.len(),
        1,
        "a later request_changes withdrew it"
    );

    fx.reviews
        .clear_human_review(&human("tanim"), t(1))
        .await
        .unwrap();
    assert!(fx.awaiting().await.is_empty(), "the mark is off");
}

#[tokio::test]
async fn board_attention_carries_human_review_for_a_waiting_proposal() {
    let fx = Fixture::new().await;
    assert!(fx.card_attention(t(1)).await.is_empty());
    fx.mark(&planner(), t(1), None).await;
    fx.mark(&planner(), t(2), None).await;
    assert!(fx.card_attention(t(1)).await.is_empty());
    fx.review(&agent("review", &["integrator"]), Verdict::Approve)
        .await;
    assert_eq!(fx.card_attention(t(1)).await, [AttentionKind::HumanReview]);
    assert!(fx.card_attention(t(2)).await.is_empty());
    let detail = fx.tasks.get_view(t(1)).await.unwrap();
    let kinds: Vec<AttentionKind> = detail.attention.iter().map(|item| item.kind).collect();
    assert_eq!(kinds, [AttentionKind::HumanReview]);

    fx.review(&human("ana"), Verdict::Approve).await;
    assert!(fx.card_attention(t(1)).await.is_empty());
    assert!(fx.tasks.get_view(t(1)).await.unwrap().attention.is_empty());
}
