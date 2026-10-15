#![cfg(test)]

pub mod reviews_support;

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use fold_core::object::ObjectHash;
use fold_core::tree::ChangeCounts;
use fold_core::{
    EventActor, ProjectConfig, RevisionId, TaskStatus, WorkerName, WorkspaceId, codes,
};
use fold_daemon::live::{LiveConfig, LiveState};
use fold_daemon::services::agents::OverviewFilter;
use fold_daemon::services::reviews::ReviewCheckout;
use fold_daemon::services::workspaces::WorkspaceRef;
use fold_store::reviews::list_versions;
use fold_store::workspaces::{
    self, CheckpointKind, CheckpointReason, WorkspaceKind, WorkspaceState,
};
use reviews_support::{
    Fixture, agent, by, details, human, names, p, refusal, t, workspace_refusal, ws,
};
use serde_json::json;

impl Fixture {
    async fn checkout_by(&self, worker: &fold_store::agents::WorkerRow) -> ReviewCheckout {
        self.reviews
            .create_review_checkout(&by(worker), p(1))
            .await
            .unwrap()
    }

    async fn claimed(&self) {
        self.reviews
            .claim_review(&by(&self.backend.worker), p(1))
            .await
            .unwrap();
    }

    fn read(&self, path: &str, file: &str) -> String {
        std::fs::read_to_string(self.project.paths().root().join(path).join(file)).unwrap()
    }
}

#[tokio::test]
async fn checkout_materialises_the_current_version_tree() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    fx.claimed().await;
    let before = fx.last_seq().await;

    let checkout = fx.checkout_by(&fx.backend.worker).await;

    let view = &checkout.workspace;
    assert_eq!(checkout.version, 1);
    assert_eq!(view.id, ws(2));
    assert_eq!(view.path, "ws/review-p-1");
    assert_eq!(view.kind, WorkspaceKind::Review);
    assert_eq!(view.task, t(1));
    assert_eq!(view.proposal, Some(p(1)));
    assert_eq!(view.worker, Some(fx.backend.worker.name.clone()));
    assert_eq!(view.base, RevisionId::from_number(1).unwrap());
    assert_eq!(view.state, WorkspaceState::Active);
    assert_eq!(view.created_at, fx.project.now());
    assert_eq!(fx.read(&view.path, "src/lib.rs"), "pub fn y() {}\n");
    assert_eq!(fx.read(&view.path, "src/new.rs"), "pub fn z() {}\n");
    assert_eq!(fx.read(&view.path, "README.md"), "# demo\n");
    let dir = fx.project.paths().root().join(&view.path);
    let link = dir.join("docs/readme");
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        std::path::Path::new("../README.md")
    );
    let mode = std::fs::metadata(dir.join("src/lib.rs"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o222, 0, "checkout files are read-only");
    assert!(!fx.project.paths().workspace_index(view.id).exists());

    let appended = fx.events_after(before).await;
    assert_eq!(names(&appended), ["workspace.created"]);
    let created = &appended[0];
    assert_eq!(created.actor, EventActor::worker(&fx.backend.worker.name));
    assert_eq!(created.task, Some(t(1)));
    assert_eq!(created.proposal, Some(p(1)));
    assert_eq!(created.workspace, Some(ws(2)));
    assert_eq!(created.revision, RevisionId::from_number(1));
    assert_eq!(
        created.data,
        json!({"kind": "review", "path": "ws/review-p-1", "worker": "backend", "version": 1})
    );

    fx.write("src/lib.rs", "pub fn v2() {}\n");
    fx.reviews
        .update_proposal(&by(&fx.frontend.worker), p(1), None)
        .await
        .unwrap();
    let second = fx.checkout_by(&fx.backend.worker).await;
    assert_eq!(second.version, 2);
    assert_eq!(second.workspace.path, "ws/review-p-1-2");
    assert_eq!(
        fx.read(&second.workspace.path, "src/lib.rs"),
        "pub fn v2() {}\n"
    );
    assert_eq!(fx.read(&view.path, "src/lib.rs"), "pub fn y() {}\n");
}

#[tokio::test]
async fn checkout_needs_the_review_claim() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    let backend = by(&fx.backend.worker);

    let error = refusal(fx.reviews.create_review_checkout(&backend, p(1)).await);
    assert_eq!(error.code(), codes::REVIEW_CLAIM_REQUIRED);
    assert_eq!(details(&error), json!({"proposal": "P-1", "holder": null}));

    fx.claimed().await;
    for actor in [by(&fx.review.worker), by(&fx.frontend.worker), human()] {
        let error = refusal(fx.reviews.create_review_checkout(&actor, p(1)).await);
        assert_eq!(error.code(), codes::REVIEW_CLAIM_REQUIRED);
        assert_eq!(
            details(&error),
            json!({"proposal": "P-1", "holder": "backend"})
        );
    }
    let ghost = agent("backend/ghost");
    let error = refusal(fx.reviews.create_review_checkout(&ghost, p(1)).await);
    assert_eq!(error.code(), codes::WORKER_NOT_FOUND);
    let error = refusal(fx.reviews.create_review_checkout(&backend, p(2)).await);
    assert_eq!(error.code(), codes::PROPOSAL_NOT_FOUND);

    fx.reviews
        .release_review(&backend, p(1), None)
        .await
        .unwrap();
    let error = refusal(fx.reviews.create_review_checkout(&backend, p(1)).await);
    assert_eq!(error.code(), codes::REVIEW_CLAIM_REQUIRED);

    fx.claimed().await;
    fx.reviews
        .close_proposal(&by(&fx.frontend.worker), p(1), "superseded")
        .await
        .unwrap();
    let error = refusal(fx.reviews.create_review_checkout(&backend, p(1)).await);
    assert_eq!(error.code(), codes::PROPOSAL_NOT_OPEN);

    assert_eq!(fx.ws_entries(), ["t-1"]);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspaces WHERE kind = 'review'")
            .await,
        0
    );
}

#[tokio::test]
async fn checkout_for_the_same_worker_and_version_is_idempotent() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    fx.claimed().await;
    let before = fx.last_seq().await;

    let first = fx.checkout_by(&fx.backend.worker).await;
    fx.advance(1);
    let again = fx.checkout_by(&fx.backend.worker).await;

    assert_eq!(again, first);
    let backend = by(&fx.backend.worker);
    let (one, two) = (fx.service(), fx.service());
    let (left, right) = tokio::join!(
        one.create_review_checkout(&backend, p(1)),
        two.create_review_checkout(&backend, p(1))
    );
    assert_eq!(left.unwrap(), first);
    assert_eq!(right.unwrap(), first);
    assert_eq!(fx.ws_entries(), ["review-p-1", "t-1"]);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspaces WHERE kind = 'review'")
            .await,
        1
    );
    assert_eq!(names(&fx.events_after(before).await), ["workspace.created"]);
}

#[tokio::test]
async fn concurrent_checkouts_keep_one_row_and_remove_the_other_directory() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    fx.claimed().await;
    let backend = by(&fx.backend.worker);
    let (one, two) = (fx.service(), fx.service());

    let (left, right) = tokio::join!(
        one.create_review_checkout(&backend, p(1)),
        two.create_review_checkout(&backend, p(1))
    );

    let (left, right) = (left.unwrap(), right.unwrap());
    assert_eq!(left, right);
    assert_eq!(
        fx.ws_entries(),
        [
            left.workspace.path.trim_start_matches("ws/").to_owned(),
            "t-1".to_owned()
        ]
    );
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspaces WHERE kind = 'review'")
            .await,
        1
    );
    let sub = fx.subagent(&fx.backend, "r1").await;
    assert_eq!(fx.checkout_by(&sub).await.workspace.id, ws(3));
}

#[tokio::test]
async fn checkout_does_not_move_the_task() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    fx.claimed().await;
    assert_eq!(fx.status(1).await, TaskStatus::Review);
    let before = fx.last_seq().await;

    let checkout = fx.checkout_by(&fx.backend.worker).await;
    fx.recompute(1).await;

    assert_eq!(fx.status(1).await, TaskStatus::Review);
    assert_eq!(names(&fx.events_after(before).await), ["workspace.created"]);
    let task_workspace = fx.workspaces.get(WorkspaceRef::Task(t(1))).await.unwrap();
    assert_eq!(task_workspace.id, ws(1));
    assert_eq!(task_workspace.kind, WorkspaceKind::Task);

    // Without its proposal and its task workspace the task has only the live checkout left, which counts for nothing.
    fx.execute(
        "UPDATE proposals SET state = 'closed', close_reason = 'gone', closed_at = 0;
         UPDATE workspaces SET state = 'abandoned', closed_at = 0 WHERE number = 1",
    )
    .await;
    fx.recompute(1).await;
    let live = fx
        .workspaces
        .get(WorkspaceRef::Workspace(checkout.workspace.id))
        .await
        .unwrap();
    assert_eq!(live.state, WorkspaceState::Active);
    assert_eq!(fx.status(1).await, TaskStatus::Todo);
}

#[tokio::test]
async fn checkout_is_not_watched_and_takes_no_checkpoint() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    fx.claimed().await;
    let checkout = fx.checkout_by(&fx.backend.worker).await;
    let id = checkout.workspace.id;
    let owner = by(&fx.backend.worker);

    let refusals = [
        workspace_refusal(fx.workspaces.record_checkpoint(&owner, id, Some("x")).await),
        workspace_refusal(fx.workspaces.record_changes(&owner, id, "edit").await),
        workspace_refusal(fx.workspaces.sync_workspace(&owner, id).await),
    ];
    for error in refusals {
        assert_eq!(error.code(), codes::INVALID_REQUEST);
        assert_eq!(
            details(&error),
            json!({"reason": "review_checkout", "workspace": "ws-2"})
        );
    }

    let tree: ObjectHash = list_versions(fx.project.store().read(), p(1))
        .await
        .unwrap()
        .remove(0)
        .tree;
    let now = fx.project.now();
    let checkpoint = fx
        .project
        .store()
        .write(async |tx| {
            workspaces::insert_checkpoint(
                tx,
                id,
                tree,
                CheckpointKind::Explicit,
                CheckpointReason::Explicit,
                None,
                now,
            )
            .await
        })
        .await;
    assert!(checkpoint.is_err());
    let changeset = fx
        .project
        .store()
        .write(async |tx| {
            workspaces::insert_changeset(
                tx,
                id,
                t(1),
                RevisionId::from_number(1).unwrap(),
                tree,
                "edit",
                ChangeCounts::default(),
                "backend",
                now,
            )
            .await
        })
        .await;
    assert!(changeset.is_err());
    for sql in [
        format!(
            "INSERT INTO checkpoints (number, workspace_id, tree_hash, kind, reason, created_at, base_revision_id)
             VALUES (99, 2, '{tree}', 'explicit', 'explicit', 0, 1)"
        ),
        format!(
            "INSERT INTO changesets (number, workspace_id, task_id, base_revision_id, tree_hash, message, stats, created_by, created_at)
             VALUES (99, 2, 1, 1, '{tree}', 'edit', '{{}}', 'backend', 0)"
        ),
    ] {
        let refused = fx
            .project
            .store()
            .write(async |tx| {
                sqlx::query(sqlx::AssertSqlSafe(sql))
                    .execute(tx.conn())
                    .await
                    .map_err(fold_store::StoreError::from)
            })
            .await
            .unwrap_err();
        assert!(refused.to_string().contains("task_workspaces_only"), "{refused}");
    }
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM checkpoints WHERE workspace_id = 2")
            .await,
        0
    );
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM changesets WHERE workspace_id = 2")
            .await,
        0
    );
    let task_workspace = workspaces::live_task_workspace(fx.project.store().read(), t(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(task_workspace.id, ws(1));
}

#[tokio::test]
async fn closing_the_proposal_archives_the_checkout_and_keeps_the_directory() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    fx.claimed().await;
    let checkout = fx.checkout_by(&fx.backend.worker).await;
    let before = fx.last_seq().await;
    fx.advance(1);

    fx.reviews
        .close_proposal(&by(&fx.frontend.worker), p(1), "superseded")
        .await
        .unwrap();

    let row = fx
        .workspaces
        .get(WorkspaceRef::Workspace(checkout.workspace.id))
        .await
        .unwrap();
    assert_eq!(row.state, WorkspaceState::Archived);
    assert_eq!(row.closed_at, Some(fx.project.now()));
    assert_eq!(fx.read(&row.path, "src/lib.rs"), "pub fn y() {}\n");
    let appended = fx.events_after(before).await;
    let archived: Vec<_> = appended
        .iter()
        .filter(|event| event.event_type.as_str() == "workspace.archived")
        .collect();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].workspace, Some(checkout.workspace.id));
    assert_eq!(archived[0].data, json!({"reason": "proposal_closed"}));
    let task_workspace = fx
        .workspaces
        .get(WorkspaceRef::Workspace(ws(1)))
        .await
        .unwrap();
    assert_eq!(task_workspace.state, WorkspaceState::Active);
}

#[tokio::test]
async fn subagent_worker_gets_its_own_checkout_with_as() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    fx.claimed().await;
    let sub = fx.subagent(&fx.backend, "r1").await;
    let main = fx.checkout_by(&fx.backend.worker).await;

    let own = fx.checkout_by(&sub).await;

    assert_ne!(own.workspace.id, main.workspace.id);
    assert_eq!(own.workspace.path, "ws/review-p-1-2");
    assert_eq!(own.workspace.worker, Some(sub.name.clone()));
    assert_eq!(own.version, 1);
    assert_eq!(fx.checkout_by(&sub).await, own);
    assert_eq!(fx.checkout_by(&fx.backend.worker).await, main);
    assert_eq!(
        fx.read(&own.workspace.path, "src/new.rs"),
        "pub fn z() {}\n"
    );
}

#[tokio::test]
async fn checkout_directory_name_never_uses_the_worker_name() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    fx.claimed().await;
    assert!(WorkerName::parse("backend/..").is_err());
    assert!(WorkerName::parse("../backend").is_err());
    let nested = fx.subagent(&fx.backend, "a-b").await;
    let deeper = fx
        .agents
        .register_worker(&fx.backend.ctx, "c_d", Some(&nested.name))
        .await
        .unwrap()
        .worker;
    assert_eq!(deeper.name.as_str(), "backend/a-b/c_d");

    let mut paths = Vec::new();
    for worker in [&deeper, &nested, &fx.backend.worker] {
        paths.push(fx.checkout_by(worker).await.workspace.path);
    }

    assert_eq!(
        paths,
        ["ws/review-p-1", "ws/review-p-1-2", "ws/review-p-1-3"]
    );
    assert_eq!(
        fx.ws_entries(),
        ["review-p-1", "review-p-1-2", "review-p-1-3", "t-1"]
    );
}

#[tokio::test]
async fn checkout_skips_a_name_a_directory_already_holds() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    fx.claimed().await;
    let stray = fx.project.paths().ws_dir().join("review-p-1");
    std::fs::create_dir_all(&stray).unwrap();
    std::fs::write(stray.join("left.txt"), "left").unwrap();

    let checkout = fx.checkout_by(&fx.backend.worker).await;

    assert_eq!(checkout.workspace.path, "ws/review-p-1-2");
    assert_eq!(
        std::fs::read_to_string(stray.join("left.txt")).unwrap(),
        "left"
    );
}

#[tokio::test]
async fn checkout_failed_leaves_no_row_and_no_directory() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    fx.claimed().await;
    let ws_dir = fx.project.paths().ws_dir();
    std::fs::set_permissions(&ws_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let result = fx
        .reviews
        .create_review_checkout(&by(&fx.backend.worker), p(1))
        .await;

    std::fs::set_permissions(&ws_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = refusal(result);
    assert_eq!(error.code(), codes::CHECKOUT_FAILED);
    assert_eq!(
        details(&error),
        json!({"proposal": "P-1", "cause": "workspace_io"})
    );
    assert_eq!(fx.ws_entries(), ["t-1"]);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM workspaces WHERE kind = 'review'")
            .await,
        0
    );
    let checkout = fx.checkout_by(&fx.backend.worker).await;
    assert_eq!(checkout.workspace.path, "ws/review-p-1");
}

#[tokio::test]
async fn checkout_never_gets_live_state_and_stays_out_of_the_overview() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    fx.claimed().await;
    let checkout = fx.checkout_by(&fx.backend.worker).await;
    let live = LiveState::new(
        fx.project.store().clone(),
        fx.bus.clone(),
        Arc::new(fx.project.clock().clone()),
        fx.project.paths().clone(),
        LiveConfig::from_project(&ProjectConfig::default()),
        Arc::default(),
    );

    live.attach(ws(1)).await.unwrap();
    let refused = workspace_refusal(live.attach(checkout.workspace.id).await);

    assert_eq!(refused.code(), codes::INVALID_REQUEST);
    assert_eq!(details(&refused), json!({"reason": "review_checkout"}));
    assert!(live.get(checkout.workspace.id).is_none());
    assert_eq!(live.tick(), 1);
    let attached: Vec<WorkspaceId> = live
        .all()
        .iter()
        .map(|state| state.view.workspace)
        .collect();
    assert_eq!(attached, [ws(1)]);

    let overview = fx
        .agents
        .overview(&OverviewFilter {
            agent: None,
            events: 0,
        })
        .await
        .unwrap();
    let owned: Vec<(String, WorkspaceId)> = overview
        .iter()
        .flat_map(|agent| agent.workers.iter())
        .flat_map(|worker| {
            worker
                .workspaces
                .iter()
                .map(|summary| (worker.name.as_str().to_owned(), summary.ws))
        })
        .collect();
    assert_eq!(owned, [("frontend".to_owned(), ws(1))]);
}
