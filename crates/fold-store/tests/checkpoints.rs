#![cfg(test)]

pub mod seed;

use std::time::Duration;

use fold_core::{CheckpointId, Timestamp, WorkspaceId};
use fold_store::testing::TestProject;
use fold_store::workspaces::{
    AUTO_CHECKPOINTS_KEPT, Checkpoint, CheckpointKind, CheckpointReason, get_checkpoint,
    insert_checkpoint, list_checkpoints, prune_auto, prune_workspace,
};
use fold_store::{Store, StoreError};

async fn checkpoint(
    store: &Store,
    workspace: WorkspaceId,
    reason: CheckpointReason,
    label: Option<&str>,
    now: Timestamp,
) -> Result<Checkpoint, StoreError> {
    store
        .write(async |tx| {
            insert_checkpoint(
                tx,
                workspace,
                seed::tree(&format!("{workspace} {reason:?}")),
                reason.kind(),
                reason,
                label,
                now,
            )
            .await
        })
        .await
}

fn ids(range: std::ops::RangeInclusive<u32>) -> Vec<CheckpointId> {
    range
        .map(|n| CheckpointId::from_number(n).unwrap())
        .collect()
}

fn numbers(checkpoints: &[Checkpoint]) -> Vec<u32> {
    checkpoints.iter().map(|c| c.id.number()).collect()
}

#[tokio::test]
async fn numbers_are_allocated_per_project_without_gaps() {
    let project = TestProject::new().await;
    let store = project.store();
    let one = seed::workspace(store, 1).await.workspace;
    let two = seed::workspace(store, 2).await.workspace;
    let now = project.now();

    let mut taken = Vec::new();
    for workspace in [one, two, one, two, two] {
        let made = checkpoint(store, workspace, CheckpointReason::Quiet, None, now)
            .await
            .unwrap();
        taken.push(made.id);
    }
    assert_eq!(taken, ids(1..=5));

    let rolled_back = store
        .write(async |tx| {
            insert_checkpoint(
                tx,
                one,
                seed::tree("x"),
                CheckpointKind::Auto,
                CheckpointReason::BeforeSync,
                None,
                now,
            )
            .await?;
            Err::<(), _>(StoreError::Invalid("roll back"))
        })
        .await;
    assert!(rolled_back.is_err());
    let missing = WorkspaceId::from_number(99).unwrap();
    let refused = checkpoint(store, missing, CheckpointReason::Explicit, None, now)
        .await
        .unwrap_err();
    assert!(matches!(
        refused,
        StoreError::Invalid("the workspace does not exist")
    ));

    let next = checkpoint(store, two, CheckpointReason::Explicit, None, now)
        .await
        .unwrap();
    assert_eq!(
        next.id.to_string(),
        "ck-6",
        "a refused or rolled-back insert leaves no gap"
    );
    let all = [
        list_checkpoints(store.read(), one, true).await.unwrap(),
        list_checkpoints(store.read(), two, true).await.unwrap(),
    ];
    assert_eq!(numbers(&all[0]), vec![1, 3]);
    assert_eq!(numbers(&all[1]), vec![2, 4, 5, 6]);
}

#[tokio::test]
async fn a_checkpoint_reads_back_as_written() {
    let project = TestProject::new().await;
    let store = project.store();
    let workspace = seed::workspace(store, 1).await.workspace;
    project.clock().advance(Duration::from_secs(90)).unwrap();
    let made = checkpoint(
        store,
        workspace,
        CheckpointReason::Explicit,
        Some("before the refactor"),
        project.now(),
    )
    .await
    .unwrap();
    assert_eq!(made.kind, CheckpointKind::Explicit);
    assert_eq!(made.workspace, workspace);
    assert_eq!(made.pruned_at, None);

    let read = get_checkpoint(store.read(), made.id).await.unwrap();
    assert_eq!(read.as_ref(), Some(&made));
    let missing = CheckpointId::from_number(7).unwrap();
    assert_eq!(get_checkpoint(store.read(), missing).await.unwrap(), None);

    let auto = checkpoint(
        store,
        workspace,
        CheckpointReason::BeforeRestore,
        None,
        project.now(),
    )
    .await
    .unwrap();
    let in_write = store
        .write(async |tx| get_checkpoint(tx.conn(), auto.id).await)
        .await
        .unwrap();
    assert_eq!(in_write, Some(auto));
}

#[tokio::test]
async fn a_reason_that_does_not_fit_the_kind_is_refused() {
    let project = TestProject::new().await;
    let store = project.store();
    let workspace = seed::workspace(store, 1).await.workspace;
    let now = project.now();
    for (kind, reason) in [
        (CheckpointKind::Auto, CheckpointReason::Explicit),
        (CheckpointKind::Explicit, CheckpointReason::Quiet),
    ] {
        let refused = store
            .write(async |tx| {
                insert_checkpoint(tx, workspace, seed::tree("t"), kind, reason, None, now).await
            })
            .await
            .unwrap_err();
        assert!(matches!(refused, StoreError::Invalid(_)), "{refused:?}");
    }
    assert!(
        list_checkpoints(store.read(), workspace, true)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn prune_auto_keeps_the_newest_twenty_and_every_explicit_one() {
    let project = TestProject::new().await;
    let store = project.store();
    let one = seed::workspace(store, 1).await.workspace;
    let two = seed::workspace(store, 2).await.workspace;
    let now = project.now();

    let mut explicit = Vec::new();
    let mut auto = Vec::new();
    for n in 0..28 {
        let reason = if n % 9 == 0 {
            CheckpointReason::Explicit
        } else {
            CheckpointReason::ALL[1 + n % 5]
        };
        let made = checkpoint(store, one, reason, None, now).await.unwrap();
        match made.kind {
            CheckpointKind::Explicit => explicit.push(made.id),
            CheckpointKind::Auto => auto.push(made.id),
        }
    }
    assert_eq!((explicit.len(), auto.len()), (4, 24));
    for _ in 0..3 {
        checkpoint(store, two, CheckpointReason::Quiet, None, now)
            .await
            .unwrap();
    }

    project.clock().advance(Duration::from_secs(60)).unwrap();
    let later = project.now();
    let pruned = store
        .write(async |tx| prune_auto(tx, one, AUTO_CHECKPOINTS_KEPT, later).await)
        .await
        .unwrap();
    assert_eq!(pruned, auto[..4].to_vec(), "the four oldest automatic ones");

    let kept = list_checkpoints(store.read(), one, false).await.unwrap();
    let kept_auto: Vec<CheckpointId> = kept
        .iter()
        .filter(|c| c.kind == CheckpointKind::Auto)
        .map(|c| c.id)
        .collect();
    assert_eq!(kept_auto, auto[4..].to_vec());
    assert_eq!(kept_auto.len(), 20);
    let kept_explicit: Vec<CheckpointId> = kept
        .iter()
        .filter(|c| c.kind == CheckpointKind::Explicit)
        .map(|c| c.id)
        .collect();
    assert_eq!(kept_explicit, explicit);

    let everything = list_checkpoints(store.read(), one, true).await.unwrap();
    assert_eq!(everything.len(), 28);
    for c in &everything {
        let expected = pruned.contains(&c.id).then_some(later);
        assert_eq!(c.pruned_at, expected, "{}", c.id);
    }
    assert!(
        list_checkpoints(store.read(), two, true)
            .await
            .unwrap()
            .iter()
            .all(|c| c.pruned_at.is_none()),
        "another workspace is never pruned"
    );

    let again = store
        .write(async |tx| prune_auto(tx, one, AUTO_CHECKPOINTS_KEPT, later).await)
        .await
        .unwrap();
    assert!(
        again.is_empty(),
        "pruned checkpoints do not count toward keep"
    );

    let none_kept = store
        .write(async |tx| prune_auto(tx, one, 0, later).await)
        .await
        .unwrap();
    assert_eq!(none_kept, auto[4..].to_vec());
    let left = list_checkpoints(store.read(), one, false).await.unwrap();
    assert_eq!(
        left.iter().map(|c| c.id).collect::<Vec<_>>(),
        explicit,
        "explicit checkpoints are never pruned automatically"
    );
}

#[tokio::test]
async fn prune_workspace_marks_one_workspace_and_no_other() {
    let project = TestProject::new().await;
    let store = project.store();
    let one = seed::workspace(store, 1).await.workspace;
    let two = seed::workspace(store, 2).await.workspace;
    let now = project.now();
    for reason in [
        CheckpointReason::Explicit,
        CheckpointReason::BeforeRecord,
        CheckpointReason::BeforeProposal,
    ] {
        checkpoint(store, one, reason, None, now).await.unwrap();
        checkpoint(store, two, reason, None, now).await.unwrap();
    }
    let early = store
        .write(async |tx| prune_auto(tx, one, 1, now).await)
        .await
        .unwrap();
    assert_eq!(early, ids(3..=3));

    project.clock().advance(Duration::from_secs(3600)).unwrap();
    let later = project.now();
    let pruned = store
        .write(async |tx| prune_workspace(tx, one, later).await)
        .await
        .unwrap();
    assert_eq!(pruned, vec![ids(1..=1)[0], ids(5..=5)[0]]);

    let marked = list_checkpoints(store.read(), one, true).await.unwrap();
    assert_eq!(numbers(&marked), vec![1, 3, 5]);
    assert!(marked.iter().all(|c| c.pruned_at.is_some()));
    assert_eq!(
        marked[1].pruned_at,
        Some(now),
        "an earlier prune keeps its time"
    );
    assert!(
        list_checkpoints(store.read(), one, false)
            .await
            .unwrap()
            .is_empty()
    );
    let other = list_checkpoints(store.read(), two, true).await.unwrap();
    assert_eq!(numbers(&other), vec![2, 4, 6]);
    assert!(other.iter().all(|c| c.pruned_at.is_none()));

    let again = store
        .write(async |tx| prune_workspace(tx, one, later).await)
        .await
        .unwrap();
    assert!(again.is_empty());
}

#[tokio::test]
async fn reasons_round_trip_and_fix_the_kind() {
    for reason in CheckpointReason::ALL {
        assert_eq!(CheckpointReason::parse(reason.as_str()), Some(reason));
        let expected = if reason == CheckpointReason::Explicit {
            CheckpointKind::Explicit
        } else {
            CheckpointKind::Auto
        };
        assert_eq!(reason.kind(), expected);
    }
    assert_eq!(CheckpointReason::parse("manual"), None);
    for kind in [CheckpointKind::Auto, CheckpointKind::Explicit] {
        assert_eq!(CheckpointKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(CheckpointKind::parse("other"), None);
}

#[tokio::test]
async fn a_damaged_row_reads_as_corrupt() {
    let project = TestProject::new().await;
    let store = project.store();
    let workspace = seed::workspace(store, 1).await.workspace;
    let good = seed::tree("good").to_string();
    let cases = [
        (1, "not a hash", "explicit", "checkpoints.tree_hash"),
        (2, good.as_str(), "someday", "checkpoints.reason"),
    ];
    for (number, tree, reason, column) in cases {
        store
            .write(async |tx| {
                sqlx::query(
                    "INSERT INTO checkpoints (number, workspace_id, tree_hash, kind, reason, created_at)
                     VALUES (?1, (SELECT id FROM workspaces WHERE number = 1), ?2, 'explicit', ?3, 0)",
                )
                .bind(number)
                .bind(tree)
                .bind(reason)
                .execute(tx.conn())
                .await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
        let id = CheckpointId::from_number(number).unwrap();
        let error = get_checkpoint(store.read(), id).await.unwrap_err();
        assert!(
            matches!(error, StoreError::Corrupt(found) if found == column),
            "{error:?}"
        );
    }
    let error = list_checkpoints(store.read(), workspace, true)
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::Corrupt(_)));
}
