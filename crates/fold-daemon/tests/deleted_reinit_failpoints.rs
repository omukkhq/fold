#![cfg(test)]

use std::time::Duration;

use fold_core::ProjectPaths;
use fold_daemon::services::projects::{self as project_service, init_project};
use fold_store::Store;
use fold_store::testing::eventually;

use crate::deleted_projects::{Running, closed};

const ROWS: i64 = 200;
const WAIT: Duration = Duration::from_secs(20);

async fn probe_rows(store: &Store) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM probe")
        .fetch_one(store.read())
        .await
        .unwrap()
}

async fn event_rows(store: &Store) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(store.read())
        .await
        .unwrap()
}

/// The new project has no session, so a `session.ended` in its database comes from the old handle's close.
async fn ended_sessions(store: &Store) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events WHERE type = 'session.ended'")
        .fetch_one(store.read())
        .await
        .unwrap()
}

#[tokio::test]
async fn the_close_of_a_deleted_fold_held_at_its_store_keeps_the_rows_of_a_new_init() {
    the_close_keeps_the_rows_of_a_new_init("store_close").await;
}

/// Held at its first step, the old handle also ends its sessions, checkpoints and closes its store after the new database holds rows.
#[tokio::test]
async fn the_close_of_a_deleted_fold_held_at_its_first_step_keeps_the_rows_of_a_new_init() {
    the_close_keeps_the_rows_of_a_new_init("checks").await;
}

/// Starts the close of the old handle of a deleted `.fold/` and holds it at `step`, once its registry row is gone.
async fn hold_the_close(running: &Running, step: &str) -> tokio::task::JoinHandle<usize> {
    fail::cfg("project_close_hold", &format!("return({step})")).unwrap();
    std::fs::remove_dir_all(running.fold_dir()).unwrap();
    let state = running.state.clone();
    let closing = tokio::spawn(async move { state.projects.close_stale().await });
    eventually(WAIT, async || {
        if *running.handle.bus.closing().borrow() && !running.registered().await {
            Ok(())
        } else {
            Err("the old handle has not begun to close")
        }
    })
    .await;
    closing
}

/// An init in the folder of an old handle that closes from `step` on waits for the close, and makes nothing in the folder until then; the new project's rows are all there after the reopen, and an open of the slug by the daemon is refused until the new init registered it.
async fn the_close_keeps_the_rows_of_a_new_init(step: &str) {
    let running = Running::start().await;
    let old = running.handle.clone();
    let closing = hold_the_close(&running, step).await;

    let (host, root, clock) = (
        running.host.clone(),
        running.root.clone(),
        running.clock.clone(),
    );
    let init = tokio::spawn(async move {
        init_project(&host, &root, None, &crate::common::ana(), &clock).await
    });
    let state = running.state.clone();
    let opening = tokio::spawn(async move { project_service::open(&state, "app").await });
    // A waiting init shows no effect to wait for, so it gets a fixed time to go on if it could.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!closing.is_finished(), "the old close is still held");
    assert!(!old.store.read().is_closed(), "the old store is still open");
    assert!(!init.is_finished(), "the init waits for the close");
    assert!(!running.fold_dir().exists(), "the init made nothing yet");
    // The registry row went before the close, so the daemon's open of the slug is refused at once and never opens the folder.
    let refused = tokio::time::timeout(WAIT, opening)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(refused.code(), Some("project_not_found"));

    fail::remove("project_close_hold");
    assert_eq!(
        tokio::time::timeout(WAIT, closing).await.unwrap().unwrap(),
        1
    );
    assert!(closed(&old));
    tokio::time::timeout(WAIT, init)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let paths = ProjectPaths::new(running.root.canonicalize().unwrap());
    let writer = Store::open_project(&paths).await.unwrap();
    let events = event_rows(&writer).await;
    assert!(events > 0, "the init wrote its events");
    writer
        .write(async |tx| -> Result<(), fold_store::StoreError> {
            sqlx::query("CREATE TABLE probe (n INTEGER NOT NULL)")
                .execute(tx.conn())
                .await?;
            for n in 0..ROWS {
                sqlx::query("INSERT INTO probe (n) VALUES (?)")
                    .bind(n)
                    .execute(tx.conn())
                    .await?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let opened = project_service::open(&running.state, "app").await.unwrap();
    assert_eq!(probe_rows(&opened.store).await, ROWS);
    assert_eq!(
        ended_sessions(&opened.store).await,
        0,
        "the old close wrote nothing into the new database"
    );
    writer.close().await.unwrap();
    // As the daemon's stop closes it; `Running::close` would also delete the folder before the reopen.
    opened.bus.close();
    opened.stop_tasks(Duration::from_secs(5)).await;
    opened.store.close().await.unwrap();
    running.state.registry.close().await.unwrap();

    let reopened = Store::open_project(&paths).await.unwrap();
    assert_eq!(probe_rows(&reopened).await, ROWS);
    assert!(event_rows(&reopened).await >= events);
    assert_eq!(ended_sessions(&reopened).await, 0);
    reopened.close().await.unwrap();
}

/// An init whose wait for the old close runs out is refused with a typed error and a hint, makes nothing in the folder, and succeeds once the close is over.
#[tokio::test]
async fn an_init_during_a_close_that_outlasts_its_wait_is_refused_and_makes_nothing() {
    let running = Running::start().await;
    let closing = hold_the_close(&running, "store_close").await;
    fail::cfg("init_close_wait_ms", "return(200)").unwrap();

    let refused = init_project(
        &running.host,
        &running.root,
        None,
        &crate::common::ana(),
        &running.clock,
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code(), Some("project_unavailable"));
    let project_service::ProjectError::Fold(error) = refused else {
        panic!("not a fold error: {refused:?}");
    };
    assert_eq!(error.details().unwrap()["reason"], "old_project_closing");
    assert!(error.hint().unwrap().contains("run `fld init`"));
    assert!(
        !running.fold_dir().exists(),
        "the refused init made nothing"
    );

    fail::remove("project_close_hold");
    assert_eq!(
        tokio::time::timeout(WAIT, closing).await.unwrap().unwrap(),
        1
    );
    init_project(
        &running.host,
        &running.root,
        None,
        &crate::common::ana(),
        &running.clock,
    )
    .await
    .unwrap();
    running.close().await;
}

/// An init that ran to its end before the daemon looked at the folder holds no lock any more, so the old handle closes after the new database holds rows in its WAL: the old store's close leaves the new WAL and every row in place.
#[tokio::test]
async fn an_old_store_that_closes_after_a_finished_init_keeps_the_new_rows() {
    let running = Running::start().await;
    let old = running.handle.clone();
    std::fs::remove_dir_all(running.fold_dir()).unwrap();
    init_project(
        &running.host,
        &running.root,
        None,
        &crate::common::ana(),
        &running.clock,
    )
    .await
    .unwrap();
    assert!(!old.store.read().is_closed(), "the old store is still open");

    let paths = ProjectPaths::new(running.root.canonicalize().unwrap());
    let writer = Store::open_project(&paths).await.unwrap();
    let events = event_rows(&writer).await;
    writer
        .write(async |tx| -> Result<(), fold_store::StoreError> {
            sqlx::query("CREATE TABLE probe (n INTEGER NOT NULL)")
                .execute(tx.conn())
                .await?;
            for n in 0..ROWS {
                sqlx::query("INSERT INTO probe (n) VALUES (?)")
                    .bind(n)
                    .execute(tx.conn())
                    .await?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let wal = paths.db().with_file_name("fold.db-wal");
    let wal_inode = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&wal).unwrap());

    assert_eq!(running.state.projects.close_stale().await, 1);
    assert!(closed(&old));
    let kept = std::fs::metadata(&wal).expect("the old close left the new WAL in place");
    assert_eq!(std::os::unix::fs::MetadataExt::ino(&kept), wal_inode);
    let again = Store::open_project(&paths).await.unwrap();
    assert_eq!(probe_rows(&again).await, ROWS);
    assert_eq!(event_rows(&again).await, events);
    assert_eq!(ended_sessions(&again).await, 0);
    again.close().await.unwrap();
    writer.close().await.unwrap();
    running.state.registry.close().await.unwrap();

    let reopened = Store::open_project(&paths).await.unwrap();
    assert_eq!(probe_rows(&reopened).await, ROWS);
    assert_eq!(event_rows(&reopened).await, events);
    reopened.close().await.unwrap();
}
