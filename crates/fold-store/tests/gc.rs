#![cfg(test)]

use crate::seed;

use std::collections::HashSet;
use std::time::Duration;

use fold_core::object::ObjectHash;
use fold_core::{Timestamp, WorkspaceId};
use fold_store::testing::{TestProject, execute};
use fold_store::workspaces::{
    ChangesetStats, CheckpointReason, closed_workspaces_due, gc_roots, insert_changeset,
    insert_checkpoint, mark_directory_removed, prune_auto, workspaces_over_auto_limit,
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
async fn closed_workspaces_are_due_after_their_retention() {
    let project = TestProject::new().await;
    let store = project.store();
    for n in 1..=6 {
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
             UPDATE workspaces SET state = 'archived', closed_at = {} WHERE number = 4;
             UPDATE workspaces SET closed_at = {} WHERE number = 5;",
            ago(15),
            ago(15),
            ago(20),
            ago(13),
            ago(30),
        ),
    )
    .await;

    let due = closed_workspaces_due(store.read(), now, 14).await.unwrap();
    let ids: Vec<u32> = due.iter().map(|closed| closed.id.number()).collect();
    assert_eq!(ids, vec![1, 2, 3]);
    assert_eq!(due[1].path, "ws/review-p-1");
    assert_eq!(due[0].closed_at.as_millis(), ago(15));
    let ids_after = |days| async move {
        closed_workspaces_due(store.read(), now, days)
            .await
            .unwrap()
            .len()
    };
    assert_eq!(ids_after(12).await, 4);
    assert_eq!(ids_after(25).await, 0);
    assert_eq!(ids_after(u64::MAX).await, 0);
}

#[tokio::test]
async fn a_removed_directory_is_not_due_again() {
    let project = TestProject::new().await;
    let store = project.store();
    let one = seed::workspace(store, 1).await.workspace;
    let two = seed::workspace(store, 2).await.workspace;
    let now = project.now();
    let closed = now.checked_sub(DAY * 30).unwrap().as_millis();
    execute(
        store,
        &format!("UPDATE workspaces SET state = 'archived', closed_at = {closed} WHERE number = 1"),
    )
    .await;
    let mark = |workspace| async move {
        store
            .write(async |tx| mark_directory_removed(tx, workspace, now).await)
            .await
            .unwrap()
    };
    assert!(!mark(two).await, "a live workspace is not marked");
    assert!(mark(one).await);
    assert!(!mark(one).await, "the first mark stays");
    assert!(
        closed_workspaces_due(store.read(), now, 14)
            .await
            .unwrap()
            .is_empty()
    );
}
