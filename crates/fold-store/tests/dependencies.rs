use fold_core::{DependencyOrigin, TaskDescription, TaskId, TaskTitle, Timestamp};
use fold_store::tasks::{self, EdgeRow, NewTask};
use fold_store::testing::TestProject;
use fold_store::{Store, StoreError};

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

#[cfg(test)]
async fn insert(
    store: &Store,
    task: u32,
    needs: u32,
    origin: DependencyOrigin,
    now: Timestamp,
) -> Result<(), StoreError> {
    store
        .write(async |tx| tasks::insert_edge(tx, id(task), id(needs), origin, "backend", now).await)
        .await
}

#[tokio::test]
async fn a_self_edge_is_refused_by_the_check_constraint() {
    let project = project_with_tasks(1).await;
    let error = insert(
        project.store(),
        1,
        1,
        DependencyOrigin::Direct,
        project.now(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.constraint().as_deref(),
        Some("dependencies_no_self_edge")
    );
    assert!(
        tasks::edges(project.store().read())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_second_row_for_the_same_pair_is_refused_by_the_primary_key() {
    let project = project_with_tasks(2).await;
    let now = project.now();
    insert(project.store(), 1, 2, DependencyOrigin::Direct, now)
        .await
        .unwrap();
    for origin in DependencyOrigin::ALL {
        let error = insert(project.store(), 1, 2, *origin, now)
            .await
            .unwrap_err();
        assert!(error.unique_violation(), "{origin}: {error}");
    }
    // The reverse direction is a different pair; the database leaves cycles to the service.
    insert(project.store(), 2, 1, DependencyOrigin::Checklist, now)
        .await
        .unwrap();
    assert_eq!(tasks::edges(project.store().read()).await.unwrap().len(), 2);
}

#[tokio::test]
async fn an_unknown_task_number_inserts_nothing() {
    let project = project_with_tasks(1).await;
    let now = project.now();
    for (task, needs) in [(1, 9), (9, 1)] {
        let error = insert(project.store(), task, needs, DependencyOrigin::Direct, now)
            .await
            .unwrap_err();
        assert!(matches!(error, StoreError::Invalid(_)), "{error}");
    }
    assert!(
        tasks::edges(project.store().read())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn edges_come_back_with_public_numbers_in_order() {
    let project = project_with_tasks(3).await;
    let first = project.now();
    insert(project.store(), 3, 1, DependencyOrigin::Direct, first)
        .await
        .unwrap();
    project
        .clock()
        .advance(std::time::Duration::from_secs(1))
        .unwrap();
    let second = project.now();
    insert(project.store(), 1, 2, DependencyOrigin::Checklist, second)
        .await
        .unwrap();
    insert(project.store(), 3, 2, DependencyOrigin::Direct, second)
        .await
        .unwrap();

    let edges = tasks::edges(project.store().read()).await.unwrap();
    let pairs: Vec<(TaskId, TaskId)> = edges.iter().map(|e| (e.task, e.needs)).collect();
    assert_eq!(pairs, vec![(id(1), id(2)), (id(3), id(1)), (id(3), id(2))]);
    assert_eq!(
        edges[0],
        EdgeRow {
            task: id(1),
            needs: id(2),
            origin: DependencyOrigin::Checklist,
            created_by: "backend".to_owned(),
            created_at: second,
        }
    );
    assert_eq!(edges[1].created_at, first);

    let mut conn = project.store().read().acquire().await.unwrap();
    assert_eq!(
        tasks::fetch_edge(&mut conn, id(1), id(2)).await.unwrap(),
        Some(edges[0].clone())
    );
    assert_eq!(
        tasks::fetch_edge(&mut conn, id(2), id(1)).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn delete_reports_whether_an_edge_was_there() {
    let project = project_with_tasks(2).await;
    insert(
        project.store(),
        1,
        2,
        DependencyOrigin::Direct,
        project.now(),
    )
    .await
    .unwrap();
    let store = project.store();
    assert!(
        store
            .write(async |tx| tasks::delete_edge(tx, id(1), id(2)).await)
            .await
            .unwrap()
    );
    assert!(
        !store
            .write(async |tx| tasks::delete_edge(tx, id(1), id(2)).await)
            .await
            .unwrap()
    );
    assert!(
        !store
            .write(async |tx| tasks::delete_edge(tx, id(1), id(9)).await)
            .await
            .unwrap()
    );
    assert!(tasks::edges(store.read()).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_corrupt_origin_is_reported_by_column() {
    let project = project_with_tasks(2).await;
    insert(
        project.store(),
        1,
        2,
        DependencyOrigin::Direct,
        project.now(),
    )
    .await
    .unwrap();
    // The CHECK constraint keeps bad origins out, so the corruption is planted with the constraint switched off.
    project
        .store()
        .write(async |tx| -> Result<(), StoreError> {
            sqlx::query("PRAGMA ignore_check_constraints = ON")
                .execute(tx.conn())
                .await?;
            sqlx::query("UPDATE dependencies SET origin = 'other'")
                .execute(tx.conn())
                .await?;
            sqlx::query("PRAGMA ignore_check_constraints = OFF")
                .execute(tx.conn())
                .await?;
            Ok(())
        })
        .await
        .unwrap();
    let error = tasks::edges(project.store().read()).await.unwrap_err();
    assert!(
        matches!(error, StoreError::Corrupt("dependencies.origin")),
        "{error}"
    );
}
