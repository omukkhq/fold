use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use fold_core::revision::Revision;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{
    HostPaths, IntegrationId, ManualClock, ProjectPaths, RevisionId, Timestamp, codes,
};
use fold_daemon::services::projects::{ProjectError, init_project, init_project_from_git};
use fold_daemon::services::workspaces::{GitExport, WorkspaceService};
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

/// A repository on `main` with a file, an executable, a symlink and a committed `.fold/`; returns its `HEAD`.
#[cfg(test)]
fn source_repo(repo: &Path) -> String {
    fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "--quiet", "--initial-branch=main"]);
    write(repo, "README.md", b"# app\n");
    write(repo, "bin/run", b"#!/bin/sh\n");
    fs::set_permissions(repo.join("bin/run"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("README.md", repo.join("link")).unwrap();
    write(repo, ".fold/fold.db", b"not a database\n");
    git(repo, &["add", "--force", "."]);
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

#[cfg(test)]
async fn export(root: &Path, repo: &Path) -> Result<GitExport, Option<&'static str>> {
    let service = WorkspaceService::open_local(ProjectPaths::new(root), Git::default())
        .await
        .unwrap();
    let result = service.export_git(repo).await;
    service.close().await;
    result.map_err(|error| error.code())
}

#[tokio::test]
async fn from_git_imports_head_and_records_the_first_sync_point() {
    let host = Host::new();
    let repo = host.path("repo");
    let head = source_repo(&repo);
    let root = host.path("app");
    let project =
        init_project_from_git(&host.paths, &root, None, &repo, Git::default(), &host.clock)
            .await
            .unwrap();
    assert_eq!(project.git_commit.as_ref().unwrap().as_str(), head);
    assert_eq!(project.imported.files, 3);
    assert!(project.imported.skipped.is_empty());

    let paths = ProjectPaths::new(&project.root);
    let store = Store::open_project(&paths).await.unwrap();
    let r1 = workspaces::head(store.read()).await.unwrap().unwrap();
    store.close().await.unwrap();
    let canonical = repo.canonicalize().unwrap();
    assert_eq!(r1.import_source.as_deref(), canonical.to_str());
    let map = fs::read_to_string(paths.fold_dir().join(SYNC_MAP_FILE)).unwrap();
    assert_eq!(map, format!("r1 {head}\n"));
    assert_eq!(
        fs::read(paths.main_dir().join("README.md")).unwrap(),
        b"# app\n"
    );
    assert!(!paths.main_dir().join(".fold/fold.db").exists());
    assert_eq!(host.registered().await, 1);
}

/// A repository whose `main` holds a tree built by `git mktree` from `lines`, so it can name paths that `git add` would refuse.
#[cfg(test)]
fn crafted_repo(repo: &Path, lines: &str) {
    source_repo(repo);
    let blob = git(repo, &["rev-parse", "HEAD:README.md"])
        .trim()
        .to_owned();
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("mktree")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(
        &mut child.stdin.take().unwrap(),
        lines.replace("BLOB", &blob).as_bytes(),
    )
    .unwrap();
    let tree = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
    let commit = git(repo, &["commit-tree", "-m", "crafted", tree.trim()]);
    git(repo, &["update-ref", "refs/heads/main", commit.trim()]);
}

#[tokio::test]
async fn a_committed_dot_dot_entry_fails_and_creates_no_revision() {
    let host = Host::new();
    let repo = host.path("repo");
    crafted_repo(&repo, "100644 blob BLOB\tok.txt\n100644 blob BLOB\t..\n");
    let root = host.path("app");
    let error = init_project_from_git(&host.paths, &root, None, &repo, Git::default(), &host.clock)
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(codes::INVALID_PATH));
    assert!(!root.join(".fold").exists());
    assert!(!root.join("main").exists());
    assert_eq!(host.registered().await, 0);
}

#[tokio::test]
async fn only_the_top_of_a_sha1_repository_imports() {
    let host = Host::new();
    let repo = host.path("repo");
    source_repo(&repo);
    let root = host.path("app");
    for source in [repo.join("bin"), host.path("plain")] {
        fs::create_dir_all(&source).unwrap();
        let error = init_project_from_git(
            &host.paths,
            &root,
            None,
            &source,
            Git::default(),
            &host.clock,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), Some(codes::INVALID_REQUEST));
    }
    let sha256 = host.path("sha256");
    fs::create_dir(&sha256).unwrap();
    git(&sha256, &["init", "--quiet", "--object-format=sha256"]);
    let error = init_project_from_git(
        &host.paths,
        &root,
        None,
        &sha256,
        Git::default(),
        &host.clock,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), Some(codes::INVALID_REQUEST));
    assert!(!root.exists());
    assert_eq!(host.registered().await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_exports_run_one_after_the_other() {
    let host = Host::new();
    let root = host.path("app");
    init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    add_revision(&root, &[("a.txt", b"one\n")], None).await;
    add_revision(&root, &[("a.txt", b"two\n")], None).await;
    let target = host.path("out");
    let (first, second) = tokio::join!(export(&root, &target), export(&root, &target));
    let mut results = [first.unwrap(), second.unwrap()];
    results.sort_by_key(|result| matches!(result, GitExport::UpToDate { .. }));
    assert!(matches!(results[0], GitExport::Exported { commits: 3, .. }));
    assert_eq!(results[1], GitExport::UpToDate { last: Some(id(3)) });
    assert_eq!(log(&target).len(), 3);
    let map = fs::read_to_string(root.join(".fold").join(SYNC_MAP_FILE)).unwrap();
    assert_eq!(map.lines().count(), 3);
}

#[tokio::test]
async fn a_missing_git_program_fails_init_with_git_not_found() {
    let host = Host::new();
    let repo = host.path("repo");
    fs::create_dir(&repo).unwrap();
    let root = host.path("app");
    let missing = Git::new(host.path("bin/git"));
    let error = init_project_from_git(&host.paths, &root, None, &repo, missing, &host.clock)
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(codes::GIT_NOT_FOUND));
    assert!(!root.join(".fold").exists());
    let not_a_dir: Result<_, ProjectError> = init_project_from_git(
        &host.paths,
        &root,
        None,
        &host.path("absent"),
        Git::default(),
        &host.clock,
    )
    .await;
    assert_eq!(not_a_dir.unwrap_err().code(), Some(codes::INVALID_REQUEST));
}

#[tokio::test]
async fn exports_are_incremental_and_refuse_a_diverged_target() {
    let host = Host::new();
    let root = host.path("app");
    init_project(&host.paths, &root, None, &host.clock)
        .await
        .unwrap();
    add_revision(&root, &[("a.txt", b"one\n")], None).await;
    add_revision(&root, &[("a.txt", b"two\n")], None).await;
    let target = host.path("out");

    let first = export(&root, &target).await.unwrap();
    let GitExport::Exported {
        first: from,
        last,
        commits,
        head,
    } = first
    else {
        panic!("expected an export, got {first:?}");
    };
    assert_eq!((from, last, commits), (id(1), id(3), 3));
    let commits = log(&target);
    assert_eq!(commits.len(), 3);
    assert_eq!(commits[0], head.as_str());
    assert_eq!(message(&target, &commits[2]), "fold r1\n\nRevision: r1\n\n");
    assert_eq!(message(&target, &commits[0]), "fold r3\n\nRevision: r3\n\n");
    let map_path = root.join(".fold").join(SYNC_MAP_FILE);
    let map = fs::read_to_string(&map_path).unwrap();
    assert_eq!(map.lines().count(), 3);

    assert_eq!(
        export(&root, &target).await.unwrap(),
        GitExport::UpToDate { last: Some(id(3)) }
    );
    assert_eq!(fs::read_to_string(&map_path).unwrap(), map);
    assert_eq!(log(&target), commits);

    add_revision(&root, &[("a.txt", b"three\n")], None).await;
    add_revision(&root, &[("a.txt", b"three\n"), ("b.txt", b"four\n")], None).await;
    let again = export(&root, &target).await.unwrap();
    assert!(matches!(
        again,
        GitExport::Exported { first, last, commits: 2, .. } if first == id(4) && last == id(5)
    ));
    let after = log(&target);
    assert_eq!(after.len(), 5);
    assert_eq!(after[2], commits[0]);
    assert_eq!(
        git(&target, &["log", "-1", "--format=%P", &after[1]]).trim(),
        commits[0]
    );

    // Another path holds none of the exported commits.
    assert_eq!(
        export(&root, &host.path("elsewhere")).await,
        Err(Some(codes::EXPORT_DIVERGED))
    );
    assert!(!host.path("elsewhere").exists());

    // A commit made in the target after the last export.
    let outside = git(
        &target,
        &[
            "commit-tree",
            "-p",
            "main",
            "-m",
            "outside fold",
            "main^{tree}",
        ],
    );
    git(&target, &["update-ref", "refs/heads/main", outside.trim()]);
    add_revision(&root, &[("a.txt", b"six\n")], None).await;
    let before = fs::read_to_string(&map_path).unwrap();
    assert_eq!(
        export(&root, &target).await,
        Err(Some(codes::EXPORT_DIVERGED))
    );
    assert_eq!(fs::read_to_string(&map_path).unwrap(), before);
    assert_eq!(log(&target).len(), 6);

    let plain = host.path("plain");
    write(&plain, "file", b"x");
    assert_eq!(
        export(&root, &plain).await,
        Err(Some(codes::INVALID_REQUEST))
    );
}

#[tokio::test]
async fn a_from_git_project_continues_its_source_repository() {
    let host = Host::new();
    let repo = host.path("repo");
    let head = source_repo(&repo);
    let root = host.path("app");
    init_project_from_git(&host.paths, &root, None, &repo, Git::default(), &host.clock)
        .await
        .unwrap();

    // Nothing after r1, and an empty directory lacks the imported commit.
    let empty = host.path("empty");
    fs::create_dir(&empty).unwrap();
    assert_eq!(
        export(&root, &empty).await,
        Err(Some(codes::EXPORT_DIVERGED))
    );
    assert!(fs::read_dir(&empty).unwrap().next().is_none());
    assert_eq!(
        export(&root, &repo).await.unwrap(),
        GitExport::UpToDate { last: Some(id(1)) }
    );

    add_revision(
        &root,
        &[("README.md", b"# app v2\n"), ("bin/run", b"#!/bin/sh\n")],
        None,
    )
    .await;
    // `main` is checked out in the source, and moving it would leave the working tree behind.
    assert_eq!(
        export(&root, &repo).await,
        Err(Some(codes::INVALID_REQUEST))
    );
    assert_eq!(log(&repo), std::slice::from_ref(&head));
    git(&repo, &["checkout", "--quiet", "--detach"]);
    let exported = export(&root, &repo).await.unwrap();
    assert!(matches!(exported, GitExport::Exported { commits: 1, .. }));
    let commits = log(&repo);
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[1], head);
    assert_eq!(message(&repo, &commits[0]), "fold r2\n\nRevision: r2\n\n");
    // r2 records only what differs from r1; the committed `.fold/` that fold never imported stays.
    let names = git(&repo, &["show", "--name-status", "--format=", &commits[0]]);
    assert_eq!(names, "M\tREADME.md\nM\tbin/run\nD\tlink\n");
    let files = git(&repo, &["ls-tree", "-r", "--name-only", &commits[0]]);
    assert_eq!(files, ".fold/fold.db\nREADME.md\nbin/run\n");
}

const PARENTS: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'worker-a', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 3, 'three', 'backend', 0, 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-3', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't', 'm', 'backend', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 4, 1, 1, 'Database schema for reports', 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_at) VALUES (1, 1, 1, 1, 't', 1, 0);
INSERT INTO integrations (id, number, kind, proposal_id, version_id, result, actor, created_at) VALUES (1, 1, 'proposal', 1, 1, 'succeeded', 'backend.worker-a', 0);
INSERT INTO integrations (id, number, kind, remote_commit, result, actor, created_at) VALUES (2, 2, 'pull', 'abc123', 'succeeded', 'human:ana', 0);
";

#[tokio::test]
async fn exported_commits_carry_the_revision_messages() {
    let host = Host::new();
    let source = host.path("source");
    write(&source, "notes.txt", b"imported\n");
    let root = host.path("app");
    fold_daemon::services::projects::init_project_from(
        &host.paths,
        &root,
        None,
        Some(&source),
        &host.clock,
    )
    .await
    .unwrap();
    let store = Store::open_project(&ProjectPaths::new(&root))
        .await
        .unwrap();
    store
        .write(async |tx| -> Result<(), fold_store::StoreError> {
            sqlx::raw_sql(PARENTS).execute(tx.conn()).await?;
            Ok(())
        })
        .await
        .unwrap();
    store.close().await.unwrap();
    let integration = |number| IntegrationId::from_number(number);
    add_revision(&root, &[("a.txt", b"proposal\n")], integration(1)).await;
    add_revision(&root, &[("a.txt", b"pulled\n")], integration(2)).await;
    add_revision(&root, &[("a.txt", b"plain\n")], None).await;

    let target = host.path("out");
    export(&root, &target).await.unwrap();
    let commits = log(&target);
    let canonical = source.canonicalize().unwrap();
    let expected = [
        "fold r4\n\nRevision: r4\n\n".to_owned(),
        "fold r3\n\nRevision: r3\n\n".to_owned(),
        "P-4: Database schema for reports\n\nRevision: r2\nTask: T-3\nIntegrated by: backend.worker-a\n\n".to_owned(),
        format!("Import {}\n\nRevision: r1\n\n", canonical.display()),
    ];
    let found: Vec<String> = commits
        .iter()
        .map(|commit| message(&target, commit))
        .collect();
    assert_eq!(found, expected);
}

#[tokio::test]
async fn a_directory_without_a_project_is_project_not_found() {
    let host = Host::new();
    let error = WorkspaceService::open_local(ProjectPaths::new(host.path("none")), Git::default())
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some(codes::PROJECT_NOT_FOUND));
    assert!(!host.path("none").exists());
}
