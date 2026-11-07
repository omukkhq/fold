#![cfg(test)]

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use fold_core::events::types;
use fold_core::{Actor, EventFilter, HumanName};
use fold_daemon::services::agents::{NeedsYouKind, needs_you};
use fold_daemon::services::projects::{
    ProjectError, RecordOutcome, ResetOutcome, ResetScope, UnstashOutcome, reset,
    restore_agent_changes,
};
use fold_vcs::git::Git;
use serde_json::{Value, json};

use crate::common::acceptance::Harness;
use crate::folder_stash::{entries, refusal, stack_files, stash_as_ana, unstash_as_ana, wait_for};
use crate::human_changes::{branch_born, integrate, notice_with, record_as_ana};

fn ana() -> Actor {
    Actor::human(HumanName::parse("ana").unwrap())
}

fn read(dir: &Path, path: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(path)).ok()
}

fn write(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

pub async fn reset_as_ana(h: &Harness) -> Result<ResetOutcome, ProjectError> {
    reset(&h.handle, &ana(), &Git::default(), ResetScope::Folder).await
}

pub async fn restore_as_ana(h: &Harness, paths: &[&str]) -> Result<ResetOutcome, ProjectError> {
    let paths = paths.iter().map(|path| (*path).to_owned()).collect();
    reset(&h.handle, &ana(), &Git::default(), ResetScope::Paths(paths)).await
}

pub async fn stored(h: &Harness, kind: fold_core::EventType) -> Vec<fold_core::Event> {
    let filter = EventFilter {
        types: vec![kind],
        ..EventFilter::default()
    };
    fold_store::events::read_after(h.handle.store.read(), 0, &filter, 100)
        .await
        .unwrap()
}

pub async fn human_change_states(h: &Harness) -> Vec<String> {
    sqlx::query_scalar("SELECT state FROM human_changes ORDER BY id")
        .fetch_all(h.handle.store.read())
        .await
        .unwrap()
}

pub async fn main_workspace_state(h: &Harness) -> String {
    sqlx::query_scalar("SELECT state FROM workspaces WHERE kind = 'main'")
        .fetch_one(h.handle.store.read())
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reset_stashes_the_changes_first_and_puts_the_folder_back_to_main() {
    let h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    let nothing = reset_as_ana(&h).await.unwrap();
    assert!(!nothing.stashed);
    assert!(nothing.files.is_empty());
    assert!(nothing.at_head);
    assert!(entries(&h).await.is_empty());

    write(&dir, "README.md", "");
    write(&dir, "junk/a", "");
    write(&dir, "b", "");
    let done = reset_as_ana(&h).await.unwrap();
    assert!(done.stashed);
    assert!(done.at_head);
    assert_eq!(done.files, ["README.md", "b", "junk/a"]);
    assert_eq!(done.entries, 1);
    assert_eq!(done.dropped_change, None);
    assert_eq!(read(&dir, "README.md").as_deref(), Some("# demo\n"));
    assert!(!dir.join("b").exists() && !dir.join("junk/a").exists());
    let pushed = stored(&h, types::STASH_PUSHED).await;
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].actor.as_str(), "human:ana");
    assert_eq!(pushed[0].data["reset"], true);
    // Nothing is lost: the safety entry brings the changes back.
    assert!(matches!(
        unstash_as_ana(&h).await.unwrap(),
        UnstashOutcome::Applied { .. }
    ));
    assert_eq!(read(&dir, "README.md").as_deref(), Some(""));
    assert_eq!(read(&dir, "junk/a").as_deref(), Some(""));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restore_puts_back_only_the_named_paths() {
    let h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    write(&dir, "README.md", "mine\n");
    write(&dir, "src/a.rs", "a\n");
    write(&dir, "src/b.rs", "b\n");
    write(&dir, "notes.md", "keep\n");
    let done = restore_as_ana(&h, &["src", "README.md", "docs"])
        .await
        .unwrap();
    assert!(done.stashed);
    assert_eq!(done.files, ["README.md", "src/a.rs", "src/b.rs"]);
    assert_eq!(done.unchanged, ["docs"]);
    assert_eq!(read(&dir, "README.md").as_deref(), Some("# demo\n"));
    assert!(!dir.join("src/a.rs").exists());
    assert_eq!(read(&dir, "notes.md").as_deref(), Some("keep\n"));
    assert_eq!(
        stack_files(&h).await,
        [vec!["README.md", "src/a.rs", "src/b.rs"]]
    );
    let nothing = restore_as_ana(&h, &["src"]).await.unwrap();
    assert!(!nothing.stashed);
    assert_eq!(nothing.unchanged, ["src"]);
    for bad in [vec!["../outside"], vec!["/etc/passwd"], vec![]] {
        let (reason, _) = refusal(restore_as_ana(&h, &bad).await.unwrap_err());
        assert!(
            reason == "invalid_path" || reason == "no_paths",
            "{bad:?}: {reason}"
        );
    }
    let error = reset(
        &h.handle,
        &Actor::System,
        &Git::default(),
        ResetScope::Folder,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), Some("human_required"));
    assert_eq!(read(&dir, "notes.md").as_deref(), Some("keep\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reset_drops_the_waiting_human_change_and_abandons_its_merge_workspace() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged("main", &["main"]).await;
    let mut inbox = h.register_inbox("main").await;
    let dir = PathBuf::from(h.root());
    write(&dir, "README.md", "# mine\n");
    write(&dir, "notes.md", "notes\n");
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::Conflicted { created: true, .. }
    ));
    let opened = h.call("main", "open_main_workspace", json!({})).await;
    let ws = opened["workspace"].as_str().unwrap().to_owned();

    // A restore that leaves a change of the waiting change in the folder keeps it waiting.
    let partial = restore_as_ana(&h, &["notes.md"]).await.unwrap();
    assert_eq!(partial.dropped_change, None);
    assert_eq!(human_change_states(&h).await, ["conflicted"]);
    assert_eq!(main_workspace_state(&h).await, "active");

    let done = reset_as_ana(&h).await.unwrap();
    assert_eq!(done.files, ["README.md"]);
    assert!(done.at_head);
    let dropped = done.dropped_change.unwrap();
    assert_eq!(dropped.conflicted, ["README.md"]);
    assert_eq!(dropped.workspace.unwrap().to_string(), ws);
    assert!(dropped.workspace_abandoned);
    assert_eq!(read(&dir, "README.md").as_deref(), Some("# fold\n"));
    assert_eq!(human_change_states(&h).await, ["superseded"]);
    assert_eq!(main_workspace_state(&h).await, "abandoned");
    let withdrawn = stored(&h, types::HUMAN_CHANGE_WITHDRAWN).await;
    assert_eq!(withdrawn.len(), 1);
    assert_eq!(withdrawn[0].data["workspace"], ws.as_str());
    assert_eq!(withdrawn[0].data["workspace_abandoned"], true);
    let abandoned = stored(&h, types::WORKSPACE_ABANDONED).await;
    assert_eq!(abandoned.len(), 1);
    assert_eq!(abandoned[0].actor.as_str(), "human:ana");
    let notice = notice_with(&mut inbox, "took back").await;
    assert!(notice.contains(&format!("fold abandoned {ws}")), "{notice}");
    assert!(
        needs_you(&h.handle.store)
            .await
            .unwrap()
            .iter()
            .all(|item| item.kind != NeedsYouKind::HumanChange)
    );
    // Both safety entries hold the changes: notes.md, then README.md.
    assert_eq!(stack_files(&h).await, [vec!["README.md"], vec!["notes.md"]]);
}

/// A restore of every unrecorded change withdraws the waiting human change as `fld stash` does, and leaves the main agent's merge workspace open: only `fld reset` abandons it, and the main agent's notice asks it to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restore_of_every_change_withdraws_the_waiting_change_and_keeps_its_merge_workspace() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged("main", &["main"]).await;
    let mut inbox = h.register_inbox("main").await;
    let dir = PathBuf::from(h.root());
    write(&dir, "README.md", "# mine\n");
    write(&dir, "notes.md", "notes\n");
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::Conflicted { created: true, .. }
    ));
    let opened = h.call("main", "open_main_workspace", json!({})).await;
    let ws = opened["workspace"].as_str().unwrap().to_owned();

    let done = restore_as_ana(&h, &["README.md", "notes.md"])
        .await
        .unwrap();
    assert!(done.stashed);
    let dropped = done.dropped_change.unwrap();
    assert_eq!(dropped.conflicted, ["README.md"]);
    assert_eq!(dropped.workspace.unwrap().to_string(), ws);
    assert!(!dropped.workspace_abandoned);
    assert_eq!(human_change_states(&h).await, ["superseded"]);
    assert_eq!(main_workspace_state(&h).await, "active");
    let withdrawn = stored(&h, types::HUMAN_CHANGE_WITHDRAWN).await;
    assert_eq!(withdrawn.len(), 1);
    assert_eq!(withdrawn[0].data["workspace_abandoned"], false);
    assert_eq!(withdrawn[0].data["restore"], true);
    assert!(stored(&h, types::WORKSPACE_ABANDONED).await.is_empty());
    let notice = notice_with(&mut inbox, "took back").await;
    assert!(notice.contains(&format!("abandon {ws}")), "{notice}");
    assert_eq!(stack_files(&h).await, [vec!["README.md", "notes.md"]]);
}

/// A reset that a later check refuses, here a tracked file over the size limit, leaves the waiting unstash conflict as it was: the entry stays conflicted and its markers stay in the folder, so `fld record` still refuses them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_reset_keeps_the_waiting_unstash_conflict() {
    let h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    write(&dir, "README.md", "# mine\n");
    stash_as_ana(&h).await.unwrap();
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    wait_for(&dir, "README.md", "# fold\n").await;
    assert!(matches!(
        unstash_as_ana(&h).await.unwrap(),
        UnstashOutcome::Conflicted { .. }
    ));
    let marked = read(&dir, "README.md").unwrap();
    let config = dir.join(".fold/config.toml");
    let kept = std::fs::read_to_string(&config).unwrap_or_default();
    std::fs::write(&config, format!("max_file_bytes = 16\n{kept}")).unwrap();

    let (reason, paths) = refusal(reset_as_ana(&h).await.unwrap_err());
    assert_eq!(reason, "skipped_tracked");
    assert_eq!(paths, json!(["README.md"]));
    let listed = entries(&h).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].conflicted, ["README.md"]);
    assert!(stored(&h, types::STASH_DISCARDED).await.is_empty());
    assert_eq!(read(&dir, "README.md").unwrap(), marked);

    std::fs::write(&config, kept).unwrap();
    let done = reset_as_ana(&h).await.unwrap();
    assert_eq!(done.dropped_unstash, ["README.md"]);
    assert_eq!(stored(&h, types::STASH_DISCARDED).await.len(), 1);
    assert_eq!(entries(&h).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reset_drops_a_waiting_unstash_conflict_and_a_restore_refuses_it() {
    let h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    write(&dir, "README.md", "# mine\n");
    stash_as_ana(&h).await.unwrap();
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    wait_for(&dir, "README.md", "# fold\n").await;
    assert!(matches!(
        unstash_as_ana(&h).await.unwrap(),
        UnstashOutcome::Conflicted { .. }
    ));
    let marked = read(&dir, "README.md").unwrap();
    let (reason, paths) = refusal(restore_as_ana(&h, &["README.md"]).await.unwrap_err());
    assert_eq!(reason, "stash_conflicted");
    assert_eq!(paths, json!(["README.md"]));

    let done = reset_as_ana(&h).await.unwrap();
    assert_eq!(done.dropped_unstash, ["README.md"]);
    assert!(done.at_head);
    assert_eq!(read(&dir, "README.md").as_deref(), Some("# fold\n"));
    let discarded = stored(&h, types::STASH_DISCARDED).await;
    assert_eq!(discarded.len(), 1);
    assert_eq!(discarded[0].data["conflicted"], json!(["README.md"]));
    // The safety entry keeps the file with its markers.
    let listed = entries(&h).await;
    assert_eq!(listed.len(), 1);
    assert!(listed[0].conflicted.is_empty());
    assert!(matches!(
        unstash_as_ana(&h).await.unwrap(),
        UnstashOutcome::Applied { .. }
    ));
    assert_eq!(read(&dir, "README.md").unwrap(), marked);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_reset_routes_serve_the_socket_and_the_signed_in_human() {
    let h = Harness::start_plain().await;
    let dir = PathBuf::from(h.root());
    write(&dir, "a.txt", "a\n");
    write(&dir, "b.txt", "b\n");
    let (status, body): (_, Value) = h
        .socket(
            Method::POST,
            "/api/v1/projects/demo/folder/restore",
            Some(&json!({"paths": ["a.txt"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["files"], json!(["a.txt"]));
    assert_eq!(body["stashed"], true);
    assert_eq!(body["dropped_change"], Value::Null);
    let (status, body): (_, Value) = h
        .http(
            Method::POST,
            "/api/v1/projects/demo/folder/restore",
            Some(&json!({"paths": ["../x"]})),
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["details"]["reason"], "invalid_path");
    let (status, body): (_, Value) = h
        .http(
            Method::POST,
            "/api/v1/projects/demo/folder/reset",
            None,
            "ana",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({
            "revision": "r1", "at_head": true, "files": ["b.txt"], "stashed": true, "entries": 2,
            "unchanged": [], "dropped_unstash": [], "dropped_change": null, "skipped": [], "unstash": null,
        })
    );
    assert!(!dir.join("b.txt").exists());
    let pushed = stored(&h, types::STASH_PUSHED).await;
    assert_eq!(pushed[1].actor.as_str(), "human:ana");
}

/// Places a change of `path` in a tool call of the agent `main`, as the supervisor's window does.
pub async fn place_agent_change(h: &Harness, path: &str) {
    let now = h.handle.clock.now();
    let agent = fold_core::AgentName::parse("main").unwrap();
    h.handle
        .store
        .write(async |tx| -> Result<bool, fold_store::StoreError> {
            fold_store::workspaces::insert_agent_change(tx, path.as_bytes(), &agent, now, now).await
        })
        .await
        .unwrap();
}

pub async fn open_agent_paths(h: &Harness) -> Vec<String> {
    fold_store::workspaces::open_agent_changes(h.handle.store.read())
        .await
        .unwrap()
        .into_iter()
        .map(|row| String::from_utf8(row.path).unwrap())
        .collect()
}

/// The restore of a worker's changes goes through the reset of named paths: the safety entry holds exactly the files the worker changed, they go back to main's version, the human's own change stays, and the marks end only once the files are back, so a refusal, here an unstash conflict that waits in the folder, keeps them open and stashes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restore_of_a_workers_changes_puts_back_only_its_paths_through_the_reset() {
    let mut h = Harness::start_plain().await;
    h.connect_tagged("main", &["main"]).await;
    let dir = PathBuf::from(h.root());
    write(&dir, "README.md", "worker\n");
    write(&dir, "junk.txt", "");
    write(&dir, "notes.md", "mine\n");
    place_agent_change(&h, "README.md").await;
    place_agent_change(&h, "junk.txt").await;

    let done = restore_agent_changes(&h.handle, &ana(), &Git::default())
        .await
        .unwrap();
    assert_eq!(done.files, ["README.md", "junk.txt"]);
    assert_eq!(done.entries, 1);
    assert_eq!(stack_files(&h).await, [vec!["README.md", "junk.txt"]]);
    assert_eq!(read(&dir, "README.md").as_deref(), Some("# demo\n"));
    assert!(!dir.join("junk.txt").exists());
    assert_eq!(read(&dir, "notes.md").as_deref(), Some("mine\n"));
    assert!(open_agent_paths(&h).await.is_empty());
    let error = restore_agent_changes(&h.handle, &ana(), &Git::default())
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("agent_change_not_found"));

    // An unstash conflict waits in the folder: the restore refuses, and the worker's file and its mark stay.
    std::fs::remove_file(dir.join("notes.md")).unwrap();
    write(&dir, "README.md", "# mine\n");
    stash_as_ana(&h).await.unwrap();
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    wait_for(&dir, "README.md", "# fold\n").await;
    assert!(matches!(
        unstash_as_ana(&h).await.unwrap(),
        UnstashOutcome::Conflicted { .. }
    ));
    write(&dir, "junk2.txt", "");
    place_agent_change(&h, "junk2.txt").await;
    let (reason, paths) = refusal(
        restore_agent_changes(&h.handle, &ana(), &Git::default())
            .await
            .unwrap_err(),
    );
    assert_eq!(reason, "stash_conflicted");
    assert_eq!(paths, json!(["README.md"]));
    assert_eq!(read(&dir, "junk2.txt").as_deref(), Some(""));
    assert_eq!(open_agent_paths(&h).await, ["junk2.txt"]);
    assert_eq!(entries(&h).await.len(), 2);
}

/// A worker's tracked file that grew past `max_file_bytes` is one the follow's read leaves out, keeping its recorded entry without seeing it: the follow keeps the worker's mark, Needs you keeps the item, and the restore still finds it and refuses the file it cannot stash, so the mark stays open until the file is readable again and goes back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_follow_keeps_the_mark_of_a_workers_file_over_the_size_limit() {
    let mut h = Harness::start_plain().await;
    h.connect_tagged("main", &["main"]).await;
    let dir = PathBuf::from(h.root());
    let config = dir.join(".fold/config.toml");
    let kept = std::fs::read_to_string(&config).unwrap_or_default();
    write(&dir, "README.md", &"x".repeat(100));
    write(&dir, "junk.txt", "");
    place_agent_change(&h, "README.md").await;
    place_agent_change(&h, "junk.txt").await;
    std::fs::write(&config, format!("max_file_bytes = 32\n{kept}")).unwrap();

    fold_daemon::services::projects::sync_folder(
        &fold_daemon::services::projects::RemoteContext::of(&h.handle),
        &Git::default(),
    )
    .await
    .unwrap();
    assert_eq!(open_agent_paths(&h).await, ["README.md", "junk.txt"]);
    let waiting = needs_you(&h.handle.store).await.unwrap();
    assert!(
        waiting
            .iter()
            .any(|item| item.kind == NeedsYouKind::AgentChange),
        "{waiting:?}"
    );

    let (reason, paths) = refusal(
        restore_agent_changes(&h.handle, &ana(), &Git::default())
            .await
            .unwrap_err(),
    );
    assert_eq!(reason, "skipped_tracked");
    assert_eq!(paths, json!(["README.md"]));
    assert!(entries(&h).await.is_empty());
    assert_eq!(read(&dir, "junk.txt").as_deref(), Some(""));
    assert_eq!(open_agent_paths(&h).await, ["README.md", "junk.txt"]);

    std::fs::write(&config, kept).unwrap();
    let done = restore_agent_changes(&h.handle, &ana(), &Git::default())
        .await
        .unwrap();
    assert_eq!(done.files, ["README.md", "junk.txt"]);
    assert_eq!(read(&dir, "README.md").as_deref(), Some("# demo\n"));
    assert!(open_agent_paths(&h).await.is_empty());
}
