use crate::common::{count, exec, open};
use fold_core::object::{ObjectKind, hash_object};
use fold_core::{ChangesetId, ProposalId, RevisionId, TaskId, Timestamp, WorkerName, WorkspaceId};
use fold_store::reviews::{self, NewProposal, NewVersion, ProposalState, RetiredState};
use fold_store::testing::assert_append_only;
use fold_store::{Store, StoreError};

/// Two tasks with a live task workspace each, a revision, and changesets `C-1` (task 1) and `C-2` (task 2). No proposal yet.
const SEED: &str = r#"
INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 0), (2, 'review', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'backend', 0), (2, 2, 'review', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'backend', 0, 0), (2, 2, 'two', 'backend', 0, 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0), (2, 2, 2, 1, 'task', 'ws/t-2', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, stats, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't2', 'm', '{"files_added":1,"files_modified":2,"files_deleted":0}', 'backend', 0), (2, 2, 2, 2, 1, 't3', 'm', '{"files_added":0,"files_modified":1,"files_deleted":0}', 'backend', 0);
"#;

#[cfg(test)]
async fn seeded() -> (tempfile::TempDir, Store) {
    let (dir, _paths, store) = open().await;
    for statement in SEED.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    (dir, store)
}

#[cfg(test)]
fn proposal_sql(number: u32, task: u32, state: &str, reason: &str) -> String {
    format!(
        "INSERT INTO proposals (number, task_id, author_worker_id, title, state, close_reason, created_at, updated_at) VALUES ({number}, {task}, 1, 'p', '{state}', {reason}, 0, 0)"
    )
}

#[cfg(test)]
fn version_sql(proposal: u32, version: u32) -> String {
    format!(
        "INSERT INTO proposal_versions (proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES ({proposal}, {version}, 1, 't2', 1, 'backend', 0)"
    )
}

#[cfg(test)]
fn trigger(error: &StoreError) -> String {
    error.to_string()
}

#[tokio::test]
async fn a_task_has_one_open_proposal_and_a_new_one_after_the_first_closes() {
    let (_dir, store) = seeded().await;
    exec(&store, &proposal_sql(1, 1, "open", "NULL"))
        .await
        .unwrap();
    let error = exec(&store, &proposal_sql(2, 1, "open", "NULL"))
        .await
        .unwrap_err();
    assert_eq!(error.constraint().as_deref(), Some("proposals.task_id"));
    exec(&store, &proposal_sql(3, 2, "open", "NULL"))
        .await
        .unwrap();

    exec(
        &store,
        "UPDATE proposals SET state = 'closed', close_reason = 'replaced' WHERE number = 1",
    )
    .await
    .unwrap();
    exec(&store, &proposal_sql(2, 1, "open", "NULL"))
        .await
        .unwrap();
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM proposals WHERE task_id = 1").await,
        2
    );
}

#[tokio::test]
async fn proposal_versions_are_append_only_and_unique_per_proposal() {
    let (_dir, store) = seeded().await;
    exec(&store, &proposal_sql(1, 1, "open", "NULL"))
        .await
        .unwrap();
    exec(&store, &version_sql(1, 1)).await.unwrap();
    assert_append_only(&store, "proposal_versions").await;

    let duplicate = exec(&store, &version_sql(1, 1)).await.unwrap_err();
    assert!(duplicate.unique_violation());
    assert_eq!(
        duplicate.constraint().as_deref(),
        Some("proposal_versions.proposal_id, proposal_versions.version")
    );
    exec(&store, &version_sql(1, 2)).await.unwrap();
    assert_eq!(
        count(&store, "SELECT COUNT(*) FROM proposal_versions").await,
        2
    );
}

#[tokio::test]
async fn a_version_names_who_added_it() {
    let (_dir, store) = seeded().await;
    exec(&store, &proposal_sql(1, 1, "open", "NULL"))
        .await
        .unwrap();
    let error = exec(
        &store,
        "INSERT INTO proposal_versions (proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_at) VALUES (1, 1, 1, 't2', 1, 0)",
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.constraint().as_deref(),
        Some("proposal_versions_created_by_present")
    );
}

#[tokio::test]
async fn a_close_reason_exists_exactly_when_the_proposal_is_closed() {
    let (_dir, store) = seeded().await;
    let refused = [
        proposal_sql(1, 1, "closed", "NULL"),
        proposal_sql(1, 1, "open", "'early'"),
        proposal_sql(1, 1, "integrated", "'merged'"),
    ];
    for sql in refused {
        let error = exec(&store, &sql).await.unwrap_err();
        assert!(
            trigger(&error).contains("proposals_close_reason_iff_closed"),
            "{sql}: {error}"
        );
    }
    exec(&store, &proposal_sql(1, 1, "integrated", "NULL"))
        .await
        .unwrap();
    exec(&store, &proposal_sql(2, 2, "open", "NULL"))
        .await
        .unwrap();
    for sql in [
        "UPDATE proposals SET state = 'closed' WHERE number = 2",
        "UPDATE proposals SET close_reason = 'why' WHERE number = 2",
    ] {
        let error = exec(&store, sql).await.unwrap_err();
        assert!(
            trigger(&error).contains("proposals_close_reason_iff_closed"),
            "{sql}: {error}"
        );
    }
    exec(
        &store,
        "UPDATE proposals SET state = 'closed', close_reason = 'why' WHERE number = 2",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_proposal_title_is_one_non_empty_line() {
    let (_dir, store) = seeded().await;
    let insert = |title: &str| {
        format!(
            "INSERT INTO proposals (number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, '{title}', 0, 0)"
        )
    };
    for title in [
        "",
        "   ",
        "two' || char(10) || 'lines",
        "cr' || char(13) || 'x",
    ] {
        let error = exec(&store, &insert(title)).await.unwrap_err();
        assert!(
            trigger(&error).contains("proposals_title_single_line"),
            "{title:?}: {error}"
        );
    }
    exec(&store, &insert("Add the report")).await.unwrap();
    let error = exec(&store, "UPDATE proposals SET title = ' ' WHERE number = 1")
        .await
        .unwrap_err();
    assert!(trigger(&error).contains("proposals_title_single_line"));
}

#[cfg(test)]
fn p(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
}

#[cfg(test)]
fn version(changeset: u32, created_by: &str) -> NewVersion<'_> {
    NewVersion {
        base: RevisionId::from_number(1).unwrap(),
        tree: hash_object(ObjectKind::Tree, b"tree"),
        head_changeset: ChangesetId::from_number(changeset).unwrap(),
        evidence: Some("tests pass"),
        created_by,
    }
}

#[cfg(test)]
async fn open_proposal(store: &Store, number: u32, task: u32, changeset: u32) {
    let at = Timestamp::from_millis(5).unwrap();
    store
        .write(async |tx| {
            let new = NewProposal {
                id: p(number),
                task: TaskId::from_number(task).unwrap(),
                author_worker_id: 1,
                title: "Add the report",
            };
            reviews::open_proposal(tx, &new, &version(changeset, "backend"), at).await
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn the_store_functions_open_version_and_retire_a_proposal() {
    let (_dir, store) = seeded().await;
    open_proposal(&store, 1, 1, 1).await;
    assert!(reviews::proposal_exists(store.read(), p(1)).await.unwrap());
    assert_eq!(
        reviews::open_for_task(store.read(), TaskId::from_number(1).unwrap())
            .await
            .unwrap(),
        Some(p(1))
    );
    let row = reviews::get_proposal(store.read(), p(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, ProposalState::Open);
    assert_eq!(row.current_version, 1);
    assert_eq!(row.author, WorkerName::parse("backend").unwrap());
    assert!(row.review_claim.is_none() && !row.changes_requested && !row.conflicted);

    exec(
        &store,
        "UPDATE proposals SET conflicted = 1 WHERE number = 1",
    )
    .await
    .unwrap();
    let later = Timestamp::from_millis(9).unwrap();
    store
        .write(async |tx| reviews::add_version(tx, p(1), 2, &version(1, "backend"), later).await)
        .await
        .unwrap();
    let row = reviews::get_proposal(store.read(), p(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((row.current_version, row.conflicted), (2, false));
    assert_eq!(row.updated_at, later);
    let versions = reviews::list_versions(store.read(), p(1)).await.unwrap();
    assert_eq!(
        versions.iter().map(|v| v.version).collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(versions[0].evidence.as_deref(), Some("tests pass"));
    assert_eq!(versions[0].created_by, "backend");
    assert_eq!(
        (
            versions[1].stats.files_added,
            versions[1].stats.files_modified
        ),
        (1, 2)
    );

    exec(
        &store,
        "INSERT INTO claims (kind, proposal_id, worker_id, acquired_at, renewed_at) VALUES ('review', 1, 2, 0, 0)",
    )
    .await
    .unwrap();
    let live_at = Timestamp::from_millis(-1).unwrap();
    let unclaimed = reviews::list_proposals(store.read(), None, Some(live_at))
        .await
        .unwrap();
    assert!(unclaimed.is_empty());
    let stale_at = Timestamp::from_millis(0).unwrap();
    let stale = reviews::list_proposals(store.read(), None, Some(stale_at))
        .await
        .unwrap();
    assert_eq!(stale.iter().map(|row| row.id).collect::<Vec<_>>(), [p(1)]);
    let claimed = reviews::list_proposals(store.read(), Some(ProposalState::Open), None)
        .await
        .unwrap();
    assert_eq!(
        claimed[0].review_claim,
        Some(WorkerName::parse("review").unwrap())
    );

    let (retired, holder) = store
        .write(async |tx| {
            let retired =
                reviews::retire(tx, p(1), RetiredState::Closed, Some("replaced"), later).await?;
            let holder = reviews::release_review_claim(tx, p(1), "closed", later).await?;
            Ok::<_, StoreError>((retired, holder))
        })
        .await
        .unwrap();
    assert!(retired);
    assert_eq!(holder, Some(WorkerName::parse("review").unwrap()));
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM claims WHERE release_reason = 'closed' AND released_at = 9"
        )
        .await,
        1
    );
    let row = reviews::get_proposal(store.read(), p(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, ProposalState::Closed);
    assert_eq!(row.close_reason.as_deref(), Some("replaced"));
    assert_eq!(row.closed_at, Some(later));
    assert!(
        reviews::list_proposals(store.read(), Some(ProposalState::Open), None)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_store_functions_refuse_what_does_not_exist_or_is_not_open() {
    let (_dir, store) = seeded().await;
    let at = Timestamp::from_millis(1).unwrap();
    let missing_task = store
        .write(async |tx| {
            let new = NewProposal {
                id: p(1),
                task: TaskId::from_number(9).unwrap(),
                author_worker_id: 1,
                title: "x",
            };
            reviews::open_proposal(tx, &new, &version(1, "backend"), at).await
        })
        .await
        .unwrap_err();
    assert!(matches!(missing_task, StoreError::Invalid(_)));
    let missing_changeset = store
        .write(async |tx| {
            let new = NewProposal {
                id: p(1),
                task: TaskId::from_number(1).unwrap(),
                author_worker_id: 1,
                title: "x",
            };
            reviews::open_proposal(tx, &new, &version(7, "backend"), at).await
        })
        .await
        .unwrap_err();
    assert!(matches!(missing_changeset, StoreError::Invalid(_)));
    assert_eq!(count(&store, "SELECT COUNT(*) FROM proposals").await, 0);

    open_proposal(&store, 1, 1, 1).await;
    let (retired, again, holder) = store
        .write(async |tx| {
            let retired = reviews::retire(tx, p(1), RetiredState::Integrated, None, at).await?;
            let again = reviews::retire(tx, p(1), RetiredState::Closed, Some("x"), at).await?;
            let holder = reviews::release_review_claim(tx, p(1), "integrated", at).await?;
            Ok::<_, StoreError>((retired, again, holder))
        })
        .await
        .unwrap();
    assert_eq!((retired, again, holder), (true, false, None));
    let not_open = store
        .write(async |tx| reviews::add_version(tx, p(1), 2, &version(1, "backend"), at).await)
        .await
        .unwrap_err();
    assert!(matches!(not_open, StoreError::Invalid(_)));
    assert!(!reviews::proposal_exists(store.read(), p(5)).await.unwrap());
    assert!(
        reviews::get_proposal(store.read(), p(5))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        reviews::live_review_workspaces(store.read(), p(1))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn live_review_workspaces_lists_only_live_review_checkouts_of_the_proposal() {
    let (_dir, store) = seeded().await;
    open_proposal(&store, 1, 1, 1).await;
    exec(
        &store,
        "INSERT INTO workspaces (number, task_id, worker_id, kind, path, base_revision_id, proposal_id, state, created_at) VALUES
         (3, 1, 2, 'review', 'ws/r-1', 1, 1, 'active', 0),
         (4, 1, 2, 'review', 'ws/r-2', 1, 1, 'archived', 0),
         (5, 1, 2, 'review', 'ws/r-3', 1, 1, 'missing', 0)",
    )
    .await
    .unwrap();
    let live = reviews::live_review_workspaces(store.read(), p(1))
        .await
        .unwrap();
    assert_eq!(
        live,
        [
            WorkspaceId::from_number(3).unwrap(),
            WorkspaceId::from_number(5).unwrap()
        ]
    );
}
