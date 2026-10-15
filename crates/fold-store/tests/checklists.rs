use fold_core::{ChecklistItemId, ChecklistName, StepText, TaskDescription, TaskId, TaskTitle};
use fold_store::tasks::checklists::{self, ItemContent, NewItem};
use fold_store::tasks::{self, NewTask};
use fold_store::testing::TestProject;
use fold_store::{Store, StoreError};

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn ci(number: u32) -> ChecklistItemId {
    ChecklistItemId::from_number(number).unwrap()
}

#[cfg(test)]
fn name(text: &str) -> ChecklistName {
    ChecklistName::parse(text).unwrap()
}

#[cfg(test)]
fn step(text: &str) -> ItemContent {
    ItemContent::Step(StepText::parse(text).unwrap())
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
                        number: t(number),
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

/// Creates `checklist` on `task` when it is missing and appends an item with the public number `number`.
#[cfg(test)]
async fn add(project: &TestProject, task: u32, checklist: &str, number: u32, content: ItemContent) {
    let now = project.now();
    let checklist = name(checklist);
    project
        .store()
        .write(async |tx| {
            checklists::ensure_checklist(tx, t(task), &checklist, now).await?;
            checklists::insert_item(
                tx,
                &NewItem {
                    id: ci(number),
                    task: t(task),
                    checklist: &checklist,
                    content: &content,
                    now,
                },
            )
            .await
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn an_item_needs_exactly_one_of_text_or_link() {
    let project = project_with_tasks(2).await;
    let store = project.store();
    exec(
        store,
        "INSERT INTO checklists (id, task_id, name, position, created_at) VALUES (1, 1, 'c', 0, 0)",
    )
    .await
    .unwrap();
    let item = |number: u32, text: &str, link: &str| {
        format!(
            "INSERT INTO checklist_items (number, checklist_id, text, linked_task_id, position, created_at) VALUES ({number}, 1, {text}, {link}, {number}, 0)"
        )
    };
    for sql in [item(1, "NULL", "NULL"), item(2, "'t'", "2")] {
        let error = exec(store, &sql).await.unwrap_err();
        assert_eq!(
            error.constraint().as_deref(),
            Some("checklist_items_text_or_link"),
            "{sql}"
        );
    }
    exec(store, &item(3, "'t'", "NULL")).await.unwrap();
    exec(store, &item(4, "NULL", "2")).await.unwrap();
}

#[tokio::test]
async fn the_schema_refuses_bad_names_steps_ticks_positions_and_numbers() {
    let project = project_with_tasks(2).await;
    let store = project.store();
    exec(
        store,
        "INSERT INTO checklists (id, task_id, name, position, created_at) VALUES (1, 1, 'c', 0, 0)",
    )
    .await
    .unwrap();
    exec(
        store,
        "INSERT INTO checklist_items (number, checklist_id, text, position, created_at) VALUES (1, 1, 'a', 0, 0)",
    )
    .await
    .unwrap();
    for (sql, rule) in [
        (
            "INSERT INTO checklists (task_id, name, position, created_at) VALUES (1, '  ', 1, 0)",
            "checklists_name_single_line",
        ),
        (
            "UPDATE checklists SET name = 'a' || char(10) WHERE id = 1",
            "checklists_name_single_line",
        ),
        (
            "INSERT INTO checklists (task_id, name, position, created_at) VALUES (1, 'c', 1, 0)",
            "checklists.task_id, checklists.name",
        ),
        (
            "INSERT INTO checklists (task_id, name, position, created_at) VALUES (1, 'd', 0, 0)",
            "checklists.task_id, checklists.position",
        ),
        (
            "INSERT INTO checklist_items (number, checklist_id, text, position, created_at) VALUES (2, 1, 'a' || char(13), 1, 0)",
            "checklist_items_text_single_line",
        ),
        (
            "INSERT INTO checklist_items (number, checklist_id, text, position, created_at) VALUES (2, 1, ' ', 1, 0)",
            "checklist_items_text_single_line",
        ),
        (
            "UPDATE checklist_items SET done_at = 5 WHERE number = 1",
            "checklist_items_done_with_actor",
        ),
        (
            "UPDATE checklist_items SET done_by = 'fold' WHERE number = 1",
            "checklist_items_done_with_actor",
        ),
        (
            "INSERT INTO checklist_items (number, checklist_id, text, position, created_at) VALUES (2, 1, 'b', 0, 0)",
            "checklist_items.checklist_id, checklist_items.position",
        ),
        (
            "INSERT INTO checklist_items (number, checklist_id, text, position, created_at) VALUES (1, 1, 'b', 1, 0)",
            "checklist_items.number",
        ),
        (
            "INSERT INTO checklist_items (number, checklist_id, linked_task_id, position, created_at) VALUES (2, 1, 1, 1, 0)",
            "checklist_items_no_self_link",
        ),
        (
            "UPDATE checklist_items SET text = NULL, linked_task_id = 1 WHERE number = 1",
            "checklist_items_no_self_link",
        ),
    ] {
        let message = exec(store, sql).await.unwrap_err().to_string();
        assert!(message.contains(rule), "{sql}: {message}");
    }
    // Another task may reuse a checklist name.
    exec(
        store,
        "INSERT INTO checklists (task_id, name, position, created_at) VALUES (2, 'c', 0, 0)",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn checklists_and_items_keep_their_order() {
    let project = project_with_tasks(3).await;
    add(&project, 1, "Subtasks", 1, step("first")).await;
    add(&project, 1, "QA", 2, ItemContent::Linked(t(2))).await;
    add(&project, 1, "Subtasks", 3, ItemContent::Linked(t(3))).await;
    add(&project, 1, "QA", 4, step("last")).await;
    let mut conn = project.store().read().acquire().await.unwrap();
    assert_eq!(
        checklists::checklists(&mut conn, t(1)).await.unwrap(),
        [name("Subtasks"), name("QA")]
    );
    let items = checklists::items(&mut conn, t(1)).await.unwrap();
    let ids: Vec<ChecklistItemId> = items.iter().map(|item| item.id).collect();
    assert_eq!(ids, [ci(1), ci(3), ci(2), ci(4)]);
    assert_eq!(items[1].content, ItemContent::Linked(t(3)));
    assert_eq!(items[1].checklist, name("Subtasks"));
    assert!(items.iter().all(|item| item.task == t(1)));
    assert!(items.iter().all(|item| item.done_at.is_none()));
    assert!(checklists::items(&mut conn, t(2)).await.unwrap().is_empty());
    assert_eq!(
        checklists::fetch_item(&mut conn, ci(4)).await.unwrap(),
        Some(items[3].clone())
    );
    assert_eq!(
        checklists::fetch_item(&mut conn, ci(9)).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn ensure_checklist_creates_a_name_once() {
    let project = project_with_tasks(1).await;
    let now = project.now();
    let created = project
        .store()
        .write(async |tx| {
            let first = checklists::ensure_checklist(tx, t(1), &name("Subtasks"), now).await?;
            let again = checklists::ensure_checklist(tx, t(1), &name("Subtasks"), now).await?;
            let missing_task = checklists::ensure_checklist(tx, t(9), &name("x"), now).await?;
            Ok::<_, StoreError>((first, again, missing_task))
        })
        .await
        .unwrap();
    assert_eq!(created, (true, false, false));
}

#[tokio::test]
async fn an_item_for_a_missing_checklist_inserts_nothing() {
    let project = project_with_tasks(1).await;
    let now = project.now();
    let content = step("a");
    let error = project
        .store()
        .write(async |tx| {
            checklists::insert_item(
                tx,
                &NewItem {
                    id: ci(1),
                    task: t(1),
                    checklist: &name("nope"),
                    content: &content,
                    now,
                },
            )
            .await
        })
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::Invalid(_)), "{error:?}");
}

#[tokio::test]
async fn steps_are_edited_ticked_and_deleted() {
    let project = project_with_tasks(2).await;
    add(&project, 1, "Subtasks", 1, step("old")).await;
    add(&project, 1, "Subtasks", 2, ItemContent::Linked(t(2))).await;
    let now = project.now();
    let new_text = StepText::parse("new").unwrap();
    let linked_edit = project
        .store()
        .write(async |tx| {
            checklists::set_text(tx, ci(1), &new_text).await?;
            checklists::set_done(tx, ci(1), Some((now, "frontend"))).await?;
            Ok::<_, StoreError>(checklists::set_text(tx, ci(2), &new_text).await)
        })
        .await
        .unwrap();
    assert!(matches!(linked_edit, Err(StoreError::Invalid(_))));
    let mut conn = project.store().read().acquire().await.unwrap();
    let item = checklists::fetch_item(&mut conn, ci(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.content, ItemContent::Step(new_text.clone()));
    assert_eq!(item.done_at, Some(now));
    assert_eq!(item.done_by.as_deref(), Some("frontend"));
    assert_eq!(checklists::progress(&mut conn, t(1)).await.unwrap(), (1, 2));
    drop(conn);

    let results = project
        .store()
        .write(async |tx| {
            checklists::set_done(tx, ci(1), None).await?;
            let missing = checklists::set_done(tx, ci(9), None).await;
            let deleted = checklists::delete_item(tx, ci(1)).await?;
            let again = checklists::delete_item(tx, ci(1)).await?;
            Ok::<_, StoreError>((missing, deleted, again))
        })
        .await
        .unwrap();
    assert!(matches!(results.0, Err(StoreError::Invalid(_))));
    assert_eq!((results.1, results.2), (true, false));
    let mut conn = project.store().read().acquire().await.unwrap();
    assert_eq!(checklists::progress(&mut conn, t(1)).await.unwrap(), (0, 1));
    assert_eq!(checklists::progress(&mut conn, t(2)).await.unwrap(), (0, 0));
}

#[tokio::test]
async fn linked_items_tick_once_their_child_is_done() {
    let project = project_with_tasks(3).await;
    add(&project, 1, "Subtasks", 1, ItemContent::Linked(t(2))).await;
    add(&project, 1, "Subtasks", 2, ItemContent::Linked(t(3))).await;
    add(&project, 1, "Subtasks", 3, step("own work")).await;
    exec(
        project.store(),
        "UPDATE tasks SET status = 'done' WHERE number = 3",
    )
    .await
    .unwrap();
    let now = project.now();
    let (first, second) = project
        .store()
        .write(async |tx| {
            let first = checklists::tick_done_children(tx, t(1), now).await?;
            let second = checklists::tick_done_children(tx, t(1), now).await?;
            Ok::<_, StoreError>((first, second))
        })
        .await
        .unwrap();
    assert_eq!(first, [(ci(2), t(3))]);
    assert!(second.is_empty());
    let mut conn = project.store().read().acquire().await.unwrap();
    let item = checklists::fetch_item(&mut conn, ci(2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item.done_at, Some(now));
    assert_eq!(item.done_by.as_deref(), Some("fold"));
    assert_eq!(checklists::progress(&mut conn, t(1)).await.unwrap(), (1, 3));
}
