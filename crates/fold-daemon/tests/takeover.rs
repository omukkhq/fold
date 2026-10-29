#![cfg(test)]

use crate::common::intents::Covering as _;
use std::fs;
use std::time::Duration;

use crate::adoption_support::{
    EDITED, Fixture, LEASE_MINUTES, by, details, names, r, snapshot, t, task_refusal,
    workspace_refusal, ws,
};
use fold_core::{Actor, HumanName, codes};
use fold_daemon::services::events::BusMessage;
use fold_daemon::services::reviews::ReviewError;
use fold_daemon::services::workspaces::WorkspaceRef;
use fold_store::tasks::claims;
use fold_store::workspaces::WorkspaceState;
use serde_json::json;

const LEASE: Duration = Duration::from_secs(LEASE_MINUTES * 60);

#[tokio::test]
async fn takeover_waits_for_the_lease_then_moves_the_claim_and_the_workspace() {
    let fx = Fixture::new().await;
    let created = fx
        .workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap()
        .workspace;
    let dir = fx.dir(&created.path);
    fs::write(dir.join("src/lib.rs"), EDITED).unwrap();
    fs::write(dir.join("notes.txt"), b"unrecorded\n").unwrap();
    let files = snapshot(&dir);
    let renewed_at = fx.now();
    let before = fx.last_seq().await;

    fx.advance(LEASE - Duration::from_secs(1));
    let early = fx
        .tasks
        .takeover_claim(&by(&fx.backend), t(1))
        .await
        .unwrap_err();
    let early = task_refusal(early);
    assert_eq!(early.code(), codes::CLAIM_NOT_STALE);
    assert_eq!(
        details(&early),
        json!({"holder": "frontend", "renewed_at": renewed_at, "lease_minutes": LEASE_MINUTES})
    );
    assert!(fx.events_after(before).await.is_empty());
    let unchanged = fx.workspaces.get(WorkspaceRef::Task(t(1))).await.unwrap();
    assert_eq!(unchanged, created);

    let at = fx.advance(Duration::from_secs(1));
    let mut bus = fx.bus.subscribe();
    let claimed = fx
        .tasks
        .takeover_claim(&by(&fx.backend), t(1))
        .await
        .unwrap();
    assert_eq!(claimed.claim.holder, fx.backend.name);
    assert_eq!(claimed.claim.acquired_at, at);
    assert!(!claimed.claim.stale);
    assert_eq!(claimed.workspace, Some(ws(1)));

    let appended = fx.events_after(before).await;
    assert_eq!(
        names(&appended),
        vec!["claim.taken_over", "workspace.adopted"]
    );
    assert_eq!(
        appended[0].data,
        json!({"kind": "implementation", "from": "frontend", "to": "backend"})
    );
    let adopted = &appended[1];
    assert_eq!(adopted.actor.as_str(), "backend");
    assert_eq!(adopted.task, Some(t(1)));
    assert_eq!(adopted.workspace, Some(ws(1)));
    assert_eq!(
        adopted.data,
        json!({"reason": "taken_over", "from": "frontend", "to": "backend"})
    );
    assert_eq!(appended[1].seq, appended[0].seq + 1);
    for event in &appended {
        assert_eq!(event.created_at, at);
        match bus.try_recv().unwrap() {
            BusMessage::Durable(published) => assert_eq!(published.seq, event.seq),
            BusMessage::Live(_) => panic!("a takeover sends no live event"),
        }
    }

    let moved = fx.workspaces.get(WorkspaceRef::Task(t(1))).await.unwrap();
    assert_eq!(moved.worker, Some(fx.backend.name.clone()));
    assert_eq!((moved.id, &moved.path), (created.id, &created.path));
    assert_eq!(moved.base, created.base);
    assert_eq!(moved.state, WorkspaceState::Active);
    assert_eq!(moved.created_at, created.created_at);
    assert_eq!(snapshot(&dir), files);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspaces").await,
        1,
        "a takeover makes no second workspace"
    );
    let reason = fx
        .count("SELECT COUNT(*) FROM claims WHERE release_reason = 'taken_over'")
        .await;
    assert_eq!(reason, 1);
    let holder = claims::unreleased_for_task(fx.project.store().read(), t(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(holder.worker_id, fx.backend.id);

    let old = fx
        .workspaces
        .record_checkpoint(&by(&fx.frontend), ws(1), None)
        .await
        .unwrap_err();
    assert_eq!(workspace_refusal(old).code(), codes::NOT_WORKSPACE_OWNER);
    let old = fx
        .tasks
        .release_task(&by(&fx.frontend), t(1))
        .await
        .unwrap_err();
    assert_eq!(task_refusal(old).code(), codes::NOT_CLAIM_HOLDER);
    fx.workspaces
        .record_checkpoint(&by(&fx.backend), ws(1), Some("picked up"))
        .await
        .unwrap();
}

/// The open proposal goes to the new holder with the claim and the workspace. The old author can no longer add a version and the new holder answers a change request with the next one.
#[tokio::test]
async fn a_takeover_hands_the_open_proposal_to_the_new_holder() {
    let fx = Fixture::new().await;
    let reviews = fx.reviews();
    let created = fx
        .workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap()
        .workspace;
    let dir = fx.dir(&created.path);
    fs::write(dir.join("src/lib.rs"), EDITED).unwrap();
    let proposal = reviews
        .create_proposal_covering(&by(&fx.frontend), t(1), "Edit the lib", None)
        .await
        .unwrap()
        .detail()
        .unwrap()
        .summary
        .id;
    let at = fx.advance(LEASE);
    let before = fx.last_seq().await;

    fx.tasks
        .takeover_claim(&by(&fx.backend), t(1))
        .await
        .unwrap();
    let appended = fx.events_after(before).await;
    assert_eq!(
        names(&appended),
        vec![
            "claim.taken_over",
            "workspace.adopted",
            "proposal.author_changed"
        ]
    );
    let changed = &appended[2];
    assert_eq!(changed.actor.as_str(), "backend");
    assert_eq!(
        (changed.task, changed.proposal),
        (Some(t(1)), Some(proposal))
    );
    assert_eq!(changed.created_at, at);
    assert_eq!(
        changed.data,
        json!({"reason": "taken_over", "from": "frontend", "to": "backend"})
    );
    let detail = reviews.get_proposal(proposal, None).await.unwrap();
    assert_eq!(detail.summary.author, fx.backend.name);
    assert_eq!(detail.summary.current_version, 1);

    fs::write(dir.join("src/lib.rs"), b"pub fn x() { done() }\n").unwrap();
    let old = reviews
        .update_proposal_covering(&by(&fx.frontend), proposal, None)
        .await
        .unwrap_err();
    let ReviewError::Fold(old) = old else {
        panic!("expected a refusal, got {old:?}");
    };
    assert_eq!(old.code(), codes::NOT_PROPOSAL_AUTHOR);
    let updated = reviews
        .update_proposal_covering(&by(&fx.backend), proposal, Some("picked up"))
        .await
        .unwrap()
        .detail()
        .unwrap();
    assert_eq!(updated.summary.current_version, 2);
    assert_eq!(updated.summary.author, fx.backend.name);
}

#[tokio::test]
async fn the_claim_and_the_workspace_move_in_one_transaction() {
    let fx = Fixture::new().await;
    fx.workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    fx.advance(LEASE);
    let before = fx.last_seq().await;
    fx.execute(
        "CREATE TRIGGER refuse_owner BEFORE UPDATE OF worker_id ON workspaces
         BEGIN SELECT RAISE(ABORT, 'refused'); END",
    )
    .await;

    fx.tasks
        .takeover_claim(&by(&fx.backend), t(1))
        .await
        .unwrap_err();
    assert!(fx.events_after(before).await.is_empty());
    let holder = claims::unreleased_for_task(fx.project.store().read(), t(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(holder.worker_id, fx.frontend.id);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM claims WHERE released_at IS NOT NULL")
            .await,
        0
    );
    let workspace = fx.workspaces.get(WorkspaceRef::Task(t(1))).await.unwrap();
    assert_eq!(workspace.worker, Some(fx.frontend.name.clone()));

    fx.execute("DROP TRIGGER refuse_owner").await;
    fx.tasks
        .takeover_claim(&by(&fx.backend), t(1))
        .await
        .unwrap();
    assert_eq!(
        names(&fx.events_after(before).await),
        vec!["claim.taken_over", "workspace.adopted"]
    );
}

#[tokio::test]
async fn a_missing_workspace_moves_with_the_claim() {
    let fx = Fixture::new().await;
    fx.workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    fx.execute("UPDATE workspaces SET state = 'missing'").await;
    fx.advance(LEASE);
    let claimed = fx
        .tasks
        .takeover_claim(&by(&fx.backend), t(1))
        .await
        .unwrap();
    assert_eq!(claimed.workspace, Some(ws(1)));
    let moved = fx.workspaces.get(WorkspaceRef::Task(t(1))).await.unwrap();
    assert_eq!(moved.state, WorkspaceState::Missing);
    assert_eq!(moved.worker, Some(fx.backend.name.clone()));
}

#[tokio::test]
async fn a_takeover_without_a_workspace_moves_only_the_claim() {
    let fx = Fixture::new().await;
    fx.tasks.claim_task(&by(&fx.frontend), t(1)).await.unwrap();
    fx.advance(LEASE);
    let before = fx.last_seq().await;
    let claimed = fx
        .tasks
        .takeover_claim(&by(&fx.backend), t(1))
        .await
        .unwrap();
    assert_eq!(claimed.workspace, None);
    assert_eq!(
        names(&fx.events_after(before).await),
        vec!["claim.taken_over"]
    );
}

#[tokio::test]
async fn a_closed_workspace_stays_with_its_worker() {
    let fx = Fixture::new().await;
    fx.workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    fx.workspaces
        .abandon_workspace(&by(&fx.frontend), ws(1), "wrong approach")
        .await
        .unwrap();
    fx.advance(LEASE);
    let before = fx.last_seq().await;
    let claimed = fx
        .tasks
        .takeover_claim(&by(&fx.backend), t(1))
        .await
        .unwrap();
    assert_eq!(claimed.workspace, None);
    assert_eq!(
        names(&fx.events_after(before).await),
        vec!["claim.taken_over"]
    );
    let closed = fx
        .workspaces
        .get(WorkspaceRef::Workspace(ws(1)))
        .await
        .unwrap();
    assert_eq!(closed.worker, Some(fx.frontend.name.clone()));
    assert_eq!(closed.base, r(1));
}

#[tokio::test]
async fn an_unclaimed_task_is_not_claimed() {
    let fx = Fixture::new().await;
    let error = fx
        .tasks
        .takeover_claim(&by(&fx.backend), t(1))
        .await
        .unwrap_err();
    let error = task_refusal(error);
    assert_eq!(error.code(), codes::NOT_CLAIMED);
    assert!(error.hint().unwrap().contains("claim_task"));
}

#[tokio::test]
async fn the_holder_is_already_holder() {
    let fx = Fixture::new().await;
    fx.workspaces
        .create_workspace(&by(&fx.frontend), t(1))
        .await
        .unwrap();
    fx.advance(LEASE);
    let error = fx
        .tasks
        .takeover_claim(&by(&fx.frontend), t(1))
        .await
        .unwrap_err();
    let error = task_refusal(error);
    assert_eq!(error.code(), codes::ALREADY_HOLDER);
    assert_eq!(details(&error), json!({"holder": "frontend"}));
}

#[tokio::test]
async fn a_done_task_is_closed() {
    let fx = Fixture::new().await;
    fx.tasks.claim_task(&by(&fx.frontend), t(2)).await.unwrap();
    fx.complete(2).await;
    fx.advance(LEASE);
    let error = fx
        .tasks
        .takeover_claim(&by(&fx.backend), t(2))
        .await
        .unwrap_err();
    let error = task_refusal(error);
    assert_eq!(error.code(), codes::TASK_CLOSED);
    assert_eq!(details(&error), json!({"reason": "done"}));
}

#[tokio::test]
async fn an_archived_task_is_closed() {
    let fx = Fixture::new().await;
    fx.tasks.claim_task(&by(&fx.frontend), t(3)).await.unwrap();
    fx.tasks
        .archive_task(&Actor::human(HumanName::parse("ana").unwrap()), t(3))
        .await
        .unwrap();
    fx.advance(LEASE);
    let error = fx
        .tasks
        .takeover_claim(&by(&fx.backend), t(3))
        .await
        .unwrap_err();
    let error = task_refusal(error);
    assert_eq!(error.code(), codes::TASK_CLOSED);
    assert_eq!(details(&error), json!({"reason": "archived"}));
}
