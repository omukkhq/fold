use std::path::Path;

use fold_core::{Clock, HostPaths, ManualClock, ProjectConfig, ProjectPaths};
use fold_daemon::services::projects::{ProjectError, init_project, load_config};
use fold_store::projects;
use fold_store::{Store, StoreError};
use tempfile::TempDir;

#[cfg(test)]
struct Host {
    dir: TempDir,
    paths: HostPaths,
    clock: ManualClock,
}

#[cfg(test)]
impl Host {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let data = dir.path().join("data");
        let paths = HostPaths::new(data.to_str(), None).unwrap();
        Self {
            dir,
            paths,
            clock: ManualClock::new(),
        }
    }

    fn root(&self, name: &str) -> std::path::PathBuf {
        self.dir.path().join(name)
    }

    async fn registry(&self) -> Store {
        Store::open_registry(&self.paths.registry_db())
            .await
            .unwrap()
    }
}

#[cfg(test)]
fn code(error: &ProjectError) -> Option<&'static str> {
    error.code()
}

#[tokio::test]
async fn init_creates_the_layout_and_registers_the_project() {
    let host = Host::new();
    let root = host.root("My App");
    let created = host.clock.now();
    let project = init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    assert_eq!(project.slug, "my-app");
    assert_eq!(project.name, "My App");
    assert_eq!(project.root, root.canonicalize().unwrap());

    let paths = ProjectPaths::new(&project.root);
    assert!(paths.db().is_file());
    let config = std::fs::read_to_string(paths.config()).unwrap();
    assert!(config.starts_with("# "));
    assert!(config.contains("# lease_minutes = 30"));
    let mut entries: Vec<_> = std::fs::read_dir(&project.root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert_eq!(entries, [".fold"]);
    let mut inside: Vec<_> = std::fs::read_dir(paths.fold_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    inside.retain(|name| !name.starts_with("fold.db-"));
    inside.sort();
    assert_eq!(inside, ["config.toml", "fold.db"]);

    let registry = host.registry().await;
    let rows = projects::list_projects(registry.read()).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].slug, "my-app");
    let stored: i64 = sqlx_created_at(&registry, "my-app").await;
    assert_eq!(stored, created.as_millis());
    registry.close().await.unwrap();
}

#[cfg(test)]
async fn sqlx_created_at(registry: &Store, slug: &str) -> i64 {
    sqlx::query_scalar("SELECT created_at FROM projects WHERE slug = ?")
        .bind(slug)
        .fetch_one(registry.read())
        .await
        .unwrap()
}

#[tokio::test]
async fn init_uses_the_given_name_for_the_slug() {
    let host = Host::new();
    let project = init_project(
        &host.paths,
        &host.root("dir"),
        Some("Shop Front!"),
        &host.clock,
    )
    .await
    .unwrap();
    assert_eq!(project.slug, "shop-front");
    assert_eq!(project.name, "Shop Front!");
}

#[tokio::test]
async fn init_twice_is_refused_with_project_exists() {
    let host = Host::new();
    let root = host.root("app");
    init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    let again = init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap_err();
    assert_eq!(code(&again), Some("project_exists"));
    assert!(
        ProjectPaths::new(root.canonicalize().unwrap())
            .db()
            .is_file()
    );

    // An unregistered `.fold/` left by a crash is refused too.
    let stale = host.root("stale");
    std::fs::create_dir_all(stale.join(".fold")).unwrap();
    let error = init_project(&host.paths, &stale, None, &host.clock)
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("project_exists"));
    assert!(stale.join(".fold").is_dir());

    let registry = host.registry().await;
    let rows = projects::list_projects(registry.read()).await.unwrap();
    assert_eq!(rows.len(), 1);
    registry.close().await.unwrap();
}

#[tokio::test]
async fn init_failure_after_layout_removes_what_it_created() {
    let host = Host::new();
    let registry = host.registry().await;
    registry
        .write(async |tx| {
            sqlx::query(
                "CREATE TRIGGER projects_refuse BEFORE INSERT ON projects BEGIN SELECT RAISE(ABORT, 'registry is read-only'); END",
            )
            .execute(tx.conn())
            .await
            .map_err(StoreError::from)?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    registry.close().await.unwrap();

    let root = host.root("app");
    let error = init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap_err();
    assert!(matches!(error, ProjectError::Store(_)));
    assert!(root.is_dir());
    assert!(!root.join(".fold").exists());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
}

#[tokio::test]
async fn slugs_stay_unique_in_one_registry() {
    let host = Host::new();
    let mut slugs = Vec::new();
    for parent in ["a", "b", "c"] {
        let root = host.root(parent).join("app");
        let project = init_project(&host.paths, &root, None, &host.clock)
            .await
            .unwrap();
        slugs.push(project.slug);
    }
    assert_eq!(slugs, ["app", "app-2", "app-3"]);
}

#[tokio::test]
async fn concurrent_inits_choose_different_slugs() {
    let host = Host::new();
    let first = host.root("x").join("app");
    let second = host.root("y").join("app");
    let (a, b) = tokio::join!(
        init_project(&host.paths, &first, None, &host.clock),
        init_project(&host.paths, &second, None, &host.clock),
    );
    let mut slugs = [a.unwrap().slug, b.unwrap().slug];
    slugs.sort();
    assert_eq!(slugs, ["app", "app-2"]);
}

#[tokio::test]
async fn load_config_returns_defaults_when_the_file_is_absent() {
    let host = Host::new();
    let paths = ProjectPaths::new(host.root("none"));
    assert_eq!(load_config(&paths).await.unwrap(), ProjectConfig::default());
}

#[tokio::test]
async fn load_config_reads_the_file_and_refuses_bad_values() {
    let host = Host::new();
    let project = init_project(&host.paths, &host.root("app"), None, &host.clock)
        .await
        .unwrap();
    let paths = ProjectPaths::new(&project.root);
    assert_eq!(load_config(&paths).await.unwrap(), ProjectConfig::default());
    std::fs::write(paths.config(), "lease_minutes = 7\n").unwrap();
    assert_eq!(load_config(&paths).await.unwrap().lease_minutes, 7);
    std::fs::write(paths.config(), "lease_minutes = 0\n").unwrap();
    let error = load_config(&paths).await.unwrap_err();
    assert_eq!(code(&error), Some("invalid_config"));
    std::fs::remove_file(paths.config()).unwrap();
    std::fs::create_dir(paths.config()).unwrap();
    let error = load_config(&paths).await.unwrap_err();
    assert_eq!(code(&error), Some("workspace_io"));
}

#[tokio::test]
async fn a_file_in_place_of_the_directory_is_an_io_error() {
    let host = Host::new();
    let file = host.root("file");
    std::fs::write(&file, "x").unwrap();
    let error = init_project(&host.paths, &file, None, &host.clock)
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("workspace_io"));
    assert!(Path::new(&file).is_file());
}
