use fold_core::{DependencyOrigin, TaskDescription, TaskId, TaskKind, TaskTitle, Timestamp};
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
                        kind: TaskKind::Proposal,
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
        let error = insert(project.store(), 1, 2, origin, now)
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

/// 2 needs 1, 3 needs 2, 4 needs 1, and 5 stands apart and needs 4.
#[tokio::test]
async fn reachability_follows_needs_edges_in_both_directions() {
    let project = project_with_tasks(6).await;
    let now = project.now();
    for (task, needs) in [(2, 1), (3, 2), (4, 1), (5, 4)] {
        insert(project.store(), task, needs, DependencyOrigin::Direct, now)
            .await
            .unwrap();
    }
    let mut conn = project.store().read().acquire().await.unwrap();
    let below_two = tasks::dependent_edges(&mut conn, &[id(2)]).await.unwrap();
    assert_eq!(below_two, [(id(3), id(2))], "only the dependents of T-2");
    let below_four = tasks::dependent_edges(&mut conn, &[id(4), id(6)])
        .await
        .unwrap();
    assert_eq!(below_four, [(id(5), id(4))]);
    let all = tasks::dependent_edges(&mut conn, &[id(1)]).await.unwrap();
    assert_eq!(
        all,
        [
            (id(2), id(1)),
            (id(3), id(2)),
            (id(4), id(1)),
            (id(5), id(4))
        ]
    );
    assert!(
        tasks::needs_transitively(&mut conn, id(3), id(1))
            .await
            .unwrap()
    );
    assert!(
        tasks::needs_transitively(&mut conn, id(5), id(5))
            .await
            .unwrap()
    );
    assert!(
        !tasks::needs_transitively(&mut conn, id(1), id(3))
            .await
            .unwrap()
    );
    assert!(
        !tasks::needs_transitively(&mut conn, id(3), id(4))
            .await
            .unwrap()
    );
    assert!(
        !tasks::needs_transitively(&mut conn, id(9), id(1))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn the_edges_of_a_task_run_both_ways() {
    let project = project_with_tasks(4).await;
    let now = project.now();
    for (task, needs) in [(2, 1), (3, 2), (4, 1)] {
        insert(project.store(), task, needs, DependencyOrigin::Direct, now)
            .await
            .unwrap();
    }
    let mut conn = project.store().read().acquire().await.unwrap();
    let pairs: Vec<(TaskId, TaskId)> = tasks::edges_of(&mut conn, id(2))
        .await
        .unwrap()
        .into_iter()
        .map(|edge| (edge.task, edge.needs))
        .collect();
    assert_eq!(pairs, [(id(2), id(1)), (id(3), id(2))]);
}
