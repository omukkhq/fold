#![cfg(test)]

use crate::seed;

use fold_core::{ChangesetId, RevisionId, TaskId, Timestamp};
use fold_store::testing::{TestProject, assert_append_only};
use fold_store::workspaces::{
    Changeset, ChangesetStats, get_changeset, insert_changeset, latest_changeset, list_changesets,
};
use fold_store::{Store, StoreError};
use sqlx::Row;

async fn record(
    store: &Store,
    at: seed::Seeded,
    message: &str,
    stats: ChangesetStats,
    now: Timestamp,
) -> Result<Changeset, StoreError> {
    store
        .write(async |tx| {
            insert_changeset(
                tx,
                at.workspace,
                Some(at.task),
                at.revision,
                seed::tree(message),
                message,
                stats,
                "frontend",
                now,
            )
            .await
        })
        .await
}

fn id(n: u32) -> ChangesetId {
    ChangesetId::from_number(n).unwrap()
}

#[tokio::test]
async fn changesets_refuse_update_and_delete() {
    let project = TestProject::new().await;
    let store = project.store();
    let at = seed::workspace(store, 1).await;
    record(store, at, "first", ChangesetStats::default(), project.now())
        .await
        .unwrap();
    assert_append_only(store, "changesets").await;
}

#[tokio::test]
async fn each_changeset_points_at_the_previous_one_of_its_workspace() {
    let project = TestProject::new().await;
    let store = project.store();
    let one = seed::workspace(store, 1).await;
    let two = seed::workspace(store, 2).await;
    let now = project.now();
    assert_eq!(
        latest_changeset(store.read(), one.workspace).await.unwrap(),
        None
    );

    let mut made = Vec::new();
    for (at, message) in [
        (one, "one a"),
        (two, "two a"),
        (one, "one b"),
        (one, "one c"),
        (two, "two b"),
    ] {
        made.push(
            record(store, at, message, ChangesetStats::default(), now)
                .await
                .unwrap(),
        );
    }
    let parents: Vec<(u32, Option<u32>)> = made
        .iter()
        .map(|c| (c.id.number(), c.parent.map(ChangesetId::number)))
        .collect();
    assert_eq!(
        parents,
        vec![
            (1, None),
            (2, None),
            (3, Some(1)),
            (4, Some(3)),
            (5, Some(2))
        ]
    );

    let listed = list_changesets(store.read(), one.workspace).await.unwrap();
    assert_eq!(
        listed,
        vec![made[0].clone(), made[2].clone(), made[3].clone()]
    );
    assert_eq!(
        latest_changeset(store.read(), one.workspace).await.unwrap(),
        Some(made[3].clone())
    );
    assert_eq!(
        latest_changeset(store.read(), two.workspace).await.unwrap(),
        Some(made[4].clone())
    );
    let read = get_changeset(store.read(), id(3)).await.unwrap().unwrap();
    assert_eq!(read, made[2]);
    assert_eq!(read.message, "one b");
    assert_eq!(read.created_by, "frontend");
    assert_eq!(read.task, Some(one.task));
    assert_eq!(read.base_revision, one.revision);
    assert_eq!(get_changeset(store.read(), id(9)).await.unwrap(), None);
}

#[tokio::test]
async fn stats_are_stored_as_file_counts() {
    let project = TestProject::new().await;
    let store = project.store();
    let at = seed::workspace(store, 1).await;
    let stats = ChangesetStats {
        files_added: 3,
        files_modified: 3,
        files_deleted: 1,
    };
    let made = record(store, at, "work", stats, project.now())
        .await
        .unwrap();
    let read = get_changeset(store.read(), made.id).await.unwrap().unwrap();
    assert_eq!(read.stats, stats);
    let stored: String = sqlx::query("SELECT stats FROM changesets WHERE number = 1")
        .fetch_one(store.read())
        .await
        .unwrap()
        .get(0);
    let json: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"files_added": 3, "files_modified": 3, "files_deleted": 1})
    );
}

#[tokio::test]
async fn rows_that_do_not_exist_or_do_not_match_are_refused() {
    let project = TestProject::new().await;
    let store = project.store();
    let one = seed::workspace(store, 1).await;
    let two = seed::workspace(store, 2).await;
    let now = project.now();
    let cases = [
        (
            seed::Seeded {
                task: two.task,
                ..one
            },
            "the workspace belongs to another task",
        ),
        (
            seed::Seeded {
                revision: RevisionId::from_number(9).unwrap(),
                ..one
            },
            "the base revision does not exist",
        ),
        (
            seed::Seeded {
                workspace: fold_core::WorkspaceId::from_number(9).unwrap(),
                task: TaskId::from_number(9).unwrap(),
                ..one
            },
            "the workspace does not exist",
        ),
    ];
    for (at, expected) in cases {
        let error = record(store, at, "m", ChangesetStats::default(), now)
            .await
            .unwrap_err();
        assert!(
            matches!(error, StoreError::Invalid(found) if found == expected),
            "{error:?}"
        );
    }
    let first = record(store, two, "m", ChangesetStats::default(), now)
        .await
        .unwrap();
    assert_eq!(first.id, id(1), "a refused insert takes no number");
    let in_write = store
        .write(async |tx| latest_changeset(tx.conn(), two.workspace).await)
        .await
        .unwrap();
    assert_eq!(in_write, Some(first));
}

#[tokio::test]
async fn a_damaged_row_reads_as_corrupt() {
    let project = TestProject::new().await;
    let store = project.store();
    let at = seed::workspace(store, 1).await;
    let good = seed::tree("good").to_string();
    let cases = [
        (1, "not a hash", "{}", "changesets.tree_hash"),
        (2, good.as_str(), "{}", "changesets.stats"),
        (
            3,
            good.as_str(),
            r#"{"files_added": -1, "files_modified": 0, "files_deleted": 0}"#,
            "changesets.stats",
        ),
    ];
    for (number, tree, stats, column) in cases {
        store
            .write(async |tx| {
                sqlx::query(
                    "INSERT INTO changesets (number, workspace_id, task_id, base_revision_id, tree_hash, message, stats, created_by, created_at)
                     VALUES (?1, 1, 1, 1, ?2, 'm', ?3, 'frontend', 0)",
                )
                .bind(number)
                .bind(tree)
                .bind(stats)
                .execute(tx.conn())
                .await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
        let error = get_changeset(store.read(), id(number)).await.unwrap_err();
        assert!(
            matches!(error, StoreError::Corrupt(found) if found == column),
            "{error:?}"
        );
    }
    let error = list_changesets(store.read(), at.workspace)
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::Corrupt(_)));
}
