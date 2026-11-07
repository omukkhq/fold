#![cfg(test)]

use std::fs;
use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use fold_core::codes;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, SLUG};

const MAIN: &str = "backend";
const WORKER: &str = "frontend";

fn paths(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|path| path.as_str().unwrap().to_owned())
        .collect()
}

/// The paths `get_workspace_changes` lists for `ws`.
async fn changed(harness: &Harness, ws: &str) -> Vec<String> {
    let changes = harness
        .call(WORKER, "get_workspace_changes", json!({"ws": ws}))
        .await;
    changes["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap().to_owned())
        .collect()
}

/// Acceptance 35: the user keeps a design folder out of Git and shares it with the workers. A worker's workspace reads it through a link to the project folder, so the main agent's edit is there at once, and the link is never a change of the workspace; a worker may not share a path itself, and a path no longer shared leaves the workspace at its next sync.
#[tokio::test]
async fn acceptance_35_a_worker_reads_a_shared_design_file_through_its_workspace() {
    // `frontend` leads its area and builds the task; `backend` is the main agent.
    let mut harness = Harness::start_without_leads().await;
    harness.lead_area(WORKER).await;
    harness.make_main(MAIN).await;
    let root = PathBuf::from(harness.root());
    let exclude = root.join(".fold/exclude");
    let mut rules = fs::read_to_string(&exclude).unwrap_or_default();
    rules.push_str("design/\n.env\n");
    fs::write(&exclude, rules).unwrap();
    fs::create_dir_all(root.join("design")).unwrap();
    fs::write(root.join("design/home.pen"), "home v1").unwrap();
    fs::write(root.join(".env"), "KEY=1").unwrap();

    // The workers cannot see either ignored path yet.
    let (status, listed) = harness
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/shared"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(paths(&listed["shared"]), Vec::<String>::new());
    assert_eq!(paths(&listed["unseen"]), [".env", "design/"]);

    let refused = harness
        .refused(WORKER, "share_paths", json!({"paths": ["design/"]}))
        .await;
    assert_eq!(refused["code"], codes::MAIN_AGENT_REQUIRED);
    let tracked = harness
        .refused(MAIN, "share_paths", json!({"paths": ["missing/"]}))
        .await;
    assert_eq!(tracked["details"]["reason"], "missing");
    let shared = harness
        .call(MAIN, "share_paths", json!({"paths": ["design/"]}))
        .await;
    assert_eq!(paths(&shared["added"]), ["design"]);
    // The link is no folder, so the folder pattern alone would not leave it out.
    assert!(
        fs::read_to_string(&exclude)
            .unwrap()
            .lines()
            .any(|line| line == "/design")
    );

    let task = harness.create_task(WORKER, "Draw the home page").await;
    let claimed = harness
        .call(WORKER, "claim_task", json!({"id": task}))
        .await;
    assert_eq!(claimed["holder"], WORKER);
    let view = harness.call(WORKER, "get_task", json!({"id": task})).await;
    assert_eq!(paths(&view["shared"]), ["design"]);
    let created = harness
        .call(WORKER, "create_workspace", json!({"task": task}))
        .await;
    let ws = created["workspace"].as_str().unwrap().to_owned();
    let dir = root.join(created["path"].as_str().unwrap());
    assert_eq!(
        fs::read_link(dir.join("design")).unwrap(),
        root.canonicalize().unwrap().join("design")
    );
    assert_eq!(
        fs::read_to_string(dir.join("design/home.pen")).unwrap(),
        "home v1"
    );
    assert!(!dir.join(".env").exists());

    // The main agent changes the design in the project folder, and the worker reads the new version at once.
    fs::write(root.join("design/home.pen"), "home v2").unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("design/home.pen")).unwrap(),
        "home v2"
    );
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src/home.html"), "<p>home</p>\n").unwrap();
    assert_eq!(changed(&harness, &ws).await, ["src/home.html"]);

    let (status, removed) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/unshare"),
            Some(&json!({"paths": ["design"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(paths(&removed["removed"]), ["design"]);
    // The line that sharing added leaves `.fold/exclude`; the user's own line stays.
    let lines: Vec<String> = fs::read_to_string(&exclude)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert!(!lines.iter().any(|line| line == "/design"), "{lines:?}");
    assert!(lines.iter().any(|line| line == "design/"), "{lines:?}");
    // A link stays until the workspace's next sync, the point at which fold writes into it.
    assert!(fs::symlink_metadata(dir.join("design")).is_ok());
    harness
        .call(WORKER, "sync_workspace", json!({"ws": ws}))
        .await;
    assert!(fs::symlink_metadata(dir.join("design")).is_err());
    assert_eq!(
        fs::read_to_string(root.join("design/home.pen")).unwrap(),
        "home v2"
    );
    assert!(Path::new(&dir).join("src/home.html").is_file());
}

/// The Needs you items of `harness` of `kind`.
async fn needs_you_of(harness: &Harness, kind: &str) -> Vec<Value> {
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
        .filter(|item| item["kind"] == kind)
        .cloned()
        .collect()
}

/// A worker's shell command writes a shared file by its path in the project folder, which no guard reads from a script. fold sees the file change during the worker's tool call and Needs you names the agent and the file until a human dismisses it; the main agent's own edit outside any tool call is no item.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_35_a_shared_file_a_worker_changed_waits_in_needs_you() {
    let fake = crate::workers_support::FakeClaude::new("bash_call").running_tool_scripts();
    let harness = Harness::start_with_launcher(fake.launcher.clone()).await;
    harness.make_main(WORKER).await;
    let root = PathBuf::from(harness.root());
    let exclude = root.join(".fold/exclude");
    let mut rules = fs::read_to_string(&exclude).unwrap_or_default();
    rules.push_str("design/\n");
    fs::write(&exclude, rules).unwrap();
    fs::create_dir_all(root.join("design")).unwrap();
    fs::write(root.join("design/home.pen"), "home v1").unwrap();
    // The main agent's file from before the share, made outside any tool call, is only noted. A file written in the second before a worker's call starts would count as the call's.
    fs::write(root.join("design/notes.md"), "notes").unwrap();
    harness
        .call(WORKER, "share_paths", json!({"paths": ["design"]}))
        .await;
    fake.set_tool_script(&format!(
        "printf 'v2' >> '{}'\n",
        root.join("design/home.pen").display()
    ));
    harness
        .call(
            WORKER,
            "create_label",
            json!({"name": "web", "color": "sage"}),
        )
        .await;
    harness
        .call(
            WORKER,
            "define_area",
            json!({"name": "web", "label": "web"}),
        )
        .await;
    let started = harness
        .call(
            WORKER,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    let lead = started["worker"]["name"].as_str().unwrap().to_owned();

    let item = fold_store::testing::eventually(crate::common::acceptance::WAIT, async || {
        let items = needs_you_of(&harness, "shared_changed").await;
        items.into_iter().next().ok_or(())
    })
    .await;
    assert_eq!(item["agent"], json!(lead));
    assert_eq!(item["paths"], json!(["design/home.pen"]));
    assert_eq!(item["screen"], "repository");
    assert!(
        item["reason"]
            .as_str()
            .unwrap()
            .contains("fld share --dismiss")
    );

    let (status, dismissed) = harness
        .socket(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/shared/dismiss"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{dismissed}");
    assert_eq!(dismissed["dismissed"], 1);
    assert!(needs_you_of(&harness, "shared_changed").await.is_empty());
}
