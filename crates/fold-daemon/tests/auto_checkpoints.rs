use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fold_core::checkpoint_policy::AutoReason;
use fold_core::object::ObjectKind;
use fold_core::revision::Revision;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    Actor, AgentName, CheckpointId, EventActor, EventFilter, HumanName, RevisionId,
    TaskDescription, TaskId, TaskTitle, Timestamp, WorkspaceId, codes,
};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::jobs::{JobProgress, QuietCheckpoints, spawn_auto_checkpoints};
use fold_daemon::projects::{ProjectHandle, Projects};
use fold_daemon::services::projects::DEFAULT_SESSION_LIFETIME;
use fold_daemon::services::reviews::ReviewService;
use fold_daemon::services::tasks::CreateTask;
use fold_daemon::services::workspaces::{QuietCheckpoint, WorkspaceError, WorkspaceService};
use fold_store::events;
use fold_store::projects::ProjectRow;
use fold_store::testing::TestProject;
use fold_store::workspaces::{
    self, Checkpoint, CheckpointKind, CheckpointReason, advance_main, create_initial_revision,
    initial_revision, insert_revision,
};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
use tokio::sync::watch;

const WAIT: Duration = Duration::from_secs(20);
const QUIET_SECONDS: u64 = 120;
const KEPT: usize = 20;

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

/// An open project in the daemon over a [`TestProject`] and its manual clock: `r1` holds `lib.txt`, task `T-1` exists and the agent `frontend` owns its workspace.
#[cfg(test)]
struct Fixture {
    project: TestProject,
    state: AppState,
    handle: Arc<ProjectHandle>,
    workspaces: WorkspaceService,
    actor: Actor,
    workspace: WorkspaceId,
    dir: PathBuf,
}

#[cfg(test)]
impl Fixture {
    async fn new() -> Self {
        let project = TestProject::new().await;
        let objects = ObjectStore::open(&project.paths().fold_dir()).unwrap();
        let tree = write_initial_tree(&objects, flat(&objects, b"base\n")).unwrap();
        let revision = initial_revision(tree, project.now()).unwrap();
        let stored = write_revision(&objects, &revision).unwrap();
        create_initial_revision(project.store(), &stored, None)
            .await
            .unwrap();
        let clock = Arc::new(project.clock().clone());
        let state: AppState = Arc::new(DaemonState {
            registry: project.registry().clone(),
            projects: Projects::default(),
            clock: clock.clone(),
            assets: AssetSource::Embedded,
            listen: "127.0.0.1:0".parse().unwrap(),
            session_lifetime: DEFAULT_SESSION_LIFETIME,
        });
        let row = ProjectRow {
            slug: "demo".to_owned(),
            name: "Demo".to_owned(),
            root: project.paths().root().to_string_lossy().into_owned(),
        };
        let handle = state.projects.open(&row, clock).await.unwrap();
        let agents = handle.agents();
        let mut session = agents.start_session(1).await.unwrap();
        agents
            .register_agent(&mut session, &AgentName::parse("frontend").unwrap(), &[])
            .await
            .unwrap();
        let actor = agents.actor(&session, None).await.unwrap();
        crate::common::roles::lead_work_areas(&handle.store).await;
        handle
            .tasks
            .create_task(
                &actor,
                CreateTask {
                    title: TaskTitle::parse("task").unwrap(),
                    description: TaskDescription::new("body"),
                    labels: crate::common::roles::work_labels(),
                },
            )
            .await
            .unwrap();
        let workspaces = WorkspaceService::for_project(&handle);
        let view = workspaces
            .create_workspace(&actor, t(1))
            .await
            .unwrap()
            .workspace;
        let dir = project.paths().root().join(&view.path);
        Self {
            project,
            state,
            handle,
            workspaces,
            actor,
            workspace: view.id,
            dir,
        }
    }

    fn reviews(&self) -> ReviewService {
        ReviewService::new(
            self.handle.store.clone(),
            self.handle.bus.clone(),
            self.handle.clock.clone(),
            self.workspaces.clone(),
            self.handle.integration.clone(),
        )
    }

    /// Writes `lib.txt` and lets the live state see it, as the watcher would.
    async fn edit(&self, content: &str) {
        fs::write(self.dir.join("lib.txt"), content).unwrap();
        self.handle.live.rescan(self.workspace).await.unwrap();
    }

    fn lib(&self) -> String {
        fs::read_to_string(self.dir.join("lib.txt")).unwrap()
    }

    fn advance(&self, seconds: u64) -> Timestamp {
        self.project
            .clock()
            .advance(Duration::from_secs(seconds))
            .unwrap()
    }

    async fn checkpoints(&self, include_pruned: bool) -> Vec<Checkpoint> {
        workspaces::list_checkpoints(self.handle.store.read(), self.workspace, include_pruned)
            .await
            .unwrap()
    }

    async fn with_reason(&self, reason: CheckpointReason) -> Vec<Checkpoint> {
        self.checkpoints(true)
            .await
            .into_iter()
            .filter(|checkpoint| checkpoint.reason == reason)
            .collect()
    }

    async fn checkpoint(&self, id: CheckpointId) -> Checkpoint {
        workspaces::get_checkpoint(self.handle.store.read(), id)
            .await
            .unwrap()
            .unwrap()
    }

    async fn events_of(&self, event_type: &str) -> Vec<fold_core::Event> {
        events::read_after(self.handle.store.read(), 0, &EventFilter::default(), 10_000)
            .await
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type.as_str() == event_type)
            .collect()
    }

    /// Records a revision whose `other.txt` is new and moves `main` to it.
    async fn move_main(&self) -> RevisionId {
        let store = &self.handle.store;
        let head = workspaces::head(store.read()).await.unwrap().unwrap();
        let objects = ObjectStore::open(&self.project.paths().fold_dir()).unwrap();
        let mut tree = flat(&objects, b"base\n");
        tree.insert(
            b"other.txt".to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash: objects.write(ObjectKind::Blob, b"from main\n").unwrap(),
            },
        );
        let number = RevisionId::from_number(head.number.number() + 1).unwrap();
        let revision = Revision {
            tree: write_initial_tree(&objects, tree).unwrap(),
            parent: Some(head.hash),
            number,
            integration: None,
            created_at: self.project.now(),
        };
        let stored = write_revision(&objects, &revision).unwrap();
        store
            .write(async |tx| {
                insert_revision(tx, &stored).await?;
                advance_main(tx, head.number, number).await
            })
            .await
            .unwrap();
        number
    }
}

#[cfg(test)]
fn flat(objects: &ObjectStore, lib: &[u8]) -> FlatTree {
    let mut tree = FlatTree::new();
    tree.insert(
        b"lib.txt".to_vec(),
        FlatEntry {
            kind: EntryKind::File,
            hash: objects.write(ObjectKind::Blob, lib).unwrap(),
        },
    );
    tree
}

#[tokio::test]
async fn an_editing_burst_then_two_minutes_of_quiet_gives_one_quiet_checkpoint() {
    let fx = Fixture::new().await;
    let mut quiet = QuietCheckpoints::default();
    for edit in 0..10 {
        fx.edit(&format!("edit {edit}\n")).await;
        fx.advance(10);
        assert_eq!(
            quiet.run(&fx.handle).await,
            0,
            "edit {edit} is not quiet yet"
        );
    }
    fx.advance(QUIET_SECONDS - 11);
    assert_eq!(quiet.run(&fx.handle).await, 0, "a second before the limit");
    fx.advance(1);
    assert_eq!(quiet.run(&fx.handle).await, 1);
    for _ in 0..3 {
        fx.advance(QUIET_SECONDS);
        assert_eq!(quiet.run(&fx.handle).await, 0, "one checkpoint per burst");
    }
    let taken = fx.with_reason(CheckpointReason::Quiet).await;
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].kind, CheckpointKind::Auto);
    let recorded = fx.events_of("checkpoint.recorded").await;
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].actor, EventActor::fold());
    assert_eq!(recorded[0].data["reason"], "quiet");
    let live = fx.handle.live.get(fx.workspace).unwrap();
    assert_eq!(live.view.checkpoints.len(), 1);
    assert!(live.view.checkpoints[0].automatic);

    // The next burst gets its own checkpoint, which restores the burst's last state.
    fx.edit("second burst\n").await;
    fx.advance(QUIET_SECONDS);
    assert_eq!(quiet.run(&fx.handle).await, 1);
    let taken = fx.with_reason(CheckpointReason::Quiet).await;
    assert_eq!(taken.len(), 2);
    fx.edit("lost work\n").await;
    fx.workspaces
        .restore_checkpoint(&fx.actor, taken[1].id)
        .await
        .unwrap();
    assert_eq!(fx.lib(), "second burst\n");
}

#[tokio::test]
async fn an_untouched_workspace_gets_no_quiet_checkpoint() {
    let fx = Fixture::new().await;
    let mut quiet = QuietCheckpoints::default();
    fx.advance(10 * QUIET_SECONDS);
    assert_eq!(quiet.run(&fx.handle).await, 0);
    assert!(fx.checkpoints(true).await.is_empty());
}

#[tokio::test]
async fn a_burst_that_returns_to_the_latest_checkpoint_adds_no_row() {
    let fx = Fixture::new().await;
    let mut quiet = QuietCheckpoints::default();
    fx.edit("kept\n").await;
    let explicit = fx
        .workspaces
        .record_checkpoint(&fx.actor, fx.workspace, Some("before trying"))
        .await
        .unwrap();
    fx.edit("tried\n").await;
    fx.edit("kept\n").await;
    fx.advance(QUIET_SECONDS);
    assert_eq!(quiet.run(&fx.handle).await, 0, "the explicit one holds it");
    let events = fx.events_of("checkpoint.recorded").await.len();
    fx.advance(QUIET_SECONDS);
    assert_eq!(quiet.run(&fx.handle).await, 0);
    let checkpoints = fx.checkpoints(true).await;
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].id, explicit.id);
    assert_eq!(fx.events_of("checkpoint.recorded").await.len(), events);
}

#[tokio::test]
async fn the_quiet_job_skips_a_workspace_another_operation_holds() {
    let fx = Fixture::new().await;
    let mut quiet = QuietCheckpoints::default();
    fx.edit("busy\n").await;
    fx.advance(QUIET_SECONDS);
    let held = fx.handle.workspace_locks.lock(fx.workspace).await;
    assert_eq!(
        fx.workspaces
            .take_quiet_checkpoint(fx.workspace)
            .await
            .unwrap(),
        QuietCheckpoint::Busy
    );
    assert_eq!(quiet.run(&fx.handle).await, 0);
    drop(held);
    assert_eq!(quiet.run(&fx.handle).await, 1);
}

#[tokio::test]
async fn each_command_first_checkpoints_the_state_it_found() {
    let fx = Fixture::new().await;
    let reviews = fx.reviews();

    fx.edit("before the record\n").await;
    let recorded = fx
        .workspaces
        .record_changes(&fx.actor, fx.workspace, "first change")
        .await
        .unwrap();
    let before_record = fx.checkpoint(recorded.checkpoint).await;
    assert_eq!(before_record.reason, CheckpointReason::BeforeRecord);

    fx.edit("before the proposal\n").await;
    let opened = reviews
        .create_proposal(&fx.actor, t(1), "A change", None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    let before_proposal = fx.with_reason(CheckpointReason::BeforeProposal).await;
    assert_eq!(before_proposal.len(), 1);
    assert_eq!(before_proposal[0].tree, opened.selected.tree);

    fx.edit("before the update\n").await;
    let updated = reviews
        .update_proposal(&fx.actor, opened.summary.id, None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    let before_update = fx.with_reason(CheckpointReason::BeforeProposalUpdate).await;
    assert_eq!(before_update.len(), 1);
    assert_eq!(before_update[0].tree, updated.selected.tree);

    fx.edit("before the sync\n").await;
    let base = fx.move_main().await;
    let synced = fx
        .workspaces
        .sync_workspace(&fx.actor, fx.workspace)
        .await
        .unwrap();
    assert_eq!(synced.base, base);
    let before_sync = fx.checkpoint(synced.checkpoint.unwrap()).await;
    assert_eq!(before_sync.reason, CheckpointReason::BeforeSync);
    assert!(fx.dir.join("other.txt").exists());

    for (checkpoint, content) in [
        (&before_sync, "before the sync\n"),
        (&before_update[0], "before the update\n"),
        (&before_proposal[0], "before the proposal\n"),
        (&before_record, "before the record\n"),
    ] {
        assert_eq!(checkpoint.kind, CheckpointKind::Auto);
        fx.workspaces
            .restore_checkpoint(&fx.actor, checkpoint.id)
            .await
            .unwrap();
        assert_eq!(fx.lib(), content, "{:?}", checkpoint.reason);
    }
    assert!(
        !fx.dir.join("other.txt").exists(),
        "the restore put the base from before the sync back"
    );
}

#[tokio::test]
async fn a_failed_checkpoint_refuses_the_command() {
    let fx = Fixture::new().await;
    fx.edit("changed\n").await;
    let human = Actor::human(HumanName::parse("ana").unwrap());
    let WorkspaceError::Fold(refused) = fx
        .workspaces
        .take_auto_checkpoint(&human, fx.workspace, AutoReason::BeforeRecord)
        .await
        .unwrap_err()
    else {
        panic!("a human is refused");
    };
    assert_eq!(refused.code(), codes::NOT_WORKSPACE_OWNER);
    assert!(fx.checkpoints(true).await.is_empty());

    fx.workspaces
        .abandon_workspace(&fx.actor, fx.workspace, "a different approach")
        .await
        .unwrap();
    // The closed workspace's directory goes after its final checkpoint, the only one it gets.
    fold_store::testing::eventually(WAIT, async || {
        let finals = fx.with_reason(CheckpointReason::BeforeRemoval).await.len();
        if !fx.dir.exists() && finals == 1 {
            Ok(())
        } else {
            Err(finals)
        }
    })
    .await;
    let mut quiet = QuietCheckpoints::default();
    fx.advance(QUIET_SECONDS);
    assert_eq!(quiet.run(&fx.handle).await, 0, "the live state is detached");
    // The removal holds the workspace's lock until it has recorded the removal, and a quiet checkpoint skips a busy workspace.
    let refused = fold_store::testing::eventually(WAIT, async || {
        match fx.workspaces.take_quiet_checkpoint(fx.workspace).await {
            Err(WorkspaceError::Fold(refused)) => Ok(refused),
            other => Err(format!("{other:?}")),
        }
    })
    .await;
    assert_eq!(refused.code(), codes::WORKSPACE_NOT_ACTIVE);
    assert_eq!(fx.checkpoints(true).await.len(), 1);
}

#[tokio::test]
async fn the_twenty_first_automatic_checkpoint_prunes_the_oldest() {
    let fx = Fixture::new().await;
    fx.edit("explicit\n").await;
    let explicit = fx
        .workspaces
        .record_checkpoint(&fx.actor, fx.workspace, Some("keep me"))
        .await
        .unwrap();
    let mut taken = Vec::new();
    for n in 0..KEPT {
        fx.edit(&format!("auto {n}\n")).await;
        let auto = fx
            .workspaces
            .take_auto_checkpoint(&fx.actor, fx.workspace, AutoReason::Quiet)
            .await
            .unwrap();
        assert!(auto.recorded);
        assert!(auto.pruned.is_empty(), "{n} automatic checkpoints");
        taken.push(auto.checkpoint);
    }
    fx.edit("the twenty-first\n").await;
    let auto = fx
        .workspaces
        .take_auto_checkpoint(&fx.actor, fx.workspace, AutoReason::BeforeRecord)
        .await
        .unwrap();
    assert_eq!(auto.pruned, [taken[0]]);
    assert!(fx.checkpoint(taken[0]).await.pruned_at.is_some());
    let pruned = fx.events_of("checkpoint.pruned").await;
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].data["checkpoint"], taken[0].to_string());
    assert_eq!(pruned[0].workspace, Some(fx.workspace));

    for n in 0..30 {
        fx.edit(&format!("more {n}\n")).await;
        fx.workspaces
            .take_auto_checkpoint(&fx.actor, fx.workspace, AutoReason::Quiet)
            .await
            .unwrap();
    }
    let unpruned = fx.checkpoints(false).await;
    assert_eq!(unpruned.len(), KEPT + 1);
    assert_eq!(unpruned[0].id, explicit.id, "the explicit one stays");
    assert!(
        unpruned[1..]
            .iter()
            .all(|checkpoint| checkpoint.kind == CheckpointKind::Auto)
    );
    let live = fx.handle.live.get(fx.workspace).unwrap();
    assert_eq!(live.view.checkpoints.len(), KEPT + 1);
}

#[tokio::test]
async fn a_workspace_identical_to_its_latest_checkpoint_gets_no_new_row() {
    let fx = Fixture::new().await;
    fx.edit("once\n").await;
    let first = fx
        .workspaces
        .take_auto_checkpoint(&fx.actor, fx.workspace, AutoReason::Quiet)
        .await
        .unwrap();
    assert!(first.recorded);
    let again = fx
        .workspaces
        .take_auto_checkpoint(&fx.actor, fx.workspace, AutoReason::BeforeRecord)
        .await
        .unwrap();
    assert!(!again.recorded);
    assert_eq!(again.checkpoint, first.checkpoint);
    let recorded = fx
        .workspaces
        .record_changes(&fx.actor, fx.workspace, "the same files")
        .await
        .unwrap();
    assert_eq!(recorded.checkpoint, first.checkpoint);
    assert_eq!(fx.checkpoints(true).await.len(), 1);
    assert_eq!(fx.events_of("checkpoint.recorded").await.len(), 1);

    // The same files on another base are another state: a restore would move the base.
    let before = fx.lib();
    fx.move_main().await;
    let synced = fx
        .workspaces
        .sync_workspace(&fx.actor, fx.workspace)
        .await
        .unwrap();
    assert_eq!(synced.checkpoint, Some(first.checkpoint));
    fs::remove_file(fx.dir.join("other.txt")).unwrap();
    fx.handle.live.rescan(fx.workspace).await.unwrap();
    assert_eq!(fx.lib(), before);
    let moved = fx
        .workspaces
        .take_auto_checkpoint(&fx.actor, fx.workspace, AutoReason::Quiet)
        .await
        .unwrap();
    assert!(moved.recorded, "the base moved");
    assert_eq!(fx.checkpoints(true).await.len(), 2);
}

#[tokio::test]
async fn the_quiet_job_runs_on_its_tick_for_every_open_project() {
    let fx = Fixture::new().await;
    let (stop, stopped) = watch::channel(false);
    let job = spawn_auto_checkpoints(fx.state.clone(), Duration::from_millis(5), stopped);
    let mut progress = job.progress();
    let wait = async |progress: &mut watch::Receiver<JobProgress>,
                      done: &dyn Fn(&JobProgress) -> bool| {
        tokio::time::timeout(WAIT, progress.wait_for(|seen| done(seen)))
            .await
            .unwrap()
            .map(|seen| *seen)
            .unwrap()
    };
    fx.edit("edited\n").await;
    let idle = wait(&mut progress, &|seen| seen.checks >= 3).await;
    assert_eq!(idle.cycles, 0, "the workspace is not quiet yet");
    fx.advance(QUIET_SECONDS);
    wait(&mut progress, &|seen| seen.cycles >= 1).await;
    stop.send(true).unwrap();
    job.stopped(WAIT).await;
    assert_eq!(fx.with_reason(CheckpointReason::Quiet).await.len(), 1);
}

#[tokio::test]
async fn garbage_collection_prunes_with_the_same_rule() {
    let fx = Fixture::new().await;
    for n in 0..=KEPT {
        fx.edit(&format!("auto {n}\n")).await;
        fx.workspaces
            .take_auto_checkpoint(&fx.actor, fx.workspace, AutoReason::Quiet)
            .await
            .unwrap();
    }
    assert_eq!(fx.checkpoints(false).await.len(), KEPT);
    let report = fold_daemon::services::projects::gc(&fx.handle, false)
        .await
        .unwrap();
    assert_eq!(report.checkpoints_pruned, 0, "nothing beyond the limit");
}

#[tokio::test]
async fn a_review_checkout_gets_no_automatic_checkpoint() {
    let fx = Fixture::new().await;
    fx.edit("read only\n").await;
    let number = i64::from(fx.workspace.number());
    fx.handle
        .store
        .write(async |tx| {
            sqlx::query("UPDATE workspaces SET kind = 'review' WHERE number = ?")
                .bind(number)
                .execute(tx.conn())
                .await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    let WorkspaceError::Fold(refused) = fx
        .workspaces
        .take_auto_checkpoint(&Actor::System, fx.workspace, AutoReason::Quiet)
        .await
        .unwrap_err()
    else {
        panic!("a review checkout is refused");
    };
    assert_eq!(refused.code(), codes::INVALID_REQUEST);
    assert_eq!(refused.details().unwrap()["reason"], "review_checkout");
    let mut quiet = QuietCheckpoints::default();
    fx.advance(QUIET_SECONDS);
    assert_eq!(quiet.run(&fx.handle).await, 0);
    assert!(fx.checkpoints(true).await.is_empty());
}
