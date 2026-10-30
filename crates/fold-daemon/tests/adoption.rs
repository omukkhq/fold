#![cfg(test)]

use crate::common::intents::Covering as _;
use std::fs;
use std::pin::pin;
use std::time::Duration;

use crate::adoption_support::{
    EDITED, Fixture, LEASE_MINUTES, LIB, by, names, r, snapshot, t, task_refusal,
    workspace_refusal, ws,
};
use fold_core::{TaskStatus, codes};
use fold_daemon::services::workspaces::WorkspaceRef;
use serde_json::json;

const STALL: Duration = Duration::from_millis(300);
const WAIT: Duration = Duration::from_secs(10);

/// Worker `frontend` creates the workspace of `T-1`, edits `src/lib.rs`, adds a file, records a checkpoint, has a conflicted path and releases the claim. `main` then moves on to `r2`.
async fn released_workspace(fx: &Fixture) {
    fx.workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    let dir = fx.dir(".fold/ws/t-1");
    fs::write(dir.join("src/lib.rs"), EDITED).unwrap();
    fx.workspaces
        .record_checkpoint(&by(&fx.frontend), ws(1), Some("halfway"))
        .await
        .unwrap();
    fs::write(dir.join("notes.txt"), b"unrecorded\n").unwrap();
    fx.execute(
        "INSERT INTO workspace_conflicts (workspace_id, path, kind)
         SELECT id, CAST('README.md' AS BLOB), 'both_modified_text' FROM workspaces",
    )
    .await;
    fx.tasks
        .release_task(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    assert_eq!(fx.move_main(b"pub fn y() {}\n").await, r(2));
}

/// A worker that released its claim while its proposal waits for review leaves the proposal with the workspace: the next claimant adopts both and can add the next version.
#[tokio::test]
async fn the_next_claimant_takes_over_the_open_proposal_with_the_workspace() {
    let fx = Fixture::new().await;
    let reviews = fx.reviews();
    fx.workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    let dir = fx.dir(".fold/ws/t-1");
    fs::write(dir.join("src/lib.rs"), EDITED).unwrap();
    let proposal = reviews
        .create_proposal_covering(&by(&fx.frontend), t(1), "Edit the lib", None)
        .await
        .unwrap()
        .detail()
        .unwrap()
        .summary
        .id;
    fx.tasks
        .release_task(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    let seq = fx.last_seq().await;

    fx.tasks.claim_task(&by(&fx.backend), t(1)).await.unwrap();
    let appended = fx.events_after(seq).await;
    assert_eq!(
        names(&appended),
        vec![
            "claim.acquired",
            "workspace.adopted",
            "proposal.author_changed"
        ]
    );
    assert_eq!(appended[2].proposal, Some(proposal));
    assert_eq!(
        appended[2].data,
        json!({"reason": "released", "from": "frontend", "to": "backend"})
    );
    fs::write(dir.join("src/lib.rs"), b"pub fn x() { done() }\n").unwrap();
    let updated = reviews
        .update_proposal_covering(&by(&fx.backend), proposal, None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    assert_eq!(updated.summary.current_version, 2);
    assert_eq!(updated.summary.author, fx.backend.name);

    let seq = fx.last_seq().await;
    fx.tasks.claim_task(&by(&fx.backend), t(1)).await.unwrap();
    assert!(
        fx.events_after(seq).await.is_empty(),
        "a repeat claim by the author hands nothing over"
    );
}

#[tokio::test]
async fn the_next_claimant_adopts_the_released_workspace_as_it_is() {
    let fx = Fixture::new().await;
    released_workspace(&fx).await;
    let before = fx
        .workspaces
        .details(WorkspaceRef::Task(t(1)))
        .await
        .unwrap();
    let files = snapshot(&fx.dir(".fold/ws/t-1"));
    let seq = fx.last_seq().await;

    let claimed = fx.tasks.claim_task(&by(&fx.backend), t(1)).await.unwrap();
    assert_eq!(claimed.claim.holder, fx.backend.name);
    assert_eq!(claimed.workspace, Some(ws(1)));

    let appended = fx.events_after(seq).await;
    assert_eq!(
        names(&appended),
        vec!["claim.acquired", "workspace.adopted"]
    );
    let adopted = &appended[1];
    assert_eq!(adopted.actor.as_str(), "backend");
    assert_eq!(adopted.task, Some(t(1)));
    assert_eq!(adopted.workspace, Some(ws(1)));
    assert_eq!(
        adopted.data,
        json!({"reason": "released", "from": "frontend", "to": "backend"})
    );

    let after = fx
        .workspaces
        .details(WorkspaceRef::Task(t(1)))
        .await
        .unwrap();
    assert_eq!(after.workspace.worker, Some(fx.backend.name.clone()));
    let mut expected = before.clone();
    expected.workspace.worker = Some(fx.backend.name.clone());
    assert_eq!(after, expected);
    assert_eq!(after.workspace.path, ".fold/ws/t-1");
    assert_eq!(after.workspace.base, r(1));
    assert_eq!(after.conflicted, vec!["README.md"]);
    assert!(after.latest_checkpoint.is_some());
    assert_eq!(snapshot(&fx.dir(".fold/ws/t-1")), files);
    assert_eq!(
        fs::read(fx.dir(".fold/ws/t-1/src/lib.rs")).unwrap(),
        EDITED,
        "main moved on, and nothing synced the adopted workspace"
    );
    assert_eq!(
        fx.tasks.get(t(1)).await.unwrap().status,
        TaskStatus::InProgress
    );

    let again = fx
        .workspaces
        .create_workspace(&by(&fx.backend), t(1))
        .await
        .unwrap_err();
    assert_eq!(workspace_refusal(again).code(), codes::WORKSPACE_EXISTS);
    assert_eq!(fx.count("SELECT COUNT(*) FROM workspaces").await, 1);

    let old = fx
        .workspaces
        .record_checkpoint(&by(&fx.frontend), ws(1), None)
        .await
        .unwrap_err();
    assert_eq!(workspace_refusal(old).code(), codes::NOT_WORKSPACE_OWNER);
    fx.workspaces
        .record_checkpoint(&by(&fx.backend), ws(1), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn create_workspace_adopts_an_orphaned_workspace_and_creates_nothing() {
    let fx = Fixture::new().await;
    released_workspace(&fx).await;
    let files = snapshot(&fx.dir(".fold/ws/t-1"));
    let seq = fx.last_seq().await;

    let created = fx
        .workspaces
        .create_workspace(&by(&fx.backend), t(1))
        .await
        .unwrap();
    assert!(created.adopted);
    assert_eq!(created.workspace.id, ws(1));
    assert_eq!(created.workspace.path, ".fold/ws/t-1");
    assert_eq!(created.workspace.base, r(1));
    assert_eq!(created.workspace.worker, Some(fx.backend.name.clone()));
    assert_eq!(created.workspace.conflicted, 1);
    assert_eq!(
        names(&fx.events_after(seq).await),
        vec!["claim.acquired", "workspace.adopted"]
    );
    assert_eq!(fx.count("SELECT COUNT(*) FROM workspaces").await, 1);
    assert_eq!(snapshot(&fx.dir(".fold/ws/t-1")), files);
    let entries: Vec<_> = fs::read_dir(fx.project.paths().ws_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["t-1"]);
}

#[tokio::test]
async fn an_abandoned_workspace_is_not_adopted() {
    let fx = Fixture::new().await;
    fx.workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    fx.workspaces
        .abandon_workspace(&by(&fx.frontend), ws(1), "wrong approach")
        .await
        .unwrap();
    fx.tasks
        .release_task(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    let seq = fx.last_seq().await;

    let claimed = fx.tasks.claim_task(&by(&fx.backend), t(1)).await.unwrap();
    assert_eq!(claimed.workspace, None);
    assert_eq!(names(&fx.events_after(seq).await), vec!["claim.acquired"]);
    let created = fx
        .workspaces
        .create_workspace(&by(&fx.backend), t(1))
        .await
        .unwrap();
    assert!(!created.adopted);
    assert_eq!(
        (created.workspace.id, created.workspace.path.as_str()),
        (ws(2), ".fold/ws/t-1-2")
    );
    let abandoned = fx
        .workspaces
        .get(WorkspaceRef::Workspace(ws(1)))
        .await
        .unwrap();
    assert_eq!(abandoned.worker, Some(fx.frontend.name.clone()));
}

#[tokio::test]
async fn a_live_holders_workspace_is_not_adopted() {
    let fx = Fixture::new().await;
    fx.workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    let seq = fx.last_seq().await;

    let claim = fx
        .tasks
        .claim_task(&by(&fx.backend), t(1))
        .await
        .unwrap_err();
    assert_eq!(task_refusal(claim).code(), codes::ALREADY_CLAIMED);
    let create = fx
        .workspaces
        .create_workspace(&by(&fx.backend), t(1))
        .await
        .unwrap_err();
    assert_eq!(workspace_refusal(create).code(), codes::ALREADY_CLAIMED);
    fx.advance(Duration::from_secs(LEASE_MINUTES * 60));
    let stale = fx
        .tasks
        .claim_task(&by(&fx.backend), t(1))
        .await
        .unwrap_err();
    assert_eq!(task_refusal(stale).code(), codes::ALREADY_CLAIMED);

    let own = fx
        .workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap_err();
    assert_eq!(workspace_refusal(own).code(), codes::WORKSPACE_EXISTS);
    let renewed = fx.tasks.claim_task(&by(&fx.frontend), t(1)).await.unwrap();
    assert_eq!(renewed.workspace, Some(ws(1)));

    assert!(fx.events_after(seq).await.is_empty());
    let workspace = fx.workspaces.get(WorkspaceRef::Task(t(1))).await.unwrap();
    assert_eq!(workspace.worker, Some(fx.frontend.name.clone()));
}

#[tokio::test]
async fn a_worker_that_released_its_own_workspace_gets_it_back_without_an_adoption() {
    let fx = Fixture::new().await;
    fx.workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    fx.tasks
        .release_task(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    let seq = fx.last_seq().await;

    let own = fx
        .workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap_err();
    assert_eq!(workspace_refusal(own).code(), codes::WORKSPACE_EXISTS);
    assert!(
        fx.events_after(seq).await.is_empty(),
        "the refusal takes no claim"
    );
    let claimed = fx.tasks.claim_task(&by(&fx.frontend), t(1)).await.unwrap();
    assert_eq!(claimed.workspace, Some(ws(1)));
    assert_eq!(names(&fx.events_after(seq).await), vec!["claim.acquired"]);
}

#[tokio::test]
async fn a_workspace_without_an_owner_is_adopted() {
    let fx = Fixture::new().await;
    fx.workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    fx.tasks
        .release_task(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    fx.execute("UPDATE workspaces SET worker_id = NULL, state = 'missing'")
        .await;
    let seq = fx.last_seq().await;

    let claimed = fx.tasks.claim_task(&by(&fx.backend), t(1)).await.unwrap();
    assert_eq!(claimed.workspace, Some(ws(1)));
    let appended = fx.events_after(seq).await;
    assert_eq!(
        names(&appended),
        vec!["claim.acquired", "workspace.adopted"]
    );
    assert_eq!(
        appended[1].data,
        json!({"reason": "released", "from": null, "to": "backend"})
    );
    let workspace = fx.workspaces.get(WorkspaceRef::Task(t(1))).await.unwrap();
    assert_eq!(workspace.worker, Some(fx.backend.name.clone()));
}

#[tokio::test]
async fn an_adoption_recomputes_the_task() {
    let fx = Fixture::new().await;
    fx.workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    fx.tasks
        .release_task(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    // A stale cache shows that the adoption ran `recompute` in its transaction.
    fx.execute("UPDATE tasks SET status = 'todo' WHERE number = 1")
        .await;
    let seq = fx.last_seq().await;

    fx.tasks.claim_task(&by(&fx.backend), t(1)).await.unwrap();
    let appended = fx.events_after(seq).await;
    assert_eq!(
        names(&appended),
        vec!["claim.acquired", "workspace.adopted", "task.status_changed"]
    );
    assert_eq!(
        appended[2].data,
        json!({"from": "todo", "to": "in_progress"})
    );
    assert_eq!(fs::read(fx.dir(".fold/ws/t-1/src/lib.rs")).unwrap(), LIB);
}

/// The orphan closes while the create waits for its operation lock, as a close that takes no lock does. The create then takes no claim for the orphan and makes a new workspace.
#[tokio::test]
async fn a_create_whose_orphan_closes_before_the_adoption_creates_a_new_workspace() {
    let fx = Fixture::new().await;
    released_workspace(&fx).await;
    let seq = fx.last_seq().await;
    let backend = by(&fx.backend);

    let held = fx.ops.lock(ws(1)).await;
    let mut create = pin!(fx.workspaces.create_workspace(&backend, t(1)));
    assert!(
        tokio::time::timeout(STALL, &mut create).await.is_err(),
        "the create did not wait for the operation lock of the orphan"
    );
    fx.execute("UPDATE workspaces SET state = 'abandoned', closed_at = 1 WHERE number = 1")
        .await;
    drop(held);
    let created = tokio::time::timeout(WAIT, create).await.unwrap().unwrap();

    assert!(!created.adopted);
    assert_eq!(
        (created.workspace.id, created.workspace.path.as_str()),
        (ws(2), ".fold/ws/t-1-2")
    );
    assert_eq!(created.workspace.worker, Some(fx.backend.name.clone()));
    let appended = fx.events_after(seq).await;
    assert!(!names(&appended).contains(&"workspace.adopted"));
    assert_eq!(
        names(&appended)
            .iter()
            .filter(|name| **name == "claim.acquired")
            .count(),
        1
    );
    let orphan = fx
        .workspaces
        .get(WorkspaceRef::Workspace(ws(1)))
        .await
        .unwrap();
    assert_eq!(orphan.worker, Some(fx.frontend.name.clone()));
    assert_eq!(
        fs::read(fx.dir(".fold/ws/t-1/src/lib.rs")).unwrap(),
        EDITED,
        "the closed orphan keeps its files"
    );
}
