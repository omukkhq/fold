#![cfg(test)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::common::roles::{give_claim, give_review_claim, give_role};
use crate::reviews_support::{Fixture, p, t};
use crate::worker_resume::{TASK_WORKER, proposed_by_a_task_worker, until_runs, until_state};
use crate::workers_support::FakeClaude;
use fold_core::WorkerState;
use fold_core::reviews::Verdict;
use fold_core::roles::Role;
use fold_daemon::projects::ProjectHandle;
use fold_daemon::services::agents::{FolderDecision, FolderKeep};
use fold_daemon::services::projects::{GcReport, gc};
use fold_daemon::services::workspaces::follow_closed_workspaces;
use fold_store::projects::ProjectRow;
use fold_store::testing::eventually;
use tokio::sync::Notify;

const DAY_MINUTES: u64 = 24 * 60;

/// Writes the files fold writes for a worker into its folder, with a skill copy in its start folder, and returns the folder.
fn folder_of(fx: &Fixture, agent: &str) -> PathBuf {
    let folder = fx.project.paths().workers_dir().join(agent);
    let skill = folder.join("start/.claude/skills/fold");
    fs::create_dir_all(&skill).unwrap();
    fs::write(folder.join("mcp.json"), "{}\n").unwrap();
    fs::write(folder.join("prompt.md"), "the prompt\n").unwrap();
    fs::write(skill.join("SKILL.md"), "the skill\n").unwrap();
    folder
}

/// The bytes [`folder_of`] writes.
const FOLDER_BYTES: u64 = 3 + 11 + 10;

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

fn decisions(report: &GcReport) -> Vec<(String, FolderDecision)> {
    report
        .workers
        .iter()
        .map(|folder| (folder.agent.to_string(), folder.decision))
        .collect()
}

/// Task workers and reviewers that stopped long ago in every state the folder rule tells apart, a worker that stopped a moment ago, and the leads `frontend` and `backend`.
async fn workers(fx: &Fixture) -> Vec<(&'static str, PathBuf)> {
    let store = fx.project.store();
    let mut folders = Vec::new();
    for (agent, role) in [
        ("reviewer-1", Role::Reviewer),
        ("web-worker-1", Role::TaskWorker),
        ("web-worker-2", Role::TaskWorker),
        ("web-worker-3", Role::TaskWorker),
        ("reviewer-2", Role::Reviewer),
        ("reviewer-3", Role::Reviewer),
    ] {
        let area = (role == Role::TaskWorker).then_some("frontend-work");
        give_role(store, agent, role, area).await;
        folders.push((agent, folder_of(fx, agent)));
    }
    let now = fx.project.now();
    give_claim(store, "web-worker-1", t(1), now).await;
    let frontend = fx.frontend.worker.agent_id;
    let start = fx.project.paths().workers_dir().join("reviewer-2/start");
    fx.execute(&format!(
        "UPDATE worker_processes SET parked_by = {frontend}, parked_for = 'the go', parked_at = 1
           WHERE agent_id = (SELECT id FROM agents WHERE name = 'web-worker-2');
         UPDATE worker_processes SET wait_reason = 'network', wait_since = 1, wait_tried_at = 1
           WHERE agent_id = (SELECT id FROM agents WHERE name = 'reviewer-3');
         UPDATE worker_processes SET stopped_at = {}
           WHERE agent_id = (SELECT id FROM agents WHERE name = 'web-worker-3');
         INSERT INTO sessions (pid, started_at, last_call_at, cwd) VALUES (4242, 0, 0, '{}');",
        now.as_millis(),
        start.display(),
    ))
    .await;
    for lead in ["frontend", "backend"] {
        folders.push((lead, folder_of(fx, lead)));
    }
    folders
}

#[tokio::test]
async fn gc_removes_the_folders_of_finished_workers_and_names_why_it_keeps_the_others() {
    let fx = Fixture::new().await;
    let folders = workers(&fx).await;
    let handle = handle(&fx).await;

    let planned = gc(&handle, true).await.unwrap();
    let expected = vec![
        ("reviewer-1".to_owned(), FolderDecision::Remove),
        (
            "reviewer-2".to_owned(),
            FolderDecision::Keep(FolderKeep::Session),
        ),
        (
            "reviewer-3".to_owned(),
            FolderDecision::Keep(FolderKeep::Waiting),
        ),
        (
            "web-worker-1".to_owned(),
            FolderDecision::Keep(FolderKeep::Claim),
        ),
        (
            "web-worker-2".to_owned(),
            FolderDecision::Keep(FolderKeep::Parked),
        ),
    ];
    assert_eq!(
        decisions(&planned),
        expected,
        "a worker that stopped within the window and the leads are not listed"
    );
    assert_eq!(planned.workers_removed, 1);
    assert_eq!(planned.worker_bytes_freed, FOLDER_BYTES);
    assert!(
        planned
            .workers
            .iter()
            .all(|folder| folder.bytes == FOLDER_BYTES)
    );
    for (agent, folder) in &folders {
        assert!(
            folder.join("prompt.md").is_file(),
            "a dry run keeps {agent}"
        );
    }

    let report = gc(&handle, false).await.unwrap();
    assert_eq!(decisions(&report), expected);
    assert_eq!(report.workers_removed, 1);
    assert_eq!(report.worker_bytes_freed, FOLDER_BYTES);
    for (agent, folder) in &folders {
        assert_eq!(folder.exists(), *agent != "reviewer-1", "{agent}");
    }
    let names: Vec<String> = fs::read_dir(fx.project.paths().workers_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(names.iter().all(|name| !name.starts_with('.')), "{names:?}");

    fx.advance(14 * DAY_MINUTES + 1);
    let later = gc(&handle, true).await.unwrap();
    assert!(
        decisions(&later).contains(&("web-worker-3".to_owned(), FolderDecision::Remove)),
        "{:?}",
        later.workers
    );
}

#[tokio::test]
async fn a_worker_folder_that_is_a_link_is_never_followed() {
    let fx = Fixture::new().await;
    give_role(fx.project.store(), "reviewer-1", Role::Reviewer, None).await;
    let outside = fx.project.paths().root().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("keep.txt"), "keep\n").unwrap();
    let workers = fx.project.paths().workers_dir();
    fs::create_dir_all(&workers).unwrap();
    std::os::unix::fs::symlink(&outside, workers.join("reviewer-1")).unwrap();

    let report = gc(&handle(&fx).await, false).await.unwrap();
    assert_eq!(
        decisions(&report),
        vec![(
            "reviewer-1".to_owned(),
            FolderDecision::Keep(FolderKeep::InvalidPath)
        )]
    );
    assert!(outside.join("keep.txt").is_file());
    assert!(workers.join("reviewer-1").is_symlink());
}

#[tokio::test]
async fn the_daemon_removes_finished_workers_folders_when_it_opens_the_project() {
    let fx = Fixture::new().await;
    give_role(fx.project.store(), "reviewer-1", Role::Reviewer, None).await;
    let folder = folder_of(&fx, "reviewer-1");
    let catch_up = Arc::new(Notify::new());
    let follower = tokio::spawn(follow_closed_workspaces(
        fx.workspaces.clone(),
        fx.bus.clone(),
        fx.bus.subscribe(),
        catch_up.clone(),
    ));
    catch_up.notify_one();
    eventually(crate::common::acceptance::WAIT, async || {
        if folder.exists() { Err(()) } else { Ok(()) }
    })
    .await;
    follower.abort();
}

/// A task worker that finished its task stops; its folder stays through the window, then goes, and a message that starts it again later writes the folder anew in the same place, where its session resumes.
#[tokio::test]
async fn a_finished_task_workers_folder_goes_after_the_window_and_a_later_start_writes_it_again() {
    let fake = FakeClaude::new("result");
    let (mut harness, _work) =
        proposed_by_a_task_worker(&fake, &[("page.html", "<p>page</p>\n")]).await;
    harness.disconnect(TASK_WORKER).await;
    let folder = Path::new(harness.root())
        .join(".fold/workers")
        .join(TASK_WORKER);
    assert!(folder.join("prompt.md").is_file());
    let report = gc(&harness.handle, false).await.unwrap();
    assert!(
        report.workers.is_empty(),
        "it stopped within the window: {:?}",
        report.workers
    );

    harness
        .project
        .clock()
        .advance(std::time::Duration::from_secs(15 * DAY_MINUTES * 60))
        .unwrap();
    let planned = gc(&harness.handle, true).await.unwrap();
    assert_eq!(
        planned
            .workers
            .iter()
            .find(|folder| folder.agent.as_str() == TASK_WORKER)
            .map(|folder| folder.decision),
        Some(FolderDecision::Keep(FolderKeep::Claim)),
        "its task's proposal is open: {:?}",
        planned.workers
    );
    harness
        .handle
        .store
        .write(async |tx| {
            sqlx::query("UPDATE claims SET released_at = 1, release_reason = 'released'")
                .execute(tx.conn())
                .await?;
            Ok::<_, fold_store::StoreError>(())
        })
        .await
        .unwrap();
    let report = gc(&harness.handle, false).await.unwrap();
    assert_eq!(report.workers_removed, 1, "{:?}", report.workers);
    assert!(!folder.exists());

    harness
        .send("frontend", &[TASK_WORKER], "One more look, please.")
        .await;
    let runs = until_runs(&fake, TASK_WORKER, 2).await;
    assert_eq!(Path::new(&runs[1].cwd), folder.join("start"));
    assert!(runs[1].has("--resume"), "the same session goes on");
    assert!(folder.join("prompt.md").is_file());
    assert!(folder.join("mcp.json").is_file());
    until_state(&harness, TASK_WORKER, WorkerState::Idle).await;
}

/// The folder of `agent` once a gc run has decided about it.
async fn decision_of(handle: &ProjectHandle, agent: &str, dry_run: bool) -> Option<FolderDecision> {
    gc(handle, dry_run)
        .await
        .unwrap()
        .workers
        .iter()
        .find(|folder| folder.agent.as_str() == agent)
        .map(|folder| folder.decision)
}

#[tokio::test]
async fn a_finished_task_workers_folder_stays_while_fold_owes_it_a_resume() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    let store = fx.project.store();
    give_role(
        store,
        "web-worker-1",
        Role::TaskWorker,
        Some("frontend-work"),
    )
    .await;
    let folder = folder_of(&fx, "web-worker-1");
    // `web-worker-1` wrote `P-1` and is bound to its task, as a task worker that fold started.
    fx.execute(
        "UPDATE proposals SET author_worker_id = (SELECT w.id FROM workers w JOIN agents a ON a.id = w.agent_id WHERE a.name = 'web-worker-1');
         INSERT INTO worker_tasks (worker_process_id, task_id)
           SELECT p.id, (SELECT id FROM tasks WHERE number = 1) FROM worker_processes p JOIN agents a ON a.id = p.agent_id WHERE a.name = 'web-worker-1';",
    )
    .await;
    give_role(store, "review", Role::Reviewer, None).await;
    give_review_claim(store, "review", p(1), fx.project.now()).await;
    let review = fx.agents.actor(&fx.review.ctx).await.unwrap();
    fx.reviews
        .submit_review(
            &review,
            p(1),
            1,
            Verdict::RequestChanges,
            "Split it.",
            Vec::new(),
        )
        .await
        .unwrap();
    let handle = handle(&fx).await;
    assert_eq!(
        decision_of(&handle, "web-worker-1", false).await,
        Some(FolderDecision::Keep(FolderKeep::OwedResume))
    );
    assert!(folder.join("prompt.md").is_file());

    // The resume that serves the requested changes moves the start; the worker stops again after it.
    let served = fx.project.now().as_millis() + 1;
    fx.execute(&format!(
        "UPDATE worker_processes SET started_at = {served}, stopped_at = {}
           WHERE agent_id = (SELECT id FROM agents WHERE name = 'web-worker-1')",
        served + 1
    ))
    .await;
    assert_eq!(
        decision_of(&handle, "web-worker-1", true).await,
        None,
        "it stopped within the window"
    );
    fx.advance(14 * DAY_MINUTES + 1);
    assert_eq!(
        decision_of(&handle, "web-worker-1", false).await,
        Some(FolderDecision::Remove)
    );
    assert!(!folder.exists());
}

#[tokio::test]
async fn a_finished_reviewers_folder_stays_while_a_message_waits_for_it() {
    let fx = Fixture::new().await;
    give_role(fx.project.store(), "reviewer-1", Role::Reviewer, None).await;
    let folder = folder_of(&fx, "reviewer-1");
    fx.execute(
        "INSERT INTO messages (number, thread_id, sender_kind, sender_human, body, created_at)
           VALUES (1, 1, 'human', 'ana', 'One more look, please.', 2);
         INSERT INTO message_recipients (message_id, worker_id, addressed_as)
           SELECT m.id, w.id, 'agent' FROM messages m, workers w JOIN agents a ON a.id = w.agent_id
           WHERE m.number = 1 AND a.name = 'reviewer-1';",
    )
    .await;
    let handle = handle(&fx).await;
    assert_eq!(
        decision_of(&handle, "reviewer-1", true).await,
        Some(FolderDecision::Keep(FolderKeep::OwedMessage))
    );
    assert_eq!(
        decision_of(&handle, "reviewer-1", false).await,
        Some(FolderDecision::Keep(FolderKeep::OwedMessage))
    );
    assert!(folder.join("prompt.md").is_file());

    // The worker read the message, so nothing waits for it.
    fx.execute("UPDATE message_recipients SET delivered_at = 3, acked_at = 3")
        .await;
    assert_eq!(
        decision_of(&handle, "reviewer-1", false).await,
        Some(FolderDecision::Remove)
    );
    assert!(!folder.exists());
}
