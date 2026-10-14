use fold_core::Timestamp;
use fold_store::Store;
use fold_store::projects::{self, ProjectRow};
use tempfile::TempDir;

#[cfg(test)]
fn row(slug: &str, root: &str) -> ProjectRow {
    ProjectRow {
        slug: slug.to_owned(),
        name: slug.to_uppercase(),
        root: root.to_owned(),
    }
}

#[cfg(test)]
async fn registry_store() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let store = Store::open_registry(&dir.path().join("registry.db"))
        .await
        .unwrap();
    (dir, store)
}

#[tokio::test]
async fn projects_are_registered_listed_by_slug_and_found() {
    let (_dir, store) = registry_store().await;
    let now = Timestamp::from_millis(5).unwrap();
    for project in [row("beta", "/b"), row("alpha", "/a")] {
        store
            .write(async |tx| projects::insert_project(tx, &project, now).await)
            .await
            .unwrap();
    }
    let listed = projects::list_projects(store.read()).await.unwrap();
    assert_eq!(listed, [row("alpha", "/a"), row("beta", "/b")]);
    assert_eq!(
        projects::find_project(store.read(), "beta").await.unwrap(),
        Some(row("beta", "/b"))
    );
    assert_eq!(
        projects::find_project(store.read(), "none").await.unwrap(),
        None
    );
    assert_eq!(
        projects::find_by_root(store.read(), "/a").await.unwrap(),
        Some(row("alpha", "/a"))
    );
    assert_eq!(
        projects::find_by_root(store.read(), "/none").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn a_repeated_slug_or_root_is_a_unique_violation() {
    let (_dir, store) = registry_store().await;
    let now = Timestamp::from_millis(5).unwrap();
    store
        .write(async |tx| projects::insert_project(tx, &row("a", "/a"), now).await)
        .await
        .unwrap();
    for duplicate in [row("a", "/other"), row("other", "/a")] {
        let error = store
            .write(async |tx| projects::insert_project(tx, &duplicate, now).await)
            .await
            .unwrap_err();
        assert!(error.unique_violation());
    }
}
