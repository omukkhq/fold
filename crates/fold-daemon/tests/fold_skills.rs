#![cfg(test)]

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use fold_core::events::types;
use fold_core::{Actor, EventFilter, codes};
use fold_daemon::projects::FolderFiles;
use fold_daemon::services::projects::{FOLD_SKILLS_TITLE, FoldSkills, keep_fold_skills_out};
use fold_store::testing::eventually;
use fold_vcs::git::Git;
use fold_vcs::ignore::{EXCLUDE_NOTE, claude_excludes};
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, WAIT};
use crate::folder_stash::wait_for;
use crate::human_changes::{branch_born, file_of, git, integrate};

/// The skills and settings an older fold left tracked on main, with two skills of the user's own, one whose name starts with `fold-`, and a role skill that the user's `.foldignore` brings back.
pub const OLD_FILES: [(&str, &str); 8] = [
    (".claude/skills/fold/SKILL.md", "old index\n"),
    (".claude/skills/fold/REFERENCE.md", "old reference\n"),
    (".claude/skills/fold-main/SKILL.md", "old main\n"),
    (".claude/skills/fold-reviewer/SKILL.md", "kept reviewer\n"),
    (".claude/skills/deploy/SKILL.md", "mine\n"),
    (".claude/skills/fold-custom/SKILL.md", "my own fold skill\n"),
    (".claude/settings.json", "{\"shared\": true}\n"),
    (".claude/settings.local.json", "{\"mine\": true}\n"),
];

/// The files fold's revision stops tracking: every file below fold's skill folders but the one the `.foldignore` brings back.
pub const STOPPED: [&str; 3] = [
    ".claude/skills/fold-main/SKILL.md",
    ".claude/skills/fold/REFERENCE.md",
    ".claude/skills/fold/SKILL.md",
];

fn root(h: &Harness) -> PathBuf {
    PathBuf::from(h.root())
}

fn read(dir: &Path, path: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(path)).ok()
}

fn exclude(dir: &Path) -> String {
    read(dir, ".fold/exclude").unwrap_or_default()
}

async fn status(h: &Harness) -> Value {
    let (status, body) = h
        .socket(Method::GET, "/api/v1/projects/demo/folder", None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// A project as an older fold left it: `.fold/exclude` without fold's Claude Code lines, and main tracking fold's skills beside the user's own Claude Code files, with the folder holding them.
pub async fn as_an_older_fold_left_it(h: &Harness, exclude_file: &str) -> String {
    std::fs::write(root(h).join(".fold/exclude"), exclude_file).unwrap();
    let mut files = OLD_FILES.to_vec();
    files.push((".foldignore", "!/.claude/skills/fold-reviewer/\n"));
    let revision = integrate(h, "frontend", &files).await;
    for (path, text) in OLD_FILES {
        wait_for(&root(h), path, text).await;
    }
    revision
}

async fn fold_events(h: &Harness, kind: fold_core::EventType) -> Vec<fold_core::Event> {
    let filter = EventFilter {
        types: vec![kind],
        ..EventFilter::default()
    };
    fold_store::events::read_after(h.handle.store.read(), 0, &filter, 100)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_open_after_an_upgrade_stops_tracking_folds_skills_once_and_keeps_the_files() {
    let mut h = Harness::start_plain().await;
    let dir = root(&h);
    let before = as_an_older_fold_left_it(&h, "local/\n").await;
    let recorded_before = fold_events(&h, types::HUMAN_CHANGE_RECORDED).await.len();

    h.restart().await;
    // The note comes first and fold's lines after the user's own, once each.
    let lines = exclude(&dir);
    assert!(
        lines.starts_with(&format!("{EXCLUDE_NOTE}\nlocal/\n")),
        "{lines}"
    );
    for line in claude_excludes() {
        assert_eq!(
            lines.lines().filter(|found| *found == line).count(),
            1,
            "{line}"
        );
    }
    let head = h.head().await;
    assert_eq!(head.parent.map(|parent| parent.to_string()), Some(before));
    let integration = head.integration.as_ref().unwrap();
    assert_eq!(integration.actor, "fold");
    assert_eq!(integration.message.as_deref(), Some(FOLD_SKILLS_TITLE));
    let revision = head.number.to_string();
    for path in STOPPED {
        assert_eq!(file_of(&h, &revision, path).await, None, "{path}");
    }
    for (path, text) in OLD_FILES {
        // Every file stays in the folder as it was.
        assert_eq!(read(&dir, path).as_deref(), Some(text), "{path}");
        if !STOPPED.contains(&path) {
            assert_eq!(
                file_of(&h, &revision, path).await.as_deref(),
                Some(text),
                "{path}"
            );
        }
    }
    // No human made the change, so no human change is recorded, and the folder is at main with nothing to record.
    assert_eq!(
        fold_events(&h, types::HUMAN_CHANGE_RECORDED).await.len(),
        recorded_before
    );
    let succeeded = fold_events(&h, types::INTEGRATION_SUCCEEDED).await;
    let last = succeeded.last().unwrap();
    assert_eq!(last.actor.as_str(), "fold");
    assert_eq!(last.data["kind"], "record");
    let body = status(&h).await;
    assert_eq!(body["head"], revision.as_str());
    assert_eq!(body["at_main"], true);
    assert_eq!(body["changes"], json!([]));
    assert_eq!(
        body["ignored_tracked"],
        json!([".claude/settings.local.json"])
    );

    // A second start finds nothing to do.
    let file = exclude(&dir);
    h.restart().await;
    assert_eq!(h.head().await.number.to_string(), revision);
    assert_eq!(exclude(&dir), file);
    assert_eq!(
        keep_fold_skills_out(&h.handle, &Actor::System, &Git::default())
            .await
            .unwrap(),
        FoldSkills::Untracked
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_folder_that_main_holds_back_keeps_the_skills_tracked_until_a_later_open() {
    let mut h = Harness::start_plain().await;
    let dir = root(&h);
    as_an_older_fold_left_it(&h, "").await;
    integrate(&h, "frontend", &[("docs.md", "a\n")]).await;
    wait_for(&dir, "docs.md", "a\n").await;
    std::fs::write(dir.join("docs.md"), "mine\n").unwrap();
    integrate(&h, "frontend", &[("docs.md", "main\n")]).await;
    eventually(WAIT, async || match h.handle.git_attention.folder_held() {
        Some((_, held)) if held == ["docs.md"] => Ok(()),
        other => Err(other),
    })
    .await;
    let held_at = h.head().await.number;

    h.restart().await;
    assert_eq!(h.head().await.number, held_at);
    assert_eq!(
        keep_fold_skills_out(&h.handle, &Actor::System, &Git::default())
            .await
            .unwrap(),
        FoldSkills::Waits("folder_held")
    );
    for path in STOPPED {
        assert!(
            file_of(&h, &held_at.to_string(), path).await.is_some(),
            "{path}"
        );
    }
    // fold's lines are in place already, and the next open after the human takes the change back stops tracking the skills.
    assert!(exclude(&dir).contains("/.claude/skills/fold/\n"));
    std::fs::write(dir.join("docs.md"), "a\n").unwrap();
    wait_for(&dir, "docs.md", "main\n").await;
    h.restart().await;
    let head = h.head().await;
    assert_eq!(
        head.integration.unwrap().message.as_deref(),
        Some(FOLD_SKILLS_TITLE)
    );
    for path in STOPPED {
        assert_eq!(
            file_of(&h, &head.number.to_string(), path).await,
            None,
            "{path}"
        );
        assert!(read(&dir, path).is_some(), "{path}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_fold_stops_tracking_its_own_skills() {
    let h = Harness::start_plain().await;
    let refused = keep_fold_skills_out(
        &h.handle,
        &Actor::human(crate::common::ana()),
        &Git::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(refused.code(), Some(codes::INVALID_REQUEST));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_git_sync_folds_revision_removes_the_skills_from_the_branch_and_the_index_only() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    let dir = root(&h);
    as_an_older_fold_left_it(&h, "/.fold/\n").await;
    let before = h.head().await.number;
    eventually(WAIT, async || {
        let listed = git(&dir, &["ls-files", ".claude"]);
        if listed.lines().count() == OLD_FILES.len() {
            Ok(())
        } else {
            Err(listed)
        }
    })
    .await;

    h.restart().await;
    let head = h.head().await;
    assert_eq!(head.parent, Some(before));
    let commit = eventually(WAIT, async || match fold_store::workspaces::get_by_number(
        h.handle.store.read(),
        head.number,
    )
    .await
    .unwrap()
    .commit
    {
        Some(commit) => Ok(commit),
        None => Err("no commit yet"),
    })
    .await;
    assert_eq!(
        git(&dir, &["log", "-1", "--format=%s", commit.as_str()]).trim(),
        FOLD_SKILLS_TITLE
    );
    let listed = git(&dir, &["ls-tree", "-r", "--name-only", commit.as_str()]);
    for path in STOPPED {
        assert!(!listed.lines().any(|found| found == path), "{listed}");
        assert_eq!(git(&dir, &["ls-files", path]), "", "{path}");
    }
    // The user's own skills, also one whose name starts with `fold-`, the kept role skill and the settings stay on the branch and in the index.
    for (path, _) in OLD_FILES {
        if !STOPPED.contains(&path) {
            assert!(
                listed.lines().any(|found| found == path),
                "{path}: {listed}"
            );
            assert_eq!(git(&dir, &["ls-files", path]), format!("{path}\n"));
        }
    }
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]).trim(), commit.as_str());
    // The files stay, and Git leaves them out through the exclude file, so the folder is clean.
    for (path, text) in OLD_FILES {
        assert_eq!(read(&dir, path).as_deref(), Some(text), "{path}");
    }
    assert_eq!(git(&dir, &["status", "--porcelain"]), "");
}

/// fold's skills as an upgraded `fld serve` writes them into the project folder at each open, before the revision runs.
fn upgraded_skills() -> FolderFiles {
    FolderFiles::new(|root: &Path| {
        for path in STOPPED {
            std::fs::write(root.join(path), format!("new {path}\n")).unwrap();
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skills_an_upgrade_writes_again_before_the_revision_stay_on_disk_and_leave_main() {
    let mut h = Harness::start_plain().await;
    let dir = root(&h);
    let before = as_an_older_fold_left_it(&h, "").await;

    h.folder_files = upgraded_skills();
    h.restart().await;
    let head = h.head().await;
    assert_eq!(head.parent.map(|parent| parent.to_string()), Some(before));
    assert_eq!(
        head.integration.unwrap().message.as_deref(),
        Some(FOLD_SKILLS_TITLE)
    );
    let revision = head.number.to_string();
    for path in STOPPED {
        assert_eq!(file_of(&h, &revision, path).await, None, "{path}");
        assert_eq!(read(&dir, path), Some(format!("new {path}\n")), "{path}");
    }
    let body = status(&h).await;
    assert_eq!(body["at_main"], true);
    assert_eq!(body["changes"], json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_git_skills_an_upgrade_writes_again_leave_the_branch_and_the_index_and_stay_in_the_work_tree()
 {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    let dir = root(&h);
    as_an_older_fold_left_it(&h, "/.fold/\n").await;
    eventually(WAIT, async || {
        let listed = git(&dir, &["ls-files", ".claude"]);
        if listed.lines().count() == OLD_FILES.len() {
            Ok(())
        } else {
            Err(listed)
        }
    })
    .await;

    h.folder_files = upgraded_skills();
    h.restart().await;
    let head = h.head().await;
    assert_eq!(
        head.integration.as_ref().unwrap().message.as_deref(),
        Some(FOLD_SKILLS_TITLE)
    );
    eventually(WAIT, async || match fold_store::workspaces::get_by_number(
        h.handle.store.read(),
        head.number,
    )
    .await
    .unwrap()
    .commit
    {
        Some(commit) if git(&dir, &["rev-parse", "HEAD"]).trim() == commit.as_str() => Ok(()),
        other => Err(other),
    })
    .await;
    for path in STOPPED {
        assert_eq!(git(&dir, &["ls-files", path]), "", "{path}");
        assert_eq!(read(&dir, path), Some(format!("new {path}\n")), "{path}");
    }
    assert_eq!(git(&dir, &["status", "--porcelain"]), "");
}
