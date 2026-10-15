#![cfg(test)]

use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::reviews_support::{Fixture, human};
use fold_core::RevisionId;
use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::tree::{EntryKind, FlatEntry};
use fold_daemon::jobs::{JobProgress, MainMaterialiser, MainQueue, spawn_main_materialiser};
use fold_daemon::services::reviews::{PullIntegration, PullOutcome};
use fold_store::workspaces;
use fold_vcs::compare::load_flat;
use fold_vcs::main_view::{self, MarkerState};
use fold_vcs::objects::ObjectStore;
use fold_vcs::platform::Capabilities;
use fold_vcs::revision_store::write_initial_tree;

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

async fn tree_of(fx: &Fixture, revision: RevisionId) -> ObjectHash {
    workspaces::get_by_number(fx.project.store().read(), revision)
        .await
        .unwrap()
        .tree_hash
}

/// The materialiser as the project handle starts it.
fn start(fx: &Fixture) -> MainMaterialiser {
    spawn_main_materialiser(
        fx.project.store().clone(),
        &fx.bus,
        fx.project.paths().clone(),
        Arc::new(OnceLock::new()),
        MainQueue::new(),
    )
}

fn progress(job: &MainMaterialiser) -> JobProgress {
    *job.progress().borrow()
}

fn marker(fx: &Fixture) -> MarkerState {
    main_view::read_state(&fx.project.paths().fold_dir()).unwrap()
}

fn read(fx: &Fixture, path: &str) -> String {
    std::fs::read_to_string(fx.project.paths().main_dir().join(path)).unwrap()
}

/// A clean pull whose remote tree is the head's with `files` set; it becomes revision `number`.
async fn pull(fx: &Fixture, number: u32, files: &[(&str, &str)]) {
    let objects = ObjectStore::open(&fx.project.paths().fold_dir()).unwrap();
    let base = tree_of(fx, r(number - 1)).await;
    let mut tree = load_flat(&objects, &base).unwrap();
    for (path, content) in files {
        tree.insert(
            path.as_bytes().to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash: objects.write(ObjectKind::Blob, content.as_bytes()).unwrap(),
            },
        );
    }
    let pull = PullIntegration {
        remote_commit: format!("{number:040x}"),
        remote: "origin".to_owned(),
        branch: "main".to_owned(),
        base_tree: base,
        remote_tree: write_initial_tree(&objects, tree).unwrap(),
    };
    match fx.reviews.integrate_pull(&human(), pull).await.unwrap() {
        PullOutcome::Succeeded { revision } => assert_eq!(revision, r(number)),
        PullOutcome::Conflicted(conflicts) => panic!("conflicted: {conflicts:?}"),
    }
}

/// A crash part-way through rewriting `main/` from r2 to r3 leaves `a.txt` at r3 and `b.txt` at r2. r4 then puts `a.txt` back to its r2 content, so no diff from r2 names it. The materialiser started again repairs `main/` to the head from what it holds.
#[tokio::test]
async fn a_main_rewrite_stopped_part_way_is_repaired_at_start() {
    let _scenario = fail::FailScenario::setup();
    let fx = Fixture::new().await;
    let paths = fx.project.paths().clone();
    main_view::update(
        &ObjectStore::open(&paths.fold_dir()).unwrap(),
        &paths.main_dir(),
        &paths.fold_dir(),
        None,
        &tree_of(&fx, r(1)).await,
        r(1),
        Capabilities::probe(&paths.fold_dir()).unwrap(),
    )
    .unwrap();
    let job = start(&fx);
    pull(&fx, 2, &[("a.txt", "two\n"), ("b.txt", "two\n")]).await;
    until(|| marker(&fx).shown == Some(r(2))).await;
    drop(job);

    fail::cfg("main_view_after_write", "panic").unwrap();
    let job = start(&fx);
    until(|| progress(&job).checks >= 1).await;
    pull(&fx, 3, &[("a.txt", "three\n"), ("b.txt", "three\n")]).await;
    until(|| progress(&job).checks >= 2).await;
    drop(job);
    fail::remove("main_view_after_write");
    assert_eq!(
        marker(&fx),
        MarkerState {
            shown: Some(r(2)),
            rewriting: Some(r(3)),
        }
    );
    assert_eq!(read(&fx, "a.txt"), "three\n");
    assert_eq!(read(&fx, "b.txt"), "two\n");

    pull(&fx, 4, &[("a.txt", "two\n")]).await;
    let job = start(&fx);
    until(|| marker(&fx).shown == Some(r(4))).await;
    until(|| progress(&job).cycles == 1).await;
    assert_eq!(marker(&fx).rewriting, None);
    assert_eq!(read(&fx, "a.txt"), "two\n");
    assert_eq!(read(&fx, "b.txt"), "three\n");
    let objects = ObjectStore::open(&paths.fold_dir()).unwrap();
    let head = load_flat(&objects, &tree_of(&fx, r(4)).await).unwrap();
    for (path, entry) in head
        .iter()
        .filter(|(_, entry)| entry.kind == EntryKind::File)
    {
        let path = String::from_utf8(path.clone()).unwrap();
        let blob = objects
            .read_expecting(&entry.hash, ObjectKind::Blob)
            .unwrap();
        assert_eq!(
            std::fs::read(paths.main_dir().join(Path::new(&path))).unwrap(),
            blob.payload.to_vec(),
            "{path}"
        );
    }
}
