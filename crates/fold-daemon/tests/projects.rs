use std::path::Path;
use std::sync::Arc;

use fold_core::{Clock, HostPaths, ManualClock, ProjectConfig, ProjectPaths};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::jobs::QuietCheckpoints;
use fold_daemon::projects::{Projects, Slot};
use fold_daemon::services::projects::{
    self as project_service, DEFAULT_SESSION_LIFETIME, Lease, ProjectError, deleted_project_slug,
    init_project, init_project_from_git, load_config,
};
use fold_store::projects;
use fold_store::{Store, StoreError};
use fold_vcs::git::Git;
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
    assert_eq!(entries, [".fold", ".foldignore", ".git", ".gitignore"]);
    assert!(
        std::fs::symlink_metadata(project.root.join(".gitignore"))
            .unwrap()
            .is_file()
    );
    assert_eq!(
        std::fs::metadata(project.root.join(".foldignore"))
            .unwrap()
            .len(),
        0
    );
    let mut inside: Vec<_> = std::fs::read_dir(paths.fold_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    inside.retain(|name| !name.starts_with("fold.db-"));
    for name in ["config.toml", "fold.db", "objects"] {
        assert!(inside.iter().any(|found| found == name), "{inside:?}");
    }
    assert!(
        !inside
            .iter()
            .any(|found| found == "main-materialised" || found == "git")
    );
    assert_eq!(project.revision.to_string(), "r1");
    // The folder's own repository holds r1's commit with the ignore files init wrote, and keeps `.fold/` out.
    assert_eq!(git(&project.root, &["log", "--format=%s"]), "fold r1\n");
    assert!(
        std::fs::read_to_string(paths.git_dir().join("info/exclude"))
            .unwrap()
            .lines()
            .any(|line| line == "/.fold/")
    );
    assert_eq!(git(&project.root, &["status", "--porcelain"]), "");
    assert_eq!(project.imported.files, 2);

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

    // An unregistered `.fold/` without a database is an init that stopped early: refused, kept, and no database is created in it.
    let stale = host.root("stale");
    std::fs::create_dir_all(stale.join(".fold")).unwrap();
    let error = init_project(&host.paths, &stale, None, &host.clock)
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("project_incomplete"));
    let ProjectError::Fold(error) = error else {
        panic!("a fold error");
    };
    let fold_dir = stale.canonicalize().unwrap().join(".fold");
    assert!(
        error
            .hint()
            .unwrap()
            .contains(&format!("delete {}", fold_dir.display()))
    );
    assert_eq!(std::fs::read_dir(&fold_dir).unwrap().count(), 0);

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
async fn the_footer_lease_parses_the_file_again_only_when_a_stat_shows_a_change() {
    let host = Host::new();
    let project = init_project(&host.paths, &host.root("app"), None, &host.clock)
        .await
        .unwrap();
    let paths = ProjectPaths::new(&project.root);
    let config = paths.config();
    let lease = Lease::config(paths.clone());
    std::fs::write(&config, "lease_minutes = 7\n").unwrap();
    assert_eq!(lease.recent_minutes().await.unwrap(), 7);
    let modified = std::fs::metadata(&config).unwrap().modified().unwrap();
    let rewrite = |text: &str, at: std::time::SystemTime| {
        std::fs::write(&config, text).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&config)
            .unwrap()
            .set_modified(at)
            .unwrap();
    };

    rewrite("lease_minutes = 0\n", modified);
    assert_eq!(lease.clone().recent_minutes().await.unwrap(), 7);
    let error = lease.minutes().await.unwrap_err();
    assert_eq!(code(&error), Some("invalid_config"));

    let later = modified + std::time::Duration::from_secs(1);
    rewrite("lease_minutes = 0\n", later);
    let error = lease.recent_minutes().await.unwrap_err();
    assert_eq!(code(&error), Some("invalid_config"));
    let ProjectError::Fold(refusal) = error else {
        panic!("an invalid file is a refusal");
    };
    assert_eq!(refusal.hint(), Some("ask a human to fix .fold/config.toml"));
    rewrite("lease_minutes = 9\n", later);
    assert_eq!(lease.recent_minutes().await.unwrap(), 9);
    std::fs::remove_file(&config).unwrap();
    assert_eq!(
        lease.recent_minutes().await.unwrap(),
        ProjectConfig::default().lease_minutes
    );
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

#[cfg(test)]
fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test]
async fn init_records_the_folder_files_as_r1_and_keeps_its_gitignore() {
    let host = Host::new();
    let root = host.root("app");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn x() {}\n").unwrap();
    std::fs::write(root.join("target/out"), "built").unwrap();
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    let project = init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    assert_eq!(project.revision.to_string(), "r1");
    assert_eq!(project.imported.files, 2);
    assert_eq!(project.git_commit, None);
    assert_eq!(
        std::fs::read_to_string(root.join(".gitignore")).unwrap(),
        "target/\n"
    );
    assert!(!root.join(".foldignore").exists());
    assert_eq!(
        git(&root, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".gitignore\nsrc/lib.rs\n"
    );
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn init_keeps_the_foldignore_of_a_folder_without_a_gitignore() {
    let host = Host::new();
    let root = host.root("app");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".foldignore"), "out/\n").unwrap();
    init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join(".foldignore")).unwrap(),
        "out/\n"
    );
    assert!(
        std::fs::symlink_metadata(root.join(".gitignore"))
            .unwrap()
            .is_file()
    );
    assert_eq!(
        git(&root, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".foldignore\n.gitignore\n"
    );
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn init_refuses_a_git_repository_and_from_git_refuses_anything_else() {
    let host = Host::new();
    let repo = host.root("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    let error = init_project(&host.paths, &repo, None, &host.clock)
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("invalid_request"));
    let ProjectError::Fold(error) = error else {
        panic!("a fold error");
    };
    assert!(error.hint().unwrap().contains("fld init --from-git"));
    assert!(!repo.join(".fold").exists());

    // A `.git` that is no repository is refused too, and kept.
    let broken = host.root("broken");
    std::fs::create_dir_all(broken.join(".git")).unwrap();
    let error = init_project(&host.paths, &broken, None, &host.clock)
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("invalid_request"));
    assert!(broken.join(".git").is_dir() && !broken.join(".fold").exists());

    let plain = host.root("plain");
    std::fs::create_dir_all(&plain).unwrap();
    for folder in [plain.clone(), host.root("absent")] {
        let error = init_project_from_git(&host.paths, &folder, None, Git::default(), &host.clock)
            .await
            .unwrap_err();
        let ProjectError::Fold(error) = error else {
            panic!("a fold error");
        };
        assert_eq!(error.code(), "invalid_request");
        assert!(error.hint().unwrap().contains("fld init"));
        assert!(!folder.join(".fold").exists());
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
async fn a_failure_keeps_the_folder_files_and_a_rerun_succeeds() {
    let host = Host::new();
    let root = host.root("app");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "a").unwrap();
    refuse_registrations(&host, true).await;
    let error = init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap_err();
    assert!(matches!(error, ProjectError::Store(_)), "{error:?}");
    let entries: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["a.txt"]);

    refuse_registrations(&host, false).await;
    init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"a");
    assert_eq!(
        git(&root, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".foldignore\n.gitignore\na.txt\n"
    );
}

/// Deletes what `fld init` made in the folder that makes it a project: `.fold/` and the repository, so a plain init can run there again.
#[cfg(test)]
fn delete_fold_data(root: &Path) {
    std::fs::remove_dir_all(root.join(".fold")).unwrap();
    std::fs::remove_dir_all(root.join(".git")).unwrap();
}

#[tokio::test]
async fn init_replaces_the_registration_of_a_folder_whose_fold_data_was_deleted() {
    let host = Host::new();
    let root = host.root("app");
    let first = init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    assert!(!first.replaced);
    assert_eq!(
        deleted_project_slug(&host.paths, &root).await.unwrap(),
        None
    );
    delete_fold_data(&root);
    assert_eq!(
        deleted_project_slug(&host.paths, &root).await.unwrap(),
        Some("app".to_owned())
    );
    assert_eq!(
        deleted_project_slug(&host.paths, &host.root("missing"))
            .await
            .unwrap(),
        None
    );

    let second = init_project(&host.paths, &root, Some("Shop"), &host.clock)
        .await
        .unwrap();
    assert!(second.replaced && !second.resumed);
    assert_eq!(second.slug, "app", "the slug stays when the name changes");
    assert_eq!(second.name, "Shop");
    assert_eq!(second.root, first.root);
    assert_eq!(second.revision.to_string(), "r1");
    assert!(ProjectPaths::new(&root).db().is_file());

    let registry = host.registry().await;
    let rows = projects::list_projects(registry.read()).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].slug.as_str(), rows[0].name.as_str()),
        ("app", "Shop")
    );
    registry.close().await.unwrap();

    // The new project holds `.fold/`, so the folder is a project again.
    let again = init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap_err();
    assert_eq!(code(&again), Some("project_exists"));
}

/// What a daemon that still held the project can write into `.fold/` after it was deleted.
#[cfg(test)]
fn write_leftovers(root: &Path) {
    let fold = root.join(".fold");
    for dir in ["objects/tmp", "index", "probe-1-2", "ws/T-1"] {
        std::fs::create_dir_all(fold.join(dir)).unwrap();
    }
    std::fs::write(fold.join("objects/tmp/staged"), "x").unwrap();
    std::fs::write(fold.join("index/ws-1"), "x").unwrap();
    std::fs::write(fold.join("probe-1-2/file"), "x").unwrap();
    std::fs::write(fold.join("fold.db-wal"), "").unwrap();
}

#[tokio::test]
async fn init_replaces_a_registered_folder_whose_fold_holds_only_leftovers_of_the_daemon() {
    let host = Host::new();
    let root = host.root("app");
    init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    delete_fold_data(&root);
    write_leftovers(&root);
    assert_eq!(
        deleted_project_slug(&host.paths, &root).await.unwrap(),
        Some("app".to_owned())
    );
    let replaced = init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    assert!(replaced.replaced);
    assert_eq!(replaced.slug, "app");
    assert_eq!(replaced.revision.to_string(), "r1");
    assert!(!root.join(".fold/index/ws-1").exists());
    assert!(!root.join(".fold/probe-1-2").exists());
}

#[tokio::test]
async fn init_refuses_a_registered_folder_whose_fold_holds_content_without_a_database() {
    let host = Host::new();
    let root = host.root("app");
    init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    for content in [
        "objects/ab/cdef",
        "ws/T-1/app.txt",
        "fold.db-wal",
        "config.toml",
    ] {
        delete_fold_data(&root);
        write_leftovers(&root);
        let path = root.join(".fold").join(content);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "data").unwrap();
        let refused = init_project(&host.paths, &root, None, &host.clock)
            .await
            .unwrap_err();
        assert_eq!(code(&refused), Some("project_incomplete"), "{content}");
        let ProjectError::Fold(refused) = refused else {
            panic!("a fold error");
        };
        let hint = refused.hint().unwrap();
        assert!(
            hint.contains("from a backup") && hint.contains("fld init again"),
            "{hint}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "data",
            "nothing is removed"
        );

        std::fs::remove_dir_all(root.join(".fold")).unwrap();
        let replaced = init_project(&host.paths, &root, None, &host.clock)
            .await
            .unwrap();
        assert!(replaced.replaced, "{content}");
        assert_eq!(replaced.slug, "app");
    }
}

#[cfg(test)]
async fn daemon_state(host: &Host) -> AppState {
    let registry = host.registry().await;
    Arc::new(DaemonState {
        projects: Projects::default().with_registry(registry.clone()),
        registry,
        clock: Arc::new(host.clock.clone()),
        assets: AssetSource::Embedded,
        listen: "127.0.0.1".parse().unwrap(),
        session_lifetime: DEFAULT_SESSION_LIFETIME,
    })
}

#[cfg(test)]
fn closed(handle: &fold_daemon::projects::ProjectHandle) -> bool {
    *handle.bus.closing().borrow() && handle.store.read().is_closed()
}

#[tokio::test]
async fn the_daemon_closes_a_project_whose_fold_data_was_deleted_and_opens_the_new_one() {
    let host = Host::new();
    let root = host.root("app");
    init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    let state = daemon_state(&host).await;
    let old = project_service::open(&state, "app").await.unwrap();
    assert!(!old.database_replaced().await);

    // `fld init` asks the daemon to open the old slug before it creates the new project, which closes the old one.
    delete_fold_data(&root);
    assert!(old.database_replaced().await);
    // Until then, the daemon's own writers leave the deleted `.fold/` deleted.
    let gc = project_service::gc(&old, false).await.unwrap_err();
    assert_eq!(code(&gc), Some("project_unavailable"));
    assert_eq!(QuietCheckpoints::default().run(&old).await, 0);
    assert!(!root.join(".fold").exists());
    let error = project_service::open(&state, "app").await.unwrap_err();
    assert_eq!(code(&error), Some("project_unavailable"));
    assert!(closed(&old));
    assert!(matches!(
        state.projects.slot("app"),
        Some(Slot::Unavailable)
    ));

    init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    let new = project_service::open(&state, "app").await.unwrap();
    assert!(!Arc::ptr_eq(&old, &new));
    assert!(!new.database_replaced().await);
    let listed = project_service::list(&state).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].available && listed[0].overview.is_some());

    // Without that request, the open after init finds a database file that is not the one the handle holds.
    delete_fold_data(&root);
    init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    let newest = project_service::open(&state, "app").await.unwrap();
    assert!(!Arc::ptr_eq(&new, &newest));
    assert!(closed(&new) && !closed(&newest));
    let again = project_service::open(&state, "app").await.unwrap();
    assert!(Arc::ptr_eq(&newest, &again));
    newest.bus.close();
    newest.store.close().await.unwrap();
    state.registry.close().await.unwrap();
}
