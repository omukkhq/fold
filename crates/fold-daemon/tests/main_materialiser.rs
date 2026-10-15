#![cfg(test)]

pub mod reviews_support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::reviews::Verdict;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{Actor, AgentName, RevisionId};
use fold_daemon::jobs::{JobProgress, MainMaterialiser, MainQueue, spawn_main_materialiser};
use fold_daemon::services::events::EventBus;
use fold_daemon::services::reviews::{IntegrationOutcome, PullIntegration, PullOutcome};
use fold_store::workspaces;
use fold_vcs::main_view;
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::Capabilities;
use fold_vcs::revision_store::write_initial_tree;
use reviews_support::{Fixture, LIB, README, human, p};

const WAIT: Duration = Duration::from_secs(10);

fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

/// Polls a fact a background task changes, within the wait.
async fn until(mut done: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the condition within the wait");
}

/// The fixture with `main/` showing `r1` and the marker naming it, as `fold init` leaves a project.
struct Project {
    fx: Fixture,
}

impl Project {
    async fn new() -> Self {
        let fx = Fixture::new().await;
        let project = Self { fx };
        let tree = project.tree_of(r(1)).await;
        main_view::update(
            &project.objects(),
            &project.main(),
            &project.fx.project.paths().fold_dir(),
            None,
            &tree,
            r(1),
            project.caps(),
        )
        .unwrap();
        project
    }

    fn main(&self) -> PathBuf {
        self.fx.project.paths().main_dir()
    }

    fn objects(&self) -> ObjectStore {
        ObjectStore::open(&self.fx.project.paths().fold_dir()).unwrap()
    }

    fn caps(&self) -> Capabilities {
        Capabilities::probe(&self.fx.project.paths().fold_dir()).unwrap()
    }

    async fn tree_of(&self, revision: RevisionId) -> ObjectHash {
        workspaces::get_by_number(self.fx.project.store().read(), revision)
            .await
            .unwrap()
            .tree_hash
    }

    fn start(&self, bus: &EventBus, queue: MainQueue) -> MainMaterialiser {
        spawn_main_materialiser(
            self.fx.project.store().clone(),
            bus,
            self.fx.project.paths().clone(),
            Arc::new(OnceLock::new()),
            queue,
        )
    }

    fn marker(&self) -> Option<RevisionId> {
        main_view::read_marker(&self.fx.project.paths().fold_dir()).unwrap()
    }

    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.main().join(path)).ok()
    }

    /// `P-1` approved and integrated by the integrator `review`.
    async fn integrate(&self) -> RevisionId {
        self.fx.proposal().await;
        self.fx
            .agents
            .set_integrator(&human(), &AgentName::parse("review").unwrap(), true)
            .await
            .unwrap();
        let review: Actor = self
            .fx
            .agents
            .actor(&self.fx.review.ctx, None)
            .await
            .unwrap();
        self.fx
            .reviews
            .submit_review(&review, p(1), Verdict::Approve, "ok", Vec::new())
            .await
            .unwrap();
        match self
            .fx
            .reviews
            .integrate_proposal(&review, p(1), None)
            .await
            .unwrap()
        {
            IntegrationOutcome::Succeeded { revision } => revision,
            IntegrationOutcome::Conflicted(conflicts) => panic!("conflicted: {conflicts:?}"),
        }
    }

    /// A clean pull whose remote tree is `r1` plus `path`.
    async fn pull_adding(&self, path: &str, content: &str) -> RevisionId {
        let objects = self.objects();
        let mut tree = FlatTree::new();
        for (name, bytes) in [
            ("src/lib.rs", LIB),
            ("README.md", README),
            (path, content.as_bytes()),
        ] {
            tree.insert(
                name.as_bytes().to_vec(),
                FlatEntry {
                    kind: EntryKind::File,
                    hash: objects.write(ObjectKind::Blob, bytes).unwrap(),
                },
            );
        }
        let pull = PullIntegration {
            remote_commit: "4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_owned(),
            sync_base: r(1),
            remote_tree: write_initial_tree(&objects, tree).unwrap(),
        };
        match self
            .fx
            .reviews
            .integrate_pull(&human(), pull)
            .await
            .unwrap()
        {
            PullOutcome::Succeeded { revision } => revision,
            PullOutcome::Conflicted(conflicts) => panic!("conflicted: {conflicts:?}"),
        }
    }
}

fn progress(job: &MainMaterialiser) -> JobProgress {
    *job.progress().borrow()
}

#[tokio::test]
async fn integration_updates_main_dir_within_a_timeout() {
    let project = Project::new().await;
    let job = project.start(&project.fx.bus, MainQueue::new());
    until(|| progress(&job).checks >= 1).await;
    assert_eq!(
        project.read("src/lib.rs").as_deref(),
        Some("pub fn x() {}\n")
    );

    let revision = project.integrate().await;
    assert_eq!(revision, r(2));

    until(|| project.marker() == Some(r(2))).await;
    assert_eq!(
        project.read("src/lib.rs").as_deref(),
        Some("pub fn y() {}\n")
    );
    assert_eq!(
        project.read("src/new.rs").as_deref(),
        Some("pub fn z() {}\n")
    );
    assert_eq!(project.read("README.md").as_deref(), Some("# demo\n"));
    assert_eq!(
        std::fs::read_link(project.main().join("docs/readme")).unwrap(),
        Path::new("../README.md")
    );
    let mode = |path: &str| {
        std::fs::symlink_metadata(project.main().join(path))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    };
    assert_eq!(mode("src/new.rs"), 0o444);
    assert_eq!(mode("src"), 0o555);
    until(|| progress(&job).cycles == 1).await;
}

#[tokio::test]
async fn queued_revisions_collapse_to_the_newest() {
    let project = Project::new().await;
    // The job hears no event: it reaches main only through the queue.
    let queue = MainQueue::new();
    let silent = EventBus::new();
    let job = project.start(&silent, queue.clone());
    until(|| progress(&job).checks >= 1).await;
    assert_eq!(progress(&job).cycles, 0);

    assert_eq!(project.integrate().await, r(2));
    assert_eq!(project.pull_adding("NOTES.md", "notes\n").await, r(3));
    assert_eq!(project.pull_adding("MORE.md", "more\n").await, r(4));
    assert_eq!(project.marker(), Some(r(1)));
    queue.queue(r(2));
    queue.queue(r(4));
    queue.queue(r(3));

    until(|| progress(&job).cycles >= 1).await;
    assert_eq!(project.marker(), Some(r(4)));
    assert_eq!(
        progress(&job),
        JobProgress {
            checks: 2,
            cycles: 1
        }
    );
    assert_eq!(
        project.read("src/new.rs").as_deref(),
        Some("pub fn z() {}\n")
    );
    assert_eq!(project.read("NOTES.md").as_deref(), Some("notes\n"));
    assert_eq!(project.read("MORE.md").as_deref(), Some("more\n"));

    // An older revision queued later changes nothing.
    queue.queue(r(2));
    assert_eq!(project.marker(), Some(r(4)));
}

#[tokio::test]
async fn start_repairs_a_main_dir_behind_the_head() {
    let project = Project::new().await;
    assert_eq!(project.integrate().await, r(2));
    assert_eq!(project.read("src/new.rs"), None);
    assert_eq!(project.marker(), Some(r(1)));

    let job = project.start(&project.fx.bus, MainQueue::new());

    until(|| project.marker() == Some(r(2))).await;
    assert_eq!(
        project.read("src/new.rs").as_deref(),
        Some("pub fn z() {}\n")
    );
    until(|| progress(&job).cycles == 1).await;

    // A project without a marker shows r1, which init wrote, so the repair writes only what changed since.
    std::fs::remove_file(
        project
            .fx
            .project
            .paths()
            .fold_dir()
            .join(main_view::MARKER),
    )
    .unwrap();
    drop(job);
    let again = project.start(&project.fx.bus, MainQueue::new());
    until(|| project.marker() == Some(r(2))).await;
    until(|| progress(&again).cycles == 1).await;
    assert_eq!(
        project.read("src/lib.rs").as_deref(),
        Some("pub fn y() {}\n")
    );
}
