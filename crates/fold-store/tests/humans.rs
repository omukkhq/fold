use fold_core::{HumanName, Timestamp};
use fold_store::projects::{self, HumanRow};
use fold_store::{Store, StoreError};
use tempfile::TempDir;

#[cfg(test)]
async fn registry_store() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let store = Store::open_registry(&dir.path().join("registry.db"))
        .await
        .unwrap();
    (dir, store)
}

#[cfg(test)]
fn name(text: &str) -> HumanName {
    HumanName::parse(text).unwrap()
}

#[cfg(test)]
fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

#[tokio::test]
async fn ensure_inserts_once_and_keeps_the_first_created_at() {
    let (_dir, store) = registry_store().await;
    assert_eq!(
        projects::find_human(store.read(), &name("tanim"))
            .await
            .unwrap(),
        None
    );
    let first = store
        .write(async |tx| projects::ensure_human(tx, &name("tanim"), at(10)).await)
        .await
        .unwrap();
    assert_eq!(first.name, name("tanim"));
    assert_eq!(first.created_at, at(10));
    let second = store
        .write(async |tx| projects::ensure_human(tx, &name("tanim"), at(20)).await)
        .await
        .unwrap();
    assert_eq!(second, first);
    let other = store
        .write(async |tx| projects::ensure_human(tx, &name("ana"), at(30)).await)
        .await
        .unwrap();
    assert_ne!(other.id, first.id);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM humans")
        .fetch_one(store.read())
        .await
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        projects::find_human(store.read(), &name("tanim"))
            .await
            .unwrap(),
        Some(first)
    );
}

#[tokio::test]
async fn a_duplicate_name_is_refused_by_the_unique_index() {
    let (_dir, store) = registry_store().await;
    let id = store
        .write(async |tx| projects::insert_human(tx, &name("tanim"), at(10)).await)
        .await
        .unwrap();
    let error = store
        .write(async |tx| projects::insert_human(tx, &name("tanim"), at(20)).await)
        .await
        .unwrap_err();
    assert!(error.unique_violation(), "{error}");
    assert_eq!(error.constraint().as_deref(), Some("humans.name"));
    assert_eq!(
        projects::find_human(store.read(), &name("tanim"))
            .await
            .unwrap(),
        Some(HumanRow {
            id,
            name: name("tanim"),
            created_at: at(10),
        })
    );
}

#[tokio::test]
async fn a_stored_time_that_no_longer_parses_is_corrupt() {
    let (_dir, store) = registry_store().await;
    store
        .write(async |tx| {
            sqlx::query("INSERT INTO humans (name, created_at) VALUES ('bad', -99999999999999999)")
                .execute(tx.conn())
                .await
                .map_err(StoreError::from)
        })
        .await
        .unwrap();
    let error = projects::find_human(store.read(), &name("bad"))
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::Corrupt("humans.created_at")));
}
