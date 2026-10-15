#![cfg(test)]

use std::fs;
use std::path::PathBuf;

use crate::common::acceptance::{Harness, SLUG};
use crate::common::sse::{Frame, Sse};
use axum::http::{Method, StatusCode};
use fold_daemon::http::events::{ChangeKind, LiveWorkspace};
use fold_store::workspaces;
use serde_json::{Value, json};

const ORIGINAL: &str = "# demo\n";
const EDITED: &str = "# demo\nlogin is live\n";

/// The harness with frontend holding a claimed task and its workspace, and an event stream opened after that, with the snapshot already read.
struct Scene {
    harness: Harness,
    sse: Sse,
    workspace: String,
    readme: PathBuf,
}

impl Scene {
    async fn new() -> Self {
        let harness = Harness::start().await;
        let task = harness
            .call("frontend", "create_task", json!({"title": "Add the login"}))
            .await;
        let task = task["id"].as_str().unwrap().to_owned();
        harness
            .call("frontend", "claim_task", json!({"id": task}))
            .await;
        let created = harness
            .call("frontend", "create_workspace", json!({"task": task}))
            .await;
        let workspace = created["workspace"].as_str().unwrap().to_owned();
        let readme = PathBuf::from(harness.root())
            .join(created["path"].as_str().unwrap())
            .join("README.md");

        let mut sse = Sse::open(&harness.state, None).await;
        let snapshot = sse.next().await;
        let listed = snapshot.snapshot().expect("the snapshot comes first");
        assert_eq!(listed.len(), 1, "{snapshot:?}");
        assert_eq!(listed[0].workspace.to_string(), workspace);
        assert!(listed[0].files.is_empty(), "{snapshot:?}");
        Self {
            harness,
            sse,
            workspace,
            readme,
        }
    }

    /// Reads frames of this workspace until `done` accepts its state. Every frame comes from the one open connection.
    async fn live(&mut self, what: &str, done: impl Fn(&LiveWorkspace) -> bool) -> LiveWorkspace {
        let workspace = self.workspace.clone();
        self.sse
            .until(what, |frame: &Frame| {
                let view = frame.workspace()?;
                assert_eq!(frame.event.as_deref(), Some("workspace.changed"));
                assert_eq!(frame.id, None, "a live frame never moves the last id");
                (view.workspace.to_string() == workspace && done(view)).then(|| view.clone())
            })
            .await
    }

    async fn get(&self, path: &str) -> Value {
        let uri = format!("/api/v1/projects/{SLUG}/{path}");
        let (status, body) = self.harness.socket(Method::GET, &uri, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }

    /// The head revision's hash and number.
    async fn head(&self) -> (String, String) {
        let head = workspaces::head(self.harness.handle.store.read())
            .await
            .unwrap()
            .unwrap();
        (head.hash.to_string(), head.number.to_string())
    }
}

/// Acceptance 4: a change in a workspace shows up on the open stream and in the diff, and main stays put.
#[tokio::test]
async fn acceptance_4_change_is_live_and_main_is_untouched() {
    let mut scene = Scene::new().await;
    let head = scene.head().await;
    let state = scene
        .harness
        .call("review", "get_repository_state", json!({}))
        .await;
    assert_eq!(state["head"], "r1");

    fs::write(&scene.readme, EDITED).unwrap();
    let view = scene
        .live("README.md modified", |view| !view.files.is_empty())
        .await;
    assert_eq!(view.files.len(), 1);
    let file = &view.files[0];
    assert_eq!(file.path, "README.md");
    assert_eq!(file.change, ChangeKind::Modified);
    assert_eq!((file.added_lines, file.removed_lines), (1, 0));
    assert_eq!(view.summary, "1 file: 1 modified (+1 -0)");

    let workspace = &scene.workspace;
    let diff = scene.get(&format!("diff?from=r1&to={workspace}")).await;
    assert_eq!(diff["view"], "files");
    assert_eq!(diff["source"], "working");
    assert_eq!(diff["files"].as_array().unwrap().len(), 1);
    assert_eq!(diff["files"][0]["path"], "README.md");
    assert_eq!(diff["files"][0]["change"], "modified");
    assert_eq!(
        diff["totals"],
        json!({"files": 1, "added_lines": 1, "removed_lines": 0})
    );
    let hunks = scene
        .get(&format!("diff?from=r1&to={workspace}&path=README.md"))
        .await;
    assert_eq!(hunks["view"], "file");
    assert_eq!(hunks["change"], "modified");
    let lines = hunks["hunks"][0]["lines"].as_array().unwrap();
    let added: Vec<&Value> = lines
        .iter()
        .filter(|line| line["kind"] == "added")
        .collect();
    assert_eq!(added.len(), 1, "{hunks}");
    assert_eq!(added[0]["text"], "login is live\n");
    assert_eq!(added[0]["old_no"], Value::Null);
    assert_eq!(added[0]["new_no"], 2);

    // Main did not move, and the tool shows the change as working changes.
    let state = scene
        .harness
        .call("review", "get_repository_state", json!({}))
        .await;
    assert_eq!(state["head"], "r1");
    assert_eq!(state["recent"].as_array().unwrap().len(), 1);
    assert_eq!(scene.head().await, head);
    let compared = scene
        .harness
        .call(
            "review",
            "compare_revisions",
            json!({"from": "r1", "to": workspace}),
        )
        .await;
    assert_eq!(compared["from"]["kind"], "revision");
    assert_eq!(compared["to"]["kind"], "workspace");
    assert_eq!(compared["to"]["id"], *workspace);
    assert_eq!(compared["files"].as_array().unwrap().len(), 1);
    assert_eq!(compared["files"][0]["path"], "README.md");
    assert_eq!(compared["files"][0]["change"], "modified");
    assert_eq!(
        compared["totals"],
        json!({"files": 1, "added": 1, "removed": 0})
    );
}

/// Acceptance 4, last step: putting the file back clears the changes on the same stream.
#[tokio::test]
async fn acceptance_4_revert_clears_changes() {
    let mut scene = Scene::new().await;
    fs::write(&scene.readme, EDITED).unwrap();
    scene
        .live("README.md modified", |view| !view.files.is_empty())
        .await;

    fs::write(&scene.readme, ORIGINAL).unwrap();
    let view = scene
        .live("no changed files", |view| view.files.is_empty())
        .await;
    assert_eq!(view.summary, "no changes");

    let workspace = &scene.workspace;
    let changes = scene
        .harness
        .call(
            "frontend",
            "get_workspace_changes",
            json!({"ws": workspace}),
        )
        .await;
    assert_eq!(changes["files"], json!([]));
    let state = scene
        .harness
        .call("review", "get_repository_state", json!({}))
        .await;
    assert_eq!(state["head"], "r1");
}
