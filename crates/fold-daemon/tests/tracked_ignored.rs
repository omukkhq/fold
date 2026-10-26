#![cfg(test)]

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use fold_core::events::types;
use fold_core::{Actor, EventFilter, HumanName, codes};
use fold_daemon::services::projects::{
    AfterUnstash, FolderSync, ProjectAttention, ProjectError, RecordOutcome, RemoteContext,
    UntrackOutcome, sync_folder, untrack,
};
use fold_store::testing::eventually;
use fold_vcs::git::Git;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, WAIT};
use crate::folder_stash::wait_for;
use crate::human_changes::{branch_born, file_of, git, integrate, record_as_ana};

fn root(h: &Harness) -> PathBuf {
    PathBuf::from(h.root())
}

fn write(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

async fn status(h: &Harness) -> Value {
    let (status, body) = h
        .socket(Method::GET, "/api/v1/projects/demo/folder", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn sync(h: &Harness) -> FolderSync {
    sync_folder(&RemoteContext::of(&h.handle), &Git::default())
        .await
        .unwrap()
}

/// Main holds `app.env` and `build/version.txt`, then an agent's change adds the rules that match both and edits `app.env` in the same proposal; the folder follows main.
async fn tracked_then_ignored(h: &Harness) -> String {
    integrate(
        h,
        "frontend",
        &[("app.env", "KEY=1\n"), ("build/version.txt", "1.0\n")],
    )
    .await;
    let revision = integrate(
        h,
        "frontend",
        &[(".foldignore", "*.env\nbuild/\n"), ("app.env", "KEY=2\n")],
    )
    .await;
    wait_for(&root(h), ".foldignore", "*.env\nbuild/\n").await;
    wait_for(&root(h), "app.env", "KEY=2\n").await;
    revision
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_checkpoint_of_an_agent_keeps_a_tracked_file_that_a_new_rule_matches() {
    let h = Harness::start_plain().await;
    let revision = tracked_then_ignored(&h).await;
    // The proposal changed the tracked file and did not delete it, although its own new rule matches it.
    assert_eq!(
        file_of(&h, &revision, "app.env").await.as_deref(),
        Some("KEY=2\n")
    );
    assert_eq!(
        file_of(&h, &revision, "build/version.txt").await.as_deref(),
        Some("1.0\n")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn record_status_and_needs_you_agree_on_a_tracked_file_the_rules_match() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    tracked_then_ignored(&h).await;
    write(&dir, "build/out.bin", "generated\n");
    write(&dir, "local.env", "SECRET=1\n");
    // Untracked files the rules match count nowhere; the tracked ones stay as main has them.
    assert!(!sync(&h).await.followed.unrecorded);
    let body = status(&h).await;
    assert_eq!(body["changes"], json!([]));
    assert_eq!(
        body["ignored_tracked"],
        json!(["app.env", "build/version.txt"])
    );
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::UpToDate { .. }
    ));

    write(&dir, "app.env", "KEY=3\n");
    write(&dir, "build/version.txt", "2.0\n");
    let synced = sync(&h).await;
    assert!(synced.followed.unrecorded);
    assert_eq!(synced.attention(), [ProjectAttention::UncommittedChanges]);
    assert_eq!(
        status(&h).await["changes"],
        json!([
            {"path": "app.env", "kind": "modified", "entry": "file", "held": false},
            {"path": "build/version.txt", "kind": "modified", "entry": "file", "held": false},
        ])
    );
    let RecordOutcome::Recorded {
        revision, files, ..
    } = record_as_ana(&h).await.unwrap()
    else {
        panic!("a record");
    };
    assert_eq!(files, ["app.env", "build/version.txt"]);
    let revision = revision.to_string();
    assert_eq!(
        file_of(&h, &revision, "app.env").await.as_deref(),
        Some("KEY=3\n")
    );
    assert_eq!(file_of(&h, &revision, "local.env").await, None);
    assert_eq!(file_of(&h, &revision, "build/out.bin").await, None);

    // A tracked file deleted from the folder is a deletion, whatever the rules say.
    std::fs::remove_file(dir.join("app.env")).unwrap();
    assert!(sync(&h).await.followed.unrecorded);
    assert_eq!(
        status(&h).await["changes"],
        json!([{"path": "app.env", "kind": "deleted", "entry": "file", "held": false}])
    );
    let RecordOutcome::Recorded { revision, .. } = record_as_ana(&h).await.unwrap() else {
        panic!("a record");
    };
    assert_eq!(file_of(&h, &revision.to_string(), "app.env").await, None);
    // Once main no longer tracks it, the rules leave the file out again.
    write(&dir, "app.env", "KEY=4\n");
    assert!(!sync(&h).await.followed.unrecorded);
    let body = status(&h).await;
    assert_eq!(body["changes"], json!([]));
    assert_eq!(body["ignored_tracked"], json!(["build/version.txt"]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tracked_file_over_the_size_limit_keeps_its_recorded_version() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    let config = dir.join(".fold/config.toml");
    let kept = std::fs::read_to_string(&config).unwrap_or_default();
    std::fs::write(&config, format!("max_file_bytes = 32\n{kept}")).unwrap();
    let large = "x".repeat(100);
    write(&dir, "README.md", &large);
    assert!(!sync(&h).await.followed.unrecorded);
    let body = status(&h).await;
    assert_eq!(body["changes"], json!([]));
    assert_eq!(
        body["skipped"],
        json!([{"path": "README.md", "reason": "too_large", "tracked": true}])
    );
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::UpToDate { .. }
    ));
    // Another change records alone, and main keeps the version of the file it had.
    write(&dir, "notes.md", "notes\n");
    let RecordOutcome::Recorded {
        revision,
        files,
        skipped,
        ..
    } = record_as_ana(&h).await.unwrap()
    else {
        panic!("a record");
    };
    assert_eq!(files, ["notes.md"]);
    assert_eq!(skipped.len(), 1);
    assert_eq!(
        file_of(&h, &revision.to_string(), "README.md")
            .await
            .as_deref(),
        Some("# demo\n")
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("README.md")).unwrap(),
        large
    );
}

async fn untrack_as_ana(h: &Harness, paths: &[&str]) -> Result<UntrackOutcome, ProjectError> {
    let paths: Vec<String> = paths.iter().map(|path| (*path).to_owned()).collect();
    let AfterUnstash { outcome, .. } = untrack(
        &h.handle,
        &Actor::human(HumanName::parse("ana").unwrap()),
        &Git::default(),
        &paths,
    )
    .await?;
    Ok(outcome)
}

fn read(dir: &Path, path: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(path)).ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fld_untrack_removes_tracked_files_from_main_and_leaves_them_and_other_changes_in_the_folder()
 {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    tracked_then_ignored(&h).await;
    write(&dir, "app.env", "KEY=3\n");
    write(&dir, "README.md", "# mine\n");
    write(&dir, "build/out.bin", "generated\n");

    let outcome = untrack_as_ana(&h, &["app.env", "build/"]).await.unwrap();
    assert_eq!(outcome.files, ["app.env", "build/version.txt"]);
    assert!(outcome.unignored.is_empty());
    assert_eq!(outcome.commit, None);
    let revision = outcome.revision.to_string();
    assert_eq!(file_of(&h, &revision, "app.env").await, None);
    assert_eq!(file_of(&h, &revision, "build/version.txt").await, None);
    assert_eq!(
        file_of(&h, &revision, "README.md").await.as_deref(),
        Some("# demo\n")
    );
    // The files stay as the human left them, and the other change is still not recorded.
    assert_eq!(read(&dir, "app.env").as_deref(), Some("KEY=3\n"));
    assert_eq!(read(&dir, "build/version.txt").as_deref(), Some("1.0\n"));
    assert_eq!(read(&dir, "build/out.bin").as_deref(), Some("generated\n"));
    let body = status(&h).await;
    assert_eq!(body["head"], revision.as_str());
    assert_eq!(body["at_main"], true);
    assert_eq!(
        body["changes"],
        json!([{"path": "README.md", "kind": "modified", "entry": "file", "held": false}])
    );
    assert_eq!(body["ignored_tracked"], json!([]));
    let filter = EventFilter {
        types: vec![types::HUMAN_CHANGE_RECORDED],
        ..EventFilter::default()
    };
    let recorded = fold_store::events::read_after(h.handle.store.read(), 0, &filter, 10)
        .await
        .unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].actor.as_str(), "human:ana");
    // A later record takes only the other change: the untracked files stay out of main.
    let RecordOutcome::Recorded { files, .. } = record_as_ana(&h).await.unwrap() else {
        panic!("a record");
    };
    assert_eq!(files, ["README.md"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fld_untrack_refuses_a_path_main_does_not_track_and_names_a_file_no_rule_matches() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    write(&dir, "notes.md", "notes\n");
    record_as_ana(&h).await.unwrap();
    let refused = untrack_as_ana(&h, &["missing.md", "notes.md"])
        .await
        .unwrap_err();
    assert_eq!(refused.code(), Some(codes::PATH_NOT_TRACKED));
    let ProjectError::Fold(error) = &refused else {
        panic!("a fold error: {refused:?}");
    };
    assert_eq!(error.details().unwrap()["paths"], json!(["missing.md"]));
    assert!(error.hint().is_some());
    for bad in [&[][..], &["../outside"][..], &["/etc"][..]] {
        let refused = untrack_as_ana(&h, bad).await.unwrap_err();
        assert!(
            matches!(
                refused.code(),
                Some(codes::INVALID_REQUEST | codes::INVALID_PATH)
            ),
            "{bad:?}: {refused:?}"
        );
    }
    let refused = untrack(
        &h.handle,
        &Actor::System,
        &Git::default(),
        &["notes.md".to_owned()],
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code(), Some(codes::HUMAN_REQUIRED));

    // A file no rule matches leaves main, and the answer says that the next record takes it again.
    let outcome = untrack_as_ana(&h, &["notes.md"]).await.unwrap();
    assert_eq!(outcome.unignored, ["notes.md"]);
    assert_eq!(read(&dir, "notes.md").as_deref(), Some("notes\n"));
    assert_eq!(
        status(&h).await["changes"],
        json!([{"path": "notes.md", "kind": "added", "entry": "file", "held": false}])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fld_untrack_is_refused_where_main_changed_the_path_since_the_folder_revision() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    integrate(&h, "frontend", &[("docs.md", "a\n")]).await;
    wait_for(&dir, "docs.md", "a\n").await;
    write(&dir, "docs.md", "mine\n");
    integrate(&h, "frontend", &[("docs.md", "main\n")]).await;
    eventually(WAIT, async || match h.handle.git_attention.folder_held() {
        Some((_, held)) if held == ["docs.md"] => Ok(()),
        other => Err(other),
    })
    .await;
    let head = h.head().await.number;
    let refused = untrack_as_ana(&h, &["docs.md"]).await.unwrap_err();
    let ProjectError::Fold(error) = &refused else {
        panic!("a fold error: {refused:?}");
    };
    assert_eq!(error.code(), codes::INVALID_REQUEST);
    assert_eq!(error.details().unwrap()["reason"], "main_changed");
    assert_eq!(error.details().unwrap()["paths"], json!(["docs.md"]));
    assert_eq!(h.head().await.number, head);
    assert_eq!(read(&dir, "docs.md").as_deref(), Some("mine\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fld_untrack_with_git_sync_deletes_the_file_from_the_branch_and_the_index_only() {
    let h = Harness::start().await;
    branch_born(&h).await;
    let dir = root(&h);
    write(&dir, "secret.env", "TOKEN=1\n");
    record_as_ana(&h).await.unwrap();
    write(&dir, ".foldignore", "*.env\n");
    assert_eq!(status(&h).await["ignored_tracked"], json!(["secret.env"]));

    let outcome = untrack_as_ana(&h, &["secret.env"]).await.unwrap();
    let commit = outcome.commit.unwrap();
    let listed = git(&dir, &["ls-tree", "-r", "--name-only", commit.as_str()]);
    assert!(!listed.lines().any(|path| path == "secret.env"), "{listed}");
    assert_eq!(git(&dir, &["ls-files", "secret.env"]), "");
    assert_eq!(read(&dir, "secret.env").as_deref(), Some("TOKEN=1\n"));
    // The .foldignore line is a change of the folder that no revision records yet.
    let body = status(&h).await;
    assert_eq!(
        body["changes"],
        json!([{"path": ".foldignore", "kind": "added", "entry": "file", "held": false}])
    );
    assert_eq!(body["ignored_tracked"], json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_untrack_route_answers_for_the_named_human() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    write(&dir, "notes.md", "notes\n");
    record_as_ana(&h).await.unwrap();
    let route = "/api/v1/projects/demo/untrack";
    let (status, body) = h
        .socket(Method::POST, route, Some(&json!({"paths": ["gone.md"]})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"]["code"], "path_not_tracked");
    let (status, body) = h
        .socket(Method::POST, route, Some(&json!({"paths": ["notes.md"]})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({
            "revision": "r3", "commit": null, "files": ["notes.md"], "unignored": ["notes.md"],
            "touched": [], "unstash": null,
        })
    );
}
