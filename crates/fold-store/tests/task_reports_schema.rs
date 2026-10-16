use std::path::Path;

use fold_core::{
    AgentName, ProjectPaths, TaskDescription, TaskId, TaskKind, TaskTitle, WorkerName,
};
use fold_store::agents;
use fold_store::tasks::completions::{self, CompletionKind};
use fold_store::tasks::reports::{self, Resolution};
use fold_store::tasks::{self, NewTask};
use fold_store::testing::TestProject;
use fold_store::{Store, StoreError};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{Connection, Row};

#[cfg(test)]
fn id(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

/// A project with T-1 of kind `no_proposal`, T-2 of kind `proposal` and the worker `designer`, whose row id it returns.
#[cfg(test)]
async fn setup() -> (TestProject, i64) {
    let project = TestProject::new().await;
    let now = project.now();
    let title = TaskTitle::parse("task").unwrap();
    let description = TaskDescription::new("body");
    let worker = project
        .store()
        .write(async |tx| {
            for (number, kind) in [(1, TaskKind::NoProposal), (2, TaskKind::Proposal)] {
                tasks::insert(
                    tx,
                    &NewTask {
                        number: id(number),
                        title: &title,
                        description: &description,
                        created_by: "frontend",
                        kind,
                        now,
                    },
                )
                .await?;
            }
            let agent =
                agents::insert_agent(tx, &AgentName::parse("designer").unwrap(), now).await?;
            agents::insert_worker(
                tx,
                agent,
                &WorkerName::parse("designer").unwrap(),
                None,
                now,
            )
            .await
        })
        .await
        .unwrap();
    (project, worker)
}

#[cfg(test)]
async fn report(
    project: &TestProject,
    task: u32,
    worker: i64,
    note: &str,
) -> Result<(), StoreError> {
    let now = project.now();
    project
        .store()
        .write(async |tx| reports::insert(tx, id(task), worker, note, now).await)
        .await
}

#[cfg(test)]
async fn resolve(
    store: &Store,
    task: u32,
    resolution: Resolution,
    note: Option<&str>,
) -> Result<(), StoreError> {
    store
        .write(async |tx| {
            let now = fold_core::Timestamp::from_millis(5).unwrap();
            reports::resolve(tx, id(task), resolution, "human:ana", note, now).await
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
async fn a_task_keeps_the_kind_it_was_created_with() {
    let (project, _) = setup().await;
    let row = tasks::get_by_number(project.store().read(), id(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.kind, TaskKind::NoProposal);
    let error = exec(
        project.store(),
        "UPDATE tasks SET kind = 'proposal' WHERE number = 1",
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("tasks_kind_once"), "{error}");
    exec(
        project.store(),
        "UPDATE tasks SET kind = 'design' WHERE number = 2",
    )
    .await
    .unwrap_err();
}

#[tokio::test]
async fn a_task_has_at_most_one_open_report_and_only_without_a_proposal() {
    let (project, worker) = setup().await;
    report(&project, 1, worker, "Drew the board").await.unwrap();
    let second = report(&project, 1, worker, "Again").await.unwrap_err();
    assert!(second.unique_violation(), "{second}");
    let other = report(&project, 2, worker, "Drew it").await.unwrap_err();
    assert!(
        other
            .to_string()
            .contains("task_reports_open_on_no_proposal_task"),
        "{other}"
    );
    report(&project, 1, worker, "   ").await.unwrap_err();
    assert!(matches!(
        report(&project, 9, worker, "No task").await,
        Err(StoreError::Invalid(_))
    ));

    let mut conn = project.store().read().acquire().await.unwrap();
    let open = reports::open_for_task(&mut conn, id(1))
        .await
        .unwrap()
        .unwrap();
    assert!(open.is_open());
    assert_eq!(open.worker.as_str(), "designer");
    assert_eq!(open.note, "Drew the board");
}

#[tokio::test]
async fn a_send_back_answers_the_open_report_once_and_a_new_report_opens_again() {
    let (project, worker) = setup().await;
    let store = project.store();
    report(&project, 1, worker, "Drew the board").await.unwrap();
    resolve(store, 1, Resolution::SentBack, None)
        .await
        .unwrap_err();
    resolve(store, 1, Resolution::Completed, Some("note"))
        .await
        .unwrap_err();
    resolve(store, 1, Resolution::SentBack, Some("Use the new colours"))
        .await
        .unwrap();
    assert!(matches!(
        resolve(store, 1, Resolution::Completed, None).await,
        Err(StoreError::Invalid(_))
    ));
    let frozen = exec(
        store,
        "UPDATE task_reports SET note = 'changed' WHERE resolution IS NOT NULL",
    )
    .await
    .unwrap_err();
    assert!(frozen.to_string().contains("task_reports_resolved_once"));
    exec(store, "DELETE FROM task_reports").await.unwrap_err();

    report(&project, 1, worker, "Changed the colours")
        .await
        .unwrap();
    resolve(store, 1, Resolution::Completed, None)
        .await
        .unwrap();
    let all = reports::for_task(store.read(), id(1), 10).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].note, "Changed the colours");
    assert_eq!(all[0].resolution, Some(Resolution::Completed));
    assert_eq!(all[0].resolved_by.as_deref(), Some("human:ana"));
    assert_eq!(all[1].resolution, Some(Resolution::SentBack));
    assert_eq!(
        all[1].send_back_note.as_deref(),
        Some("Use the new colours")
    );
    assert_eq!(
        reports::latest_for_task(store.read(), id(1))
            .await
            .unwrap()
            .unwrap()
            .note,
        "Changed the colours"
    );
    assert!(
        reports::owed_send_backs(
            store.read(),
            &[
                fold_core::WorkerState::Stopped,
                fold_core::WorkerState::Failed
            ]
        )
        .await
        .unwrap()
        .is_empty()
    );
    let history = store
        .write(async |tx| tasks::history(tx.conn(), id(1)).await)
        .await
        .unwrap();
    assert!(history.report && history.any());
}

#[tokio::test]
async fn a_lead_check_completion_has_no_integration() {
    let (project, _) = setup().await;
    let now = project.now();
    project
        .store()
        .write(async |tx| completions::insert(tx, id(1), CompletionKind::Check, None, now).await)
        .await
        .unwrap();
    assert_eq!(CompletionKind::Check.as_str(), "check");
    exec(
        project.store(),
        "INSERT INTO integrations (kind, number, remote_commit, result, actor, created_at)
         VALUES ('pull', 7, 'abc123', 'succeeded', 'human:ana', 1)",
    )
    .await
    .unwrap();
    let refused = exec(
        project.store(),
        "INSERT INTO completions (task_id, kind, integration_id, created_at)
         SELECT t.id, 'check', i.id, 1 FROM tasks t, integrations i WHERE t.number = 2",
    )
    .await
    .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("completions_integration_matches_kind"),
        "{refused}"
    );
    fold_store::testing::assert_append_only(project.store(), "completions").await;
}

#[tokio::test]
async fn completions_made_before_the_check_kind_survive_the_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let before = dir.path().join("before");
    std::fs::create_dir(&before).unwrap();
    let source = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations/project"));
    for entry in std::fs::read_dir(source).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        let version: u32 = name.split('_').next().unwrap().parse().unwrap();
        if name.ends_with(".sql") && version < 29 {
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
        "INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'a', 'x', 0, 0), (2, 2, 'b', 'x', 0, 0)",
        "INSERT INTO integrations (id, kind, number, remote_commit, result, actor, created_at) VALUES (4, 'pull', 1, 'abc123', 'succeeded', 'human:ana', 1)",
        "INSERT INTO completions (task_id, kind, integration_id, created_at) VALUES (1, 'integration', 4, 5), (2, 'children', NULL, 6)",
    ] {
        sqlx::query(sql).execute(&mut conn).await.unwrap();
    }
    conn.close().await.unwrap();

    let store = Store::open_project(&paths).await.unwrap();
    let rows: Vec<(i64, String, Option<i64>, i64)> = sqlx::query(
        "SELECT task_id, kind, integration_id, created_at FROM completions ORDER BY task_id",
    )
    .fetch_all(store.read())
    .await
    .unwrap()
    .iter()
    .map(|row| (row.get(0), row.get(1), row.get(2), row.get(3)))
    .collect();
    assert_eq!(
        rows,
        [
            (1, "integration".to_owned(), Some(4), 5),
            (2, "children".to_owned(), None, 6)
        ]
    );
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM tasks ORDER BY id")
        .fetch_all(store.read())
        .await
        .unwrap();
    assert_eq!(kinds, ["proposal", "proposal"]);
    let references: Vec<String> = sqlx::query_scalar(
        "SELECT \"table\" FROM pragma_foreign_key_list('completions') ORDER BY \"table\"",
    )
    .fetch_all(store.read())
    .await
    .unwrap();
    assert_eq!(references, ["integrations", "tasks"]);
    let broken = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(store.read())
        .await
        .unwrap();
    assert!(broken.is_empty());

    exec(
        &store,
        "INSERT INTO tasks (id, number, title, created_by, created_at, updated_at, kind) VALUES (3, 3, 'c', 'x', 0, 0, 'no_proposal')",
    )
    .await
    .unwrap();
    exec(
        &store,
        "INSERT INTO completions (task_id, kind, created_at) VALUES (3, 'check', 7)",
    )
    .await
    .unwrap();
    exec(
        &store,
        "INSERT INTO completions (task_id, kind, created_at) VALUES (9, 'check', 7)",
    )
    .await
    .unwrap_err();
    fold_store::testing::assert_append_only(&store, "completions").await;
    store.close().await.unwrap();
}
