use fold_core::{IntegrationId, TaskDescription, TaskId, TaskTitle};
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::tasks::{self, NewTask};
use fold_store::testing::{TestProject, assert_append_only};
use fold_store::{Store, StoreError};
use sqlx::Row;

#[cfg(test)]
async fn exec(store: &Store, sql: &str) -> Result<(), StoreError> {
    store
        .write(async |tx| {
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(tx.conn())
                .await?;
            Ok::<_, StoreError>(())
        })
        .await
}

#[cfg(test)]
async fn count(store: &Store, sql: &str) -> i64 {
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .fetch_one(store.read())
        .await
        .unwrap()
        .get::<i64, _>(0)
}

#[cfg(test)]
fn id(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
async fn project_with_tasks(count: u32) -> TestProject {
    let project = TestProject::new().await;
    let title = TaskTitle::parse("task").unwrap();
    let description = TaskDescription::new("body");
    let now = project.now();
    for number in 1..=count {
        project
            .store()
            .write(async |tx| {
                tasks::insert(
                    tx,
                    &NewTask {
                        number: id(number),
                        title: &title,
                        description: &description,
                        created_by: "frontend",
                        now,
                    },
                )
                .await
            })
            .await
            .unwrap();
    }
    project
}

/// A pull integration needs no proposal, so it is the smallest valid `integrations` row. It takes number 7, so the row id and the number differ.
#[cfg(test)]
async fn pull_integration(store: &Store) -> IntegrationId {
    exec(
        store,
        "INSERT INTO integrations (kind, number, remote_commit, result, actor, created_at)
         VALUES ('pull', 7, 'abc123', 'succeeded', 'human:ana', 1)",
    )
    .await
    .unwrap();
    IntegrationId::from_number(7).unwrap()
}

#[cfg(test)]
async fn complete(
    project: &TestProject,
    task: u32,
    kind: CompletionKind,
    integration: Option<IntegrationId>,
) -> Result<(), StoreError> {
    let now = project.now();
    project
        .store()
        .write(async |tx| completions::insert(tx, id(task), kind, integration, now).await)
        .await
}

#[tokio::test]
async fn update_and_delete_on_completions_fail() {
    let project = project_with_tasks(1).await;
    complete(&project, 1, CompletionKind::Children, None)
        .await
        .unwrap();
    assert_append_only(project.store(), "completions").await;
}

#[tokio::test]
async fn insert_records_the_kind_the_integration_and_the_time() {
    let project = project_with_tasks(2).await;
    let integration = pull_integration(project.store()).await;
    complete(&project, 1, CompletionKind::Integration, Some(integration))
        .await
        .unwrap();
    complete(&project, 2, CompletionKind::Children, None)
        .await
        .unwrap();
    let now = project.now().as_millis();
    assert_eq!(
        count(
            project.store(),
            &format!(
                "SELECT COUNT(*) FROM completions c JOIN tasks t ON t.id = c.task_id
                 WHERE c.created_at = {now}
                   AND ((t.number = 1 AND c.kind = 'integration' AND c.integration_id = (SELECT id FROM integrations WHERE number = {}))
                     OR (t.number = 2 AND c.kind = 'children' AND c.integration_id IS NULL))",
                integration.to_db()
            )
        )
        .await,
        2
    );
}

#[tokio::test]
async fn the_kind_and_integration_check_rejects_mismatched_rows() {
    let project = project_with_tasks(2).await;
    let integration = pull_integration(project.store()).await;

    let error = complete(&project, 1, CompletionKind::Integration, None)
        .await
        .unwrap_err();
    assert_eq!(
        error.constraint().as_deref(),
        Some("completions_integration_matches_kind")
    );
    let error = complete(&project, 2, CompletionKind::Children, Some(integration))
        .await
        .unwrap_err();
    assert_eq!(
        error.constraint().as_deref(),
        Some("completions_integration_matches_kind")
    );

    let error = exec(
        project.store(),
        "INSERT INTO completions (task_id, kind, created_at) VALUES (1, 'merged', 1)",
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.constraint().as_deref(),
        Some("completions_kind_values")
    );
    assert_eq!(
        count(project.store(), "SELECT COUNT(*) FROM completions").await,
        0
    );
}

#[tokio::test]
async fn a_task_completes_once_and_must_exist() {
    let project = project_with_tasks(1).await;
    complete(&project, 1, CompletionKind::Children, None)
        .await
        .unwrap();
    let error = complete(&project, 1, CompletionKind::Children, None)
        .await
        .unwrap_err();
    assert!(error.unique_violation(), "{error}");

    let error = complete(&project, 9, CompletionKind::Children, None)
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::Invalid(_)), "{error}");

    // The integration must exist too.
    let project = project_with_tasks(1).await;
    pull_integration(project.store()).await;
    let error = complete(
        &project,
        1,
        CompletionKind::Integration,
        Some(IntegrationId::from_number(8).unwrap()),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, StoreError::Invalid(_)), "{error}");
    assert_eq!(
        count(project.store(), "SELECT COUNT(*) FROM completions").await,
        0
    );
}
