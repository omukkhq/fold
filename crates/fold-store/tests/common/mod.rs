use fold_core::ProjectPaths;
use fold_store::{Store, StoreError};
use sqlx::Row;
use tempfile::TempDir;

pub async fn open() -> (TempDir, ProjectPaths, Store) {
    let dir = TempDir::new().unwrap();
    let paths = ProjectPaths::new(dir.path().join("project"));
    let store = Store::open_project(&paths).await.unwrap();
    (dir, paths, store)
}

pub async fn exec(store: &Store, sql: &str) -> Result<(), StoreError> {
    store
        .write(async |tx| {
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(tx.conn())
                .await?;
            Ok::<_, StoreError>(())
        })
        .await
}

pub async fn count(store: &Store, sql: &str) -> i64 {
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .fetch_one(store.read())
        .await
        .unwrap()
        .get::<i64, _>(0)
}
