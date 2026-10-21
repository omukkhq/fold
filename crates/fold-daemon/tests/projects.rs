use std::path::Path;
use std::sync::Arc;

use fold_core::{Clock, HostPaths, ManualClock, ProjectConfig, ProjectPaths};
use fold_daemon::daemon::{AppState, DaemonState};
use fold_daemon::http::assets::AssetSource;
use fold_daemon::jobs::QuietCheckpoints;
use fold_daemon::projects::Projects;
use fold_daemon::services::projects::{
    self as project_service, DEFAULT_SESSION_LIFETIME, Lease, ProjectError, RepositorySetup,
    deleted_project_slug, init_project, init_project_from_git, load_config,
};
use fold_store::projects;
use fold_store::testing::remove_dir_eventually;
use fold_store::{Store, StoreError};
use fold_vcs::git::Git;
use fold_vcs::git_discovery::Discovery;
use tempfile::TempDir;

use crate::common::acceptance::WAIT;

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
    let project = init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
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
    // A plain init writes no Git data and no `.gitignore`, only an empty `.foldignore`.
    assert_eq!(entries, [".fold", ".foldignore"]);
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
            .any(|found| found == "main-materialised" || found == "git" || found == "git-sync.map")
    );
    assert_eq!(project.revision.to_string(), "r1");
    assert!(!project.git_sync && project.gitignore.is_none());
    assert_eq!(project.imported.files, 1);
    // The genesis `r0` holds the empty tree, and the folder's files are `r1`, the human's change on top of it.
    let store = Store::open_project(&paths).await.unwrap();
    let genesis = fold_store::workspaces::genesis(store.read())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(genesis.number, fold_core::RevisionId::GENESIS);
    assert_eq!(genesis.tree_hash, fold_core::tree::empty_tree_hash());
    let r1 = fold_store::workspaces::head(store.read())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r1.parent, Some(fold_core::RevisionId::GENESIS));
    let integration = r1.integration.unwrap();
    assert_eq!(
        (integration.kind, integration.actor.as_str()),
        (fold_store::workspaces::IntegrationKind::Record, "human:ana")
    );
    let mut conn = store.read().acquire().await.unwrap();
    let change = fold_store::workspaces::newest_human_change(&mut conn, true)
        .await
        .unwrap()
        .unwrap();
    drop(conn);
    assert_eq!(change.commit, None);
    assert_eq!(change.human, "ana");
    assert_eq!(
        (change.base_tree, change.tree),
        (genesis.tree_hash, r1.tree_hash)
    );
    let events: Vec<String> = sqlx::query_scalar("SELECT type FROM events ORDER BY seq")
        .fetch_all(store.read())
        .await
        .unwrap();
    assert_eq!(
        events,
        [
            "integration.succeeded",
            "main.advanced",
            "human_change.recorded"
        ]
    );
    assert!(
        fold_store::workspaces::git_sync(store.read())
            .await
            .unwrap()
            .is_none()
    );
    store.close().await.unwrap();

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
        &crate::common::ana(),
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
    init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
        .await
        .unwrap();
    let again = init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
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
    let error = init_project(
        &host.paths,
        &stale,
        None,
        &crate::common::ana(),
        &host.clock,
    )
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
    let error = init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
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
        let project = init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
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
    let ana = fold_core::HumanName::parse("ana").unwrap();
    let (a, b) = tokio::join!(
        init_project(&host.paths, &first, None, &ana, &host.clock),
        init_project(&host.paths, &second, None, &ana, &host.clock),
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
    let project = init_project(
        &host.paths,
        &host.root("app"),
        None,
        &crate::common::ana(),
        &host.clock,
    )
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
    let project = init_project(
        &host.paths,
        &host.root("app"),
        None,
        &crate::common::ana(),
        &host.clock,
    )
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
    let error = init_project(&host.paths, &file, None, &crate::common::ana(), &host.clock)
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
async fn a_plain_init_keeps_the_gitignore_and_writes_no_git_data() {
    let host = Host::new();
    let root = host.root("app");
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("target/out"), "built").unwrap();
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    let project = init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
        .await
        .unwrap();
    assert_eq!(project.revision.to_string(), "r1");
    // `.gitignore` and the empty `.foldignore`; the ignore rules leave `target/` out.
    assert_eq!(project.imported.files, 2);
    assert_eq!(read(&root, ".gitignore"), "target/\n");
    assert_eq!(read(&root, ".foldignore"), "");
    assert!(!root.join(".git").exists());
}

#[tokio::test]
async fn init_with_git_records_the_folder_as_r1_and_the_gitignore_as_r2() {
    let host = Host::new();
    let root = host.root("app");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn x() {}\n").unwrap();
    std::fs::write(root.join("target/out"), "built").unwrap();
    std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
    let project = with_git(&host, &root).await.unwrap();
    assert_eq!(project.revision.to_string(), "r1");
    assert_eq!(
        project.gitignore.map(|r| r.to_string()).as_deref(),
        Some("r2")
    );
    assert!(project.git_sync);
    assert_eq!(project.imported.files, 3);
    assert_eq!(project.git_commit, None);
    assert_eq!(read(&root, ".gitignore"), "target/\n.fold/\n");
    assert_eq!(read(&root, ".foldignore"), "");
    assert_eq!(
        git(&root, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".foldignore\n.gitignore\nsrc/lib.rs\n"
    );
    // The empty genesis gets no commit: the history starts at r1.
    assert_eq!(
        git(&root, &["log", "--format=%s"]),
        "Record the changes of the project folder\nRecord the changes of the project folder\n"
    );
    assert_eq!(
        git(&root, &["rev-list", "--max-parents=0", "HEAD"])
            .lines()
            .count(),
        1
    );
    assert!(
        std::fs::read_to_string(root.join(".git/info/exclude"))
            .unwrap()
            .lines()
            .any(|line| line == "/.fold/")
    );
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
    let store = Store::open_project(&ProjectPaths::new(&project.root))
        .await
        .unwrap();
    let events: Vec<String> = sqlx::query_scalar("SELECT type FROM events ORDER BY seq")
        .fetch_all(store.read())
        .await
        .unwrap();
    store.close().await.unwrap();
    assert_eq!(
        events,
        [
            "integration.succeeded",
            "main.advanced",
            "human_change.recorded",
            "integration.succeeded",
            "main.advanced",
            "human_change.recorded",
            "git.sync_enabled",
            "git.committed",
            "git.committed"
        ]
    );
}

#[cfg(test)]
async fn with_git(
    host: &Host,
    root: &Path,
) -> Result<project_service::InitializedProject, ProjectError> {
    project_service::init_new_project(
        &host.paths,
        root,
        None,
        &crate::common::ana(),
        Git::default(),
        None,
        Some(Box::new(|_: &Path| {})),
        &host.clock,
    )
    .await
}

#[tokio::test]
async fn init_copies_the_foldignore_into_a_new_gitignore() {
    let host = Host::new();
    let root = host.root("app");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".foldignore"), "out/\n").unwrap();
    with_git(&host, &root).await.unwrap();
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
        std::fs::read_to_string(root.join(".gitignore")).unwrap(),
        "out/\n.fold/\n"
    );
    assert_eq!(
        git(&root, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".foldignore\n.gitignore\n"
    );
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
}

#[cfg(test)]
/// Runs an init with Git in a folder holding `files` and returns the folder.
async fn init_with_files(files: &[(&str, &str)]) -> (Host, std::path::PathBuf) {
    let host = Host::new();
    let root = host.root("app");
    std::fs::create_dir_all(&root).unwrap();
    for (name, text) in files {
        std::fs::write(root.join(name), text).unwrap();
    }
    with_git(&host, &root).await.unwrap();
    (host, root)
}

#[cfg(test)]
fn read(root: &std::path::Path, name: &str) -> String {
    std::fs::read_to_string(root.join(name)).unwrap()
}

#[tokio::test]
async fn init_adds_a_missing_newline_before_the_fold_line() {
    let (_host, root) = init_with_files(&[(".foldignore", "out/")]).await;
    assert_eq!(read(&root, ".gitignore"), "out/\n.fold/\n");
    assert_eq!(read(&root, ".foldignore"), "out/");
    let (_host, root) = init_with_files(&[(".gitignore", "target")]).await;
    assert_eq!(read(&root, ".gitignore"), "target\n.fold/\n");
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn init_with_an_empty_foldignore_writes_only_the_fold_line() {
    let (_host, root) = init_with_files(&[(".foldignore", "")]).await;
    assert_eq!(read(&root, ".gitignore"), ".fold/\n");
}

#[tokio::test]
async fn init_keeps_both_ignore_files_and_adds_fold_to_the_gitignore() {
    let (_host, root) =
        init_with_files(&[(".gitignore", "target/\n"), (".foldignore", "out/\n")]).await;
    assert_eq!(read(&root, ".gitignore"), "target/\n.fold/\n");
    assert_eq!(read(&root, ".foldignore"), "out/\n");
    assert_eq!(
        git(&root, &["ls-tree", "-r", "--name-only", "HEAD"]),
        ".foldignore\n.gitignore\n"
    );
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn init_leaves_a_gitignore_that_already_ignores_fold() {
    for line in [".fold", ".fold/", "/.fold", "/.fold/", ".fold/  "] {
        let text = format!("target/\n{line}\n");
        let (_host, root) = init_with_files(&[(".gitignore", &text)]).await;
        assert_eq!(read(&root, ".gitignore"), text, "{line}");
    }
    // Only the four exact forms count: Git keeps leading spaces, and a negation added before the line loses to it.
    for line in [
        "# .fold/", "!.fold/", ".folder/", "  .fold/", ".fold/*", "**/.fold",
    ] {
        let text = format!("{line}\n");
        let (_host, root) = init_with_files(&[(".gitignore", &text)]).await;
        assert_eq!(
            read(&root, ".gitignore"),
            format!("{text}.fold/\n"),
            "{line}"
        );
        assert_eq!(git(&root, &["status", "--porcelain"]), "", "{line}");
    }
}

#[tokio::test]
async fn init_ends_the_fold_line_like_the_crlf_lines_of_the_gitignore() {
    let (_host, root) = init_with_files(&[(".gitignore", "target/\r\n")]).await;
    assert_eq!(read(&root, ".gitignore"), "target/\r\n.fold/\r\n");
    let (_host, root) = init_with_files(&[(".gitignore", "a\r\nb")]).await;
    assert_eq!(read(&root, ".gitignore"), "a\r\nb\r\n.fold/\r\n");
    let (_host, root) = init_with_files(&[(".gitignore", "target/\r\n.fold/\r\n")]).await;
    assert_eq!(read(&root, ".gitignore"), "target/\r\n.fold/\r\n");
    let (_host, root) = init_with_files(&[(".foldignore", "out/\r\n")]).await;
    assert_eq!(read(&root, ".gitignore"), "out/\r\n.fold/\r\n");
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn init_leaves_an_ignore_file_that_is_not_a_regular_file() {
    let host = Host::new();
    let root = host.root("app");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(host.root("rules"), "target/\n").unwrap();
    std::os::unix::fs::symlink("../rules", root.join(".gitignore")).unwrap();
    with_git(&host, &root).await.unwrap();
    assert!(
        std::fs::symlink_metadata(root.join(".gitignore"))
            .unwrap()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(host.root("rules")).unwrap(),
        "target/\n"
    );
    assert_eq!(read(&root, ".foldignore"), "");
    assert_eq!(git(&root, &["status", "--porcelain"]), "");

    // A `.foldignore` that is not a regular file counts as empty.
    let host = Host::new();
    let root = host.root("app");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(host.root("rules"), "out/\n").unwrap();
    std::os::unix::fs::symlink("../rules", root.join(".foldignore")).unwrap();
    with_git(&host, &root).await.unwrap();
    assert_eq!(read(&root, ".gitignore"), ".fold/\n");
    assert!(
        std::fs::symlink_metadata(root.join(".foldignore"))
            .unwrap()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(host.root("rules")).unwrap(),
        "out/\n"
    );
    assert_eq!(git(&root, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn a_failed_init_writes_back_the_gitignore_it_changed() {
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
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".gitignore"), "target").unwrap();
    let error = with_git(&host, &root).await.unwrap_err();
    assert!(matches!(error, ProjectError::Store(_)));
    assert_eq!(read(&root, ".gitignore"), "target");
    assert!(!root.join(".foldignore").exists());
    assert!(!root.join(".fold").exists() && !root.join(".git").exists());

    std::fs::remove_file(root.join(".gitignore")).unwrap();
    std::fs::write(root.join(".foldignore"), "out/\n").unwrap();
    with_git(&host, &root).await.unwrap_err();
    assert_eq!(read(&root, ".foldignore"), "out/\n");
    assert!(!root.join(".gitignore").exists());
}

#[tokio::test]
async fn init_refuses_a_git_repository_and_from_git_refuses_anything_else() {
    let host = Host::new();
    let repo = host.root("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    let error = init_project(&host.paths, &repo, None, &crate::common::ana(), &host.clock)
        .await
        .unwrap_err();
    assert_eq!(code(&error), Some("invalid_request"));
    let ProjectError::Fold(error) = error else {
        panic!("a fold error");
    };
    assert!(error.hint().unwrap().contains("`fld init --with-git`"));
    assert!(!repo.join(".fold").exists());

    // A `.git` that is no repository is refused too, and kept.
    let broken = host.root("broken");
    std::fs::create_dir_all(broken.join(".git")).unwrap();
    let error = init_project(
        &host.paths,
        &broken,
        None,
        &crate::common::ana(),
        &host.clock,
    )
    .await
    .unwrap_err();
    assert_eq!(code(&error), Some("invalid_request"));
    assert!(broken.join(".git").is_dir() && !broken.join(".fold").exists());

    let plain = host.root("plain");
    std::fs::create_dir_all(&plain).unwrap();
    for folder in [plain.clone(), host.root("absent")] {
        let error = init_project_from_git(
            &host.paths,
            &folder,
            None,
            &crate::common::ana(),
            Git::default(),
            &host.clock,
        )
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

#[tokio::test]
async fn a_plain_init_without_git_refuses_a_folder_inside_a_repository() {
    let host = Host::new();
    // The folder above the test's own is a ceiling, so a repository around it never changes an answer.
    let above = host.dir.path().canonicalize().unwrap();
    let missing = Git::new(host.root("bin/git")).with_discovery(Discovery {
        ceiling_directories: above.parent().map(|parent| parent.as_os_str().to_owned()),
        across_filesystem: None,
    });
    let init = async |folder: &Path| {
        fold_daemon::services::projects::init_new_project(
            &host.paths,
            folder,
            None,
            &crate::common::ana(),
            missing.clone(),
            None,
            None,
            &host.clock,
        )
        .await
    };
    let repo = host.root("repo");
    std::fs::create_dir_all(repo.join("sub/deeper")).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    // A working tree whose `.git` is a file naming its Git directory elsewhere.
    let linked = host.root("linked");
    git(
        host.dir.path(),
        &["init", "--quiet", "--separate-git-dir=store", "linked"],
    );
    std::fs::create_dir_all(linked.join("sub")).unwrap();
    for (folder, top) in [
        (repo.join("sub"), &repo),
        (repo.join("sub/deeper"), &repo),
        (linked.join("sub"), &linked),
    ] {
        let ProjectError::Fold(error) = init(&folder).await.unwrap_err() else {
            panic!("a fold error");
        };
        assert_eq!(error.details().unwrap()["reason"], "inside_git_repository");
        let top = top.canonicalize().unwrap();
        assert!(
            error
                .hint()
                .unwrap()
                .contains(&format!("`fld init --with-git` in {}", top.display())),
            "{error:?}"
        );
        assert!(!folder.join(".fold").exists());
    }
    for top in [&repo, &linked] {
        let ProjectError::Fold(error) = init(top).await.unwrap_err() else {
            panic!("a fold error");
        };
        assert_eq!(error.details().unwrap()["reason"], "git_repository");
    }
    let plain = host.root("plain");
    init(&plain).await.unwrap();
    assert!(plain.join(".fold").is_dir() && !plain.join(".git").exists());
}

#[tokio::test]
async fn both_inits_refuse_a_folder_in_a_git_directory_with_or_without_git() {
    let host = Host::new();
    // The folder above the test's own is a ceiling, so a repository around it never changes an answer.
    let above = host.dir.path().canonicalize().unwrap();
    let discovery = Discovery {
        ceiling_directories: above.parent().map(|parent| parent.as_os_str().to_owned()),
        across_filesystem: None,
    };
    let present = Git::default().with_discovery(discovery.clone());
    let missing = Git::new(host.root("bin/git")).with_discovery(discovery);
    let repo = host.root("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "--initial-branch=main"]);
    git(host.dir.path(), &["init", "--quiet", "--bare", "bare.git"]);
    let (dot_git, bare) = (repo.join(".git"), host.root("bare.git"));
    for (folder, git_dir) in [
        (dot_git.join("notes"), &dot_git),
        (dot_git.join("refs/heads"), &dot_git),
        (dot_git.clone(), &dot_git),
        (bare.join("notes"), &bare),
        (bare.clone(), &bare),
    ] {
        let git_dir = git_dir.canonicalize().unwrap();
        for (program, with_git) in [(&present, false), (&present, true), (&missing, false)] {
            let setup = with_git.then(|| -> RepositorySetup { Box::new(|_: &Path| {}) });
            let ProjectError::Fold(error) = fold_daemon::services::projects::init_new_project(
                &host.paths,
                &folder,
                None,
                &crate::common::ana(),
                program.clone(),
                None,
                setup,
                &host.clock,
            )
            .await
            .unwrap_err() else {
                panic!("a fold error");
            };
            assert_eq!(error.code(), "invalid_request", "{error:?}");
            assert_eq!(error.details().unwrap()["reason"], "inside_git_directory");
            assert!(
                error
                    .hint()
                    .unwrap()
                    .contains(&format!("choose a folder outside {}", git_dir.display())),
                "{error:?}"
            );
            assert!(!folder.join(".fold").exists(), "{}", folder.display());
        }
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
    let error = init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
        .await
        .unwrap_err();
    assert!(matches!(error, ProjectError::Store(_)), "{error:?}");
    let entries: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["a.txt"]);

    refuse_registrations(&host, false).await;
    init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
        .await
        .unwrap();
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"a");
    let mut entries: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert_eq!(entries, [".fold", ".foldignore", "a.txt"]);
}

/// Deletes what `fld init` made in the folder that makes it a project: `.fold/`, so a plain init can run there again. A project the daemon still has open can write into `.fold/` during the removal.
#[cfg(test)]
async fn delete_fold_data(root: &Path) {
    remove_dir_eventually(&root.join(".fold"), WAIT).await;
}

#[tokio::test]
async fn init_replaces_the_registration_of_a_folder_whose_fold_data_was_deleted() {
    let host = Host::new();
    let root = host.root("app");
    let first = init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
        .await
        .unwrap();
    assert!(!first.replaced);
    assert_eq!(
        deleted_project_slug(&host.paths, &root).await.unwrap(),
        None
    );
    delete_fold_data(&root).await;
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

    let second = init_project(
        &host.paths,
        &root,
        Some("Shop"),
        &crate::common::ana(),
        &host.clock,
    )
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
    let again = init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
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
    init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
        .await
        .unwrap();
    delete_fold_data(&root).await;
    write_leftovers(&root);
    assert_eq!(
        deleted_project_slug(&host.paths, &root).await.unwrap(),
        Some("app".to_owned())
    );
    let replaced = init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
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
    init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
        .await
        .unwrap();
    for content in [
        "objects/ab/cdef",
        "ws/T-1/app.txt",
        "fold.db-wal",
        "config.toml",
    ] {
        delete_fold_data(&root).await;
        write_leftovers(&root);
        let path = root.join(".fold").join(content);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "data").unwrap();
        let refused = init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
            .await
            .unwrap_err();
        assert_eq!(code(&refused), Some("project_incomplete"), "{content}");
        let ProjectError::Fold(refused) = refused else {
            panic!("a fold error");
        };
        let hint = refused.hint().unwrap();
        assert!(
            hint.contains("from a backup") && hint.contains("`fld init` again"),
            "{hint}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "data",
            "nothing is removed"
        );

        std::fs::remove_dir_all(root.join(".fold")).unwrap();
        let replaced = init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
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
        listen: "127.0.0.1:0".parse().unwrap(),
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
    init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
        .await
        .unwrap();
    let state = daemon_state(&host).await;
    let old = project_service::open(&state, "app").await.unwrap();
    assert!(!old.database_replaced().await);

    // `fld init` asks the daemon to open the old slug before it creates the new project, which closes the old one.
    delete_fold_data(&root).await;
    assert!(old.database_replaced().await);
    // Until then, the daemon's own writers leave the deleted `.fold/` deleted.
    let gc = project_service::gc(&old, false).await.unwrap_err();
    assert_eq!(code(&gc), Some("project_unavailable"));
    assert_eq!(QuietCheckpoints::default().run(&old).await, 0);
    assert!(!root.join(".fold").exists());
    let error = project_service::open(&state, "app").await.unwrap_err();
    assert_eq!(code(&error), Some("project_not_found"));
    assert!(closed(&old));
    assert!(state.projects.slot("app").is_none());
    assert!(
        projects::find_project(state.registry.read(), "app")
            .await
            .unwrap()
            .is_none(),
        "the open removed the project"
    );

    let fresh = init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
        .await
        .unwrap();
    assert!(!fresh.replaced && fresh.slug == "app");
    let new = project_service::open(&state, "app").await.unwrap();
    assert!(!Arc::ptr_eq(&old, &new));
    assert!(!new.database_replaced().await);
    let listed = project_service::list(&state, false).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].available && listed[0].overview.is_some());

    // Without that request, the open after init finds a database file that is not the one the handle holds.
    delete_fold_data(&root).await;
    init_project(&host.paths, &root, None, &crate::common::ana(), &host.clock)
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
