#![cfg(test)]

use std::path::{Path, PathBuf};

use fold_daemon::services::agents::{NeedsYouKind, needs_you};
use fold_daemon::services::projects::{
    ProjectAttention, ProjectError, RecordOutcome, RemoteContext, sync_folder,
};
use fold_store::testing::eventually;
use fold_vcs::git::Git;

use crate::common::acceptance::{Harness, WAIT};
use crate::human_changes::{branch_born, git, integrate, record_as_ana};

fn root(h: &Harness) -> PathBuf {
    PathBuf::from(h.root())
}

fn read(dir: &Path, path: &str) -> String {
    std::fs::read_to_string(dir.join(path)).unwrap()
}

fn write(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

async fn checkout_revision(h: &Harness) -> String {
    fold_store::workspaces::checkout(h.handle.store.read())
        .await
        .unwrap()
        .unwrap()
        .revision
        .to_string()
}

/// One run of the project's Git job, as an integration or a change of the branch starts it.
async fn sync(h: &Harness) -> fold_daemon::services::projects::FolderSync {
    let context = RemoteContext::of(&h.handle);
    sync_folder(&context, &Git::default()).await.unwrap()
}

fn head_moved_reason(error: ProjectError) -> String {
    let ProjectError::Fold(error) = error else {
        panic!("a fold error");
    };
    assert_eq!(error.details().unwrap()["reason"], "head_moved");
    error.details().unwrap()["moved"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plain_folder_follows_main_and_keeps_the_files_a_human_changed() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    let revision = integrate(
        &h,
        "frontend",
        &[("README.md", "# fold\n"), ("src/new.rs", "fn new() {}\n")],
    )
    .await;
    eventually(WAIT, async || {
        let at = checkout_revision(&h).await;
        if at == revision { Ok(()) } else { Err(at) }
    })
    .await;
    // The human's file stays theirs; main's other file is written.
    assert_eq!(read(&dir, "README.md"), "# mine\n");
    assert_eq!(read(&dir, "src/new.rs"), "fn new() {}\n");
    let synced = sync(&h).await;
    assert_eq!(synced.followed.kept, ["README.md"]);
    assert!(synced.followed.unrecorded);
    assert_eq!(synced.attention(), [ProjectAttention::UncommittedChanges]);
    assert!(!dir.join(".git").exists());

    // The human takes main's text; the record finds nothing new and the attention clears.
    write(&dir, "README.md", "# fold\n");
    let synced = sync(&h).await;
    assert!(synced.followed.kept.is_empty() && !synced.followed.unrecorded);
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::UpToDate { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_a_human_added_or_removed_is_theirs_and_main_removes_its_own_files() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    integrate(
        &h,
        "frontend",
        &[("docs/a.md", "a\n"), ("docs/b.md", "b\n")],
    )
    .await;
    eventually(WAIT, async || {
        if dir.join("docs/b.md").is_file() {
            Ok(())
        } else {
            Err("the folder follows main")
        }
    })
    .await;
    std::fs::remove_file(dir.join("docs/a.md")).unwrap();
    write(&dir, "notes.md", "mine\n");
    // An agent removes both files of `docs/`.
    let task = h.create_task("backend", "Remove the docs").await;
    let work = h.start_work("backend", &task).await;
    std::fs::remove_dir_all(work.dir.join("docs")).unwrap();
    let proposal = h.propose("backend", &work, "Remove the docs").await;
    h.approve(&proposal).await;
    h.call(
        "review",
        "integrate_proposal",
        serde_json::json!({"id": proposal, "expected_version": 1}),
    )
    .await;
    eventually(WAIT, async || {
        if dir.join("docs").exists() {
            Err("main removes its files")
        } else {
            Ok(())
        }
    })
    .await;
    assert_eq!(read(&dir, "notes.md"), "mine\n");
    let synced = sync(&h).await;
    assert!(synced.followed.kept.is_empty());
    assert!(synced.followed.unrecorded, "notes.md is not recorded");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_branch_checked_out_stops_the_folder_until_the_branch_is_back() {
    let h = Harness::start().await;
    branch_born(&h).await;
    let dir = root(&h);
    git(&dir, &["checkout", "--quiet", "-b", "mine"]);
    let synced = sync(&h).await;
    assert_eq!(synced.attention(), [ProjectAttention::HeadMoved]);
    eventually(WAIT, async || match h.handle.git_attention.head_moved() {
        Some((_, hint)) if hint.contains("git switch main") => Ok(()),
        other => Err(other),
    })
    .await;
    let items = needs_you(&h.handle.store).await.unwrap();
    assert!(
        items
            .iter()
            .all(|item| item.kind != NeedsYouKind::HeadMoved)
    );
    let (status, body): (_, serde_json::Value) = h
        .http(
            axum::http::Method::GET,
            "/api/v1/projects/demo/needs-you",
            None,
            "ana",
        )
        .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let item = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "head_moved")
        .unwrap();
    assert!(
        item["reason"].as_str().unwrap().contains("git switch main"),
        "{item}"
    );
    assert_eq!(item["screen"], "repository");

    // An integration meanwhile reaches neither the folder nor the branch, and fld record refuses.
    let revision = integrate(&h, "frontend", &[("src.txt", "agent\n")]).await;
    let synced = sync(&h).await;
    assert_eq!(synced.followed.at, None);
    assert!(!dir.join("src.txt").exists());
    assert_eq!(
        head_moved_reason(record_as_ana(&h).await.unwrap_err()),
        "other_branch"
    );

    git(&dir, &["switch", "--quiet", "main"]);
    eventually(WAIT, async || {
        if dir.join("src.txt").is_file() && h.handle.git_attention.head_moved().is_none() {
            Ok(())
        } else {
            Err("the folder follows main once the branch is back")
        }
    })
    .await;
    assert_eq!(checkout_revision(&h).await, revision);
    assert_eq!(git(&dir, &["status", "--porcelain"]), "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stash_or_a_reset_fold_did_not_make_stops_the_folder_until_it_is_undone() {
    let h = Harness::start().await;
    branch_born(&h).await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    git(&dir, &["stash", "--quiet"]);
    assert_eq!(
        head_moved_reason(record_as_ana(&h).await.unwrap_err()),
        "stash"
    );
    assert!(
        sync(&h)
            .await
            .moved()
            .unwrap()
            .hint("main")
            .contains("git stash pop")
    );
    git(&dir, &["stash", "pop", "--quiet"]);
    assert!(sync(&h).await.moved().is_none());
    // A stash dropped instead of popped leaves no stash, which is no change of the folder either.
    git(&dir, &["stash", "--quiet"]);
    assert!(sync(&h).await.moved().is_some());
    git(&dir, &["stash", "drop", "--quiet"]);
    assert!(sync(&h).await.moved().is_none());
    write(&dir, "README.md", "# mine\n");

    // The human's change is recorded; then a reset to the commit before it moves the branch where fold did not.
    let RecordOutcome::Recorded { commit, .. } = record_as_ana(&h).await.unwrap() else {
        panic!("a clean record");
    };
    let fold_commit = commit.unwrap();
    git(&dir, &["reset", "--quiet", "--hard", "HEAD~1"]);
    assert_eq!(
        head_moved_reason(record_as_ana(&h).await.unwrap_err()),
        "branch_moved"
    );
    let synced = sync(&h).await;
    let hint = synced.moved().unwrap().hint("main");
    assert!(
        hint.contains(&format!("git reset --soft {}", &fold_commit.as_str()[..12])),
        "{hint}"
    );
    // The hint brings the branch back and keeps the files: the reset's files are a change of the human's again.
    git(&dir, &["reset", "--quiet", "--soft", fold_commit.as_str()]);
    let synced = sync(&h).await;
    assert!(synced.moved().is_none());
    assert!(synced.followed.unrecorded);
    assert_eq!(read(&dir, "README.md"), "# demo\n");
}

/// `git stash` in `dir` with its commits dated `date`.
fn stash_at(dir: &Path, date: &str) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.hooksPath=/dev/null", "stash", "--quiet"])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Ana")
        .env("GIT_AUTHOR_EMAIL", "ana@example.com")
        .env("GIT_COMMITTER_NAME", "Ana")
        .env("GIT_COMMITTER_EMAIL", "ana@example.com")
        .env("GIT_COMMITTER_DATE", date)
        .env("GIT_AUTHOR_DATE", date)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_stash_of_the_older_ones_is_no_change_and_a_new_one_is() {
    let h = Harness::start().await;
    branch_born(&h).await;
    let dir = root(&h);
    // Two stashes the human had when fold turned Git sync on.
    write(&dir, "README.md", "# first\n");
    stash_at(&dir, "2020-01-01T00:00:00Z");
    write(&dir, "README.md", "# second\n");
    stash_at(&dir, "2020-01-02T00:00:00Z");
    git(&dir, &["update-ref", "refs/fold/stash", "refs/stash"]);
    assert!(sync(&h).await.moved().is_none());
    // Dropping the newest leaves the older one on top, which fold saw already.
    git(&dir, &["stash", "drop", "--quiet"]);
    assert!(sync(&h).await.moved().is_none());
    write(&dir, "README.md", "# third\n");
    git(&dir, &["stash", "--quiet"]);
    assert!(
        sync(&h)
            .await
            .moved()
            .unwrap()
            .hint("main")
            .contains("git stash pop")
    );
    git(&dir, &["stash", "pop", "--quiet"]);
    assert!(sync(&h).await.moved().is_none());
}

/// Main's head number as the project's read pool sees it.
async fn head_of(store: &fold_store::Store) -> String {
    fold_store::workspaces::head(store.read())
        .await
        .unwrap()
        .unwrap()
        .number
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_git_commit_made_while_the_daemon_was_down_is_recorded_once_the_handle_is_shared() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    let dir = root(&h);
    h.stop().await;
    // A lost map line, which the Git job's first run writes again, shows when that run is over.
    let map = dir.join(".fold/git-sync.map");
    let lines = std::fs::read_to_string(&map).unwrap();
    std::fs::write(&map, "").unwrap();
    write(&dir, "offline.md", "while down\n");
    git(&dir, &["add", "offline.md"]);
    git(&dir, &["commit", "--quiet", "-m", "offline"]);
    let row = fold_store::projects::ProjectRow {
        slug: "demo".to_owned(),
        name: "Demo".to_owned(),
        root: h.root().to_owned(),
    };
    let handle = fold_daemon::projects::ProjectHandle::open(
        &row,
        std::sync::Arc::new(h.project.clock().clone()),
    )
    .await
    .unwrap();
    eventually(WAIT, async || {
        let now = std::fs::read_to_string(&map).unwrap();
        if now == lines { Ok(()) } else { Err(now) }
    })
    .await;
    // That run had no shared handle, so the commit waits.
    assert_eq!(head_of(&handle.store).await, "r1");
    let handle = handle.share();
    eventually(WAIT, async || {
        let head = head_of(&handle.store).await;
        if head == "r2" { Ok(()) } else { Err(head) }
    })
    .await;
    handle.bus.close();
    handle.stop_tasks(WAIT).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_merge_commit_of_the_human_is_recorded_as_one_change_with_the_merge_as_its_commit() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    let dir = root(&h);
    h.stop().await;
    git(&dir, &["switch", "--quiet", "-c", "side"]);
    write(&dir, "side.md", "side\n");
    git(&dir, &["add", "side.md"]);
    git(&dir, &["commit", "--quiet", "-m", "side"]);
    git(&dir, &["switch", "--quiet", "main"]);
    write(&dir, "main.md", "main\n");
    git(&dir, &["add", "main.md"]);
    git(&dir, &["commit", "--quiet", "-m", "main"]);
    git(
        &dir,
        &["merge", "--quiet", "--no-ff", "side", "-m", "merge side"],
    );
    let merge = git(&dir, &["rev-parse", "HEAD"]).trim().to_owned();
    let parents = git(&dir, &["rev-list", "--parents", "-n", "1", "HEAD"]);
    assert_eq!(parents.split_whitespace().count(), 3, "{parents}");
    h.start_again().await;
    eventually(WAIT, async || {
        let head = h.head().await.number.to_string();
        if head == "r2" { Ok(()) } else { Err(head) }
    })
    .await;
    for (path, text) in [("side.md", "side\n"), ("main.md", "main\n")] {
        assert_eq!(
            crate::human_changes::file_of(&h, "r2", path)
                .await
                .as_deref(),
            Some(text)
        );
    }
    let (kind, remote): (String, Option<String>) =
        sqlx::query_as("SELECT kind, remote_commit FROM integrations ORDER BY number DESC LIMIT 1")
            .fetch_one(h.handle.store.read())
            .await
            .unwrap();
    assert_eq!(
        (kind.as_str(), remote.as_deref()),
        ("record", Some(merge.as_str()))
    );
    // Main did not move, so r2's commit is the merge itself and the branch stays on it.
    eventually(WAIT, async || {
        let mapped: Option<String> = sqlx::query_scalar(
            r#"SELECT c."commit" FROM revision_commits c JOIN revisions r ON r.id = c.revision_id WHERE r.number = 2"#,
        )
        .fetch_optional(h.handle.store.read())
        .await
        .unwrap();
        if mapped.as_deref() == Some(merge.as_str()) { Ok(()) } else { Err(mapped) }
    })
    .await;
    assert_eq!(git(&dir, &["rev-parse", "main"]).trim(), merge);
    assert_eq!(git(&dir, &["status", "--porcelain"]), "");
}
