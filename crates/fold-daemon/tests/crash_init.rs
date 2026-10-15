#![cfg(test)]

use std::path::{Path, PathBuf};

use crate::common::invariants::assert_store_consistent;
use fold_core::object::{ObjectKind, hash_object};
use fold_core::{HostPaths, ManualClock, ProjectPaths};
use fold_daemon::services::projects::{InitializedProject, ProjectError, init_project_from};
use fold_store::{Store, projects};
use fold_vcs::objects::ObjectStore;
use tempfile::TempDir;

const LIB: &str = "pub fn x() {}\n";
const README: &str = "hello\n";

struct Host {
    dir: TempDir,
    paths: HostPaths,
    clock: ManualClock,
    source: PathBuf,
}

impl Host {
    /// A host registry and a source holding `src/lib.rs` and `README.md`.
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let paths = HostPaths::new(dir.path().join("data").to_str(), None).unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir_all(source.join("src")).unwrap();
        std::fs::write(source.join("src/lib.rs"), LIB).unwrap();
        std::fs::write(source.join("README.md"), README).unwrap();
        Self {
            dir,
            paths,
            clock: ManualClock::new(),
            source,
        }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("app")
    }

    fn project_paths(&self) -> ProjectPaths {
        ProjectPaths::new(self.root().canonicalize().unwrap())
    }

    /// `fld init app --from source`.
    async fn init(&self) -> Result<InitializedProject, ProjectError> {
        init_project_from(
            &self.paths,
            &self.root(),
            None,
            Some(&self.source),
            &self.clock,
        )
        .await
    }

    /// Runs the init with `failpoint` firing, which stops it there as a crash would.
    async fn stopped_at(&self, failpoint: &str) {
        fail::cfg(failpoint, "return").unwrap();
        let result = self.init().await;
        fail::remove(failpoint);
        assert_eq!(result.unwrap_err().code(), Some("failpoint"));
    }

    async fn registered(&self) -> Vec<String> {
        let registry = Store::open_registry(&self.paths.registry_db())
            .await
            .unwrap();
        let rows = projects::list_projects(registry.read()).await.unwrap();
        registry.close().await.unwrap();
        rows.into_iter().map(|row| row.slug).collect()
    }
}

async fn assert_project_consistent(paths: &ProjectPaths) {
    let store = Store::open_project(paths).await.unwrap();
    assert_store_consistent(&store, paths).await;
    store.close().await.unwrap();
}

/// Every file below `dir` with its content, the database's WAL and shared-memory files left out, since opening the database may create them.
fn files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(at) = pending.pop() {
        for entry in std::fs::read_dir(&at).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if path.is_dir() {
                pending.push(path);
            } else if !name.ends_with("-wal") && !name.ends_with("-shm") {
                let content = std::fs::read(&path).unwrap();
                found.push((path, content));
            }
        }
    }
    found.sort();
    found
}

#[tokio::test]
async fn an_init_stopped_before_the_registry_insert_finishes_on_the_next_run() {
    let scenario = fail::FailScenario::setup();
    let host = Host::new();
    host.stopped_at("init_before_register").await;
    let paths = host.project_paths();
    assert!(paths.db().is_file());
    assert!(host.registered().await.is_empty());
    // The stopped run filled `main/`; one file taken out stands for a run that stopped while it wrote them.
    std::fs::remove_file(paths.main_dir().join("src/lib.rs")).unwrap();

    let project = host.init().await.unwrap();
    assert!(project.resumed);
    assert_eq!(project.slug, "app");
    assert_eq!(project.root, paths.root());
    assert_eq!(project.revision.to_string(), "r1");
    assert_eq!(project.imported.files, 2);
    assert_eq!(project.imported.bytes, (LIB.len() + README.len()) as u64);
    assert!(project.imported.skipped.is_empty());
    assert_eq!(project.git_commit, None);
    assert_eq!(
        std::fs::read_to_string(paths.main_dir().join("src/lib.rs")).unwrap(),
        LIB
    );
    assert_eq!(host.registered().await, ["app"]);
    assert_project_consistent(&paths).await;

    let again = host.init().await.unwrap_err();
    assert_eq!(again.code(), Some("project_exists"));
    assert_eq!(host.registered().await, ["app"]);
    scenario.teardown();
}

#[tokio::test]
async fn an_init_stopped_inside_the_import_is_refused_and_nothing_is_deleted() {
    let scenario = fail::FailScenario::setup();
    let host = Host::new();
    host.stopped_at("init_after_import").await;
    let paths = host.project_paths();
    let before = files(host.dir.path());
    assert!(
        before
            .iter()
            .any(|(path, _)| path.starts_with(paths.fold_dir())),
        "the stopped import left its objects"
    );

    let error = host.init().await.unwrap_err();
    assert_eq!(error.code(), Some("project_incomplete"));
    let ProjectError::Fold(error) = error else {
        panic!("a fold error");
    };
    assert!(
        error
            .message()
            .contains(&paths.root().display().to_string())
    );
    assert!(error.hint().unwrap().contains(&format!(
        "delete {} and run fld init again",
        paths.fold_dir().display()
    )));
    assert_eq!(files(host.dir.path()), before);
    assert!(host.registered().await.is_empty());

    std::fs::remove_dir_all(paths.fold_dir()).unwrap();
    let project = host.init().await.unwrap();
    assert!(!project.resumed);
    assert_eq!(host.registered().await, ["app"]);
    assert_project_consistent(&paths).await;
    scenario.teardown();
}

#[tokio::test]
async fn a_stopped_init_with_a_missing_object_is_refused_and_kept() {
    let scenario = fail::FailScenario::setup();
    let host = Host::new();
    host.stopped_at("init_before_register").await;
    let paths = host.project_paths();
    let objects = ObjectStore::open(&paths.fold_dir()).unwrap();
    std::fs::remove_file(objects.path(&hash_object(ObjectKind::Blob, LIB.as_bytes()))).unwrap();
    let before = files(host.dir.path());

    let error = host.init().await.unwrap_err();
    assert_eq!(error.code(), Some("project_incomplete"));
    assert_eq!(files(host.dir.path()), before);
    assert!(host.registered().await.is_empty());
    scenario.teardown();
}

#[tokio::test]
async fn a_stopped_init_that_another_init_holds_is_refused_and_finished_once_free() {
    let scenario = fail::FailScenario::setup();
    let host = Host::new();
    host.stopped_at("init_before_register").await;
    let paths = host.project_paths();
    // A running init holds this lock on `.fold/` until it returns.
    let running = std::fs::File::open(paths.fold_dir()).unwrap();
    running.lock().unwrap();

    let error = host.init().await.unwrap_err();
    assert_eq!(error.code(), Some("project_incomplete"));
    assert!(host.registered().await.is_empty());
    assert!(paths.db().is_file());

    drop(running);
    let project = host.init().await.unwrap();
    assert!(project.resumed);
    assert_eq!(host.registered().await, ["app"]);
    assert_project_consistent(&paths).await;
    scenario.teardown();
}
