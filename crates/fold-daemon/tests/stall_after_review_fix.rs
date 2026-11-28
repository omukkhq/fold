#![cfg(test)]

use std::path::Path;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use fold_core::WorkerState;
use fold_daemon::live::WorkspaceLive;
use fold_daemon::services::workspaces::check_stalls;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, SLUG, Work};
use crate::worker_resume::{TASK_WORKER, task_worker_for, until_state};
use crate::workers_support::FakeClaude;

/// The default `stall_minutes`.
const STALL: Duration = Duration::from_secs(5 * 60);

async fn view(harness: &Harness, work: &Work) -> fold_daemon::live::LiveWorkspace {
    check_stalls(&harness.handle, harness.project.now())
        .await
        .unwrap();
    let workspace = work.workspace.parse().unwrap();
    WorkspaceLive::clone(&harness.handle.live.get(workspace).unwrap()).view
}

async fn stalled_item(harness: &Harness) -> Option<Value> {
    let (status, body) = harness
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/needs-you"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["kind"] == "worker_stalled")
        .cloned()
}

/// A task worker sent `P-1` v1 for `T-1`, the first of `titles`; `review` fixes the proposal as v2 in its checkout and approves its fix, so the author's workspace still holds v1 while v2 waits for its integration. With one title fold stopped the worker after its proposal.
async fn fixed_by_the_reviewer(fake: &FakeClaude, titles: &[&str]) -> (Harness, Work) {
    let harness = task_worker_for(fake, titles, WorkerState::Idle).await;
    let work = harness.start_work(TASK_WORKER, "T-1").await;
    work.write(&[("page.txt", "page\n")]);
    assert_eq!(
        harness.propose(TASK_WORKER, &work, "Draw the page").await,
        "P-1"
    );
    if titles.len() == 1 {
        until_state(&harness, TASK_WORKER, WorkerState::Stopped).await;
    }
    harness.give_review("review", "P-1").await;
    let checkout = harness
        .call("review", "create_review_checkout", json!({"id": "P-1"}))
        .await;
    let dir = Path::new(harness.root()).join(checkout["path"].as_str().unwrap());
    std::fs::write(dir.join("page.txt"), "page with a title\n").unwrap();
    let fixed = harness
        .call(
            "review",
            "submit_review_fix",
            json!({
                "intent": "Make the change the task asks for.",
                "id": "P-1",
                "version": 1,
                "fixes": [{"path": "page.txt", "reason": "the page had no title"}],
                "checks": "none",
            }),
        )
        .await;
    assert_eq!(fixed["version"], "v2", "{fixed}");
    harness
        .call(
            "review",
            "submit_review",
            json!({"id": "P-1", "version": 2, "verdict": "approve", "body": "Fix: page.txt has its title."}),
        )
        .await;
    harness
        .handle
        .live
        .rescan(work.workspace.parse().unwrap())
        .await
        .unwrap();
    (harness, work)
}

/// A worker whose workspace holds its own sent v1 while the reviewer's v2 waits is not stalled over several stall windows, and Needs you names no stall.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_author_is_not_stalled_while_the_reviewers_version_waits() {
    let fake = FakeClaude::new("result");
    let (harness, work) = fixed_by_the_reviewer(&fake, &["Draw the page"]).await;
    assert_eq!(view(&harness, &work).await.edits_after_version, Some(false));
    never_stalls(&harness, &work).await;
}

/// The worker also holds `T-2`, which does not wait on others, so only the fact of the workspace keeps `T-1`'s workspace from a stall: it holds the author's own v1, which is sent work while the reviewer's v2 waits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_with_other_work_is_not_stalled_on_the_workspace_whose_fix_waits() {
    let fake = FakeClaude::new("result");
    let (harness, work) = fixed_by_the_reviewer(&fake, &["Draw the page", "Draw the footer"]).await;
    assert_eq!(view(&harness, &work).await.edits_after_version, Some(false));
    never_stalls(&harness, &work).await;
}

/// Over several stall windows, decided after each, `work`'s worker shows no stall and Needs you names none.
async fn never_stalls(harness: &Harness, work: &Work) {
    for _ in 0..4 {
        harness.project.clock().advance(STALL).unwrap();
        let seen = view(harness, work).await;
        assert!(!seen.stalled);
        assert!(
            seen.activity
                .as_ref()
                .is_none_or(|activity| !activity.is_stall()),
            "{:?}",
            seen.activity
        );
    }
    assert!(stalled_item(harness).await.is_none());
}

/// An edit of the author after its own v1 is an edit after the version while the reviewer's v2 waits, and the stall rule applies to it after the window. The worker also holds `T-2`, which does not wait on others, so the stall is not quiet for a worker whose every claim waits.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_after_the_authors_version_counts_while_the_reviewers_version_waits() {
    let fake = FakeClaude::new("result");
    let (harness, work) = fixed_by_the_reviewer(&fake, &["Draw the page", "Draw the footer"]).await;
    work.write(&[("notes.txt", "written after v1\n")]);
    harness
        .handle
        .live
        .rescan(work.workspace.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(view(&harness, &work).await.edits_after_version, Some(true));
    harness.project.clock().advance(STALL).unwrap();
    let seen = view(&harness, &work).await;
    assert!(
        seen.activity
            .as_ref()
            .is_some_and(|activity| activity.is_stall()),
        "{:?}",
        seen.activity
    );
}
