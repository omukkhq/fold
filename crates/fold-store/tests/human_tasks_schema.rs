use std::path::Path;

use fold_core::{
    AgentName, EventActor, HoldMark, HoldReason, ProjectPaths, TaskDescription, TaskId, TaskKind,
    TaskTitle, Timestamp, WorkerName,
};
use fold_store::agents;
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::tasks::{self, NewTask, claims};
use fold_store::testing::TestProject;
use fold_store::{Store, StoreError};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{Connection, Row};

#[cfg(test)]
fn id(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

/// A project with T-1 of kind `human`, T-2 of kind `proposal` and the worker `backend`, whose row id it returns.
#[cfg(test)]
async fn setup() -> (TestProject, i64) {
    let project = TestProject::new().await;
    let now = project.now();
    let title = TaskTitle::parse("task").unwrap();
    let description = TaskDescription::new("body");
    let worker = project
        .store()
        .write(async |tx| {
            for (number, kind) in [(1, TaskKind::Human), (2, TaskKind::Proposal)] {
                tasks::insert(
                    tx,
                    &NewTask {
                        number: id(number),
                        title: &title,
                        description: &description,
                        created_by: "human:ana",
                        kind,
                        now,
                    },
                )
                .await?;
            }
            let agent =
                agents::insert_agent(tx, &AgentName::parse("backend").unwrap(), now).await?;
            agents::insert_worker(tx, agent, &WorkerName::parse("backend").unwrap(), now).await
        })
        .await
        .unwrap();
    (project, worker)
}

#[cfg(test)]
fn hold(reason: &str, at: i64) -> HoldMark {
    HoldMark {
        reason: HoldReason::parse(reason).unwrap(),
        set_by: EventActor::parse("human:ana").unwrap(),
        set_at: Timestamp::from_millis(at).unwrap(),
    }
}

#[cfg(test)]
async fn claim(store: &Store, task: u32, worker: i64) -> Result<(), StoreError> {
    store
        .write(async |tx| {
            let now = Timestamp::from_millis(9).unwrap();
            claims::insert_implementation(tx, id(task), worker, now).await
        })
        .await
}

#[cfg(test)]
async fn complete(store: &Store, task: u32, kind: CompletionKind) -> Result<(), StoreError> {
    store
        .write(async |tx| {
            let now = Timestamp::from_millis(9).unwrap();
            completions::insert(tx, id(task), kind, None, now).await
        })
        .await
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

#[tokio::test]
async fn a_human_task_keeps_its_kind_and_takes_no_claim() {
    let (project, worker) = setup().await;
    let row = tasks::get_by_number(project.store().read(), id(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((row.kind, row.hold), (TaskKind::Human, None));
    let changed = exec(
        project.store(),
        "UPDATE tasks SET kind = 'proposal' WHERE number = 1",
    )
    .await
    .unwrap_err();
    assert!(changed.to_string().contains("tasks_kind_once"), "{changed}");
    let refused = claim(project.store(), 1, worker).await.unwrap_err();
    assert!(
        refused.to_string().contains("claims_task_open_to_agents"),
        "{refused}"
    );
    claim(project.store(), 2, worker).await.unwrap();
}

#[tokio::test]
async fn only_a_human_task_takes_the_human_completion() {
    let (project, _) = setup().await;
    let store = project.store();
    let wrong = complete(store, 2, CompletionKind::Human).await.unwrap_err();
    assert!(
        wrong.to_string().contains("completions_human_kind"),
        "{wrong}"
    );
    for kind in [CompletionKind::Children, CompletionKind::Check] {
        let refused = complete(store, 1, kind).await.unwrap_err();
        assert!(
            refused.to_string().contains("completions_human_kind"),
            "{refused}"
        );
    }
    complete(store, 1, CompletionKind::Human).await.unwrap();
    assert_eq!(CompletionKind::Human.as_str(), "human");
    fold_store::testing::assert_append_only(store, "completions").await;
}

#[tokio::test]
async fn a_hold_is_set_replaced_and_cleared_whole_and_refuses_claims() {
    let (project, worker) = setup().await;
    let store = project.store();
    let now = project.now();
    let first = hold("Wait for the Meta review", 3);
    store
        .write(async |tx| tasks::set_hold(tx, id(2), &first, now).await)
        .await
        .unwrap();
    let row = tasks::get_by_number(store.read(), id(2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.hold, Some(first));
    let refused = claim(store, 2, worker).await.unwrap_err();
    assert!(
        refused.to_string().contains("claims_task_open_to_agents"),
        "{refused}"
    );
    let second = hold("Wait for the logo", 4);
    store
        .write(async |tx| tasks::set_hold(tx, id(2), &second, now).await)
        .await
        .unwrap();
    let row = tasks::get_by_number(store.read(), id(2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.hold, Some(second));
    store
        .write(async |tx| tasks::clear_hold(tx, id(2), now).await)
        .await
        .unwrap();
    let row = tasks::get_by_number(store.read(), id(2))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.hold, None);
    claim(store, 2, worker).await.unwrap();
    let missing = store
        .write(async |tx| tasks::set_hold(tx, id(9), &hold("x", 1), now).await)
        .await;
    assert!(matches!(missing, Err(StoreError::Invalid(_))));
    let missing = store
        .write(async |tx| tasks::clear_hold(tx, id(9), now).await)
        .await;
    assert!(matches!(missing, Err(StoreError::Invalid(_))));
}

#[tokio::test]
async fn a_hold_has_its_reason_setter_and_time_together() {
    let (project, _) = setup().await;
    let store = project.store();
    for sql in [
        "UPDATE tasks SET hold_reason = 'wait' WHERE number = 2",
        "UPDATE tasks SET hold_reason = 'wait', hold_set_by = 'human:ana' WHERE number = 2",
        "UPDATE tasks SET hold_set_by = 'human:ana', hold_set_at = 1 WHERE number = 2",
        "INSERT INTO tasks (number, title, created_by, created_at, updated_at, hold_reason) VALUES (3, 'c', 'x', 0, 0, 'wait')",
    ] {
        let refused = exec(store, sql).await.unwrap_err();
        assert!(
            refused.to_string().contains("tasks_hold_shape"),
            "{sql}: {refused}"
        );
    }
    let blank = exec(
        store,
        "UPDATE tasks SET hold_reason = '  ', hold_set_by = 'human:ana', hold_set_at = 1 WHERE number = 2",
    )
    .await
    .unwrap_err();
    assert!(
        blank.to_string().contains("tasks_hold_reason_present"),
        "{blank}"
    );
    exec(store, "UPDATE tasks SET kind = 'design' WHERE number = 2")
        .await
        .unwrap_err();
}

#[tokio::test]
async fn tasks_and_completions_made_before_the_human_kind_survive_the_change() {
    let dir = tempfile::tempdir().unwrap();
    let before = dir.path().join("before");
    std::fs::create_dir(&before).unwrap();
    let source = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations/project"));
    for entry in std::fs::read_dir(source).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        let version: u32 = name.split('_').next().unwrap().parse().unwrap();
        if name.ends_with(".sql") && version < 33 {
            std::fs::copy(source.join(&name), before.join(&name)).unwrap();
        }
    }
    let paths = ProjectPaths::new(dir.path().join("project"));
    std::fs::create_dir_all(paths.db().parent().unwrap()).unwrap();
    let options = SqliteConnectOptions::new()
        .filename(paths.db())
        .create_if_missing(true)
        .foreign_keys(true);
    let mut conn = SqliteConnection::connect_with(&options).await.unwrap();
    sqlx::migrate::Migrator::new(before.as_path())
        .await
        .unwrap()
        .run(&mut conn)
        .await
        .unwrap();
    for sql in [
        "INSERT INTO tasks (id, number, title, created_by, created_at, updated_at, kind) VALUES (1, 1, 'a', 'x', 0, 0, 'no_proposal'), (2, 2, 'b', 'x', 0, 0, 'proposal')",
        "INSERT INTO labels (id, name, color, created_at) VALUES (1, 'web', 'sage', 0)",
        "INSERT INTO task_labels (task_id, label_id) VALUES (1, 1), (2, 1)",
        "INSERT INTO completions (task_id, kind, integration_id, created_at) VALUES (1, 'check', NULL, 5)",
    ] {
        sqlx::query(sql).execute(&mut conn).await.unwrap();
    }
    conn.close().await.unwrap();

    let store = Store::open_project(&paths).await.unwrap();
    let rows: Vec<(i64, String, Option<String>)> =
        sqlx::query("SELECT number, kind, hold_reason FROM tasks ORDER BY number")
            .fetch_all(store.read())
            .await
            .unwrap()
            .iter()
            .map(|row| (row.get(0), row.get(1), row.get(2)))
            .collect();
    assert_eq!(
        rows,
        [
            (1, "no_proposal".to_owned(), None),
            (2, "proposal".to_owned(), None)
        ]
    );
    let completed: Vec<(i64, String)> = sqlx::query("SELECT task_id, kind FROM completions")
        .fetch_all(store.read())
        .await
        .unwrap()
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
    assert_eq!(completed, [(1, "check".to_owned())]);
    let labelled: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_labels")
        .fetch_one(store.read())
        .await
        .unwrap();
    assert_eq!(labelled, 2);
    let broken = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(store.read())
        .await
        .unwrap();
    assert!(broken.is_empty());
    fold_store::testing::assert_append_only(&store, "completions").await;
    exec(
        &store,
        "UPDATE tasks SET kind = 'proposal' WHERE number = 1",
    )
    .await
    .unwrap_err();
    exec(
        &store,
        "INSERT INTO tasks (id, number, title, created_by, created_at, updated_at, kind) VALUES (3, 3, 'c', 'x', 0, 0, 'human')",
    )
    .await
    .unwrap();
    store.close().await.unwrap();
}
