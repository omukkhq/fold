use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use fold_core::revision::Revision;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    HostPaths, IntegrationId, ManualClock, ProjectPaths, RevisionId, Timestamp, codes,
};
use fold_daemon::services::projects::{ProjectError, commit_main, init_project_from_git};
use fold_daemon::services::workspaces::WorkspaceService;
use fold_store::workspaces::{self, advance_main, insert_revision};
use fold_store::{Store, projects};
use fold_vcs::git::Git;
use fold_vcs::git_sync_map::SYNC_MAP_FILE;
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::{write_initial_tree, write_revision};
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
        let paths = HostPaths::new(dir.path().join("data").to_str(), None).unwrap();
        Self {
            dir,
            paths,
            clock: ManualClock::new(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    async fn registered(&self) -> usize {
        let registry = Store::open_registry(&self.paths.registry_db())
            .await
            .unwrap();
        let count = projects::list_projects(registry.read())
            .await
            .unwrap()
            .len();
        registry.close().await.unwrap();
        count
    }
}

#[cfg(test)]
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
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

#[cfg(test)]
fn write(root: &Path, path: &str, bytes: &[u8]) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// A repository on `main` with a file, an executable and a symlink; returns its `HEAD`.
#[cfg(test)]
fn source_repo(repo: &Path) -> String {
    fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "--quiet", "--initial-branch=main"]);
    write(repo, "README.md", b"# app\n");
    write(repo, "bin/run", b"#!/bin/sh\n");
    fs::set_permissions(repo.join("bin/run"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("README.md", repo.join("link")).unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "--quiet", "-m", "start"]);
    git(repo, &["rev-parse", "HEAD"]).trim().to_owned()
}

#[cfg(test)]
fn log(repo: &Path) -> Vec<String> {
    git(repo, &["log", "--first-parent", "--format=%H", "main"])
        .lines()
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
fn message(repo: &Path, commit: &str) -> String {
    git(repo, &["log", "-1", "--format=%B", commit])
}

#[cfg(test)]
fn id(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

/// Records a revision on top of `main` holding `files`, and moves `main` to it, as the integration procedure would.
#[cfg(test)]
async fn add_revision(
    root: &Path,
    files: &[(&str, &[u8])],
    integration: Option<IntegrationId>,
) -> RevisionId {
    let paths = ProjectPaths::new(root);
    let store = Store::open_project(&paths).await.unwrap();
    let head = workspaces::head(store.read()).await.unwrap().unwrap();
    let objects = ObjectStore::open(&paths.fold_dir()).unwrap();
    let mut flat = FlatTree::new();
    for (path, bytes) in files {
        let hash = objects
            .write(fold_core::object::ObjectKind::Blob, bytes)
            .unwrap();
        flat.insert(
            path.as_bytes().to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash,
            },
        );
    }
    let number = id(head.number.number() + 1);
    let revision = Revision {
        tree: write_initial_tree(&objects, flat).unwrap(),
        parent: Some(head.hash),
        number,
        integration,
        created_at: Timestamp::from_millis(1_700_000_000_000).unwrap(),
    };
    let stored = write_revision(&objects, &revision).unwrap();
    store
        .write(async |tx| {
            insert_revision(tx, &stored).await?;
            advance_main(tx, head.number, number).await
        })
        .await
        .unwrap();
    store.close().await.unwrap();
    number
}

/// Writes the project's missing commits into its repository, as its Git job does.
#[cfg(test)]
async fn commit(root: &Path) {
    let paths = ProjectPaths::new(root);
    let store = Store::open_project(&paths).await.unwrap();
    commit_main(&store, &paths, &Git::default(), &ManualClock::new())
        .await
        .unwrap();
    store.close().await.unwrap();
}

#[tokio::test]
async fn from_git_makes_the_repository_the_project_and_head_its_first_commit() {
    let host = Host::new();
    let repo = host.path("repo");
    let head = source_repo(&repo);
    git(&repo, &["checkout", "--quiet", "-b", "trunk"]);
    let project = init_project_from_git(&host.paths, &repo, None, Git::default(), &host.clock)
        .await
        .unwrap();
    assert_eq!(project.root, repo.canonicalize().unwrap());
    assert_eq!(project.git_commit.as_ref().unwrap().as_str(), head);
    assert_eq!(project.imported.files, 3);
    assert!(project.imported.skipped.is_empty());

    let paths = ProjectPaths::new(&project.root);
    let store = Store::open_project(&paths).await.unwrap();
    let r1 = workspaces::head(store.read()).await.unwrap().unwrap();
    store.close().await.unwrap();
    assert_eq!(
        r1.import_source.as_deref(),
        repo.canonicalize().unwrap().to_str()
    );
    let map = fs::read_to_string(paths.fold_dir().join(SYNC_MAP_FILE)).unwrap();
    assert_eq!(map, format!("r1 {head}\n"));
    // The history stays, the checked-out branch is the `[git]` branch, and `.fold/` stays out of Git.
    assert_eq!(git(&repo, &["rev-list", "--count", "trunk"]).trim(), "1");
    assert!(
        fs::read_to_string(paths.config())
            .unwrap()
            .ends_with("[git]\nbranch = \"trunk\"\n")
    );
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
    assert!(!repo.join(".foldignore").exists());
    assert_eq!(fs::read(repo.join("README.md")).unwrap(), b"# app\n");
    assert!(paths.db().is_file());
    assert_eq!(host.registered().await, 1);
}

#[tokio::test]
async fn from_git_refuses_a_dirty_tree_and_a_detached_head() {
    let host = Host::new();
    let repo = host.path("repo");
    let head = source_repo(&repo);
    write(&repo, "README.md", b"# changed\n");
    let refused = |error: ProjectError| {
        let ProjectError::Fold(error) = error else {
            panic!("a fold error");
        };
        assert_eq!(error.code(), codes::INVALID_REQUEST);
        error.details().unwrap()["reason"].clone()
    };
    let error = init_project_from_git(&host.paths, &repo, None, Git::default(), &host.clock)
        .await
        .unwrap_err();
    assert_eq!(refused(error), "uncommitted_changes");
    git(&repo, &["checkout", "--quiet", "--", "README.md"]);
    git(&repo, &["checkout", "--quiet", "--detach", &head]);
    let error = init_project_from_git(&host.paths, &repo, None, Git::default(), &host.clock)
        .await
        .unwrap_err();
    assert_eq!(refused(error), "detached_head");
    let empty = host.path("empty");
    fs::create_dir(&empty).unwrap();
    git(&empty, &["init", "--quiet", "--initial-branch=main"]);
    let error = init_project_from_git(&host.paths, &empty, None, Git::default(), &host.clock)
        .await
        .unwrap_err();
    assert_eq!(refused(error), "no_commit");
    assert!(!repo.join(".fold").exists() && !empty.join(".fold").exists());
    assert_eq!(host.registered().await, 0);
}

#[tokio::test]
async fn only_the_top_of_a_sha1_repository_imports() {
    let host = Host::new();
    let repo = host.path("repo");
    source_repo(&repo);
    for source in [repo.join("bin"), host.path("plain")] {
        fs::create_dir_all(&source).unwrap();
        let error = init_project_from_git(&host.paths, &source, None, Git::default(), &host.clock)
            .await
            .unwrap_err();
        assert_eq!(error.code(), Some(codes::INVALID_REQUEST));
        assert!(!source.join(".fold").exists());
    }
    let sha256 = host.path("sha256");
    fs::create_dir(&sha256).unwrap();
    git(&sha256, &["init", "--quiet", "--object-format=sha256"]);
    let error = init_project_from_git(&host.paths, &sha256, None, Git::default(), &host.clock)
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(codes::INVALID_REQUEST));
    assert!(!sha256.join(".fold").exists());
    assert_eq!(host.registered().await, 0);
}

#[tokio::test]
async fn a_missing_git_program_fails_init_with_git_not_found() {
    let host = Host::new();
    let repo = host.path("repo");
    fs::create_dir(&repo).unwrap();
    let missing = Git::new(host.path("bin/git"));
    let error = init_project_from_git(&host.paths, &repo, None, missing.clone(), &host.clock)
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(codes::GIT_NOT_FOUND));
    assert!(!repo.join(".fold").exists());
    let error = fold_daemon::services::projects::init_new_project(
        &host.paths,
        &repo,
        None,
        missing,
        None,
        &host.clock,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), Some(codes::GIT_NOT_FOUND));
    assert!(!repo.join(".fold").exists());
    let not_a_dir: Result<_, ProjectError> = init_project_from_git(
        &host.paths,
        &host.path("absent"),
        None,
        Git::default(),
        &host.clock,
    )
    .await;
    assert_eq!(not_a_dir.unwrap_err().code(), Some(codes::INVALID_REQUEST));
}

const PARENTS: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'worker-a', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 3, 'three', 'backend', 0, 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', '.fold/ws/t-3', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't', 'm', 'backend', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 4, 1, 1, 'Database schema for reports', 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't', 1, 'frontend', 0);
INSERT INTO integrations (id, number, kind, proposal_id, version_id, result, actor, created_at) VALUES (1, 1, 'proposal', 1, 1, 'succeeded', 'backend.worker-a', 0);
INSERT INTO reviews (id, version_id, reviewer_human, verdict, body, created_at) VALUES (1, 1, 'bo', 'request_changes', 'b', 1);
INSERT INTO reviews (id, version_id, reviewer_worker_id, verdict, body, created_at) VALUES (2, 1, 1, 'approve', 'b', 2);
INSERT INTO reviews (id, version_id, reviewer_human, verdict, body, created_at) VALUES (3, 1, 'ana', 'approve', 'b', 3);
";

#[tokio::test]
async fn commits_carry_the_revision_messages() {
    let host = Host::new();
    let root = host.path("app");
    write(&root, "notes.txt", b"imported\n");
    fold_daemon::services::projects::init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    let store = Store::open_project(&ProjectPaths::new(&root))
        .await
        .unwrap();
    // The pulled commit sits in the project's repository, as a fetch leaves it.
    let pulled = source_repo(&host.path("remote"));
    let mirror = root.join(".git");
    let remote = host.path("remote");
    git(
        &mirror,
        &[
            "fetch",
            "--quiet",
            remote.to_str().unwrap(),
            "+refs/heads/main:refs/remotes/origin/main",
        ],
    );
    let pull = format!(
        "INSERT INTO integrations (id, number, kind, remote_commit, result, actor, created_at) VALUES (2, 2, 'pull', '{pulled}', 'succeeded', 'human:ana', 0);"
    );
    store
        .write(async |tx| -> Result<(), fold_store::StoreError> {
            sqlx::raw_sql(PARENTS).execute(tx.conn()).await?;
            sqlx::raw_sql(sqlx::AssertSqlSafe(pull))
                .execute(tx.conn())
                .await?;
            Ok(())
        })
        .await
        .unwrap();
    store.close().await.unwrap();
    let integration = |number| IntegrationId::from_number(number);
    add_revision(&root, &[("a.txt", b"proposal\n")], integration(1)).await;
    add_revision(&root, &[("a.txt", b"pulled\n")], integration(2)).await;
    add_revision(&root, &[("a.txt", b"plain\n")], None).await;

    commit(&root).await;
    let target = root.join(".git");
    let commits = log(&target);
    let expected = [
        "fold r4\n\nFold-Revision: r4\n\n".to_owned(),
        format!(
            "Merge remote commit {}\n\nFold-Revision: r3\nIntegrated-by: human:ana\n\n",
            &pulled[..12]
        ),
        "Database schema for reports\n\nthree\n\nFold-Proposal: P-4\nFold-Version: 1\nFold-Task: T-3\nFold-Revision: r2\nReviewed-by: worker-a\nReviewed-by: human:ana\nIntegrated-by: backend.worker-a\n\n".to_owned(),
        "fold r1\n\nFold-Revision: r1\n\n".to_owned(),
    ];
    let found: Vec<String> = commits
        .iter()
        .map(|commit| message(&target, commit))
        .collect();
    assert_eq!(found, expected);
    // The pull revision's commit names the pulled commit as its second parent.
    let parents = git(&target, &["log", "-1", "--format=%P", &commits[1]]);
    assert_eq!(parents.trim(), format!("{} {pulled}", commits[2]));
}

#[tokio::test]
async fn a_directory_without_a_project_is_project_not_found() {
    let host = Host::new();
    let paths = ProjectPaths::new(host.path("none"));
    let clock = Arc::new(host.clock.clone());
    let error = WorkspaceService::open_local(paths, clock)
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(codes::PROJECT_NOT_FOUND));
    assert!(!host.path("none").exists());
}
