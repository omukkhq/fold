#![cfg(test)]

use std::fs::{self, Permissions};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::common::roles::give_role;
use crate::reviews_support::{Fixture, by, p, t, ws};
use fold_core::events::types;
use fold_core::reviews::Verdict;
use fold_core::roles::Role;
use fold_core::{Actor, AgentActor, EventActor, MAIN_TAG, NewEvent, Tag};
use fold_daemon::projects::ProjectHandle;
use fold_daemon::services::events::EventBuffer;
use fold_daemon::services::projects::gc;
use fold_daemon::services::reviews::IntegrationOutcome;
use fold_daemon::services::workspaces::{DirectoryRemoval, follow_closed_workspaces};
use fold_store::StoreError;
use fold_store::projects::ProjectRow;
use fold_store::testing::eventually;
use fold_store::workspaces::{CheckpointReason, list_checkpoints};
use fold_vcs::compare::load_flat;
use fold_vcs::objects::ObjectStore;
use serde_json::json;
use tokio::sync::Notify;

const WAIT: Duration = Duration::from_secs(10);
const UNRECORDED: &str = "pub fn unrecorded() {}\n";

/// The project task that removes closed workspace directories, over the fixture's bus. Dropping it stops the task.
struct Follower(tokio::task::JoinHandle<()>);

impl Drop for Follower {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn follow(fx: &Fixture) -> Follower {
    Follower(tokio::spawn(follow_closed_workspaces(
        fx.workspaces.clone(),
        fx.bus.clone(),
        fx.bus.subscribe(),
        Arc::new(Notify::new()),
    )))
}

/// `frontend` takes `T-1`, changes `src/lib.rs` without recording it, builds into the ignored `target/` and abandons the workspace.
async fn abandoned_with_changes(fx: &Fixture) {
    let author = by(&fx.frontend.worker);
    fx.workspaces.create_workspace(&author, t(1)).await.unwrap();
    fx.write("src/lib.rs", UNRECORDED);
    fx.write(".gitignore", "target/\n");
    fx.write("target/debug/app", "build output\n");
    fx.workspaces
        .abandon_workspace(&author, ws(1), "the plan changed")
        .await
        .unwrap();
}

async fn removed(fx: &Fixture, workspace: u32) -> bool {
    fx.count(&format!(
        "SELECT COUNT(*) FROM workspaces WHERE number = {workspace} AND directory_removed_at IS NOT NULL"
    ))
    .await
        == 1
}

async fn final_checkpoints(fx: &Fixture) -> i64 {
    fx.count("SELECT COUNT(*) FROM checkpoints WHERE reason = 'before_removal'")
        .await
}

async fn until_removed(fx: &Fixture, workspace: u32, dir: &Path) {
    eventually(WAIT, async || {
        let state = (dir.exists(), removed(fx, workspace).await);
        if state == (false, true) {
            Ok(())
        } else {
            Err(state)
        }
    })
    .await;
}

async fn review_lead(fx: &Fixture) -> Actor {
    give_role(fx.project.store(), "review", Role::ReviewLead, None).await;
    fx.agents.actor(&fx.review.ctx, None).await.unwrap()
}

async fn handle(fx: &Fixture) -> ProjectHandle {
    let row = ProjectRow {
        slug: "demo".to_owned(),
        name: "demo".to_owned(),
        root: fx.project.paths().root().to_string_lossy().into_owned(),
    };
    ProjectHandle::open(&row, Arc::new(fx.project.clock().clone()))
        .await
        .unwrap()
}

#[tokio::test]
async fn an_integrated_workspace_and_its_review_checkout_lose_their_directories() {
    let fx = Fixture::new().await;
    let _follower = follow(&fx);
    fx.proposal().await;
    let review = review_lead(&fx).await;
    fx.reviews
        .submit_review(&review, p(1), 1, Verdict::Approve, "fine", Vec::new())
        .await
        .unwrap();
    fx.reviews.claim_review(&review, p(1)).await.unwrap();
    let checkout = fx
        .reviews
        .create_review_checkout(&review, p(1))
        .await
        .unwrap()
        .workspace;
    let checkout_dir = fx.project.paths().root().join(&checkout.path);
    assert!(checkout_dir.join("src/new.rs").is_file());

    let outcome = fx
        .reviews
        .integrate_proposal(&review, p(1), None)
        .await
        .unwrap();
    assert!(matches!(outcome, IntegrationOutcome::Succeeded { .. }));

    until_removed(&fx, 1, &fx.task_dir()).await;
    until_removed(&fx, checkout.id.number(), &checkout_dir).await;
    assert_eq!(
        final_checkpoints(&fx).await,
        0,
        "the integrated files are the proposal's version"
    );
    assert!(fx.ws_entries().iter().all(|name| !name.starts_with("t-1")));
    assert!(fx.project.paths().root().join("README.md").is_file());
}

#[tokio::test]
async fn unrecorded_tracked_files_get_a_checkpoint_before_the_directory_goes() {
    let fx = Fixture::new().await;
    let _follower = follow(&fx);
    abandoned_with_changes(&fx).await;

    until_removed(&fx, 1, &fx.task_dir()).await;
    let checkpoints = list_checkpoints(fx.project.store().read(), ws(1), false)
        .await
        .unwrap();
    let last = checkpoints.last().unwrap();
    assert_eq!(last.reason, CheckpointReason::BeforeRemoval);
    assert_eq!(final_checkpoints(&fx).await, 1);
    let objects = ObjectStore::open(&fx.project.paths().fold_dir()).unwrap();
    let files = load_flat(&objects, &last.tree).unwrap();
    let paths: Vec<String> = files
        .keys()
        .map(|path| String::from_utf8(path.clone()).unwrap())
        .collect();
    assert!(paths.contains(&".gitignore".to_owned()), "{paths:?}");
    assert!(paths.contains(&"README.md".to_owned()), "{paths:?}");
    assert!(
        paths.iter().all(|path| !path.starts_with("target")),
        "ignored build output is not checkpointed: {paths:?}"
    );
    let lib = objects
        .read(&files[b"src/lib.rs".as_slice()].hash)
        .unwrap()
        .payload;
    assert_eq!(lib, UNRECORDED.as_bytes());
    let recorded = fx
        .events_after(0)
        .await
        .into_iter()
        .rfind(|event| event.event_type == types::CHECKPOINT_RECORDED)
        .unwrap();
    assert_eq!(recorded.actor.as_str(), "fold");
    assert_eq!(recorded.data["reason"], "before_removal");
}

#[tokio::test]
async fn a_closed_workspace_waits_until_the_worker_process_of_its_agent_ends() {
    let fx = Fixture::new().await;
    let _follower = follow(&fx);
    // `frontend` leads an area, so its process row is there, stopped; it runs again.
    fx.execute(&format!(
        "UPDATE worker_processes SET state = 'working', stopped_at = NULL WHERE agent_id = {}",
        fx.frontend.worker.agent_id
    ))
    .await;
    abandoned_with_changes(&fx).await;
    assert_eq!(
        fx.workspaces.remove_closed_directory(ws(1)).await.unwrap(),
        DirectoryRemoval::NotDue
    );
    assert!(fx.task_dir().join("src/lib.rs").is_file());
    assert_eq!(final_checkpoints(&fx).await, 0);

    let now = fx.project.now();
    let mut buffer = EventBuffer::new();
    fx.project
        .store()
        .write(async |tx| {
            sqlx::query("UPDATE worker_processes SET state = 'stopped', stopped_at = 1")
                .execute(tx.conn())
                .await?;
            let event = NewEvent::new(types::WORKER_STOPPED, EventActor::fold())
                .unwrap()
                .data(json!({
                    "agent": "frontend",
                    "exit_code": 0,
                    "turns": 0,
                    "cost_micro_usd": 0,
                    "cache_read_tokens": 0,
                    "cache_write_tokens": 0,
                }))
                .unwrap();
            buffer.append(tx, &event, now).await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    fx.bus.publish(buffer);

    until_removed(&fx, 1, &fx.task_dir()).await;
    assert_eq!(final_checkpoints(&fx).await, 1);
}

#[tokio::test]
async fn the_project_folder_the_main_workspace_and_active_workspaces_are_never_touched() {
    let fx = Fixture::with_main_frontend().await;
    let root = fx.project.paths().root().to_owned();
    let main = Actor::Agent(AgentActor {
        agent: fx.frontend.worker.name.agent(),
        worker: fx.frontend.worker.name.clone(),
        tags: vec![Tag::parse(MAIN_TAG).unwrap()],
    });
    let opened = fx.workspaces.open_main_workspace(&main).await.unwrap();
    let main_dir = root.join(&opened.workspace.path);
    let task = fx
        .workspaces
        .create_workspace(&by(&fx.backend.worker), t(1))
        .await
        .unwrap();
    let task_dir = root.join(&task.workspace.path);
    let ws_dir = fx.project.paths().ws_dir();
    fs::create_dir_all(root.join("outside")).unwrap();
    fs::write(root.join("outside/keep.txt"), "keep\n").unwrap();
    fs::create_dir_all(ws_dir.join("nested/deep")).unwrap();
    fs::write(ws_dir.join("nested/deep/keep.txt"), "keep\n").unwrap();
    symlink(&root, ws_dir.join("link")).unwrap();
    let absolute = root.to_string_lossy().into_owned();
    let paths = [
        ".",
        "",
        ".fold",
        ".fold/ws",
        absolute.as_str(),
        ".fold/ws/../../outside",
        ".fold/ws/link",
        ".fold/ws/nested/deep",
    ];
    for (n, path) in (90..).zip(paths) {
        fx.execute(&format!(
            "INSERT INTO workspaces (number, task_id, kind, path, base_revision_id, state, created_at, closed_at)
             VALUES ({n}, (SELECT id FROM tasks WHERE number = 2), 'task', '{path}', 1, 'archived', 0, 0)"
        ))
        .await;
        let error = fx
            .workspaces
            .remove_closed_directory(ws(n))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some("invalid_path"), "{path}");
    }
    for live in [opened.workspace.id, task.workspace.id] {
        assert_eq!(
            fx.workspaces.remove_closed_directory(live).await.unwrap(),
            DirectoryRemoval::NotDue
        );
    }
    assert_eq!(fx.workspaces.remove_closed_directories(&|| false).await, 0);

    assert!(root.join("README.md").is_file());
    assert!(fx.project.paths().db().is_file());
    assert!(main_dir.join("README.md").is_file());
    assert!(task_dir.join("src/lib.rs").is_file());
    assert!(root.join("outside/keep.txt").is_file());
    assert!(ws_dir.join("nested/deep/keep.txt").is_file());
    assert!(ws_dir.join("link").is_symlink());
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspaces WHERE directory_removed_at IS NOT NULL OR removal_started_at IS NOT NULL")
            .await,
        0
    );
    assert_eq!(final_checkpoints(&fx).await, 0);
}

#[tokio::test]
async fn a_removal_that_fails_leaves_the_row_for_the_next_gc() {
    let fx = Fixture::new().await;
    abandoned_with_changes(&fx).await;
    let ws_dir = fx.project.paths().ws_dir();
    let mode = fs::metadata(&ws_dir).unwrap().permissions().mode();
    fs::set_permissions(&ws_dir, Permissions::from_mode(0o500)).unwrap();
    if fs::create_dir(ws_dir.join("probe")).is_ok() {
        fs::remove_dir(ws_dir.join("probe")).unwrap();
        fs::set_permissions(&ws_dir, Permissions::from_mode(mode)).unwrap();
        eprintln!("skipped: a read-only folder does not stop this user from deleting");
        return;
    }
    let failed = fx.workspaces.remove_closed_directory(ws(1)).await;
    fs::set_permissions(&ws_dir, Permissions::from_mode(mode)).unwrap();
    assert!(failed.is_err());
    assert!(fx.task_dir().exists());
    assert!(!removed(&fx, 1).await);
    assert_eq!(final_checkpoints(&fx).await, 1);

    let handle = handle(&fx).await;
    let planned = gc(&handle, true).await.unwrap();
    assert_eq!(planned.workspaces_removed, 1);
    let report = gc(&handle, false).await.unwrap();
    assert_eq!(report.workspaces_removed, 1);
    assert!(!fx.task_dir().exists());
    assert!(removed(&fx, 1).await);
    assert_eq!(
        final_checkpoints(&fx).await,
        1,
        "what is left of a started removal is only deleted"
    );
}

#[tokio::test]
async fn a_tracked_file_the_final_checkpoint_cannot_read_keeps_the_directory() {
    let fx = Fixture::new().await;
    abandoned_with_changes(&fx).await;
    let lib = fx.task_dir().join("src/lib.rs");
    fs::set_permissions(&lib, Permissions::from_mode(0o000)).unwrap();
    if fs::read(&lib).is_ok() {
        fs::set_permissions(&lib, Permissions::from_mode(0o644)).unwrap();
        eprintln!("skipped: this user reads a file without read permission");
        return;
    }
    let refused = fx.workspaces.remove_closed_directory(ws(1)).await;
    fs::set_permissions(&lib, Permissions::from_mode(0o644)).unwrap();
    assert_eq!(refused.unwrap_err().code(), Some("unrecorded_changes"));
    assert_eq!(fs::read_to_string(&lib).unwrap(), UNRECORDED);
    assert_eq!(final_checkpoints(&fx).await, 0);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspaces WHERE number = 1 AND (removal_started_at IS NOT NULL OR directory_removed_at IS NOT NULL)")
            .await,
        0,
        "nothing starts, so the next try takes the checkpoint"
    );

    let removal = fx.workspaces.remove_closed_directory(ws(1)).await.unwrap();
    assert!(
        matches!(
            removal,
            DirectoryRemoval::Removed {
                checkpoint: Some(_)
            }
        ),
        "{removal:?}"
    );
    assert!(!fx.task_dir().exists());
    assert_eq!(final_checkpoints(&fx).await, 1);
}
