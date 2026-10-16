use fold_core::{TaskDescription, TaskId, TaskKind, TaskTitle};
use fold_store::tasks::{self, NewTask, TaskHistory};
use fold_store::testing::TestProject;
use fold_store::{StoreError, workspaces};

#[cfg(test)]
fn id(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
async fn project_with_task() -> TestProject {
    let project = TestProject::new().await;
    let title = TaskTitle::parse("task").unwrap();
    let description = TaskDescription::new("body");
    let now = project.now();
    project
        .store()
        .write(async |tx| {
            tasks::insert(
                tx,
                &NewTask {
                    number: id(1),
                    title: &title,
                    description: &description,
                    created_by: "frontend",
                    kind: TaskKind::Proposal,
                    now,
                },
            )
            .await
        })
        .await
        .unwrap();
    project
}

#[tokio::test]
async fn an_unknown_task_has_no_history() {
    let project = project_with_task().await;
    let mut conn = project.store().read().acquire().await.unwrap();
    let history = tasks::history(&mut conn, id(9)).await.unwrap();
    assert_eq!(history, TaskHistory::default());
    assert!(!history.any());
    assert!(!tasks::history(&mut conn, id(1)).await.unwrap().any());
}

#[tokio::test]
async fn a_task_is_archived_once() {
    let project = project_with_task().await;
    let now = project.now();
    let result = project
        .store()
        .write(async |tx| {
            tasks::set_archived(tx, id(1), now).await?;
            tasks::set_archived(tx, id(1), now).await
        })
        .await;
    assert!(matches!(result, Err(StoreError::Invalid(_))), "{result:?}");
    let result = project
        .store()
        .write(async |tx| tasks::set_archived(tx, id(9), now).await)
        .await;
    assert!(matches!(result, Err(StoreError::Invalid(_))), "{result:?}");
}

#[tokio::test]
async fn deleting_an_unknown_task_fails() {
    let project = project_with_task().await;
    let result = project
        .store()
        .write(async |tx| tasks::delete(tx, id(9)).await)
        .await;
    assert!(matches!(result, Err(StoreError::Invalid(_))), "{result:?}");
}

#[tokio::test]
async fn nothing_to_release_or_archive_reports_nothing() {
    let project = project_with_task().await;
    let (workspace, items, edges) = project
        .store()
        .write(async |tx| {
            Ok::<_, StoreError>((
                workspaces::live_task_workspace(tx.conn(), id(1)).await?,
                tasks::delete_linked_items(tx, id(1)).await?,
                tasks::delete_edges_touching(tx, id(1)).await?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(workspace, None);
    assert_eq!((items, edges), (0, 0));
}

#[tokio::test]
async fn a_message_about_a_task_is_history() {
    let project = project_with_task().await;
    let history = project
        .store()
        .write(async |tx| {
            sqlx::query(
                "INSERT INTO messages (number, thread_id, sender_kind, sender_human, task_id, body, created_at)
                 VALUES (1, 1, 'human', 'ana', (SELECT id FROM tasks WHERE number = 1), 'hello', 1)",
            )
            .execute(tx.conn())
            .await?;
            tasks::history(tx.conn(), id(1)).await
        })
        .await
        .unwrap();
    assert_eq!(
        history,
        TaskHistory {
            message: true,
            ..TaskHistory::default()
        }
    );
    assert!(history.any());
}
