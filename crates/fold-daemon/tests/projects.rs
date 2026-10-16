use std::path::Path;

use fold_core::{Clock, HostPaths, ManualClock, ProjectConfig, ProjectPaths};
use fold_daemon::services::projects::{
    ProjectError, init_project, init_project_from_git, load_config,
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
    assert_eq!(
        std::fs::read_link(project.root.join(".gitignore")).unwrap(),
        Path::new(".foldignore")
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
