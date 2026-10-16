use fold_core::{
    IdKind, LabelColor, LabelName, TaskDescription, TaskId, TaskKind, TaskStatus, TaskTitle,
    Timestamp,
};
use fold_store::tasks::{self, NewTask};
use fold_store::testing::TestProject;
use fold_store::{StoreError, counters};

#[cfg(test)]
fn title(text: &str) -> TaskTitle {
    TaskTitle::parse(text).unwrap()
}

#[cfg(test)]
fn label(text: &str) -> LabelName {
    LabelName::parse(text).unwrap()
}

#[cfg(test)]
fn id(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
async fn add(project: &TestProject, number: u32, name: &str) -> Result<(), StoreError> {
    let title = title(name);
    let description = TaskDescription::new("body");
    let now = project.now();
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
}

#[cfg(test)]
async fn add_label(project: &TestProject, name: &str, color: LabelColor) -> Result<(), StoreError> {
    let name = label(name);
    let now = project.now();
    project
        .store()
        .write(async |tx| tasks::insert_label(tx, &name, color, now).await)
        .await
}

#[tokio::test]
async fn numbers_are_allocated_in_order_and_never_reused() {
    let project = TestProject::new().await;
    let mut numbers = Vec::new();
    for _ in 0..3 {
        let number = project
            .store()
            .write(async |tx| counters::allocate(tx, IdKind::Task).await)
            .await
            .unwrap();
        numbers.push(number);
    }
    assert_eq!(numbers, [1, 2, 3]);

    // A rolled-back write returns its number to nobody: the counter row rolls back with it.
    let failed: Result<(), StoreError> = project
        .store()
        .write(async |tx| {
            counters::allocate(tx, IdKind::Task).await?;
            Err(StoreError::Invalid("stop"))
        })
        .await;
    assert!(failed.is_err());

    // A committed number stays used even when its task is gone.
    add(&project, 4, "four").await.unwrap();
    project
        .store()
        .write(async |tx| {
            sqlx::query("DELETE FROM tasks WHERE number = 4")
                .execute(tx.conn())
                .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let next = project
        .store()
        .write(async |tx| counters::allocate(tx, IdKind::Task).await)
        .await
        .unwrap();
    assert_eq!(next, 4);
    let after = project
        .store()
        .write(async |tx| counters::allocate(tx, IdKind::Task).await)
        .await
        .unwrap();
    assert_eq!(after, 5);
}

#[tokio::test]
async fn a_duplicate_number_is_refused_by_the_constraint() {
    let project = TestProject::new().await;
    add(&project, 1, "first").await.unwrap();
    let error = add(&project, 1, "second").await.unwrap_err();
    assert!(error.unique_violation());
    assert_eq!(error.constraint().as_deref(), Some("tasks.number"));
}

#[tokio::test]
async fn a_duplicate_label_name_is_refused_by_the_constraint() {
    let project = TestProject::new().await;
    add_label(&project, "bug", LabelColor::Plum).await.unwrap();
    let error = add_label(&project, "bug", LabelColor::Gold)
        .await
        .unwrap_err();
    assert!(error.unique_violation());
    assert_eq!(error.constraint().as_deref(), Some("labels.name"));
    add_label(&project, "Bug", LabelColor::Gold).await.unwrap();
}

#[tokio::test]
async fn an_invalid_status_is_refused_by_the_check() {
    let project = TestProject::new().await;
    add(&project, 1, "first").await.unwrap();
    let error = project
        .store()
        .write(async |tx| {
            sqlx::query("UPDATE tasks SET status = 'blocked' WHERE number = 1")
                .execute(tx.conn())
                .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap_err();
    assert_eq!(error.constraint().as_deref(), Some("tasks_status_values"));
}

#[tokio::test]
async fn a_new_task_has_the_defaults_and_reads_back() {
    let project = TestProject::new().await;
    add(&project, 1, "  first ").await.unwrap();
    let row = tasks::get_by_number(project.store().read(), id(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.id, id(1));
    assert_eq!(row.title.as_str(), "first");
    assert_eq!(row.description.as_str(), "body");
    assert_eq!(row.created_by, "frontend");
    assert_eq!(row.created_at, project.now());
    assert_eq!(row.updated_at, project.now());
    assert_eq!(row.archived_at, None);
    assert_eq!(row.status, TaskStatus::Backlog);
    assert!(!row.blocked);
    assert_eq!(row.human_review, None);
    assert!(
        tasks::get_by_number(project.store().read(), id(2))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn list_leaves_out_archived_tasks_unless_asked() {
    let project = TestProject::new().await;
    for number in 1..=3 {
        add(&project, number, "task").await.unwrap();
    }
    project
        .store()
        .write(async |tx| {
            sqlx::query("UPDATE tasks SET archived_at = 5 WHERE number = 2")
                .execute(tx.conn())
                .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let live = tasks::list(project.store().read(), false).await.unwrap();
    assert_eq!(
        live.iter().map(|t| t.id.number()).collect::<Vec<_>>(),
        [1, 3]
    );
    let all = tasks::list(project.store().read(), true).await.unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[1].archived_at, Some(Timestamp::from_millis(5).unwrap()));
}

#[tokio::test]
async fn labels_attach_replace_and_list_in_name_order() {
    let project = TestProject::new().await;
    add(&project, 1, "first").await.unwrap();
    add_label(&project, "b", LabelColor::Sage).await.unwrap();
    add_label(&project, "a", LabelColor::Teal).await.unwrap();
    add_label(&project, "c", LabelColor::Ink).await.unwrap();

    let names = vec![label("b"), label("a"), label("a")];
    project
        .store()
        .write(async |tx| tasks::replace_labels(tx, id(1), &names).await)
        .await
        .unwrap();
    let attached = tasks::labels_for(project.store().read(), id(1))
        .await
        .unwrap();
    assert_eq!(
        attached.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(attached[0].color, LabelColor::Teal);

    let names = vec![label("c")];
    project
        .store()
        .write(async |tx| tasks::replace_labels(tx, id(1), &names).await)
        .await
        .unwrap();
    let attached = tasks::labels_for(project.store().read(), id(1))
        .await
        .unwrap();
    assert_eq!(attached.len(), 1);
    assert_eq!(attached[0].name.as_str(), "c");
    assert_eq!(
        tasks::list_labels(project.store().read())
            .await
            .unwrap()
            .len(),
        3
    );

    let mut conn = project.store().read().acquire().await.unwrap();
    let missing = tasks::missing_labels(&mut conn, &[label("a"), label("zzz")])
        .await
        .unwrap();
    assert_eq!(missing, [label("zzz")]);
}

#[tokio::test]
async fn text_updates_stamp_the_time_and_a_missing_task_is_refused() {
    let project = TestProject::new().await;
    add(&project, 1, "first").await.unwrap();
    let later = Timestamp::from_millis(project.now().as_millis() + 1000).unwrap();
    let new_title = title("renamed");
    let new_description = TaskDescription::new("# new");
    project
        .store()
        .write(async |tx| tasks::update_text(tx, id(1), &new_title, &new_description, later).await)
        .await
        .unwrap();
    let row = tasks::get_by_number(project.store().read(), id(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.title, new_title);
    assert_eq!(row.description, new_description);
    assert_eq!(row.updated_at, later);
    assert_eq!(row.created_at, project.now());

    let latest = Timestamp::from_millis(later.as_millis() + 1).unwrap();
    project
        .store()
        .write(async |tx| tasks::touch(tx, id(1), latest).await)
        .await
        .unwrap();
    let row = tasks::get_by_number(project.store().read(), id(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.updated_at, latest);

    let missing = project
        .store()
        .write(async |tx| tasks::touch(tx, id(9), latest).await)
        .await;
    assert!(matches!(missing, Err(StoreError::Invalid(_))));
}

#[tokio::test]
async fn deleting_a_label_or_a_task_drops_its_links() {
    let project = TestProject::new().await;
    add(&project, 1, "first").await.unwrap();
    add(&project, 2, "second").await.unwrap();
    add_label(&project, "a", LabelColor::Sage).await.unwrap();
    add_label(&project, "b", LabelColor::Teal).await.unwrap();
    let names = vec![label("a"), label("b")];
    project
        .store()
        .write(async |tx| {
            tasks::replace_labels(tx, id(1), &names).await?;
            tasks::replace_labels(tx, id(2), &names).await
        })
        .await
        .unwrap();
    let delete = async |sql: &'static str| {
        project
            .store()
            .write(async |tx| {
                sqlx::query(sql).execute(tx.conn()).await?;
                Ok::<_, StoreError>(())
            })
            .await
            .unwrap();
    };

    delete("DELETE FROM labels WHERE name = 'a'").await;
    for task in [1, 2] {
        let attached = tasks::labels_for(project.store().read(), id(task))
            .await
            .unwrap();
        assert_eq!(
            attached.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            ["b"]
        );
    }

    delete("DELETE FROM tasks WHERE number = 1").await;
    let links: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_labels")
        .fetch_one(project.store().read())
        .await
        .unwrap();
    assert_eq!(links, 1);
    assert_eq!(
        tasks::labels_for(project.store().read(), id(2))
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn a_sync_task_is_found_by_its_commit_until_done_or_archived_and_keeps_it() {
    let project = TestProject::new().await;
    for number in 1..=4 {
        add(&project, number, "task").await.unwrap();
    }
    let read = project.store().read();
    assert_eq!(tasks::open_sync_task(read, "c1").await.unwrap(), None);
    project
        .store()
        .write(async |tx| {
            for number in [1, 2, 3] {
                tasks::set_remote_commit(tx, id(number), "c1").await?;
            }
            tasks::set_remote_commit(tx, id(4), "c2").await
        })
        .await
        .unwrap();
    assert_eq!(
        tasks::open_sync_task(read, "c1").await.unwrap(),
        Some(id(3))
    );
    assert_eq!(
        tasks::remote_commit(read, id(4)).await.unwrap().as_deref(),
        Some("c2")
    );
    project
        .store()
        .write(async |tx| {
            sqlx::query("UPDATE tasks SET status = 'done' WHERE number = 3")
                .execute(tx.conn())
                .await?;
            sqlx::query("UPDATE tasks SET archived_at = 5 WHERE number = 2")
                .execute(tx.conn())
                .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    assert_eq!(
        tasks::open_sync_task(read, "c1").await.unwrap(),
        Some(id(1))
    );
    let changed = project
        .store()
        .write(async |tx| tasks::set_remote_commit(tx, id(1), "c3").await)
        .await;
    assert!(changed.is_err());
    assert_eq!(
        tasks::remote_commit(read, id(1)).await.unwrap().as_deref(),
        Some("c1")
    );
    let missing = project
        .store()
        .write(async |tx| tasks::set_remote_commit(tx, id(9), "c1").await)
        .await;
    assert!(missing.is_err());
}
