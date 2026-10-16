use std::time::Duration;

use fold_core::{AgentName, TaskDescription, TaskId, TaskKind, TaskTitle, Timestamp, WorkerName};
use fold_store::agents;
use fold_store::tasks::claims::{self, ClaimRow, ReleaseReason};
use fold_store::tasks::{self, NewTask};
use fold_store::testing::TestProject;
use fold_store::{Store, StoreError};

#[cfg(test)]
fn id(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[cfg(test)]
fn worker_name(name: &str) -> WorkerName {
    WorkerName::parse(name).unwrap()
}

/// A project with tasks T-1 and T-2 and the agents `frontend` (workers `frontend` and `frontend/ui`) and `backend`. Returns the three worker row ids in that order.
#[cfg(test)]
async fn setup() -> (TestProject, [i64; 3]) {
    let project = TestProject::new().await;
    let now = project.now();
    let title = TaskTitle::parse("task").unwrap();
    let description = TaskDescription::new("body");
    let workers = project
        .store()
        .write(async |tx| {
            for number in 1..=2 {
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
                .await?;
            }
            let frontend =
                agents::insert_agent(tx.conn(), &AgentName::parse("frontend").unwrap(), now)
                    .await?;
            let main =
                agents::insert_worker(tx.conn(), frontend, &worker_name("frontend"), None, now)
                    .await?;
            let ui = agents::insert_worker(
                tx.conn(),
                frontend,
                &worker_name("frontend/ui"),
                Some(main),
                now,
            )
            .await?;
            let backend =
                agents::insert_agent(tx.conn(), &AgentName::parse("backend").unwrap(), now).await?;
            let other =
                agents::insert_worker(tx.conn(), backend, &worker_name("backend"), None, now)
                    .await?;
            Ok::<_, StoreError>([main, ui, other])
        })
        .await
        .unwrap();
    (project, workers)
}

#[cfg(test)]
async fn claim(store: &Store, task: TaskId, worker: i64, now: Timestamp) -> Result<(), StoreError> {
    store
        .write(async |tx| claims::insert_implementation(tx, task, worker, now).await)
        .await
}

#[cfg(test)]
async fn release(
    store: &Store,
    task: TaskId,
    worker: i64,
    now: Timestamp,
) -> Result<(), StoreError> {
    store
        .write(async |tx| claims::release(tx, task, worker, ReleaseReason::Released, now).await)
        .await
}

#[cfg(test)]
async fn holder(store: &Store, task: TaskId) -> Option<ClaimRow> {
    claims::unreleased_for_task(store.read(), task)
        .await
        .unwrap()
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
async fn a_second_unreleased_implementation_claim_is_refused_by_the_index() {
    let (project, [main, _, other]) = setup().await;
    let store = project.store();
    let now = project.now();
    claim(store, id(1), main, now).await.unwrap();

    let error = claim(store, id(1), other, now).await.unwrap_err();
    assert!(error.unique_violation(), "{error}");
    assert_eq!(error.constraint().as_deref(), Some("claims.task_id"));
    // The holder repeating the insert is refused the same way; the service turns that into a renewal.
    assert!(
        claim(store, id(1), main, now)
            .await
            .unwrap_err()
            .unique_violation()
    );

    let row = holder(store, id(1)).await.unwrap();
    assert_eq!(
        row,
        ClaimRow {
            task: id(1),
            worker_id: main,
            worker: worker_name("frontend"),
            acquired_at: now,
            renewed_at: now,
            stale_at: None,
        }
    );
    // Another task takes its own claim.
    claim(store, id(2), other, now).await.unwrap();
    assert_eq!(
        holder(store, id(2)).await.unwrap().worker,
        worker_name("backend")
    );
}

#[tokio::test]
async fn a_released_claim_allows_a_new_one() {
    let (project, [main, _, other]) = setup().await;
    let store = project.store();
    let start = project.now();
    claim(store, id(1), main, start).await.unwrap();

    let error = release(store, id(1), other, start).await.unwrap_err();
    assert!(matches!(error, StoreError::Invalid(_)), "{error}");
    assert_eq!(holder(store, id(1)).await.unwrap().worker_id, main);

    let later = project.clock().advance(Duration::from_secs(60)).unwrap();
    release(store, id(1), main, later).await.unwrap();
    assert_eq!(holder(store, id(1)).await, None);
    let released: (i64, String) =
        sqlx::query_as("SELECT released_at, release_reason FROM claims WHERE worker_id = ?")
            .bind(main)
            .fetch_one(store.read())
            .await
            .unwrap();
    assert_eq!(released, (later.as_millis(), "released".to_owned()));
    assert!(matches!(
        release(store, id(1), main, later).await.unwrap_err(),
        StoreError::Invalid(_)
    ));

    claim(store, id(1), other, later).await.unwrap();
    let row = holder(store, id(1)).await.unwrap();
    assert_eq!(row.worker, worker_name("backend"));
    assert_eq!(row.acquired_at, later);
}

#[tokio::test]
async fn a_claim_on_a_missing_task_inserts_nothing() {
    let (project, [main, _, _]) = setup().await;
    let error = claim(project.store(), id(9), main, project.now())
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::Invalid(_)), "{error}");
    assert_eq!(holder(project.store(), id(9)).await, None);
}

#[tokio::test]
async fn the_kind_and_subject_check_refuses_mismatched_rows() {
    let (project, _) = setup().await;
    let store = project.store();
    for sql in [
        "INSERT INTO claims (kind, worker_id, acquired_at, renewed_at) VALUES ('implementation', 1, 0, 0)",
        "INSERT INTO claims (kind, task_id, proposal_id, worker_id, acquired_at, renewed_at) VALUES ('implementation', 1, 1, 1, 0, 0)",
        "INSERT INTO claims (kind, task_id, worker_id, acquired_at, renewed_at) VALUES ('review', 1, 1, 0, 0)",
        "INSERT INTO claims (kind, worker_id, acquired_at, renewed_at) VALUES ('review', 1, 0, 0)",
    ] {
        let error = exec(store, sql).await.unwrap_err();
        assert_eq!(
            error.constraint().as_deref(),
            Some("claims_target_matches_kind"),
            "{sql}: {error}"
        );
    }
    let error = exec(
        store,
        "INSERT INTO claims (kind, task_id, worker_id, acquired_at, renewed_at) VALUES ('ownership', 1, 1, 0, 0)",
    )
    .await
    .unwrap_err();
    assert_eq!(error.constraint().as_deref(), Some("claims_kind_values"));
}

#[tokio::test]
async fn renew_touches_only_unreleased_claims_of_the_given_workers() {
    let (project, [main, ui, other]) = setup().await;
    let store = project.store();
    let start = project.now();
    claim(store, id(1), ui, start).await.unwrap();
    claim(store, id(2), other, start).await.unwrap();
    exec(store, "UPDATE claims SET stale_at = 1").await.unwrap();

    let later = project.clock().advance(Duration::from_secs(600)).unwrap();
    let renewed = store
        .write(async |tx| claims::renew(tx, &[main, ui], later).await)
        .await
        .unwrap();
    assert_eq!(renewed, 1);
    let mine = holder(store, id(1)).await.unwrap();
    assert_eq!(
        (mine.renewed_at, mine.stale_at, mine.acquired_at),
        (later, None, start)
    );
    let theirs = holder(store, id(2)).await.unwrap();
    assert_eq!(theirs.renewed_at, start);
    assert!(theirs.stale_at.is_some());

    let none = store
        .write(async |tx| claims::renew(tx, &[], later).await)
        .await
        .unwrap();
    assert_eq!(none, 0);

    release(store, id(1), ui, later).await.unwrap();
    let after_release = project.clock().advance(Duration::from_secs(60)).unwrap();
    let renewed = store
        .write(async |tx| claims::renew(tx, &[ui], after_release).await)
        .await
        .unwrap();
    assert_eq!(renewed, 0);
}

#[tokio::test]
async fn worker_ids_lists_every_worker_of_one_agent() {
    let (project, [main, ui, other]) = setup().await;
    let frontend = agents::find_agent(
        project.store().read(),
        &AgentName::parse("frontend").unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    let backend = agents::find_agent(
        project.store().read(),
        &AgentName::parse("backend").unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        agents::worker_ids(project.store().read(), frontend.id)
            .await
            .unwrap(),
        vec![main, ui]
    );
    assert_eq!(
        agents::worker_ids(project.store().read(), backend.id)
            .await
            .unwrap(),
        vec![other]
    );
}
