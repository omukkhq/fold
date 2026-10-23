#![cfg(test)]

use std::path::{Path, PathBuf};
use std::process::Command;

use fold_core::events::types;
use fold_core::{Actor, EventFilter, HumanName};
use fold_daemon::services::agents::{NeedsYouKind, needs_you};
use fold_daemon::services::projects::{
    AfterUnstash, ProjectAttention, ProjectError, RecordOutcome, RemoteContext, StashEntry,
    StashOutcome, UnstashOutcome, stash, stash_list, sync_folder, unstash,
};
use fold_vcs::git::Git;
use serde_json::{Value, json};

use crate::common::acceptance::Harness;
use crate::human_changes::{branch_born, git, integrate, record_as_ana};

fn ana() -> Actor {
    Actor::human(HumanName::parse("ana").unwrap())
}

fn root(h: &Harness) -> PathBuf {
    PathBuf::from(h.root())
}

fn read(dir: &Path, path: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(path)).ok()
}

fn write(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

/// Waits until the folder's follow after an integration has written `text` at `path`.
pub async fn wait_for(dir: &Path, path: &str, text: &str) {
    fold_store::testing::eventually(crate::common::acceptance::WAIT, async || {
        let found = read(dir, path);
        if found.as_deref() == Some(text) {
            Ok(())
        } else {
            Err(found)
        }
    })
    .await;
}

pub async fn stash_as_ana(h: &Harness) -> Result<StashOutcome, ProjectError> {
    Ok(stash_finishing_as_ana(h).await?.outcome)
}

/// `fld stash` with the outcome of an unstash it finished first.
pub async fn stash_finishing_as_ana(
    h: &Harness,
) -> Result<AfterUnstash<StashOutcome>, ProjectError> {
    stash(&h.handle, &ana(), &Git::default()).await
}

pub async fn unstash_as_ana(h: &Harness) -> Result<UnstashOutcome, ProjectError> {
    unstash(&h.handle, &ana(), &Git::default()).await
}

pub async fn entries(h: &Harness) -> Vec<StashEntry> {
    stash_list(&h.handle).await.unwrap()
}

/// The files of each entry, newest first.
pub async fn stack_files(h: &Harness) -> Vec<Vec<String>> {
    entries(h)
        .await
        .into_iter()
        .map(|entry| entry.files)
        .collect()
}

/// The `reason` and `paths` of a refusal.
pub fn refusal(error: ProjectError) -> (String, Value) {
    let ProjectError::Fold(error) = error else {
        panic!("a fold error");
    };
    assert_eq!(error.code(), "invalid_request", "{error}");
    let details = error.details().unwrap();
    (
        details["reason"].as_str().unwrap().to_owned(),
        details.get("paths").cloned().unwrap_or(Value::Null),
    )
}

async fn sync(h: &Harness) -> fold_daemon::services::projects::FolderSync {
    sync_folder(&RemoteContext::of(&h.handle), &Git::default())
        .await
        .unwrap()
}

async fn stored(h: &Harness, kind: fold_core::EventType) -> Vec<fold_core::Event> {
    let filter = EventFilter {
        types: vec![kind],
        ..EventFilter::default()
    };
    fold_store::events::read_after(h.handle.store.read(), 0, &filter, 100)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stash_puts_the_changes_aside_and_an_unstash_brings_them_back() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    write(&dir, "docs/notes.md", "notes\n");
    let StashOutcome::Stashed {
        revision,
        files,
        entries: count,
        ..
    } = stash_as_ana(&h).await.unwrap()
    else {
        panic!("a stash");
    };
    assert_eq!(revision.to_string(), "r1");
    assert_eq!(files, ["README.md", "docs/notes.md"]);
    assert_eq!(count, 1);
    // The paths hold the record's entries again, and a new file goes with the directory it leaves empty.
    assert_eq!(read(&dir, "README.md").as_deref(), Some("# demo\n"));
    assert!(!dir.join("docs").exists());
    assert!(!sync(&h).await.followed.unrecorded);
    let listed = entries(&h).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].position, 0);
    assert_eq!(listed[0].base.to_string(), "r1");
    assert_eq!(listed[0].actor, "human:ana");
    assert!(listed[0].conflicted.is_empty());
    let pushed = stored(&h, types::STASH_PUSHED).await;
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].actor.as_str(), "human:ana");
    assert_eq!(
        pushed[0].data["files"],
        json!(["README.md", "docs/notes.md"])
    );
    assert_eq!(
        stash_as_ana(&h).await.unwrap(),
        StashOutcome::Nothing {
            revision: "r1".parse().unwrap(),
            entries: 1,
            skipped: Vec::new(),
        }
    );

    let UnstashOutcome::Applied {
        files,
        entries: left,
    } = unstash_as_ana(&h).await.unwrap()
    else {
        panic!("a clean unstash");
    };
    assert_eq!(files, ["README.md", "docs/notes.md"]);
    assert_eq!(left, 0);
    assert_eq!(read(&dir, "README.md").as_deref(), Some("# mine\n"));
    assert_eq!(read(&dir, "docs/notes.md").as_deref(), Some("notes\n"));
    assert!(entries(&h).await.is_empty());
    assert!(
        h.handle
            .git_attention
            .kinds()
            .contains(&ProjectAttention::UncommittedChanges)
    );
    assert_eq!(stored(&h, types::STASH_APPLIED).await.len(), 1);
    // The files are the human's unrecorded changes again, which fld record records.
    let synced = sync(&h).await;
    assert!(synced.followed.unrecorded);
    assert_eq!(synced.attention(), [ProjectAttention::UncommittedChanges]);
    let RecordOutcome::Recorded { files, .. } = record_as_ana(&h).await.unwrap() else {
        panic!("a record");
    };
    assert_eq!(files, ["README.md", "docs/notes.md"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stash_is_a_stack_and_the_second_unstash_applies_the_first_stash() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    write(&dir, "a.txt", "one\n");
    stash_as_ana(&h).await.unwrap();
    write(&dir, "b.txt", "two\n");
    let StashOutcome::Stashed { entries: count, .. } = stash_as_ana(&h).await.unwrap() else {
        panic!("a stash");
    };
    assert_eq!(count, 2);
    assert_eq!(stack_files(&h).await, [vec!["b.txt"], vec!["a.txt"]]);
    assert_eq!(
        entries(&h)
            .await
            .iter()
            .map(|entry| entry.position)
            .collect::<Vec<_>>(),
        [0, 1]
    );

    unstash_as_ana(&h).await.unwrap();
    assert_eq!(read(&dir, "b.txt").as_deref(), Some("two\n"));
    assert!(read(&dir, "a.txt").is_none());
    assert_eq!(stack_files(&h).await, [vec!["a.txt"]]);
    let UnstashOutcome::Applied {
        files,
        entries: left,
    } = unstash_as_ana(&h).await.unwrap()
    else {
        panic!("a clean unstash");
    };
    assert_eq!((files, left), (vec!["a.txt".to_owned()], 0));
    assert_eq!(read(&dir, "a.txt").as_deref(), Some("one\n"));
    assert_eq!(read(&dir, "b.txt").as_deref(), Some("two\n"));
    let (reason, _) = refusal(unstash_as_ana(&h).await.unwrap_err());
    assert_eq!(reason, "stash_empty");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unstash_merges_the_change_into_a_newer_main_cleanly() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    integrate(&h, "frontend", &[("docs.md", "a\nb\nc\n")]).await;
    wait_for(&dir, "docs.md", "a\nb\nc\n").await;
    write(&dir, "docs.md", "a\nb\nmine\n");
    stash_as_ana(&h).await.unwrap();
    assert_eq!(read(&dir, "docs.md").as_deref(), Some("a\nb\nc\n"));
    let revision = integrate(&h, "frontend", &[("docs.md", "main\nb\nc\n")]).await;
    // Nothing of the human's holds the folder, so it follows main at once.
    wait_for(&dir, "docs.md", "main\nb\nc\n").await;
    assert_eq!(
        sync(&h).await.followed.at.map(|at| at.to_string()),
        Some(revision.clone())
    );

    assert!(matches!(
        unstash_as_ana(&h).await.unwrap(),
        UnstashOutcome::Applied { .. }
    ));
    assert_eq!(read(&dir, "docs.md").as_deref(), Some("main\nb\nmine\n"));
    let RecordOutcome::Recorded { files, .. } = record_as_ana(&h).await.unwrap() else {
        panic!("a record");
    };
    assert_eq!(files, ["docs.md"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conflicting_unstash_writes_markers_keeps_the_entry_and_holds_the_folder_until_the_record()
 {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    stash_as_ana(&h).await.unwrap();
    let revision = integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    wait_for(&dir, "README.md", "# fold\n").await;

    let UnstashOutcome::Conflicted {
        revision: folder,
        conflicted,
        entries: count,
        ..
    } = unstash_as_ana(&h).await.unwrap()
    else {
        panic!("a conflict");
    };
    assert_eq!(folder.to_string(), revision);
    assert_eq!(count, 1);
    assert_eq!(conflicted.len(), 1);
    assert_eq!(conflicted[0].path, "README.md");
    assert_eq!(conflicted[0].sidecar, None);
    assert_eq!(
        read(&dir, "README.md").unwrap(),
        format!("<<<<<<< main ({revision})\n# fold\n=======\n# mine\n>>>>>>> stash@{{0}}\n")
    );
    let listed = entries(&h).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].conflicted, ["README.md"]);
    assert_eq!(
        h.handle.git_attention.folder_held().map(|(_, held)| held),
        Some(vec!["README.md".to_owned()])
    );
    assert_eq!(stored(&h, types::STASH_CONFLICTED).await.len(), 1);
    let items = needs_you(&h.handle.store).await.unwrap();
    let item = items
        .iter()
        .find(|item| item.kind == NeedsYouKind::UnstashConflicted)
        .expect("the unstash conflict waits in Needs you");
    assert_eq!(item.paths, ["README.md"]);
    assert!(item.reason.as_deref().unwrap().contains("fld record"));

    // The folder holds while the markers wait: main's next change is not written.
    integrate(&h, "frontend", &[("src/new.rs", "fn new() {}\n")]).await;
    let synced = sync(&h).await;
    assert_eq!(synced.followed.at, None);
    assert_eq!(synced.followed.held, ["README.md"]);
    assert!(!dir.join("src/new.rs").exists());
    // Neither a second stash nor a second unstash starts while the conflict waits.
    assert_eq!(
        refusal(stash_as_ana(&h).await.unwrap_err()).0,
        "stash_conflicted"
    );
    assert_eq!(
        refusal(unstash_as_ana(&h).await.unwrap_err()).0,
        "stash_conflicted"
    );
    // fld record refuses the file that still holds markers and names it.
    let (reason, paths) = refusal(record_as_ana(&h).await.unwrap_err());
    assert_eq!(reason, "stash_markers");
    assert_eq!(paths, json!(["README.md"]));
    assert_eq!(entries(&h).await.len(), 1);

    write(&dir, "README.md", "# fold, and mine\n");
    let RecordOutcome::Recorded { files, .. } = record_as_ana(&h).await.unwrap() else {
        panic!("a record");
    };
    assert_eq!(files, ["README.md"]);
    assert!(entries(&h).await.is_empty());
    assert_eq!(stored(&h, types::STASH_DROPPED).await.len(), 1);
    // The folder follows main again.
    assert_eq!(read(&dir, "src/new.rs").as_deref(), Some("fn new() {}\n"));
    assert!(
        !needs_you(&h.handle.store)
            .await
            .unwrap()
            .iter()
            .any(|item| item.kind == NeedsYouKind::UnstashConflicted)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unstash_merges_into_new_unrecorded_changes_and_puts_a_binary_side_beside_its_file() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    write(&dir, "notes.md", "stashed\n");
    std::fs::write(dir.join("logo.bin"), [0xff_u8, 0, 1]).unwrap();
    stash_as_ana(&h).await.unwrap();
    // New changes of the human's since the stash, which the unstash keeps.
    write(&dir, "README.md", "# mine\n");
    std::fs::write(dir.join("logo.bin"), [0xff_u8, 0, 9]).unwrap();

    let UnstashOutcome::Conflicted { conflicted, .. } = unstash_as_ana(&h).await.unwrap() else {
        panic!("a conflict on logo.bin");
    };
    assert_eq!(conflicted.len(), 1);
    assert_eq!(conflicted[0].path, "logo.bin");
    assert_eq!(
        conflicted[0].sidecar.as_deref(),
        Some("logo.bin.fold-stash")
    );
    assert_eq!(read(&dir, "README.md").as_deref(), Some("# mine\n"));
    assert_eq!(read(&dir, "notes.md").as_deref(), Some("stashed\n"));
    assert_eq!(std::fs::read(dir.join("logo.bin")).unwrap(), [0xff, 0, 9]);
    assert_eq!(
        std::fs::read(dir.join("logo.bin.fold-stash")).unwrap(),
        [0xff, 0, 1]
    );
    let (reason, paths) = refusal(record_as_ana(&h).await.unwrap_err());
    assert_eq!(reason, "stash_markers");
    assert_eq!(paths, json!(["logo.bin"]));

    std::fs::remove_file(dir.join("logo.bin.fold-stash")).unwrap();
    let RecordOutcome::Recorded { files, .. } = record_as_ana(&h).await.unwrap() else {
        panic!("a record");
    };
    assert_eq!(files, ["README.md", "logo.bin", "notes.md"]);
    assert!(entries(&h).await.is_empty());
}

/// The paths whose index entries differ from `HEAD`'s.
fn staged(dir: &Path) -> String {
    git(dir, &["diff", "--cached", "--name-only"])
}

fn has_git_stash(dir: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--verify", "--quiet", "refs/stash"])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap()
        .status
        .success()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_git_sync_a_stash_resets_the_staged_entries_makes_no_git_stash_and_the_watch_goes_on()
{
    let h = Harness::start().await;
    branch_born(&h).await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    write(&dir, "plan.md", "the plan\n");
    git(&dir, &["add", "README.md", "plan.md"]);
    assert_eq!(staged(&dir), "README.md\nplan.md\n");

    stash_as_ana(&h).await.unwrap();
    assert_eq!(staged(&dir), "");
    assert_eq!(
        git(&dir, &["status", "--porcelain", "--untracked-files=no"]),
        ""
    );
    assert!(!dir.join("plan.md").exists());
    assert!(!has_git_stash(&dir));
    let synced = sync(&h).await;
    assert!(synced.moved().is_none());
    assert!(!synced.attention().contains(&ProjectAttention::HeadMoved));

    // main changes the stashed file; the folder and the branch follow, since nothing of the human's holds them.
    let tip = git(&dir, &["rev-parse", "HEAD"]);
    let revision = integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    wait_for(&dir, "README.md", "# fold\n").await;
    fold_store::testing::eventually(crate::common::acceptance::WAIT, async || {
        let now = git(&dir, &["rev-parse", "HEAD"]);
        if now == tip { Err(now) } else { Ok(()) }
    })
    .await;
    assert_eq!(
        sync(&h).await.followed.at.map(|at| at.to_string()),
        Some(revision)
    );

    let UnstashOutcome::Conflicted { .. } = unstash_as_ana(&h).await.unwrap() else {
        panic!("README.md conflicts with main");
    };
    assert_eq!(read(&dir, "plan.md").as_deref(), Some("the plan\n"));
    // The unstashed files are changes in the folder that the human did not stage, and Git's stash stays untouched.
    assert_eq!(staged(&dir), "");
    assert!(!has_git_stash(&dir));
    assert!(sync(&h).await.moved().is_none());
    write(&dir, "README.md", "# fold, and mine\n");
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::Recorded { .. }
    ));
    assert!(entries(&h).await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_socket_routes_stash_list_and_unstash_for_the_named_human() {
    let h = Harness::start_plain().await;
    write(&root(&h), "plan.md", "the plan\n");
    let (status, body): (_, Value) = h
        .socket(
            axum::http::Method::POST,
            "/api/v1/projects/demo/stash",
            None,
        )
        .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({"outcome": "stashed", "revision": "r1", "files": ["plan.md"], "entries": 1, "skipped": [], "unstash": null})
    );
    let (status, body): (_, Value) = h
        .socket(axum::http::Method::GET, "/api/v1/projects/demo/stash", None)
        .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(body["entries"][0]["stash"], "stash@{0}");
    assert_eq!(body["entries"][0]["base"], "r1");
    assert_eq!(body["entries"][0]["actor"], "human:ana");
    assert_eq!(body["entries"][0]["files"], json!(["plan.md"]));
    let (status, body): (_, Value) = h
        .socket(
            axum::http::Method::POST,
            "/api/v1/projects/demo/unstash",
            None,
        )
        .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "applied");
    assert_eq!(body["revision"], Value::Null);
    let (status, body): (_, Value) = h
        .socket(
            axum::http::Method::POST,
            "/api/v1/projects/demo/unstash",
            None,
        )
        .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"]["details"]["reason"], "stash_empty");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stash_closes_the_conflicted_human_change_that_waits_for_the_main_agent() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged("main", &["main"]).await;
    let mut inbox = h.register_inbox("main").await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    assert!(matches!(
        record_as_ana(&h).await.unwrap(),
        RecordOutcome::Conflicted { created: true, .. }
    ));
    let opened = h.call("main", "open_main_workspace", json!({})).await;
    let ws = opened["workspace"].as_str().unwrap().to_owned();

    let StashOutcome::Stashed { files, .. } = stash_as_ana(&h).await.unwrap() else {
        panic!("a stash");
    };
    assert_eq!(files, ["README.md"]);
    let states: Vec<String> = sqlx::query_scalar("SELECT state FROM human_changes ORDER BY id")
        .fetch_all(h.handle.store.read())
        .await
        .unwrap();
    assert_eq!(states, ["superseded"]);
    let withdrawn = stored(&h, types::HUMAN_CHANGE_WITHDRAWN).await;
    assert_eq!(withdrawn.len(), 1);
    assert_eq!(withdrawn[0].actor.as_str(), "human:ana");
    assert_eq!(withdrawn[0].data["workspace"], ws.as_str());
    let notice = crate::human_changes::notice_with(&mut inbox, "took back").await;
    assert!(
        notice.contains(&format!("abandon {ws}, which holds its merge")),
        "{notice}"
    );
    assert!(
        needs_you(&h.handle.store)
            .await
            .unwrap()
            .iter()
            .all(|item| item.kind != NeedsYouKind::HumanChange)
    );
    // Nothing is lost: the folder follows main and the entry brings the change back.
    assert_eq!(read(&dir, "README.md").as_deref(), Some("# fold\n"));
    assert!(matches!(
        unstash_as_ana(&h).await.unwrap(),
        UnstashOutcome::Conflicted { .. }
    ));
    assert!(read(&dir, "README.md").unwrap().contains("# mine\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stash_and_an_unstash_keep_removals_executables_and_symlinks_and_leave_excluded_files_and_empty_directories()
 {
    use std::os::unix::fs::PermissionsExt;
    let mode = |dir: &Path, path: &str| {
        std::fs::metadata(dir.join(path))
            .unwrap()
            .permissions()
            .mode()
            & 0o111
    };
    let h = Harness::start_plain().await;
    let dir = root(&h);
    integrate(
        &h,
        "frontend",
        &[("docs.md", "docs\n"), ("tool.sh", "echo\n")],
    )
    .await;
    wait_for(&dir, "tool.sh", "echo\n").await;
    std::fs::write(dir.join(".fold/exclude"), "secret.txt\n").unwrap();
    write(&dir, "secret.txt", "secret\n");
    std::fs::remove_file(dir.join("docs.md")).unwrap();
    std::fs::set_permissions(dir.join("tool.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("README.md", dir.join("link")).unwrap();
    std::fs::create_dir(dir.join("empty")).unwrap();

    let StashOutcome::Stashed { files, .. } = stash_as_ana(&h).await.unwrap() else {
        panic!("a stash");
    };
    assert_eq!(files, ["docs.md", "link", "tool.sh"]);
    assert_eq!(read(&dir, "docs.md").as_deref(), Some("docs\n"));
    assert_eq!(mode(&dir, "tool.sh"), 0);
    assert!(std::fs::symlink_metadata(dir.join("link")).is_err());
    // An excluded file and an empty directory are no change fold records, so they stay where they are.
    assert_eq!(read(&dir, "secret.txt").as_deref(), Some("secret\n"));
    assert!(dir.join("empty").is_dir());

    assert!(matches!(
        unstash_as_ana(&h).await.unwrap(),
        UnstashOutcome::Applied { .. }
    ));
    assert!(!dir.join("docs.md").exists());
    assert_ne!(mode(&dir, "tool.sh"), 0);
    assert_eq!(
        std::fs::read_link(dir.join("link")).unwrap(),
        PathBuf::from("README.md")
    );
    assert_eq!(read(&dir, "secret.txt").as_deref(), Some("secret\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stash_refuses_a_tracked_file_over_the_size_limit_and_writes_nothing() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    let config = dir.join(".fold/config.toml");
    let kept = std::fs::read_to_string(&config).unwrap_or_default();
    std::fs::write(&config, format!("max_file_bytes = 32\n{kept}")).unwrap();
    let large = "x".repeat(100);
    write(&dir, "README.md", &large);
    write(&dir, "a.txt", "small\n");

    let (reason, paths) = refusal(stash_as_ana(&h).await.unwrap_err());
    assert_eq!(reason, "skipped_tracked");
    assert_eq!(paths, json!(["README.md"]));
    assert!(entries(&h).await.is_empty());
    assert_eq!(read(&dir, "README.md"), Some(large));
    assert_eq!(read(&dir, "a.txt").as_deref(), Some("small\n"));
    // A new file over the limit is no file of the record, so it stays in the folder and the rest is stashed.
    write(&dir, "README.md", "# demo\n");
    write(&dir, "big.txt", &"y".repeat(100));
    let StashOutcome::Stashed { files, skipped, .. } = stash_as_ana(&h).await.unwrap() else {
        panic!("a stash");
    };
    assert_eq!(files, ["a.txt"]);
    assert_eq!(
        skipped
            .iter()
            .map(|skip| (skip.path.to_string(), skip.reason.as_str()))
            .collect::<Vec<_>>(),
        [("big.txt".to_owned(), "too_large")]
    );
    assert_eq!(read(&dir, "big.txt").map(|text| text.len()), Some(100));
    let (status, body): (_, Value) = h
        .socket(
            axum::http::Method::POST,
            "/api/v1/projects/demo/stash",
            None,
        )
        .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "nothing");
    assert_eq!(
        body["skipped"],
        json!([{"path": "big.txt", "reason": "too_large"}])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unstash_refuses_to_write_where_the_folder_holds_an_ignored_file() {
    let h = Harness::start_plain().await;
    let dir = root(&h);
    write(&dir, "notes.md", "stashed\n");
    stash_as_ana(&h).await.unwrap();
    std::fs::write(dir.join(".fold/exclude"), "notes.md\n").unwrap();
    write(&dir, "notes.md", "ignored\n");

    let (reason, paths) = refusal(unstash_as_ana(&h).await.unwrap_err());
    assert_eq!(reason, "unstash_path_occupied");
    assert_eq!(paths, json!(["notes.md"]));
    assert_eq!(read(&dir, "notes.md").as_deref(), Some("ignored\n"));
    assert_eq!(stack_files(&h).await, [vec!["notes.md"]]);

    std::fs::remove_file(dir.join("notes.md")).unwrap();
    std::fs::write(dir.join(".fold/exclude"), "").unwrap();
    assert!(matches!(
        unstash_as_ana(&h).await.unwrap(),
        UnstashOutcome::Applied { .. }
    ));
    assert_eq!(read(&dir, "notes.md").as_deref(), Some("stashed\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_git_commit_that_records_the_markers_of_an_unstash_drops_the_entry_and_makes_a_task_to_fix_them()
 {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged("main", &["main"]).await;
    let mut inbox = h.register_inbox("main").await;
    let dir = root(&h);
    write(&dir, "README.md", "# mine\n");
    stash_as_ana(&h).await.unwrap();
    integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    wait_for(&dir, "README.md", "# fold\n").await;
    let UnstashOutcome::Conflicted { .. } = unstash_as_ana(&h).await.unwrap() else {
        panic!("README.md conflicts with main");
    };
    let marked = read(&dir, "README.md").unwrap();

    // The human commits the file with the markers left in; the Git job records the commit as any other.
    git(&dir, &["commit", "--quiet", "-am", "wip"]);
    sync(&h).await;
    let head = fold_store::workspaces::head(h.handle.store.read())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::human_changes::file_of(&h, &head.number.to_string(), "README.md")
            .await
            .as_deref(),
        Some(marked.as_str())
    );
    assert!(entries(&h).await.is_empty());
    let dropped = stored(&h, types::STASH_DROPPED).await;
    assert_eq!(dropped.len(), 1);
    assert_eq!(dropped[0].actor.as_str(), "fold");
    assert_eq!(dropped[0].data["conflicted"], json!(["README.md"]));
    let task = dropped[0].data["task"].as_str().unwrap().to_owned();
    assert_eq!(
        dropped[0].task.map(|task| task.to_string()),
        Some(task.clone())
    );
    let created = h.call("main", "get_task", json!({"id": task})).await;
    assert!(
        created
            .to_string()
            .contains("Fix the conflict markers of fld unstash on main"),
        "{created}"
    );
    assert!(created.to_string().contains("README.md"), "{created}");
    let notice = crate::human_changes::notice_with(&mut inbox, &task).await;
    assert!(notice.contains(&task), "{notice}");
    assert!(
        needs_you(&h.handle.store)
            .await
            .unwrap()
            .iter()
            .all(|item| item.kind != NeedsYouKind::UnstashConflicted)
    );
    // The folder holds nothing of the entry any more, so it follows main again.
    integrate(&h, "frontend", &[("src/new.rs", "fn new() {}\n")]).await;
    wait_for(&dir, "src/new.rs", "fn new() {}\n").await;
    // A second follow finds the entry closed and makes no second task.
    sync(&h).await;
    assert_eq!(stored(&h, types::STASH_DROPPED).await.len(), 1);
}
