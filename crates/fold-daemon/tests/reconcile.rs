#![cfg(test)]

pub mod daemon_support;

use std::fs::{self, File};
use std::time::Duration;

use axum::http::StatusCode;
use daemon_support::{Opened, t};
use fold_core::TaskStatus;
use fold_core::events::types;
use fold_daemon::http::events::ChangeKind;
use fold_daemon::jobs::reconcile_workspaces;
use fold_daemon::services::workspaces::{Reconciled, Reconciliation};
use fold_store::tasks;
use fold_store::workspaces::{self, WorkspaceState};

#[cfg(test)]
impl Opened {
    async fn state_of(&self, number: u32) -> WorkspaceState {
        workspaces::get_workspace(self.handle.store.read(), daemon_support::ws(number))
            .await
            .unwrap()
            .unwrap()
            .state
    }

    async fn status_of(&self, task: u32) -> TaskStatus {
        tasks::get_by_number(self.handle.store.read(), t(task))
            .await
            .unwrap()
            .unwrap()
            .status
    }
}

#[tokio::test]
async fn a_gone_directory_marks_the_workspace_missing_and_its_return_recovers_it() {
    let mut fx = Opened::new().await;
    let (id, dir) = fx.create(1).await;
    assert_eq!(fx.status_of(1).await, TaskStatus::InProgress);
    let aside = fx.project.paths().root().join("aside");

    fx.restart(|| fs::rename(&dir, &aside).unwrap()).await;
    assert_eq!(fx.state_of(1).await, WorkspaceState::Missing);
    let missing = fx.events(types::WORKSPACE_MISSING.as_str()).await;
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].workspace, Some(id));
    assert_eq!(missing[0].task, Some(t(1)));
    assert_eq!(missing[0].actor.as_str(), "fold");
    assert_eq!(missing[0].created_at, fx.project.now());
    assert_eq!(fx.status_of(1).await, TaskStatus::InProgress);
    assert!(fx.handle.live.get(id).is_none());
    let (status, body) = fx.get(&format!("/workspaces/{id}/changes")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "workspace_not_active");
    let (_, list) = fx.get("/workspaces").await;
    assert_eq!(list["workspaces"][0]["state"], "missing");
    assert!(list["workspaces"][0]["live"].is_null());

    fx.restart(|| {}).await;
    assert_eq!(fx.state_of(1).await, WorkspaceState::Missing);
    assert_eq!(fx.events(types::WORKSPACE_MISSING.as_str()).await.len(), 1);

    fx.restart(|| fs::rename(&aside, &dir).unwrap()).await;
    assert_eq!(fx.state_of(1).await, WorkspaceState::Active);
    let recovered = fx.events(types::WORKSPACE_RECOVERED.as_str()).await;
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].workspace, Some(id));
    assert_eq!(fx.status_of(1).await, TaskStatus::InProgress);
    assert!(fx.handle.live.get(id).is_some());

    fx.restart(|| {}).await;
    assert_eq!(
        fx.events(types::WORKSPACE_RECOVERED.as_str()).await.len(),
        1
    );
    assert_eq!(fx.events(types::WORKSPACE_MISSING.as_str()).await.len(), 1);
}

#[tokio::test]
async fn an_edit_made_while_the_daemon_was_closed_is_in_the_reopened_live_state() {
    let mut fx = Opened::new().await;
    let (id, dir) = fx.create(1).await;
    fx.restart(|| fs::write(dir.join("src/lib.rs"), "pub fn x() {}\npub fn z() {}\n").unwrap())
        .await;
    let state = fx.handle.live.get(id).unwrap();
    let files: Vec<(&str, ChangeKind, u32)> = state
        .view
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.change, file.added_lines))
        .collect();
    assert_eq!(files, [("src/lib.rs", ChangeKind::Modified, 1)]);
    assert!(
        fx.events(types::WORKSPACE_MISSING.as_str())
            .await
            .is_empty()
    );
    assert!(
        fx.events(types::WORKSPACE_RECOVERED.as_str())
            .await
            .is_empty()
    );
}

#[tokio::test]
async fn a_corrupt_stat_index_is_rebuilt_by_hashing() {
    let mut fx = Opened::new().await;
    let (id, dir) = fx.create(1).await;
    let index = fx.project.paths().workspace_index(id);
    assert!(index.is_file());
    let readme = dir.join("README.md");
    fx.restart(|| {
        // Same size and modification time, so only hashing finds the edit.
        let modified = fs::metadata(&readme).unwrap().modified().unwrap();
        fs::write(&readme, "# DEMO\n").unwrap();
        File::options()
            .write(true)
            .open(&readme)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        fs::write(&index, b"not an index").unwrap();
    })
    .await;
    let state = fx.handle.live.get(id).unwrap();
    let paths: Vec<&str> = state
        .view
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(paths, ["README.md"]);
}

#[tokio::test]
async fn a_closed_workspace_is_left_alone_and_a_gone_one_goes_missing_once() {
    let fx = Opened::new().await;
    let (first, _) = fx.create(1).await;
    let (second, second_dir) = fx.create(2).await;
    fx.workspaces()
        .abandon_workspace(&daemon_support::agent("frontend"), first, "start again")
        .await
        .unwrap();
    fs::remove_dir_all(&second_dir).unwrap();
    let done = tokio::time::timeout(Duration::from_secs(20), reconcile_workspaces(&fx.handle))
        .await
        .unwrap();
    assert_eq!(
        done,
        [Reconciliation {
            workspace: second,
            outcome: Reconciled::Missing,
            unfinished: None,
        }]
    );
    let again = fx.workspaces().reconcile_workspace(first).await.unwrap();
    assert_eq!(again.outcome, Reconciled::Skipped);
    let still = fx.workspaces().reconcile_workspace(second).await.unwrap();
    assert_eq!(still.outcome, Reconciled::StillMissing);
    assert_eq!(fx.state_of(1).await, WorkspaceState::Abandoned);
}
