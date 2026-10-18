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

/// Waits until the Git job notes that the folder waits on the human's changes to `paths`, or that it waits on nothing for an empty `paths`.
async fn held_on(h: &Harness, paths: &[&str]) {
    eventually(WAIT, async || {
        let held = h.handle.git_attention.folder_held().map(|(_, held)| held);
        let expected = (!paths.is_empty()).then(|| paths.iter().map(ToString::to_string).collect());
        if held == expected { Ok(()) } else { Err(held) }
    })
    .await;
}

/// Every file of the project folder with its text, leaving out fold's and Git's own folders and the `.gitignore` that Git sync writes.
fn folder_files(dir: &Path) -> std::collections::BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, files: &mut std::collections::BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            if [".fold", ".git", ".gitignore"].contains(&name.as_str()) {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                files.insert(name, std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    walk(dir, dir, &mut files);
    files
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_change_on_a_path_main_changed_holds_the_whole_folder_until_it_is_taken_back() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    write(&dir, "docs/own.md", "mine\n");
    let revision = integrate(
        &h,
        "frontend",
        &[("README.md", "# fold\n"), ("src/new.rs", "fn new() {}\n")],
    )
    .await;
    held_on(&h, &["README.md"]).await;
    // Nothing of main's is written while one path main changed holds the human's change.
    assert_eq!(checkout_revision(&h).await, "r1");
    assert_eq!(read(&dir, "README.md"), "# mine\n");
    assert!(!dir.join("src/new.rs").exists());
    let synced = sync(&h).await;
    assert_eq!(synced.followed.at, None);
    assert_eq!(synced.followed.held, ["README.md"]);
    assert!(synced.followed.unrecorded);
    assert_eq!(synced.attention(), [ProjectAttention::UncommittedChanges]);
    assert!(!dir.join(".git").exists());

    // The human takes main's text back and removes their own file, so a record finds nothing to record and the folder catches up at once.
    write(&dir, "README.md", "# fold\n");
    std::fs::remove_file(dir.join("docs/own.md")).unwrap();
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::UpToDate { .. }
    ));
    assert_eq!(h.handle.git_attention.folder_held(), None);
    // A new file of the human's on a path main did not change neither holds the folder nor is touched.
    write(&dir, "docs/own.md", "mine\n");
    let synced = sync(&h).await;
    assert_eq!(
        synced.followed.at.map(|at| at.to_string()),
        Some(revision.clone())
    );
    assert!(synced.followed.held.is_empty());
    assert!(synced.followed.unrecorded, "docs/own.md is not recorded");
    assert_eq!(checkout_revision(&h).await, revision);
    assert_eq!(read(&dir, "src/new.rs"), "fn new() {}\n");
    assert_eq!(read(&dir, "docs/own.md"), "mine\n");
}

/// A human change on a path main changes, one on a path main does not change and a new file of the human's, then `fld record`: what the folder holds at each step, which is the same with Git sync on or off.
async fn a_held_folder_catches_up_after_the_record(
    git_sync: bool,
) -> Vec<std::collections::BTreeMap<String, String>> {
    let h = if git_sync {
        let h = Harness::start().await;
        branch_born(&h).await;
        h
    } else {
        Harness::start_plain().await
    };
    let dir = root(&h);
    let mut steps = Vec::new();
    integrate(&h, "frontend", &[("plan.md", "a\nb\nc\nd\ne\n")]).await;
    eventually(WAIT, async || {
        if dir.join("plan.md").is_file() {
            Ok(())
        } else {
            Err("the folder follows main")
        }
    })
    .await;
    held_on(&h, &[]).await;
    write(&dir, "plan.md", "A\nb\nc\nd\ne\n");
    write(&dir, "README.md", "# mine\n");
    write(&dir, "notes.md", "draft\n");
    let readme = std::fs::metadata(dir.join("README.md"))
        .unwrap()
        .modified()
        .unwrap();
    integrate(
        &h,
        "frontend",
        &[
            ("plan.md", "a\nb\nc\nd\nE\n"),
            ("src/new.rs", "fn new() {}\n"),
        ],
    )
    .await;
    held_on(&h, &["plan.md"]).await;
    // A later integration while the folder waits reaches it neither.
    let newest = integrate(&h, "backend", &[("src/two.rs", "fn two() {}\n")]).await;
    let synced = sync(&h).await;
    assert_eq!(synced.followed.at, None);
    assert_eq!(synced.followed.held, ["plan.md"]);
    assert!(!dir.join("src/new.rs").exists());
    assert!(!dir.join("src/two.rs").exists());
    steps.push(folder_files(&dir));

    // Needs you names the paths that hold the folder back and the command that lets it go on.
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
        .find(|item| item["kind"] == "folder_held")
        .unwrap();
    assert_eq!(item["paths"], serde_json::json!(["plan.md"]), "{item}");
    assert!(
        item["reason"].as_str().unwrap().contains("fld record"),
        "{item}"
    );
    assert_eq!(item["screen"], "repository");
    // The repository view tells the revision the folder holds from main's head.
    let (status, state): (_, serde_json::Value) = h
        .http(
            axum::http::Method::GET,
            "/api/v1/projects/demo/repository",
            None,
            "ana",
        )
        .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(
        state["folder"].as_str(),
        Some(checkout_revision(&h).await.as_str())
    );
    assert_ne!(state["folder"], state["head"], "{state}");

    let told = uncommitted_events(&h).await.len();
    let RecordOutcome::Recorded { files, .. } = record_as_ana(&h).await.unwrap() else {
        panic!("a clean record");
    };
    assert_eq!(files, ["README.md", "notes.md", "plan.md"]);
    // The clean record ends the attention itself: Needs you drops the folder, and no event says that changes are not recorded.
    assert_eq!(h.handle.git_attention.folder_held(), None);
    assert!(RemoteContext::of(&h.handle).attention().is_empty());
    assert_eq!(
        cleared_kinds(&h).await.last().map(String::as_str),
        Some("uncommitted_changes")
    );
    assert_eq!(uncommitted_events(&h).await.len(), told);
    let (status, body): (_, serde_json::Value) = h
        .http(
            axum::http::Method::GET,
            "/api/v1/projects/demo/needs-you",
            None,
            "ana",
        )
        .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["kind"] != "folder_held"),
        "{body}"
    );
    eventually(WAIT, async || {
        if dir.join("src/new.rs").is_file() {
            Ok(())
        } else {
            Err("the folder follows main after the record")
        }
    })
    .await;
    held_on(&h, &[]).await;
    // The folder catches up to the newest main in one step.
    assert_eq!(read(&dir, "src/two.rs"), "fn two() {}\n");
    let head = fold_store::workspaces::head(h.handle.store.read())
        .await
        .unwrap()
        .unwrap()
        .number
        .to_string();
    assert_eq!(checkout_revision(&h).await, head);
    assert_ne!(head, newest);
    // A path main never changed is never written.
    assert_eq!(
        std::fs::metadata(dir.join("README.md"))
            .unwrap()
            .modified()
            .unwrap(),
        readme
    );
    if git_sync {
        assert_eq!(git(&dir, &["status", "--porcelain"]), "");
    }
    // The Git job's run after the record's integration finds nothing new to tell.
    assert_eq!(uncommitted_events(&h).await.len(), told);
    steps.push(folder_files(&dir));
    steps
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_folder_is_the_same_with_and_without_git_sync() {
    let plain = a_held_folder_catches_up_after_the_record(false).await;
    assert_eq!(plain[0]["plan.md"], "A\nb\nc\nd\ne\n");
    assert_eq!(plain[1]["plan.md"], "A\nb\nc\nd\nE\n");
    assert_eq!(plain[1]["README.md"], "# mine\n");
    assert_eq!(plain[1]["src/new.rs"], "fn new() {}\n");
    assert_eq!(a_held_folder_catches_up_after_the_record(true).await, plain);
}

/// A change on a path main changes holds the folder; the human then takes it back by hand, with no record, integration or restart. What the folder holds once it caught up, which is the same with Git sync on or off.
async fn a_held_change_taken_back_by_hand(
    git_sync: bool,
) -> std::collections::BTreeMap<String, String> {
    let h = if git_sync {
        let h = Harness::start().await;
        branch_born(&h).await;
        h
    } else {
        Harness::start_plain().await
    };
    let dir = root(&h);
    integrate(&h, "frontend", &[("plan.md", "a\nb\nc\nd\ne\n")]).await;
    eventually(WAIT, async || {
        if dir.join("plan.md").is_file() {
            Ok(())
        } else {
            Err("the folder follows main")
        }
    })
    .await;
    write(&dir, "plan.md", "A\nb\nc\nd\ne\n");
    integrate(
        &h,
        "frontend",
        &[
            ("plan.md", "a\nb\nc\nd\nE\n"),
            ("src/new.rs", "fn new() {}\n"),
        ],
    )
    .await;
    held_on(&h, &["plan.md"]).await;
    assert!(!dir.join("src/new.rs").exists());

    write(&dir, "plan.md", "a\nb\nc\nd\ne\n");
    held_on(&h, &[]).await;
    eventually(WAIT, async || {
        let attention = RemoteContext::of(&h.handle).attention();
        if dir.join("src/new.rs").is_file() && attention.is_empty() {
            Ok(())
        } else {
            Err(attention)
        }
    })
    .await;
    assert_eq!(
        checkout_revision(&h).await,
        fold_store::workspaces::head(h.handle.store.read())
            .await
            .unwrap()
            .unwrap()
            .number
            .to_string()
    );
    if git_sync {
        assert_eq!(git(&dir, &["status", "--porcelain"]), "");
    }
    folder_files(&dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_change_taken_back_by_hand_lets_the_folder_catch_up_with_and_without_git_sync() {
    let plain = a_held_change_taken_back_by_hand(false).await;
    assert_eq!(plain["plan.md"], "a\nb\nc\nd\nE\n");
    assert_eq!(plain["src/new.rs"], "fn new() {}\n");
    assert_eq!(a_held_change_taken_back_by_hand(true).await, plain);
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
    assert!(synced.followed.held.is_empty());
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

/// While another branch is checked out, the folder holds that branch's files, which Git puts back when the branch returns: a file of it that matches main's tells nothing of what fold wrote.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_of_another_branch_that_matches_main_is_written_once_the_branch_is_back() {
    let h = Harness::start().await;
    branch_born(&h).await;
    let dir = root(&h);
    git(&dir, &["checkout", "--quiet", "-b", "mine"]);
    write(&dir, "plan.md", "agent\n");
    git(&dir, &["add", "plan.md"]);
    git(&dir, &["commit", "--quiet", "-m", "plan"]);
    let revision = integrate(
        &h,
        "frontend",
        &[("plan.md", "agent\n"), ("src.txt", "agent\n")],
    )
    .await;
    let synced = sync(&h).await;
    assert!(synced.moved().is_some());
    assert!(synced.followed.held.is_empty());

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
    assert_eq!(read(&dir, "plan.md"), "agent\n");
    assert_eq!(git(&dir, &["status", "--porcelain"]), "");
    assert!(!sync(&h).await.followed.unrecorded);
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

/// The stored `git.uncommitted_changes` events, oldest first, with their data.
async fn uncommitted_events(h: &Harness) -> Vec<serde_json::Value> {
    let filter = fold_core::EventFilter {
        types: vec![fold_core::events::types::GIT_UNCOMMITTED_CHANGES],
        ..fold_core::EventFilter::default()
    };
    fold_store::events::read_after(h.handle.store.read(), 0, &filter, 50)
        .await
        .unwrap()
        .into_iter()
        .map(|event| event.data)
        .collect()
}

/// The kinds that the stored `git.attention_cleared` events name, oldest first.
async fn cleared_kinds(h: &Harness) -> Vec<String> {
    let filter = fold_core::EventFilter {
        types: vec![fold_core::events::types::GIT_ATTENTION_CLEARED],
        ..fold_core::EventFilter::default()
    };
    fold_store::events::read_after(h.handle.store.read(), 0, &filter, 50)
        .await
        .unwrap()
        .into_iter()
        .map(|event| event.data["kind"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_change_taken_back_while_another_file_waits_tells_needs_you_once() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    write(&dir, "docs/own.md", "mine\n");
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    held_on(&h, &["README.md"]).await;
    eventually(WAIT, async || {
        match uncommitted_events(&h).await.as_slice() {
            [raised] => Ok(raised.clone()),
            other => Err(other.len()),
        }
    })
    .await;
    assert_eq!(
        uncommitted_events(&h).await[0]["held"],
        serde_json::json!(["README.md"])
    );

    // The human takes the held change back; docs/own.md stays unrecorded, so the attention stays and only the held set changes.
    write(&dir, "README.md", "# fold\n");
    integrate(&h, "backend", &[("src/new.rs", "fn new() {}\n")]).await;
    held_on(&h, &[]).await;
    eventually(WAIT, async || {
        let events = uncommitted_events(&h).await;
        if events.len() == 2 {
            Ok(())
        } else {
            Err(events)
        }
    })
    .await;
    let events = uncommitted_events(&h).await;
    assert_eq!(events[1]["held"], serde_json::json!([]), "{events:?}");
    assert_eq!(events[1]["held_count"], 0);
    assert_eq!(
        RemoteContext::of(&h.handle).attention(),
        [ProjectAttention::UncommittedChanges]
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
    assert!(
        body["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["kind"] != "folder_held"),
        "{body}"
    );

    // A quiet folder writes no more events.
    integrate(&h, "backend", &[("src/more.rs", "fn more() {}\n")]).await;
    eventually(WAIT, async || {
        if dir.join("src/more.rs").is_file() {
            Ok(())
        } else {
            Err("the folder follows main")
        }
    })
    .await;
    assert_eq!(read(&dir, "docs/own.md"), "mine\n");
    assert_eq!(uncommitted_events(&h).await.len(), 2);
}
