use crate::common::intents::Covering as _;
use crate::common::intents::{covering_with, whole_change};
use fold_core::reviews::ProposalIntent;
use std::sync::Arc;
use std::time::Duration;

use crate::common::roles::give_role;
use fold_core::object::ObjectKind;
use fold_core::reviews::Verdict;
use fold_core::roles::Role;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentActor, AgentName, CheckpointId, EventFilter, FoldError, HumanName, ProposalId, Tag,
    TaskDescription, TaskId, TaskStatus, TaskTitle, WorkerName, WorkerState, WorkspaceId, codes,
};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::events::EventBus;
use fold_daemon::services::projects::ensure_human;
use fold_daemon::services::reviews::{ReviewError, ReviewService};
use fold_daemon::services::tasks::{CreateTask, TaskError, TaskService};
use fold_daemon::services::workspaces::{WorkspaceError, WorkspaceService};
use fold_store::testing::TestProject;
use fold_store::workspaces::{self, CheckpointReason, create_initial_revision, initial_revision};
use fold_store::{StoreError, agents, events};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use serde_json::json;

const LEASE_MINUTES: u64 = 5;
const EDITED: &str = "pub fn y() {}\n// checked by a human\n";

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
fn agent(worker: &str) -> Actor {
    let worker = WorkerName::parse(worker).unwrap();
    Actor::Agent(AgentActor {
        agent: worker.agent(),
        worker,
        tags: Vec::<Tag>::new(),
    })
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
fn workspace_refusal<T: std::fmt::Debug>(result: Result<T, WorkspaceError>) -> FoldError {
    match result {
        Err(WorkspaceError::Fold(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[cfg(test)]
struct Fixture {
    project: TestProject,
    tasks: TaskService,
    workspaces: WorkspaceService,
    reviews: ReviewService,
}

#[cfg(test)]
impl Fixture {
    /// `frontend` holds task 1 and has opened `P-1` on it, which changes `src/lib.rs`; `review` is a reviewer. The registry knows `tanim` and `ana`.
    async fn new() -> Self {
        let project = TestProject::new().await;
        let bus = EventBus::new();
        let clock = Arc::new(project.clock().clone());
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let mut tree = FlatTree::new();
        for (path, content) in [
            ("src/lib.rs", b"pub fn x() {}\n".as_slice()),
            ("logo.bin", b"\0\x01\x02".as_slice()),
        ] {
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
        crate::common::roles::lead_work_areas(project.store()).await;
        tasks
            .create_task(
                &agent("planner"),
                CreateTask {
                    title: TaskTitle::parse("auth").unwrap(),
                    description: TaskDescription::new(""),
                    labels: crate::common::roles::work_labels(),
                },
            )
            .await
            .unwrap();
        for name in ["frontend", "review", "planner"] {
            let mut ctx = agents.start_session(1).await.unwrap();
            agents
                .register_agent(&mut ctx, &AgentName::parse(name).unwrap(), &[])
                .await
                .unwrap();
        }
        give_role(project.store(), "review", Role::Reviewer, None).await;
        let author = agent("frontend");
        workspaces.create_workspace(&author, t(1)).await.unwrap();
        std::fs::write(Self::file_in(&project), "pub fn y() {}\n").unwrap();
        reviews
            .create_proposal_covering(&author, t(1), "Add auth", Some("tests pass"))
            .await
            .unwrap()
            .detail()
            .unwrap();
        Self {
            project,
            tasks,
            workspaces,
            reviews,
        }
    }

    fn file_in(project: &TestProject) -> std::path::PathBuf {
        project.paths().root().join(".fold/ws/t-1/src/lib.rs")
    }

    async fn mark(&self, reviewer: Option<&str>) {
        self.reviews
            .require_human_review(&agent("planner"), t(1), reviewer.map(name), "touches auth")
            .await
            .unwrap();
    }

    async fn review(&self, actor: &Actor, verdict: Verdict) {
        self.project
            .clock()
            .advance(Duration::from_secs(1))
            .unwrap();
        let read = self.reviews.get_proposal(p(1), None).await.unwrap();
        self.reviews
            .submit_review(
                actor,
                p(1),
                read.summary.current_version,
                verdict,
                "looked",
                Vec::new(),
            )
            .await
            .unwrap();
    }

    async fn edit(
        &self,
        actor: &Actor,
        version: u32,
        path: &str,
        content: &str,
    ) -> Result<fold_daemon::services::reviews::HumanEdit, ReviewError> {
        self.project
            .clock()
            .advance(Duration::from_secs(1))
            .unwrap();
        self.reviews
            .edit_as_human(actor, p(1), version, path, content, None)
            .await
    }

    async fn head(&self) -> u64 {
        events::head_seq(self.project.store().read()).await.unwrap()
    }

    async fn workspace(&self) -> WorkspaceId {
        workspaces::live_task_workspace(self.project.store().read(), t(1))
            .await
            .unwrap()
            .unwrap()
            .id
    }

    async fn set_sync_mark(&self, workspace: WorkspaceId, checkpoint: Option<CheckpointId>) {
        self.project
            .store()
            .write(async |tx| {
                workspaces::set_sync_checkpoint(tx, workspace, checkpoint).await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
    }

    async fn set_frontend_state(&self, state: WorkerState) {
        let agent = AgentName::parse("frontend").unwrap();
        let now = self.project.now();
        self.project
            .store()
            .write(async |tx| {
                let row = agents::find_worker_process(tx.conn(), &agent)
                    .await?
                    .unwrap();
                agents::set_worker_state(tx, row.id, state, now).await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_human_edit_is_a_new_version_that_keeps_the_approvals() {
    let fx = Fixture::new().await;
    fx.mark(Some("tanim")).await;
    fx.review(&agent("review"), Verdict::Approve).await;
    let before = fx.head().await;

    let edit = fx
        .edit(&human("tanim"), 1, "src/lib.rs", EDITED)
        .await
        .unwrap();
    assert_eq!((edit.from_version, edit.version), (1, 2));
    assert_eq!(edit.kept_approvals, 1);
    let detail = &edit.detail;
    assert_eq!(detail.summary.current_version, 2);
    assert_eq!(detail.summary.author.as_str(), "frontend");
    assert_eq!(detail.summary.latest_verdict, Some(Verdict::Approve));
    assert_eq!(detail.task_status, TaskStatus::Review);
    let (first, second) = (&detail.versions[0], &detail.versions[1]);
    assert_eq!(second.created_by, "human:tanim");
    assert_eq!(second.base, first.base);
    assert_eq!(second.evidence.as_deref(), Some("tests pass"));
    assert_ne!(second.tree, first.tree);
    assert_ne!(second.changeset, first.changeset);
    assert!(!second.review_again);
    assert!(second.human_edit);
    assert!(!first.human_edit);

    let file = fx
        .reviews
        .version_file(p(1), None, "src/lib.rs")
        .await
        .unwrap();
    assert_eq!(file.bytes, EDITED.as_bytes());
    let old = fx
        .reviews
        .version_file(p(1), Some(1), "src/lib.rs")
        .await
        .unwrap();
    assert_eq!(old.bytes, b"pub fn y() {}\n");

    let view = fx
        .reviews
        .view_proposal(&human("tanim"), p(1), None)
        .await
        .unwrap();
    assert!(view.agent_approved, "the reviewer's approval is kept");
    let kept: Vec<_> = view
        .reviews
        .iter()
        .filter(|review| review.version == 2)
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].verdict, Verdict::Approve);
    assert_eq!(kept[0].reviewer.as_str(), "review");
    assert_eq!(kept[0].kept_from, Some(1));
    assert_eq!(view.last_reviewed, Some(1));

    let written = events::read_after(
        fx.project.store().read(),
        before,
        &EventFilter::default(),
        100,
    )
    .await
    .unwrap();
    let types: Vec<&str> = written.iter().map(|e| e.event_type.as_str()).collect();
    assert_eq!(
        types,
        [
            "changeset.recorded",
            "checkpoint.recorded",
            "proposal.human_edited"
        ]
    );
    assert!(written.iter().all(|e| e.actor.as_str() == "human:tanim"));
    let edited = &written[2];
    assert_eq!(edited.proposal, Some(p(1)));
    assert_eq!(edited.task, Some(t(1)));
    assert_eq!(edited.data["version"], json!(2));
    assert_eq!(edited.data["from_version"], json!(1));
    assert_eq!(edited.data["kept_approvals"], json!(1));
    assert_eq!(
        edited.data["checkpoint"],
        json!(edit.checkpoint.to_string())
    );
    assert!(!edited.data.to_string().contains("checked by a human"));

    let workspace = fx.workspace().await;
    let checkpoint = workspaces::get_checkpoint(fx.project.store().read(), edit.checkpoint)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.reason, CheckpointReason::HumanEdit);
    assert_eq!(checkpoint.tree, second.tree);
    assert_eq!(
        workspaces::sync_checkpoint(fx.project.store().read(), workspace)
            .await
            .unwrap(),
        Some(edit.checkpoint)
    );

    // fold leaves the files alone: the worker's sync writes the edit.
    let on_disk = Fixture::file_in(&fx.project);
    assert_eq!(
        std::fs::read_to_string(&on_disk).unwrap(),
        "pub fn y() {}\n"
    );
    let refused = workspace_refusal(
        fx.workspaces
            .record_changes(&agent("frontend"), workspace, "more")
            .await,
    );
    assert_eq!(refused.code(), codes::WORKSPACE_WRITE_UNFINISHED);
    assert!(refused.message().contains("human edit"));
    assert_eq!(
        refused.hint(),
        Some("call sync_workspace to write the human edit into the files")
    );
    fx.workspaces
        .sync_workspace(&agent("frontend"), workspace)
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&on_disk).unwrap(), EDITED);
    assert_eq!(
        workspaces::sync_checkpoint(fx.project.store().read(), workspace)
            .await
            .unwrap(),
        None
    );

    // The kept agent approval lets the marked human approve the edit.
    fx.review(&human("tanim"), Verdict::Approve).await;
    let summary = fx.reviews.get_proposal(p(1), None).await.unwrap().summary;
    assert!(summary.human_approved);
}

#[tokio::test]
async fn a_human_edit_keeps_no_human_approval_so_it_never_integrates_on_one() {
    let fx = Fixture::new().await;
    fx.mark(Some("tanim")).await;
    fx.review(&agent("review"), Verdict::Approve).await;
    fx.review(&human("tanim"), Verdict::Approve).await;
    let read = fx.reviews.get_proposal(p(1), None).await.unwrap();
    assert!(read.summary.human_approved);

    let edit = fx
        .edit(&human("tanim"), 1, "src/lib.rs", EDITED)
        .await
        .unwrap();
    assert_eq!(edit.kept_approvals, 1, "only the agent's approval is kept");
    assert!(!edit.detail.summary.human_approved);
    let view = fx
        .reviews
        .view_proposal(&human("tanim"), p(1), None)
        .await
        .unwrap();
    assert!(view.agent_approved);
    let kept: Vec<_> = view
        .reviews
        .iter()
        .filter(|review| review.version == 2)
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].reviewer.as_str(), "review");
    assert_eq!(kept[0].kept_from, Some(1));

    assert!(fx.reviews.integrate_ready().await.unwrap().is_empty());
    let waiting = fx.reviews.get_proposal(p(1), None).await.unwrap();
    assert_eq!(waiting.summary.current_version, 2);
    assert_eq!(waiting.task_status, TaskStatus::Review);

    fx.review(&human("tanim"), Verdict::Approve).await;
    let integrated = fx.reviews.integrate_ready().await.unwrap();
    assert_eq!(integrated.len(), 1);
    assert_eq!(integrated[0].proposal, p(1));
}

#[tokio::test]
async fn sending_an_edit_for_review_again_withdraws_the_kept_approvals() {
    let fx = Fixture::new().await;
    fx.mark(None).await;
    fx.review(&agent("review"), Verdict::Approve).await;

    let unedited = refusal(
        fx.reviews
            .send_for_review_again(&human("ana"), p(1), 1)
            .await,
    );
    assert_eq!(unedited.code(), codes::INVALID_REQUEST);
    assert_eq!(
        unedited.details().unwrap()["reason"],
        json!("not_human_edit")
    );

    fx.edit(&human("ana"), 1, "src/lib.rs", EDITED)
        .await
        .unwrap();
    let stale = refusal(
        fx.reviews
            .send_for_review_again(&human("ana"), p(1), 1)
            .await,
    );
    assert_eq!(stale.code(), codes::STALE_VERSION);
    let other = refusal(
        fx.reviews
            .send_for_review_again(&agent("review"), p(1), 2)
            .await,
    );
    assert_eq!(other.code(), codes::HUMAN_EDIT_DENIED);

    let before = fx.head().await;
    let detail = fx
        .reviews
        .send_for_review_again(&human("tanim"), p(1), 2)
        .await
        .unwrap();
    assert_eq!(detail.summary.latest_verdict, None);
    assert_eq!(detail.task_status, TaskStatus::Review);
    assert!(detail.versions[1].review_again);
    let view = fx
        .reviews
        .view_proposal(&human("tanim"), p(1), None)
        .await
        .unwrap();
    assert!(!view.agent_approved);
    assert!(view.reviews.iter().all(|review| review.version == 1));
    let written = events::read_after(
        fx.project.store().read(),
        before,
        &EventFilter::default(),
        100,
    )
    .await
    .unwrap();
    assert_eq!(
        written[0].event_type.as_str(),
        "proposal.review_again_requested"
    );
    assert_eq!(written[0].actor.as_str(), "human:tanim");
    assert_eq!(written[0].data, json!({"version": 2}));

    let again = refusal(
        fx.reviews
            .send_for_review_again(&human("tanim"), p(1), 2)
            .await,
    );
    assert_eq!(again.code(), codes::INVALID_REQUEST);
    assert_eq!(again.details().unwrap()["reason"], json!("already_sent"));

    // The reviewer's new verdict decides the version again.
    fx.review(&agent("review"), Verdict::Approve).await;
    let view = fx
        .reviews
        .view_proposal(&human("tanim"), p(1), None)
        .await
        .unwrap();
    assert!(view.agent_approved);
    assert_eq!(view.reviews.last().unwrap().kept_from, None);
}

#[tokio::test]
async fn a_request_for_changes_withdraws_a_kept_approval_and_none_is_kept_after_one() {
    let fx = Fixture::new().await;
    fx.mark(None).await;
    fx.review(&agent("review"), Verdict::Approve).await;
    fx.review(&agent("review"), Verdict::RequestChanges).await;
    let edit = fx
        .edit(&human("tanim"), 1, "src/lib.rs", EDITED)
        .await
        .unwrap();
    assert_eq!(edit.kept_approvals, 0);
    assert_eq!(edit.detail.summary.latest_verdict, None);
    assert_eq!(edit.detail.task_status, TaskStatus::Review);
}

#[tokio::test]
async fn a_human_edit_is_refused_outside_a_human_review() {
    let fx = Fixture::new().await;
    let before = fx.head().await;
    let unmarked = refusal(fx.edit(&human("tanim"), 1, "src/lib.rs", EDITED).await);
    assert_eq!(unmarked.code(), codes::HUMAN_EDIT_DENIED);
    assert_eq!(unmarked.details().unwrap()["proposal"], json!("P-1"));

    fx.mark(Some("ana")).await;
    let marked = fx.head().await;
    let other = refusal(fx.edit(&human("tanim"), 1, "src/lib.rs", EDITED).await);
    assert_eq!(other.code(), codes::HUMAN_EDIT_DENIED);
    assert_eq!(other.details().unwrap()["reviewer"], json!("ana"));
    for actor in [agent("frontend"), agent("review"), Actor::System] {
        let denied = refusal(fx.edit(&actor, 1, "src/lib.rs", EDITED).await);
        assert_eq!(denied.code(), codes::HUMAN_EDIT_DENIED, "{actor:?}");
    }
    assert_eq!(fx.head().await, marked);
    assert!(marked > before);
    let detail = fx.reviews.get_proposal(p(1), None).await.unwrap();
    assert_eq!(detail.summary.current_version, 1);
}

#[tokio::test]
async fn a_human_edit_of_another_version_is_stale() {
    let fx = Fixture::new().await;
    fx.mark(None).await;
    for version in [0, 2] {
        let stale = refusal(
            fx.edit(&human("tanim"), version, "src/lib.rs", EDITED)
                .await,
        );
        assert_eq!(stale.code(), codes::STALE_VERSION);
        assert_eq!(stale.details().unwrap()["current_version"], json!(1));
    }
    fx.edit(&human("tanim"), 1, "src/lib.rs", EDITED)
        .await
        .unwrap();
    let stale = refusal(fx.edit(&human("tanim"), 1, "src/lib.rs", "x\n").await);
    assert_eq!(stale.code(), codes::STALE_VERSION);
}

#[tokio::test]
async fn a_human_edit_waits_while_the_task_worker_may_change_the_workspace() {
    let fx = Fixture::new().await;
    fx.mark(None).await;
    for state in [
        WorkerState::Starting,
        WorkerState::Working,
        WorkerState::Stopping,
    ] {
        fx.set_frontend_state(state).await;
        let busy = refusal(fx.edit(&human("tanim"), 1, "src/lib.rs", EDITED).await);
        assert_eq!(busy.code(), codes::WORKER_BUSY, "{state:?}");
        assert_eq!(busy.details().unwrap()["reason"], json!("working"));
        assert_eq!(busy.details().unwrap()["worker"], json!("frontend"));
    }
    fx.set_frontend_state(WorkerState::Idle).await;

    // A sync or restore that stopped before it finished left the files half written.
    let workspace = fx.workspace().await;
    let stopped = fx
        .workspaces
        .record_checkpoint(&agent("frontend"), workspace, None)
        .await
        .unwrap();
    fx.set_sync_mark(workspace, Some(stopped.id)).await;
    let unfinished = refusal(fx.edit(&human("tanim"), 1, "src/lib.rs", EDITED).await);
    assert_eq!(unfinished.code(), codes::WORKER_BUSY);
    assert_eq!(
        unfinished.details().unwrap()["reason"],
        json!("unfinished_write")
    );
    fx.set_sync_mark(workspace, None).await;

    fx.edit(&human("tanim"), 1, "src/lib.rs", EDITED)
        .await
        .unwrap();

    // A second edit of a stopped worker's proposal replaces the write that waits for its sync.
    fx.set_frontend_state(WorkerState::Stopped).await;
    let second = fx
        .edit(&human("tanim"), 2, "src/lib.rs", "x\n")
        .await
        .unwrap();
    assert_eq!((second.from_version, second.version), (2, 3));
    assert_eq!(
        workspaces::sync_checkpoint(fx.project.store().read(), workspace)
            .await
            .unwrap(),
        Some(second.checkpoint)
    );
    let on_disk = Fixture::file_in(&fx.project);
    assert_eq!(
        std::fs::read_to_string(&on_disk).unwrap(),
        "pub fn y() {}\n"
    );

    fx.workspaces
        .sync_workspace(&agent("frontend"), workspace)
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&on_disk).unwrap(), "x\n");
    std::fs::write(&on_disk, "pub fn z() {}\n").unwrap();
    fx.workspaces
        .record_changes(&agent("frontend"), workspace, "more work")
        .await
        .unwrap();
    let newer = refusal(fx.edit(&human("tanim"), 3, "src/lib.rs", "y\n").await);
    assert_eq!(newer.code(), codes::WORKER_BUSY);
    assert_eq!(newer.details().unwrap()["reason"], json!("newer_work"));
}

#[tokio::test]
async fn a_human_edit_changes_only_a_text_file_of_the_version() {
    let fx = Fixture::new().await;
    fx.mark(None).await;
    let before = fx.head().await;
    for path in ["../etc/passwd", "/src/lib.rs", ".fold/x", ""] {
        let invalid = refusal(fx.edit(&human("tanim"), 1, path, EDITED).await);
        assert_eq!(invalid.code(), codes::INVALID_PATH, "{path}");
    }
    for (path, content, reason) in [
        ("src/missing.rs", EDITED, "missing"),
        ("src", EDITED, "missing"),
        ("logo.bin", EDITED, "binary"),
        ("src/lib.rs", "a\0b", "binary"),
    ] {
        let refused = refusal(fx.edit(&human("tanim"), 1, path, content).await);
        assert_eq!(refused.code(), codes::FILE_NOT_EDITABLE, "{path}");
        assert_eq!(
            refused.details().unwrap()["reason"],
            json!(reason),
            "{path}"
        );
    }
    let too_large = "a".repeat(1024 * 1024 + 1);
    let refused = refusal(fx.edit(&human("tanim"), 1, "src/lib.rs", &too_large).await);
    assert_eq!(refused.details().unwrap()["reason"], json!("too_large"));
    let unchanged = refusal(
        fx.edit(&human("tanim"), 1, "src/lib.rs", "pub fn y() {}\n")
            .await,
    );
    assert_eq!(unchanged.code(), codes::VERSION_UNCHANGED);
    assert_eq!(fx.head().await, before);
    let tasks = fx.tasks.get(t(1)).await.unwrap();
    assert_eq!(tasks.status, TaskStatus::Review);
}

#[tokio::test]
async fn a_human_edit_keeps_the_intent_unless_the_human_writes_one() {
    let fx = Fixture::new().await;
    fx.mark(Some("tanim")).await;
    let tanim = human("tanim");
    let blank = ProposalIntent {
        summary: " ".to_owned(),
        ..whole_change(&["src/lib.rs"])
    };
    let error = refusal(
        fx.reviews
            .edit_as_human(&tanim, p(1), 1, "src/lib.rs", EDITED, Some(blank))
            .await,
    );
    assert_eq!(error.code(), codes::INVALID_INTENT);
    assert_eq!(
        serde_json::Value::Object(error.details().unwrap().clone()),
        json!({"field": "intent.summary", "reason": "empty"})
    );
    let kept = fx.edit(&tanim, 1, "src/lib.rs", EDITED).await.unwrap();
    let (first, second) = (&kept.detail.versions[0], &kept.detail.versions[1]);
    assert_eq!(second.intent, first.intent);
    let mut decided = second.intent.clone().unwrap();
    assert_eq!(decided.summary, "Make the change the task asks for.");
    let files = decided.parts[0].files.clone();
    decided.summary = "Returns 3, the value the product decided.".to_owned();
    decided.parts[0].files.clear();
    let uncovered = refusal(
        fx.reviews
            .edit_as_human(
                &tanim,
                p(1),
                2,
                "src/lib.rs",
                "pub fn x() -> u8 { 3 }\n",
                Some(decided.clone()),
            )
            .await,
    );
    assert_eq!(
        uncovered.details().unwrap()["reason"],
        json!("part_files_uncovered")
    );
    assert_eq!(uncovered.details().unwrap()["uncovered"], json!(files));
    decided.parts[0].files = files;
    let written = fx
        .reviews
        .edit_as_human(
            &tanim,
            p(1),
            2,
            "src/lib.rs",
            "pub fn x() -> u8 { 3 }\n",
            Some(decided.clone()),
        )
        .await
        .unwrap();
    assert_eq!(written.detail.selected.intent, Some(decided));
}

#[tokio::test]
async fn a_kept_intent_must_match_the_files_of_the_edit() {
    let fx = Fixture::new().await;
    fx.mark(Some("tanim")).await;
    let tanim = human("tanim");
    let before = fx.head().await;
    // The edit gives src/lib.rs its base content back, so the version no longer changes it.
    let error = refusal(fx.edit(&tanim, 1, "src/lib.rs", "pub fn x() {}\n").await);
    assert_eq!(error.code(), codes::INVALID_INTENT);
    assert_eq!(
        error.details().unwrap()["reason"],
        json!("part_file_outside_diff")
    );
    assert_eq!(error.details().unwrap()["outside"], json!(["src/lib.rs"]));
    assert!(error.hint().unwrap().contains("src/lib.rs"));
    assert_eq!(fx.head().await, before, "a refused edit writes nothing");
}

#[tokio::test]
async fn a_human_edit_of_a_version_without_an_intent_needs_one() {
    let fx = Fixture::new().await;
    fx.mark(Some("tanim")).await;
    fx.project
        .store()
        .write(async |tx| {
            for statement in [
                "DROP TRIGGER proposal_versions_no_update",
                "UPDATE proposal_versions SET intent_summary = NULL",
            ] {
                sqlx::query(statement).execute(tx.conn()).await?;
            }
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let tanim = human("tanim");
    let before = fx.head().await;
    let error = refusal(fx.edit(&tanim, 1, "src/lib.rs", EDITED).await);
    assert_eq!(error.code(), codes::INVALID_INTENT);
    assert_eq!(error.details().unwrap()["reason"], json!("missing"));
    assert!(
        error
            .hint()
            .unwrap()
            .starts_with("call edit_proposal_file again with `intent`")
    );
    assert_eq!(fx.head().await, before, "a refused edit writes nothing");
    let asked = ProposalIntent {
        summary: "Adds the check a human asked for.".to_owned(),
        ..whole_change(&[])
    };
    let written = covering_with(asked, async |intent| {
        fx.reviews
            .edit_as_human(&tanim, p(1), 1, "src/lib.rs", EDITED, Some(intent))
            .await
    })
    .await
    .unwrap();
    assert_eq!(written.detail.versions[0].intent, None);
    assert_eq!(
        written
            .detail
            .selected
            .intent
            .as_ref()
            .map(|intent| intent.summary.as_str()),
        Some("Adds the check a human asked for.")
    );
}
