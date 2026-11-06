#![cfg(test)]

use std::path::PathBuf;

use axum::http::{Method, StatusCode};
use fold_core::events::types;
use fold_core::{Actor, EventFilter, HumanName};
use fold_daemon::services::projects::{ProjectError, StashDrop, StashDropped, drop_stash_entries};
use fold_vcs::git::Git;
use serde_json::{Value, json};

use crate::common::acceptance::Harness;
use crate::folder_stash::{entries, refusal, stack_files, stash_as_ana, unstash_as_ana, wait_for};
use crate::human_changes::integrate;

fn ana() -> Actor {
    Actor::human(HumanName::parse("ana").unwrap())
}

async fn drop_as(
    h: &Harness,
    actor: &Actor,
    target: StashDrop,
) -> Result<StashDropped, ProjectError> {
    drop_stash_entries(&h.handle, actor, &Git::default(), target).await
}

async fn discarded(h: &Harness) -> Vec<fold_core::Event> {
    let filter = EventFilter {
        types: vec![types::STASH_DISCARDED],
        ..EventFilter::default()
    };
    fold_store::events::read_after(h.handle.store.read(), 0, &filter, 100)
        .await
        .unwrap()
}

/// Stashes `text` in `plan.md` as one entry, so each entry holds one change.
async fn stash_plan(h: &Harness, text: &str) {
    std::fs::write(PathBuf::from(h.root()).join("plan.md"), text).unwrap();
    stash_as_ana(h).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_drop_removes_one_entry_for_good_and_leaves_the_folder_as_it_is() {
    let h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    stash_plan(&h, "first\n").await;
    std::fs::write(dir.join("notes.md"), "second\n").unwrap();
    stash_as_ana(&h).await.unwrap();
    stash_plan(&h, "third\n").await;
    std::fs::write(dir.join("draft.md"), "not stashed\n").unwrap();
    assert_eq!(entries(&h).await.len(), 3);
    let dropped_tree = {
        let mut conn = h.handle.store.read().acquire().await.unwrap();
        fold_store::workspaces::stash_stack(&mut conn)
            .await
            .unwrap()[1]
            .tree
    };

    // stash@{1} is the entry of notes.md.
    let done = drop_as(&h, &ana(), StashDrop::One(1)).await.unwrap();
    assert_eq!(done.entries, 2);
    assert_eq!(done.dropped.len(), 1);
    assert_eq!(done.dropped[0].position, 1);
    assert_eq!(done.dropped[0].files, ["notes.md"]);
    assert_eq!(stack_files(&h).await, [vec!["plan.md"], vec!["plan.md"]]);
    // The folder keeps every file as it was, the unrecorded one too.
    assert_eq!(
        std::fs::read_to_string(dir.join("draft.md")).unwrap(),
        "not stashed\n"
    );
    assert!(!dir.join("notes.md").exists());
    // fld gc may collect the dropped entry's tree; the others stay.
    let roots = fold_store::workspaces::gc_roots(h.handle.store.read())
        .await
        .unwrap();
    assert!(!roots.contains(&dropped_tree));
    let events = discarded(&h).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].actor.as_str(), "human:ana");
    assert_eq!(events[0].data["stash"], "stash@{1}");
    assert_eq!(events[0].data["dropped"], 1);
    assert_eq!(events[0].data["entries"], 2);
    assert_eq!(events[0].data["files"], json!(["notes.md"]));
    assert_eq!(events[0].data["file_count"], 1);

    // The newest by default: the third change is gone, and the unstash brings back the first.
    let done = drop_as(&h, &ana(), StashDrop::One(0)).await.unwrap();
    assert_eq!((done.dropped[0].position, done.entries), (0, 1));
    std::fs::remove_file(dir.join("draft.md")).unwrap();
    unstash_as_ana(&h).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("plan.md")).unwrap(),
        "first\n"
    );
    assert!(entries(&h).await.is_empty());
    assert_eq!(
        refusal(drop_as(&h, &ana(), StashDrop::One(0)).await.unwrap_err()).0,
        "stash_empty"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_clear_drops_every_entry_and_a_place_past_the_stack_is_refused() {
    let h = Harness::start_plain().await;
    let cleared = drop_as(&h, &ana(), StashDrop::All).await.unwrap();
    assert_eq!((cleared.dropped.len(), cleared.entries), (0, 0));
    assert!(discarded(&h).await.is_empty());

    stash_plan(&h, "first\n").await;
    stash_plan(&h, "second\n").await;
    let error = drop_as(&h, &ana(), StashDrop::One(2)).await.unwrap_err();
    let ProjectError::Fold(fold) = &error else {
        panic!("a fold error");
    };
    assert_eq!(fold.details().unwrap()["entries"], 2);
    assert_eq!(refusal(error).0, "stash_entry_missing");

    let cleared = drop_as(&h, &ana(), StashDrop::All).await.unwrap();
    assert_eq!(cleared.entries, 0);
    assert_eq!(
        cleared
            .dropped
            .iter()
            .map(|entry| entry.position)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert!(entries(&h).await.is_empty());
    let events = discarded(&h).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].data, json!({"dropped": 2, "entries": 0}));
    assert_eq!(
        refusal(unstash_as_ana(&h).await.unwrap_err()).0,
        "stash_empty"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_a_human_drops_and_an_entry_whose_conflict_waits_is_refused() {
    let h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    std::fs::write(dir.join("README.md"), "# mine\n").unwrap();
    stash_as_ana(&h).await.unwrap();
    for actor in [Actor::System] {
        let error = drop_as(&h, &actor, StashDrop::All).await.unwrap_err();
        assert_eq!(error.code(), Some("human_required"));
    }
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    wait_for(&dir, "README.md", "# fold\n").await;
    unstash_as_ana(&h).await.unwrap();
    for target in [StashDrop::One(0), StashDrop::All] {
        let (reason, paths) = refusal(drop_as(&h, &ana(), target).await.unwrap_err());
        assert_eq!(reason, "stash_conflicted");
        assert_eq!(paths, json!(["README.md"]));
    }
    assert_eq!(entries(&h).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_drop_routes_serve_the_socket_and_the_signed_in_human() {
    let h = Harness::start_plain().await;
    stash_plan(&h, "first\n").await;
    stash_plan(&h, "second\n").await;
    stash_plan(&h, "third\n").await;
    let (status, body): (_, Value) = h
        .http(Method::GET, "/api/v1/projects/demo/stash", None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entries"].as_array().unwrap().len(), 3);

    let (status, body): (_, Value) = h
        .socket(Method::DELETE, "/api/v1/projects/demo/stash/2", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entries"], 2);
    assert_eq!(body["dropped"][0]["stash"], "stash@{2}");
    assert_eq!(body["dropped"][0]["files"], json!(["plan.md"]));

    let (status, body): (_, Value) = h
        .http(Method::DELETE, "/api/v1/projects/demo/stash/x", None, "ana")
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["details"]["reason"], "invalid_entry");
    let (status, body): (_, Value) = h
        .http(
            Method::DELETE,
            "/api/v1/projects/demo/stash/stash@%7B0%7D",
            None,
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entries"], 1);

    let (status, body): (_, Value) = h
        .http(Method::DELETE, "/api/v1/projects/demo/stash", None, "ana")
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["entries"], 0);
    assert_eq!(body["dropped"].as_array().unwrap().len(), 1);
    assert_eq!(discarded(&h).await.len(), 3);
    assert_eq!(discarded(&h).await[1].actor.as_str(), "human:ana");
}
