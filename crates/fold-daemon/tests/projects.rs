use std::path::Path;

use fold_core::{Clock, HostPaths, ManualClock, ProjectConfig, ProjectPaths};
use fold_daemon::services::projects::{ProjectError, init_project, init_project_from, load_config};
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
    assert_eq!(entries, [".fold", "main"]);
    let mut inside: Vec<_> = std::fs::read_dir(paths.fold_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    inside.retain(|name| !name.starts_with("fold.db-"));
    inside.sort();
    assert_eq!(
        inside,
        ["config.toml", "fold.db", "main-materialised", "objects"]
    );
    assert_eq!(project.revision.to_string(), "r1");
    assert_eq!(
        fold_vcs::main_view::read_marker(&paths.fold_dir()).unwrap(),
        Some(project.revision)
    );
    assert_eq!(project.imported.files, 0);

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

#[tokio::test]
async fn init_from_records_the_source_as_r1_and_fills_main() {
    let host = Host::new();
    let source = host.root("source");
    std::fs::create_dir_all(source.join("src")).unwrap();
    std::fs::write(source.join("src/lib.rs"), "pub fn x() {}\n").unwrap();
    let root = host.root("app");
    let project = init_project_from(&host.paths, &root, None, Some(&source), &host.clock)
        .await
        .unwrap();
    assert_eq!(project.revision.to_string(), "r1");
    assert_eq!(project.imported.files, 1);
    assert_eq!(project.imported.bytes, 14);
    let main = root.join("main/src/lib.rs");
    assert_eq!(std::fs::read(&main).unwrap(), b"pub fn x() {}\n");
    assert!(std::fs::metadata(&main).unwrap().permissions().readonly());
}

#[tokio::test]
async fn a_main_dir_that_init_did_not_make_survives_a_failed_init() {
    let host = Host::new();
    let root = host.root("app");
    std::fs::create_dir_all(root.join("main")).unwrap();
    std::fs::write(root.join("main/notes.txt"), "mine").unwrap();
    let error = init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("target_not_empty"));
    assert!(!root.join(".fold").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("main/notes.txt")).unwrap(),
        "mine"
    );
    let registry = host.registry().await;
    assert!(
        projects::list_projects(registry.read())
            .await
            .unwrap()
            .is_empty()
    );
    registry.close().await.unwrap();
}

#[tokio::test]
async fn a_source_that_is_not_a_directory_is_an_invalid_request() {
    let host = Host::new();
    let file = host.root("file.txt");
    std::fs::write(&file, "x").unwrap();
    let root = host.root("app");
    for source in [file, host.root("absent")] {
        let error = init_project_from(&host.paths, &root, None, Some(&source), &host.clock)
            .await
            .unwrap_err();
        assert_eq!(code(&error), Some("invalid_request"));
        assert!(!root.exists());
    }
}

#[cfg(test)]
async fn refuse_registrations(host: &Host, refuse: bool) {
    let sql = if refuse {
        "CREATE TRIGGER refuse BEFORE INSERT ON projects BEGIN SELECT RAISE(ABORT, 'refused'); END"
    } else {
        "DROP TRIGGER refuse"
    };
    let registry = host.registry().await;
    registry
        .write(async |tx| {
            sqlx::query(sql).execute(tx.conn()).await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    registry.close().await.unwrap();
}

#[tokio::test]
async fn a_failure_after_main_is_filled_puts_main_back_and_a_rerun_succeeds() {
    let host = Host::new();
    let source = host.root("source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("a.txt"), "a").unwrap();
    let made = host.root("made");
    let kept = host.root("kept");
    std::fs::create_dir_all(kept.join("main")).unwrap();
    refuse_registrations(&host, true).await;
    for root in [&made, &kept] {
        let error = init_project_from(&host.paths, root, None, Some(&source), &host.clock)
            .await
            .unwrap_err();
        assert!(matches!(error, ProjectError::Store(_)), "{error:?}");
        assert!(!root.join(".fold").exists());
    }
    assert!(!made.join("main").exists());
    assert_eq!(std::fs::read_dir(kept.join("main")).unwrap().count(), 0);

    refuse_registrations(&host, false).await;
    for root in [&made, &kept] {
        init_project_from(&host.paths, root, None, Some(&source), &host.clock)
            .await
            .unwrap();
        assert_eq!(std::fs::read(root.join("main/a.txt")).unwrap(), b"a");
    }
}

#[tokio::test]
async fn a_project_directory_at_or_below_the_source_is_refused_before_anything_is_created() {
    let host = Host::new();
    let source = host.root("source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("a.txt"), "a").unwrap();
    let inside = source.join("absent/../app/deeper");
    for root in [source.clone(), inside, source.join("./app")] {
        let error = init_project_from(&host.paths, &root, None, Some(&source), &host.clock)
            .await
            .unwrap_err();
        assert_eq!(code(&error), Some("invalid_request"), "{root:?}");
    }
    let entries: Vec<_> = std::fs::read_dir(&source)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["a.txt"]);

    let beside = host.root("absent/../beside");
    init_project_from(&host.paths, &beside, None, Some(&source), &host.clock)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_symlink_or_a_file_in_place_of_main_is_refused() {
    let host = Host::new();
    let elsewhere = host.root("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let linked = host.root("linked");
    std::fs::create_dir_all(&linked).unwrap();
    std::os::unix::fs::symlink(&elsewhere, linked.join("main")).unwrap();
    let file = host.root("file");
    std::fs::create_dir_all(&file).unwrap();
    std::fs::write(file.join("main"), "x").unwrap();
    for root in [&linked, &file] {
        let error = init_project(&host.paths, root, None, &host.clock)
            .await
            .unwrap_err();
        assert_eq!(code(&error), Some("target_not_empty"));
        assert!(!root.join(".fold").exists());
    }
    assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
}
