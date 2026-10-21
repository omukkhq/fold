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

#[tokio::test]
async fn a_project_is_used_when_registered_and_each_touch_after_the_period_moves_it() {
    let (_dir, store) = registry_store().await;
    let at = |millis| Timestamp::from_millis(millis).unwrap();
    for (project, now) in [(row("alpha", "/a"), at(5)), (row("beta", "/b"), at(7))] {
        store
            .write(async |tx| projects::insert_project(tx, &project, now).await)
            .await
            .unwrap();
    }
    let uses = async || -> Vec<(String, i64)> {
        projects::list_project_uses(store.read())
            .await
            .unwrap()
            .into_iter()
            .map(|entry| (entry.row.slug, entry.last_used_at.as_millis()))
            .collect()
    };
    assert_eq!(
        uses().await,
        [("alpha".to_owned(), 5), ("beta".to_owned(), 7)]
    );
    let touch = async |slug: &'static str, now, stale_before| {
        store
            .write(async |tx| projects::touch_project(tx, slug, at(now), at(stale_before)).await)
            .await
            .unwrap()
    };
    assert!(touch("alpha", 20, 10).await);
    assert!(
        !touch("alpha", 25, 15).await,
        "a use inside the period is kept"
    );
    assert!(!touch("none", 25, 15).await);
    assert_eq!(
        uses().await,
        [("alpha".to_owned(), 20), ("beta".to_owned(), 7)]
    );
    let found = projects::find_project_use(store.read(), "alpha")
        .await
        .unwrap();
    assert_eq!(found.map(|entry| entry.last_used_at), Some(at(20)));
    assert_eq!(
        projects::find_project_use(store.read(), "none")
            .await
            .unwrap(),
        None
    );
}
