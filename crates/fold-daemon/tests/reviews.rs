use crate::common::roles::give_role;
use fold_core::roles::Role;
use std::sync::Arc;
use std::time::Duration;

use fold_core::object::ObjectKind;
use fold_core::reviews::{Side, Verdict, changes_requested, latest_verdict};
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentActor, AgentName, CommentId, Event, EventActor, EventFilter, FoldError, HumanName,
    ProposalId, Tag, TaskDescription, TaskId, TaskStatus, TaskTitle, WorkerName, codes,
};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::events::EventBus;
use fold_daemon::services::reviews::{
    CommentInput, CommentLocation, ReviewError, ReviewService, SubmittedReview, Turn,
};
use fold_daemon::services::tasks::{CreateTask, TaskError, TaskService};
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::agents::WorkerRow;
use fold_store::events;
use fold_store::reviews::{self, ProposalState};
use fold_store::testing::TestProject;
use fold_store::workspaces::{create_initial_revision, initial_revision};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use serde_json::json;
use sqlx::AssertSqlSafe;

const LEASE_MINUTES: u64 = 5;
/// `src/lib.rs` in `r1`: one line.
const LIB: &[u8] = b"pub fn x() {}\n";
/// `src/lib.rs` in `v1`: five lines.
const LIB_V1: &str = "pub fn a() {}\npub fn b() {}\npub fn c() {}\npub fn d() {}\npub fn e() {}\n";

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn p(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
}

#[cfg(test)]
fn rc(number: u32) -> CommentId {
    CommentId::from_number(number).unwrap()
}

#[cfg(test)]
struct Fixture {
    project: TestProject,
    tasks: TaskService,
    reviews: ReviewService,
    /// The author: it holds task 1 and opens `P-1`.
    frontend: WorkerRow,
    /// A reviewer without tags.
    backend: WorkerRow,
}

#[cfg(test)]
impl Fixture {
    /// `r1` holds `src/lib.rs` (one line) and `README.md`. Task 1, the agents `frontend`, `backend` and `review`.
    async fn new() -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let mut tree = FlatTree::new();
        for (path, content) in [("src/lib.rs", LIB), ("README.md", b"# demo\n".as_slice())] {
            let hash = objects.write(ObjectKind::Blob, content).unwrap();
            tree.insert(
                path.as_bytes().to_vec(),
                FlatEntry {
                    kind: EntryKind::File,
                    hash,
                },
            );
        }
        let tree = write_initial_tree(&objects, tree).unwrap();
        let revision = initial_revision(tree, project.now()).unwrap();
        let stored = write_revision(&objects, &revision).unwrap();
        create_initial_revision(project.store(), &stored, None)
            .await
            .unwrap();
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
        );
        crate::common::roles::lead_work_areas(project.store()).await;
        tasks
            .create_task(
                &agent("frontend", &[]),
                CreateTask {
                    title: TaskTitle::parse("task").unwrap(),
                    description: TaskDescription::new("body"),
                    labels: crate::common::roles::work_labels(),
                },
            )
            .await
            .unwrap();
        let frontend = register(&agents, "frontend").await;
        let backend = register(&agents, "backend").await;
        register(&agents, "review").await;
        // `backend` leads its area and reviews without the right to decide; `review` decides as a reviewer.
        give_role(project.store(), "review", Role::Reviewer, None).await;
        workspaces
            .create_workspace(&agent("frontend", &[]), t(1))
            .await
            .unwrap();
        let fx = Self {
            project,
            tasks,
            reviews,
            frontend,
            backend,
        };
        fx.write("src/lib.rs", LIB_V1);
        fx.write("src/new.rs", "pub fn z() {}\npub fn w() {}\n");
        fx.write("logo.bin", "\0\u{1}\u{2}\0");
        fx.reviews
            .create_proposal(&fx.author(), t(1), "Add the report", None)
            .await
            .unwrap()
            .detail()
            .unwrap();
        fx
    }

    fn author(&self) -> Actor {
        by(&self.frontend)
    }

    fn reviewer(&self) -> Actor {
        by(&self.backend)
    }

    fn write(&self, path: &str, content: &str) {
        let file = self.project.paths().root().join(".fold/ws/t-1").join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }

    /// `v<n+1>` of `P-1` with `src/lib.rs` changed again.
    async fn new_version(&self, content: &str) {
        self.write("src/lib.rs", content);
        self.reviews
            .update_proposal(&self.author(), p(1), None)
            .await
            .unwrap()
            .detail()
            .unwrap();
    }

    /// Reviews the version of `P-1` that is current when the call starts, as a reviewer that has just read it.
    async fn submit(
        &self,
        actor: &Actor,
        verdict: Verdict,
        body: &str,
        comments: Vec<CommentInput>,
    ) -> Result<SubmittedReview, ReviewError> {
        self.project
            .clock()
            .advance(Duration::from_secs(1))
            .unwrap();
        let version = self.current().await;
        self.reviews
            .submit_review(actor, p(1), version, verdict, body, comments)
            .await
    }

    async fn current(&self) -> u32 {
        self.reviews
            .get_proposal(p(1), None)
            .await
            .unwrap()
            .summary
            .current_version
    }

    async fn status(&self) -> TaskStatus {
        self.tasks.get(t(1)).await.unwrap().status
    }

    async fn turn(&self) -> Option<Turn> {
        self.reviews
            .get_proposal(p(1), None)
            .await
            .unwrap()
            .summary
            .turn
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

    async fn last_seq(&self) -> u64 {
        self.events_after(0)
            .await
            .last()
            .map_or(0, |event| event.seq)
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(AssertSqlSafe(sql))
            .fetch_one(self.project.store().read())
            .await
            .unwrap()
    }
}

#[cfg(test)]
fn by(worker: &WorkerRow) -> Actor {
    Actor::Agent(AgentActor {
        agent: worker.name.agent(),
        worker: worker.name.clone(),
        tags: Vec::new(),
    })
}

#[cfg(test)]
fn agent(name: &str, tags: &[&str]) -> Actor {
    let worker = WorkerName::parse(name).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: tags.iter().map(|tag| Tag::parse(tag).unwrap()).collect(),
    })
}

#[cfg(test)]
fn reviewer() -> Actor {
    agent("review", &[])
}

#[cfg(test)]
fn human() -> Actor {
    Actor::human(HumanName::parse("tanim").unwrap())
}

#[cfg(test)]
async fn register(agents: &AgentService, name: &str) -> WorkerRow {
    let mut ctx = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
        .await
        .unwrap();
    agents.resolve_worker(&ctx).await.unwrap()
}

#[cfg(test)]
fn lines(path: &str, side: Side, start_line: u32, end_line: u32) -> CommentInput {
    CommentInput {
        location: Some(CommentLocation {
            path: path.to_owned(),
            side,
            start_line,
            end_line,
        }),
        reply_to: None,
        body: format!("about {path}"),
    }
}

#[cfg(test)]
fn whole(body: &str) -> CommentInput {
    CommentInput {
        location: None,
        reply_to: None,
        body: body.to_owned(),
    }
}

#[cfg(test)]
fn refusal<T: std::fmt::Debug>(result: Result<T, ReviewError>) -> FoldError {
    match result {
        Err(ReviewError::Fold(error) | ReviewError::Task(TaskError::Fold(error))) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[cfg(test)]
fn names(events: &[Event]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| event.event_type.as_str())
        .collect()
}

#[tokio::test]
async fn request_changes_moves_task_to_in_progress() {
    let fx = Fixture::new().await;
    assert_eq!(fx.status().await, TaskStatus::Review);
    let submitted = fx
        .submit(
            &fx.reviewer(),
            Verdict::RequestChanges,
            "split it",
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(submitted.version, 1);
    assert_eq!(submitted.task, t(1));
    assert_eq!(submitted.summary.state, ProposalState::Open);
    assert_eq!(submitted.summary.turn, Some(Turn::WithAuthor));
    assert_eq!(fx.status().await, TaskStatus::InProgress);
    assert!(format!("{:?}", fx.reviews).starts_with("ReviewService"));
}

#[tokio::test]
async fn approve_after_request_changes_returns_task_to_review() {
    let fx = Fixture::new().await;
    fx.submit(
        &fx.reviewer(),
        Verdict::RequestChanges,
        "split it",
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(fx.status().await, TaskStatus::InProgress);
    let approved = fx
        .submit(&reviewer(), Verdict::Approve, "good now", Vec::new())
        .await
        .unwrap();
    assert_eq!(approved.summary.turn, Some(Turn::WithReviewer));
    assert_eq!(fx.status().await, TaskStatus::Review);
}

#[tokio::test]
async fn comment_after_request_changes_leaves_task_in_progress() {
    let fx = Fixture::new().await;
    fx.submit(
        &fx.reviewer(),
        Verdict::RequestChanges,
        "split it",
        Vec::new(),
    )
    .await
    .unwrap();
    fx.submit(
        &fx.reviewer(),
        Verdict::Comment,
        "one more thing",
        Vec::new(),
    )
    .await
    .unwrap();
    fx.reviews
        .add_comment(&human(), p(1), 1, lines("src/lib.rs", Side::New, 2, 2))
        .await
        .unwrap();
    assert_eq!(fx.status().await, TaskStatus::InProgress);
    assert_eq!(fx.turn().await, Some(Turn::WithAuthor));
}

#[tokio::test]
async fn new_version_returns_task_to_review_and_starts_unapproved() {
    let fx = Fixture::new().await;
    fx.submit(&reviewer(), Verdict::Approve, "fine", Vec::new())
        .await
        .unwrap();
    fx.submit(
        &fx.reviewer(),
        Verdict::RequestChanges,
        "split it",
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(fx.status().await, TaskStatus::InProgress);

    fx.new_version("pub fn split() {}\n").await;
    assert_eq!(fx.status().await, TaskStatus::Review);
    assert_eq!(fx.turn().await, Some(Turn::WithReviewer));
    let read = fx.project.store().read();
    let v2 = reviews::review_facts(read, p(1), 2).await.unwrap();
    assert!(v2.is_empty());
    assert_eq!(latest_verdict(&v2), None);
    let v1 = reviews::review_facts(read, p(1), 1).await.unwrap();
    assert_eq!(
        v1.iter().map(|fact| fact.verdict).collect::<Vec<_>>(),
        [Verdict::Approve, Verdict::RequestChanges]
    );

    let approved = fx
        .submit(&reviewer(), Verdict::Approve, "fine", Vec::new())
        .await
        .unwrap();
    assert_eq!(approved.version, 2);
    let v2 = reviews::review_facts(read, p(1), 2).await.unwrap();
    assert_eq!(v2.len(), 1);
    assert_eq!(v2[0].reviewer.as_str(), "review");
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM reviews WHERE version_id = 1")
            .await,
        2
    );
}

#[tokio::test]
async fn comment_covers_a_single_line_and_a_range() {
    let fx = Fixture::new().await;
    let single = fx
        .reviews
        .add_comment(
            &fx.reviewer(),
            p(1),
            1,
            lines("src/lib.rs", Side::New, 3, 3),
        )
        .await
        .unwrap();
    assert_eq!(single.id, rc(1));
    assert_eq!(single.verdict, Verdict::Comment);
    assert_eq!(single.author.as_str(), "backend");
    assert!(!single.outdated);
    let location = single.location.as_ref().unwrap();
    assert_eq!(
        (location.start_line, location.end_line, location.side),
        (3, 3, Side::New)
    );

    let mut reply = whole("agreed");
    reply.reply_to = Some(rc(1));
    let submitted = fx
        .submit(
            &fx.reviewer(),
            Verdict::RequestChanges,
            "",
            vec![lines("src/lib.rs", Side::New, 2, 5), reply],
        )
        .await
        .unwrap();
    assert_eq!(submitted.comments, [rc(2), rc(3)]);
    let comments = fx.reviews.list_comments(p(1), None).await.unwrap();
    assert_eq!(comments.len(), 3);
    let range = comments[1].location.as_ref().unwrap();
    assert_eq!(
        (range.path.as_str(), range.start_line, range.end_line),
        ("src/lib.rs", 2, 5)
    );
    assert_eq!(comments[1].verdict, Verdict::RequestChanges);
    assert_eq!(comments[1].body, "about src/lib.rs");
    assert_eq!(comments[2].location, None);
    assert_eq!(comments[2].reply_to, Some(rc(1)));
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM reviews WHERE body = '' AND verdict = 'request_changes'")
            .await,
        1
    );

    let mut stray = whole("to nowhere");
    stray.reply_to = Some(rc(99));
    let error = refusal(
        fx.submit(&fx.reviewer(), Verdict::Comment, "", vec![stray])
            .await,
    );
    assert_eq!(error.code(), codes::INVALID_REQUEST);
    assert!(error.hint().unwrap().contains("get_review_comments"));
}

#[tokio::test]
async fn old_side_comment_checks_the_base_tree() {
    let fx = Fixture::new().await;
    fx.reviews
        .add_comment(
            &fx.reviewer(),
            p(1),
            1,
            lines("src/lib.rs", Side::Old, 1, 1),
        )
        .await
        .unwrap();
    let past = refusal(
        fx.reviews
            .add_comment(
                &fx.reviewer(),
                p(1),
                1,
                lines("src/lib.rs", Side::Old, 2, 2),
            )
            .await,
    );
    assert_eq!(past.code(), codes::INVALID_COMMENT_RANGE);
    let absent = refusal(
        fx.reviews
            .add_comment(
                &fx.reviewer(),
                p(1),
                1,
                lines("src/new.rs", Side::Old, 1, 1),
            )
            .await,
    );
    assert_eq!(absent.code(), codes::UNKNOWN_COMMENT_PATH);
    let added = fx
        .reviews
        .add_comment(
            &fx.reviewer(),
            p(1),
            1,
            lines("src/new.rs", Side::New, 1, 2),
        )
        .await
        .unwrap();
    assert_eq!(added.location.unwrap().side, Side::New);
}

#[tokio::test]
async fn comments_show_outdated_on_a_later_version() {
    let fx = Fixture::new().await;
    fx.reviews
        .add_comment(
            &fx.reviewer(),
            p(1),
            1,
            lines("src/lib.rs", Side::New, 4, 5),
        )
        .await
        .unwrap();
    fx.new_version("pub fn short() {}\n").await;
    let seq = fx.last_seq().await;
    let stale = refusal(
        fx.reviews
            .add_comment(
                &fx.reviewer(),
                p(1),
                1,
                lines("src/lib.rs", Side::New, 4, 5),
            )
            .await,
    );
    assert_eq!(stale.code(), codes::STALE_VERSION);
    assert_eq!(fx.last_seq().await, seq);
    fx.reviews
        .add_comment(
            &fx.reviewer(),
            p(1),
            2,
            lines("src/lib.rs", Side::New, 1, 1),
        )
        .await
        .unwrap();

    let all = fx.reviews.list_comments(p(1), None).await.unwrap();
    let flags: Vec<(u32, bool)> = all.iter().map(|c| (c.version, c.outdated)).collect();
    assert_eq!(flags, [(1, true), (2, false)]);
    let first = fx.reviews.list_comments(p(1), Some(1)).await.unwrap();
    assert_eq!(first.len(), 1);
    assert!(first[0].outdated);
    assert_eq!(first[0].location.as_ref().unwrap().end_line, 5);
    for missing in [0, 3] {
        let error = refusal(fx.reviews.list_comments(p(1), Some(missing)).await);
        assert_eq!(error.code(), codes::PROPOSAL_NOT_FOUND);
    }
    let error = refusal(fx.reviews.list_comments(p(9), None).await);
    assert_eq!(error.code(), codes::PROPOSAL_NOT_FOUND);
}

/// The author adds v2 after the reviewer checked out v1. The approval of the checked-out version is refused before its comments are checked, so it never lands on a version nobody read and nothing is written; the reviewer checks out v2 and approves that.
#[tokio::test]
async fn a_version_added_between_the_checkout_and_the_verdict_refuses_the_approval() {
    let fx = Fixture::new().await;
    crate::common::roles::give_review_claim(fx.project.store(), "review", p(1), fx.project.now())
        .await;
    let checkout = fx
        .reviews
        .create_review_checkout(&reviewer(), p(1))
        .await
        .unwrap();
    assert_eq!(checkout.version, 1);
    fx.new_version("pub fn split() {}\n").await;
    let seq = fx.last_seq().await;

    let error = refusal(
        fx.reviews
            .submit_review(
                &reviewer(),
                p(1),
                checkout.version,
                Verdict::Approve,
                "fine",
                vec![lines("src/lib.rs", Side::New, 5, 5)],
            )
            .await,
    );
    assert_eq!(error.code(), codes::STALE_VERSION);
    let details = error.details().unwrap();
    assert_eq!(details.get("expected_version"), Some(&json!(1)));
    assert_eq!(details.get("current_version"), Some(&json!(2)));
    assert_eq!(fx.last_seq().await, seq);
    assert_eq!(fx.count("SELECT COUNT(*) FROM reviews").await, 0);
    assert_eq!(fx.count("SELECT COUNT(*) FROM review_comments").await, 0);
    assert_eq!(fx.turn().await, Some(Turn::WithReviewer));

    let checkout = fx
        .reviews
        .create_review_checkout(&reviewer(), p(1))
        .await
        .unwrap();
    assert_eq!(checkout.version, 2);
    let approved = fx
        .reviews
        .submit_review(
            &reviewer(),
            p(1),
            checkout.version,
            Verdict::Approve,
            "fine",
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(approved.version, 2);
}

#[tokio::test]
async fn reject_closes_the_proposal_with_the_reason() {
    let fx = Fixture::new().await;
    let empty = refusal(
        fx.submit(&reviewer(), Verdict::Reject, "  ", vec![whole("x")])
            .await,
    );
    assert_eq!(empty.code(), codes::EMPTY_REVIEW);
    let before = fx.last_seq().await;
    let rejected = fx
        .submit(&reviewer(), Verdict::Reject, "wrong approach", Vec::new())
        .await
        .unwrap();
    assert_eq!(rejected.summary.state, ProposalState::Closed);
    assert_eq!(
        rejected.summary.close_reason.as_deref(),
        Some("wrong approach")
    );
    assert_eq!(rejected.summary.turn, None);
    assert_eq!(fx.status().await, TaskStatus::InProgress);
    let appended = fx.events_after(before).await;
    assert_eq!(
        names(&appended),
        ["review.submitted", "proposal.closed", "task.status_changed"]
    );
    assert_eq!(
        appended[1].data,
        json!({"reason": "wrong approach", "verdict": "reject"})
    );
    assert_eq!(appended[1].actor.as_str(), "review");

    let again = refusal(
        fx.submit(&fx.reviewer(), Verdict::Comment, "late", Vec::new())
            .await,
    );
    assert_eq!(again.code(), codes::PROPOSAL_NOT_OPEN);
}

#[tokio::test]
async fn author_agent_cannot_comment_or_approve() {
    let fx = Fixture::new().await;
    let actors = [
        fx.author(),
        agent("frontend/other", &[]),
        agent("frontend", &["review"]),
    ];
    for actor in &actors {
        for verdict in [Verdict::Comment, Verdict::Approve] {
            let error = refusal(fx.submit(actor, verdict, "mine", Vec::new()).await);
            assert_eq!(
                error.code(),
                codes::AUTHOR_CANNOT_REVIEW,
                "{actor:?} {verdict}"
            );
        }
        let error = refusal(
            fx.reviews
                .add_comment(actor, p(1), 1, whole("self review"))
                .await,
        );
        assert_eq!(error.code(), codes::AUTHOR_CANNOT_REVIEW);
    }
    assert_eq!(fx.count("SELECT COUNT(*) FROM reviews").await, 0);
}

#[tokio::test]
async fn approve_and_reject_need_the_right_to_decide() {
    let fx = Fixture::new().await;
    for verdict in [Verdict::Approve, Verdict::Reject] {
        let error = refusal(fx.submit(&fx.reviewer(), verdict, "ok", Vec::new()).await);
        assert_eq!(error.code(), codes::INTEGRATOR_REQUIRED);
    }
    let system = refusal(
        fx.submit(&Actor::System, Verdict::Comment, "x", Vec::new())
            .await,
    );
    assert_eq!(system.code(), codes::HUMAN_REQUIRED);
    let approved = fx
        .submit(&reviewer(), Verdict::Approve, "ok", Vec::new())
        .await
        .unwrap();
    assert_eq!(approved.verdict, Verdict::Approve);
    assert_eq!(fx.status().await, TaskStatus::Review);
    // An unknown agent has no role and no `main` tag, so its approval is refused before its worker is looked up; a comment reaches the lookup.
    let unregistered = refusal(
        fx.submit(&agent("ghost", &[]), Verdict::Approve, "ok", Vec::new())
            .await,
    );
    assert_eq!(unregistered.code(), codes::INTEGRATOR_REQUIRED);
    let unregistered = refusal(
        fx.submit(&agent("ghost", &[]), Verdict::Comment, "ok", Vec::new())
            .await,
    );
    assert_eq!(unregistered.code(), codes::WORKER_NOT_FOUND);
}

#[tokio::test]
async fn human_can_request_changes() {
    let fx = Fixture::new().await;
    let submitted = fx
        .submit(
            &human(),
            Verdict::RequestChanges,
            "please rename",
            vec![lines("src/lib.rs", Side::New, 1, 2)],
        )
        .await
        .unwrap();
    assert_eq!(submitted.summary.turn, Some(Turn::WithAuthor));
    assert_eq!(fx.status().await, TaskStatus::InProgress);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM reviews WHERE reviewer_human = 'tanim' AND reviewer_worker_id IS NULL")
            .await,
        1
    );
    let comments = fx.reviews.list_comments(p(1), None).await.unwrap();
    assert_eq!(
        comments[0].author,
        EventActor::parse("human:tanim").unwrap()
    );
    let facts = reviews::review_facts(fx.project.store().read(), p(1), 1)
        .await
        .unwrap();
    assert_eq!(facts[0].reviewer.as_str(), "human:tanim");
}

#[tokio::test]
async fn range_past_end_of_file_is_refused() {
    let fx = Fixture::new().await;
    for (start, end) in [(5, 6), (6, 6), (0, 2), (3, 2)] {
        let error = refusal(
            fx.reviews
                .add_comment(
                    &fx.reviewer(),
                    p(1),
                    1,
                    lines("src/lib.rs", Side::New, start, end),
                )
                .await,
        );
        assert_eq!(error.code(), codes::INVALID_COMMENT_RANGE, "{start}-{end}");
    }
    fx.reviews
        .add_comment(
            &fx.reviewer(),
            p(1),
            1,
            lines("src/lib.rs", Side::New, 5, 5),
        )
        .await
        .unwrap();
    assert_eq!(fx.count("SELECT COUNT(*) FROM review_comments").await, 1);
}

#[tokio::test]
async fn only_text_files_inside_the_tree_take_line_comments() {
    let fx = Fixture::new().await;
    for (path, code) in [
        ("src", codes::UNKNOWN_COMMENT_PATH),
        ("logo.bin", codes::UNKNOWN_COMMENT_PATH),
        ("src/gone.rs", codes::UNKNOWN_COMMENT_PATH),
        ("../escape.rs", codes::INVALID_PATH),
        ("/etc/passwd", codes::INVALID_PATH),
        ("", codes::INVALID_PATH),
    ] {
        let error = refusal(
            fx.reviews
                .add_comment(&fx.reviewer(), p(1), 1, lines(path, Side::New, 1, 1))
                .await,
        );
        assert_eq!(error.code(), code, "{path:?}");
    }
    let empty = refusal(
        fx.submit(&fx.reviewer(), Verdict::Approve, " ", Vec::new())
            .await,
    );
    assert_eq!(empty.code(), codes::EMPTY_REVIEW);
    let blank = refusal(
        fx.submit(&fx.reviewer(), Verdict::Comment, "", vec![whole(" ")])
            .await,
    );
    assert_eq!(blank.code(), codes::INVALID_REQUEST);
    let long = "x".repeat(20_001);
    let error = refusal(
        fx.submit(&fx.reviewer(), Verdict::Comment, &long, Vec::new())
            .await,
    );
    assert_eq!(error.code(), codes::INVALID_REQUEST);
    let missing = refusal(
        fx.reviews
            .submit_review(&fx.reviewer(), p(9), 1, Verdict::Comment, "x", Vec::new())
            .await,
    );
    assert_eq!(missing.code(), codes::PROPOSAL_NOT_FOUND);
    assert_eq!(fx.count("SELECT COUNT(*) FROM reviews").await, 0);
}

/// The status cache and `fold_core::reviews::latest_verdict` read the same reviews the same way.
#[tokio::test]
async fn latest_verdict_agrees_with_the_status_cache() {
    let fx = Fixture::new().await;
    let steps = [
        (reviewer(), Verdict::Comment),
        (fx.reviewer(), Verdict::RequestChanges),
        (human(), Verdict::Comment),
        (reviewer(), Verdict::Approve),
        (fx.reviewer(), Verdict::RequestChanges),
        (human(), Verdict::Approve),
    ];
    for (actor, verdict) in steps {
        fx.submit(&actor, verdict, "note", Vec::new())
            .await
            .unwrap();
        let facts = reviews::review_facts(fx.project.store().read(), p(1), 1)
            .await
            .unwrap();
        let with_author = changes_requested(&facts);
        assert_eq!(
            fx.turn().await == Some(Turn::WithAuthor),
            with_author,
            "{verdict}"
        );
        let expected = if with_author {
            TaskStatus::InProgress
        } else {
            TaskStatus::Review
        };
        assert_eq!(fx.status().await, expected, "{verdict}");
    }
}

#[tokio::test]
async fn review_events_commit_with_their_rows() {
    let fx = Fixture::new().await;
    let before = fx.last_seq().await;
    fx.submit(
        &fx.reviewer(),
        Verdict::RequestChanges,
        "two notes",
        vec![lines("src/lib.rs", Side::New, 1, 1), whole("overall")],
    )
    .await
    .unwrap();
    let appended = fx.events_after(before).await;
    assert_eq!(
        names(&appended),
        [
            "review.submitted",
            "review.comment_added",
            "review.comment_added",
            "task.status_changed"
        ]
    );
    assert_eq!(
        appended[0].data,
        json!({"verdict": "request_changes", "version": 1, "comments": 2})
    );
    assert_eq!(appended[1].data, json!({"comment": "rc-1", "version": 1}));
    assert_eq!(appended[2].data, json!({"comment": "rc-2", "version": 1}));
    for event in &appended[..3] {
        assert_eq!(event.proposal, Some(p(1)));
        assert_eq!(event.task, Some(t(1)));
        assert_eq!(event.actor.as_str(), "backend");
        assert_eq!(event.created_at, fx.project.now());
    }
    assert_eq!(
        fx.count(&format!(
            "SELECT COUNT(*) FROM reviews WHERE created_at = {}",
            fx.project.now().as_millis()
        ))
        .await,
        1
    );

    let mut stray = whole("to nowhere");
    stray.reply_to = Some(rc(99));
    let seq = fx.last_seq().await;
    refusal(
        fx.submit(
            &fx.reviewer(),
            Verdict::Comment,
            "",
            vec![whole("fine"), stray],
        )
        .await,
    );
    assert_eq!(fx.last_seq().await, seq);
    assert_eq!(fx.count("SELECT COUNT(*) FROM reviews").await, 1);
    assert_eq!(fx.count("SELECT COUNT(*) FROM review_comments").await, 2);
    let next = fx
        .reviews
        .add_comment(&fx.reviewer(), p(1), 1, whole("after"))
        .await
        .unwrap();
    assert_eq!(next.id, rc(3));
}

/// A version added after the comment ranges were checked and before the review's transaction has other trees, so the review of the version the reviewer read is refused with `stale_version` instead of being filed against a version whose files it did not check. Nothing of the review is written, and the same review on the new version goes through.
#[cfg(feature = "failpoints")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_version_added_after_the_range_check_refuses_the_review() {
    use std::sync::atomic::{AtomicBool, Ordering};

    const FAILPOINT: &str = "review_after_range_check";
    let fx = Fixture::new().await;
    let scenario = fail::FailScenario::setup();
    let reached = Arc::new(AtomicBool::new(false));
    let open = Arc::new(AtomicBool::new(false));
    let (held_reached, held_open) = (reached.clone(), open.clone());
    fail::cfg_callback(FAILPOINT, move || {
        held_reached.store(true, Ordering::SeqCst);
        while !held_open.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
    })
    .unwrap();
    let reviews = fx.reviews.clone();
    let reviewer = fx.reviewer();
    let review = tokio::spawn(async move {
        reviews
            .submit_review(
                &reviewer,
                p(1),
                1,
                Verdict::RequestChanges,
                "",
                vec![lines("src/lib.rs", Side::New, 5, 5)],
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(20), async {
        while !reached.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    fx.new_version("pub fn a() {}\n").await;
    let seq = fx.last_seq().await;
    open.store(true, Ordering::SeqCst);
    let error = refusal(review.await.unwrap());
    fail::remove(FAILPOINT);
    scenario.teardown();

    assert_eq!(error.code(), codes::STALE_VERSION);
    let details = error.details().unwrap();
    assert_eq!(details.get("expected_version"), Some(&json!(1)));
    assert_eq!(details.get("current_version"), Some(&json!(2)));
    assert_eq!(fx.last_seq().await, seq);
    assert_eq!(fx.count("SELECT COUNT(*) FROM reviews").await, 0);
    assert_eq!(fx.count("SELECT COUNT(*) FROM review_comments").await, 0);
    assert_eq!(fx.status().await, TaskStatus::Review);

    let range = refusal(
        fx.submit(
            &fx.reviewer(),
            Verdict::RequestChanges,
            "",
            vec![lines("src/lib.rs", Side::New, 5, 5)],
        )
        .await,
    );
    assert_eq!(range.code(), codes::INVALID_COMMENT_RANGE);
    let submitted = fx
        .submit(
            &fx.reviewer(),
            Verdict::RequestChanges,
            "",
            vec![lines("src/lib.rs", Side::New, 1, 1)],
        )
        .await
        .unwrap();
    assert_eq!(submitted.version, 2);
    assert_eq!(fx.status().await, TaskStatus::InProgress);
}

/// A version added after a checkout's files were written and before its transaction leaves those files at the old version, so the checkout is refused with `stale_version`: no row names it and its directory is removed. The next call checks out the new version.
#[cfg(feature = "failpoints")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_version_added_while_a_checkout_is_written_refuses_the_checkout() {
    use std::sync::atomic::{AtomicBool, Ordering};

    const FAILPOINT: &str = "review_checkout_after_files";
    let fx = Fixture::new().await;
    crate::common::roles::give_review_claim(fx.project.store(), "review", p(1), fx.project.now())
        .await;
    let scenario = fail::FailScenario::setup();
    let reached = Arc::new(AtomicBool::new(false));
    let open = Arc::new(AtomicBool::new(false));
    let (held_reached, held_open) = (reached.clone(), open.clone());
    fail::cfg_callback(FAILPOINT, move || {
        held_reached.store(true, Ordering::SeqCst);
        while !held_open.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
    })
    .unwrap();
    let reviews = fx.reviews.clone();
    let checkout =
        tokio::spawn(async move { reviews.create_review_checkout(&reviewer(), p(1)).await });
    tokio::time::timeout(Duration::from_secs(20), async {
        while !reached.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    fx.new_version("pub fn a() {}\n").await;
    let seq = fx.last_seq().await;
    open.store(true, Ordering::SeqCst);
    let error = refusal(checkout.await.unwrap());
    fail::remove(FAILPOINT);
    scenario.teardown();

    assert_eq!(error.code(), codes::STALE_VERSION);
    let details = error.details().unwrap();
    assert_eq!(details.get("expected_version"), Some(&json!(1)));
    assert_eq!(details.get("current_version"), Some(&json!(2)));
    assert_eq!(fx.last_seq().await, seq);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspaces WHERE kind = 'review'")
            .await,
        0
    );
    assert!(
        !fx.project
            .paths()
            .root()
            .join(".fold/ws/review-p-1")
            .exists()
    );

    let checkout = fx
        .reviews
        .create_review_checkout(&reviewer(), p(1))
        .await
        .unwrap();
    assert_eq!(checkout.version, 2);
}
