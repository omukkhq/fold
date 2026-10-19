#![cfg(test)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::common::acceptance::{AGENTS, Harness, WAIT, read_tree};
use fold_core::WorkspaceId;
use fold_daemon::http::events::LiveWorkspace;
use fold_daemon::jobs::spawn_auto_checkpoints;
use fold_store::testing::eventually;
use serde_json::{Value, json};
use tokio::sync::{Barrier, watch};

/// What one worker writes: relative path and content. Both workers write `shared/config.txt`.
type Files = [(&'static str, &'static str); 2];

const FRONTEND_FILES: Files = [
    ("shared/config.txt", "theme = dark\n"),
    ("src/screen.txt", "login screen\n"),
];
const BACKEND_FILES: Files = [
    ("shared/config.txt", "port = 8080\n"),
    ("src/endpoint.txt", "POST /login\n"),
];

/// The paths of `files` with the changes `get_workspace_changes` and the live state must both list.
fn expected(files: &Files) -> Vec<String> {
    let mut paths: Vec<String> = files.iter().map(|(path, _)| (*path).to_owned()).collect();
    paths.sort();
    paths
}

/// The `(path, change, added, removed)` rows of the live state.
fn live_rows(view: &LiveWorkspace) -> Vec<(String, String, u64, u64)> {
    view.files
        .iter()
        .map(|file| {
            let change = serde_json::to_value(file.change).unwrap();
            (
                file.path.clone(),
                change.as_str().unwrap().to_owned(),
                u64::from(file.added_lines),
                u64::from(file.removed_lines),
            )
        })
        .collect()
}

/// The same rows from the `files` of `get_workspace_changes`.
fn tool_rows(files: &Value) -> Vec<(String, String, u64, u64)> {
    files
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap().to_owned(),
                file["change"].as_str().unwrap().to_owned(),
                file["added"].as_u64().unwrap(),
                file["removed"].as_u64().unwrap(),
            )
        })
        .collect()
}

/// Acceptance 3: two workers edit independent workspaces at the same time. Neither sees the other's files, the file both wrote keeps two contents, each workspace gets its own automatic checkpoint, and main stays at `r1`.
#[tokio::test]
async fn acceptance_3_parallel_workspaces_do_not_change_each_other() {
    let harness = Harness::start().await;
    for agent in AGENTS {
        harness
            .call(agent, "register_agent", json!({"name": agent}))
            .await;
    }
    fs::write(
        harness.project.paths().config(),
        "auto_checkpoint_quiet_seconds = 1\n",
    )
    .unwrap();

    let mut tasks = Vec::new();
    for (title, lead) in [
        ("Build the login screen", "frontend"),
        ("Add the login endpoint", "backend"),
    ] {
        let created = harness
            .call(
                "review",
                "create_task",
                json!({"title": title, "labels": Harness::work_labels_of(lead)}),
            )
            .await;
        tasks.push(created["id"].as_str().unwrap().to_owned());
    }
    let (screen, endpoint) = (tasks[0].clone(), tasks[1].clone());
    for (agent, task) in [("frontend", &screen), ("backend", &endpoint)] {
        let claimed = harness.call(agent, "claim_task", json!({"id": task})).await;
        assert_eq!(claimed["holder"], agent);
    }

    // Both workers create their workspace and write at the same moment.
    let barrier = Arc::new(Barrier::new(2));
    let work = async |agent: &'static str, task: String, files: Files| {
        let created = harness
            .call(agent, "create_workspace", json!({"task": task}))
            .await;
        let dir = Path::new(harness.root()).join(created["path"].as_str().unwrap());
        barrier.wait().await;
        for (path, content) in files {
            let target = dir.join(path);
            tokio::fs::create_dir_all(target.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&target, content).await.unwrap();
        }
        let id: WorkspaceId = created["workspace"].as_str().unwrap().parse().unwrap();
        (id, created["workspace"].as_str().unwrap().to_owned(), dir)
    };
    let (frontend, backend) = tokio::time::timeout(WAIT, async {
        tokio::join!(
            work("frontend", screen.clone(), FRONTEND_FILES),
            work("backend", endpoint.clone(), BACKEND_FILES),
        )
    })
    .await
    .expect("both workers write within the wait");
    assert_ne!(frontend.0, backend.0);

    // Each live state lists its own changes.
    for ((id, _, _), files) in [(&frontend, &FRONTEND_FILES), (&backend, &BACKEND_FILES)] {
        let id = *id;
        let handle = &harness.handle;
        let view = eventually(WAIT, async || {
            let view = handle.live.get(id).map(|state| state.view.clone());
            match view {
                Some(view)
                    if view.files.len() == files.len()
                        && view.files.iter().all(|file| file.added_lines == 1) =>
                {
                    Ok(view)
                }
                other => Err(other.map(|view| view.summary)),
            }
        })
        .await;
        let mut paths: Vec<String> = view.files.iter().map(|file| file.path.clone()).collect();
        paths.sort();
        assert_eq!(paths, expected(files), "{id}");
    }

    // The workspaces list only their own files, `get_workspace_changes` agrees with the live state, and each directory holds exactly what its worker wrote.
    let mut content = BTreeMap::new();
    for (agent, (id, name, dir), files, other) in [
        ("frontend", &frontend, &FRONTEND_FILES, &BACKEND_FILES),
        ("backend", &backend, &BACKEND_FILES, &FRONTEND_FILES),
    ] {
        let live = harness.handle.live.get(*id).unwrap().view.clone();
        let changes = harness
            .call(agent, "get_workspace_changes", json!({"ws": name}))
            .await;
        assert_eq!(tool_rows(&changes["files"]), live_rows(&live), "{agent}");
        let listed: Vec<&str> = live.files.iter().map(|file| file.path.as_str()).collect();
        for (path, _) in other {
            let own = files.iter().any(|(mine, _)| mine == path);
            assert_eq!(listed.contains(path), own, "{agent} lists {path}");
        }
        assert!(
            live_rows(&live).iter().all(
                |(_, change, added, removed)| change == "added" && (*added, *removed) == (1, 0)
            ),
            "{agent}: {live:?}"
        );

        let mut want: BTreeMap<String, Vec<u8>> = files
            .iter()
            .map(|(path, text)| ((*path).to_owned(), text.as_bytes().to_vec()))
            .collect();
        want.insert("README.md".to_owned(), b"# demo\n".to_vec());
        assert_eq!(read_tree(dir), want, "{agent}'s directory");
        content.insert(agent, fs::read(dir.join("shared/config.txt")).unwrap());
    }
    assert_ne!(content["frontend"], content["backend"]);

    // Both tasks are In Progress and main has not moved.
    for (task, holder) in [(&screen, "frontend"), (&endpoint, "backend")] {
        let detail = harness
            .call("review", "get_task", json!({"id": task}))
            .await;
        assert_eq!(detail["status"], "in_progress");
        assert_eq!(detail["claim"]["holder"], holder);
    }
    let repository = harness
        .call("review", "get_repository_state", json!({}))
        .await;
    assert_eq!(repository["head"], "r1");
    assert_eq!(repository["recent"].as_array().unwrap().len(), 1);

    // Past the quiet period the job gives each workspace its own automatic checkpoint.
    harness
        .project
        .clock()
        .advance(Duration::from_secs(2))
        .unwrap();
    let (stop, stopped) = watch::channel(false);
    let job = spawn_auto_checkpoints(harness.state.clone(), Duration::from_millis(10), stopped);
    let mut ids = Vec::new();
    for (agent, name) in [("frontend", &frontend.1), ("backend", &backend.1)] {
        let listed = eventually(WAIT, async || {
            let listed = harness
                .call(agent, "list_checkpoints", json!({"ws": name}))
                .await;
            let listed = listed["checkpoints"].as_array().unwrap().clone();
            if listed.is_empty() {
                Err(())
            } else {
                Ok(listed)
            }
        })
        .await;
        assert_eq!(listed.len(), 1, "{agent}");
        assert_eq!(listed[0]["kind"], "auto");
        assert_eq!(listed[0]["reason"], "quiet");
        ids.push(listed[0]["checkpoint"].as_str().unwrap().to_owned());
    }
    stop.send(true).unwrap();
    job.stopped(WAIT).await;
    assert_ne!(ids[0], ids[1], "each workspace has its own checkpoint");
    for (checkpoint, files) in ids.iter().zip([&FRONTEND_FILES, &BACKEND_FILES]) {
        let compared = harness
            .call(
                "review",
                "compare_revisions",
                json!({"from": "r1", "to": checkpoint}),
            )
            .await;
        let paths: Vec<&str> = compared["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, expected(files), "{checkpoint} holds its own files");
    }

    let repository = harness
        .call("review", "get_repository_state", json!({}))
        .await;
    assert_eq!(repository["head"], "r1");
}
