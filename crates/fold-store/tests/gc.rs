#![cfg(test)]

use crate::seed;

use std::collections::HashSet;
use std::time::Duration;

use fold_core::object::ObjectHash;
use fold_core::{Timestamp, WorkspaceId};
use fold_store::testing::{TestProject, execute};
use fold_store::workspaces::{
    ChangesetStats, CheckpointReason, closed_checkpoints_due, closed_directories_due, gc_roots,
    insert_changeset, insert_checkpoint, mark_directory_removed, prune_auto,
    start_directory_removal, tree_recorded, workspaces_over_auto_limit,
};
use fold_store::{Store, StoreError};

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

async fn checkpoint(
    store: &Store,
    workspace: WorkspaceId,
    tree: ObjectHash,
    reason: CheckpointReason,
) {
    store
        .write(async |tx| {
            insert_checkpoint(
                tx,
                workspace,
                tree,
                reason.kind(),
                reason,
                None,
                Timestamp::EPOCH,
            )
            .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn roots_name_every_referenced_hash_once_and_skip_pruned_checkpoints() {
    let project = TestProject::new().await;
    let store = project.store();
    assert!(gc_roots(store.read()).await.unwrap().is_empty());
    let one = seed::workspace(store, 1).await;
    seed::workspace(store, 2).await;
    checkpoint(
        store,
        one.workspace,
        seed::tree("pruned"),
        CheckpointReason::Quiet,
    )
    .await;
    store
        .write(async |tx| {
            prune_auto(tx, one.workspace, 0, Timestamp::EPOCH).await?;
            insert_changeset(
                tx,
                one.workspace,
                Some(one.task),
                one.revision,
                seed::tree("changeset"),
                "record",
                ChangesetStats::default(),
                "frontend",
                Timestamp::EPOCH,
            )
            .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    checkpoint(
        store,
        one.workspace,
        seed::tree("explicit"),
        CheckpointReason::Explicit,
    )
    .await;
    execute(
        store,
        &format!(
            "INSERT INTO agents (id, name, created_at) VALUES (1, 'frontend', 0);
             INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'frontend', 0);
             INSERT INTO proposals (number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 'p', 0, 0);
             INSERT INTO proposal_versions (proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at)
             VALUES (1, 1, 1, '{}', 1, 'frontend', 0);",
            seed::tree("proposal")
        ),
    )
    .await;

    let roots = gc_roots(store.read()).await.unwrap();
    let unique: HashSet<_> = roots.iter().copied().collect();
    assert_eq!(unique.len(), roots.len());
    let expected: HashSet<_> = [
        seed::tree("revision 1"),
        seed::tree("revision 2"),
        seed::tree("revision tree"),
        seed::tree("changeset"),
        seed::tree("explicit"),
        seed::tree("proposal"),
    ]
    .into();
    assert_eq!(unique, expected);
}

#[tokio::test]
async fn only_workspaces_over_the_limit_need_auto_pruning() {
    let project = TestProject::new().await;
    let store = project.store();
    let one = seed::workspace(store, 1).await.workspace;
    let two = seed::workspace(store, 2).await.workspace;
    for n in 0..3 {
        checkpoint(
            store,
            one,
            seed::tree(&format!("one {n}")),
            CheckpointReason::Quiet,
        )
        .await;
    }
    checkpoint(
        store,
        one,
        seed::tree("one explicit"),
        CheckpointReason::Explicit,
    )
    .await;
    checkpoint(store, two, seed::tree("two"), CheckpointReason::BeforeSync).await;
    assert_eq!(
        workspaces_over_auto_limit(store.read(), 2).await.unwrap(),
        vec![one]
    );
    assert_eq!(
        workspaces_over_auto_limit(store.read(), 0).await.unwrap(),
        vec![one, two]
    );
    assert!(
        workspaces_over_auto_limit(store.read(), 3)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn every_closed_workspace_directory_is_due_at_once_until_it_is_removed() {
    let project = TestProject::new().await;
    let store = project.store();
    for n in 1..=5 {
        seed::workspace(store, n).await;
    }
    let now = project.now();
    let ago = |days: u32| now.checked_sub(DAY * days).unwrap().as_millis();
    execute(
        store,
        &format!(
            "UPDATE workspaces SET state = 'archived', closed_at = {} WHERE number = 1;
             UPDATE workspaces SET state = 'archived', kind = 'review', path = 'ws/review-p-1', closed_at = {} WHERE number = 2;
             UPDATE workspaces SET state = 'abandoned', closed_at = {} WHERE number = 3;
             UPDATE workspaces SET closed_at = {} WHERE number = 4;
             UPDATE workspaces SET state = 'missing' WHERE number = 5;",
            now.as_millis(),
            ago(15),
            ago(1),
            ago(30),
        ),
    )
    .await;

    let due = closed_directories_due(store.read(), None).await.unwrap();
    let ids: Vec<u32> = due.iter().map(|closed| closed.id.number()).collect();
    assert_eq!(ids, vec![1, 2, 3]);
    assert_eq!(due[1].path, "ws/review-p-1");
    assert_eq!(due[1].closed_at.as_millis(), ago(15));
    let only = |n: u32| async move {
        closed_directories_due(store.read(), Some(WorkspaceId::from_number(n).unwrap()))
            .await
            .unwrap()
            .len()
    };
    assert_eq!((only(3).await, only(4).await, only(5).await), (1, 0, 0));

    let mark = |workspace| async move {
        store
            .write(async |tx| mark_directory_removed(tx, workspace, now).await)
            .await
            .unwrap()
    };
    let four = WorkspaceId::from_number(4).unwrap();
    let one = WorkspaceId::from_number(1).unwrap();
    let start = |workspace| async move {
        store
            .write(async |tx| start_directory_removal(tx, workspace, now).await)
            .await
            .unwrap()
    };
    assert!(!due[0].removal_started);
    assert!(!start(four).await, "a live workspace does not start");
    assert!(start(one).await);
    assert!(!start(one).await, "the first start stays");
    let due = closed_directories_due(store.read(), Some(one))
        .await
        .unwrap();
    assert!(due[0].removal_started);
    assert!(!mark(four).await, "a live workspace is not marked");
    assert!(mark(one).await);
    assert!(!mark(one).await, "the first mark stays");
    let ids: Vec<u32> = closed_directories_due(store.read(), None)
        .await
        .unwrap()
        .iter()
        .map(|closed| closed.id.number())
        .collect();
    assert_eq!(ids, vec![2, 3]);
}

#[tokio::test]
async fn a_closed_workspace_waits_while_a_worker_process_of_its_agent_runs() {
    let project = TestProject::new().await;
    let store = project.store();
    for n in 1..=2 {
        seed::workspace(store, n).await;
    }
    execute(
        store,
        "INSERT INTO agents (id, name, created_at) VALUES (1, 'coder', 0), (2, 'other', 0);
         INSERT INTO workers (id, agent_id, name, parent_worker_id, created_at) VALUES (1, 1, 'coder', NULL, 0), (2, 1, 'coder/sub', 1, 0), (3, 2, 'other', NULL, 0);
         INSERT INTO worker_processes (agent_id, role, session_id, state, started_by, started_at) VALUES (1, 'review_lead', 's1', 'working', 'main', 0);
         UPDATE workspaces SET state = 'archived', closed_at = 0, worker_id = 2 WHERE number = 1;
         UPDATE workspaces SET state = 'abandoned', closed_at = 0, worker_id = 3 WHERE number = 2;",
    )
    .await;
    let due = || async {
        closed_directories_due(store.read(), None)
            .await
            .unwrap()
            .iter()
            .map(|closed| closed.id.number())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        due().await,
        vec![2],
        "a subagent's workspace waits for its agent's process"
    );
    for state in ["starting", "idle", "stopping"] {
        execute(
            store,
            &format!(
                "UPDATE worker_processes SET state = '{state}', idle_since = CASE WHEN '{state}' = 'idle' THEN 1 END WHERE agent_id = 1"
            ),
        )
        .await;
        assert_eq!(due().await, vec![2], "{state}");
    }
    for state in ["stopped", "failed"] {
        execute(
            store,
            &format!(
                "UPDATE worker_processes SET state = '{state}', idle_since = NULL, stopped_at = 1 WHERE agent_id = 1"
            ),
        )
        .await;
        assert_eq!(due().await, vec![1, 2], "{state}");
        execute(
            store,
            "UPDATE worker_processes SET state = 'stopping', stopped_at = NULL WHERE agent_id = 1",
        )
        .await;
    }
}

#[tokio::test]
async fn checkpoints_of_a_closed_workspace_are_due_once_its_close_and_removal_are_old() {
    let project = TestProject::new().await;
    let store = project.store();
    for n in 1..=5 {
        let seeded = seed::workspace(store, n).await;
        checkpoint(
            store,
            seeded.workspace,
            seed::tree(&format!("checkpoint {n}")),
            CheckpointReason::Explicit,
        )
        .await;
    }
    let now = project.now();
    let ago = |days: u32| now.checked_sub(DAY * days).unwrap().as_millis();
    execute(
        store,
        &format!(
            "UPDATE workspaces SET state = 'archived', closed_at = {old}, directory_removed_at = {old} WHERE number = 1;
             UPDATE workspaces SET state = 'abandoned', closed_at = {old}, directory_removed_at = {recent} WHERE number = 2;
             UPDATE workspaces SET state = 'archived', closed_at = {old} WHERE number = 3;
             UPDATE workspaces SET closed_at = {old}, directory_removed_at = {old} WHERE number = 4;
             UPDATE workspaces SET state = 'archived', closed_at = {old}, directory_removed_at = {old} WHERE number = 5;
             UPDATE checkpoints SET pruned_at = 0 WHERE workspace_id = 5;",
            old = ago(15),
            recent = ago(13),
        ),
    )
    .await;
    let due = |days| async move {
        closed_checkpoints_due(store.read(), now, days)
            .await
            .unwrap()
            .iter()
            .map(|workspace| workspace.number())
            .collect::<Vec<_>>()
    };
    assert_eq!(due(14).await, vec![1]);
    assert_eq!(due(12).await, vec![1, 2]);
    assert!(due(16).await.is_empty());
    assert!(due(u64::MAX).await.is_empty());
}

#[tokio::test]
async fn a_tree_is_recorded_when_a_row_of_the_workspace_or_its_task_holds_it() {
    let project = TestProject::new().await;
    let store = project.store();
    let one = seed::workspace(store, 1).await;
    let two = seed::workspace(store, 2).await;
    checkpoint(
        store,
        one.workspace,
        seed::tree("pruned"),
        CheckpointReason::Quiet,
    )
    .await;
    checkpoint(
        store,
        one.workspace,
        seed::tree("kept"),
        CheckpointReason::Quiet,
    )
    .await;
    checkpoint(
        store,
        one.workspace,
        seed::tree("explicit"),
        CheckpointReason::Explicit,
    )
    .await;
    store
        .write(async |tx| {
            prune_auto(tx, one.workspace, 1, Timestamp::EPOCH).await?;
            insert_changeset(
                tx,
                one.workspace,
                Some(one.task),
                one.revision,
                seed::tree("changeset"),
                "record",
                ChangesetStats::default(),
                "frontend",
                Timestamp::EPOCH,
            )
            .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    execute(
        store,
        &format!(
            "INSERT INTO agents (id, name, created_at) VALUES (1, 'frontend', 0);
             INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'frontend', 0);
             INSERT INTO proposals (number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 'p', 0, 0);
             INSERT INTO proposal_versions (proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at)
             VALUES (1, 1, 1, '{}', 1, 'frontend', 0);",
            seed::tree("version")
        ),
    )
    .await;
    let recorded = |workspace, label: &str| {
        let tree = seed::tree(label);
        async move { tree_recorded(store.read(), workspace, &tree).await.unwrap() }
    };
    for label in ["revision tree", "explicit", "changeset", "version"] {
        assert!(recorded(one.workspace, label).await, "{label}");
    }
    // An automatic checkpoint is no record: pruning beyond the kept number can remove it later.
    for label in ["pruned", "kept", "other"] {
        assert!(!recorded(one.workspace, label).await, "{label}");
    }
    assert!(recorded(two.workspace, "revision tree").await);
    for label in ["explicit", "changeset", "version"] {
        assert!(!recorded(two.workspace, label).await, "{label}");
    }
}
