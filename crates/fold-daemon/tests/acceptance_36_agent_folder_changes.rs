#![cfg(test)]

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, SLUG, WAIT};
use crate::human_changes::notice_with;
use crate::workers_support::FakeClaude;

const MAIN: &str = "frontend";

/// The main agent defines area `web` and starts its lead, whose first turn makes one Bash call; returns the lead's name.
async fn start_lead(h: &Harness) -> String {
    h.call(
        MAIN,
        "create_label",
        json!({"name": "web", "color": "sage"}),
    )
    .await;
    h.call(MAIN, "define_area", json!({"name": "web", "label": "web"}))
        .await;
    let started = h
        .call(
            MAIN,
            "start_worker",
            json!({"role": "area_lead", "area": "web"}),
        )
        .await;
    started["worker"]["name"].as_str().unwrap().to_owned()
}

async fn items(h: &Harness) -> Vec<Value> {
    let (status, body) = h
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/needs-you"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"].as_array().unwrap().clone()
}

async fn agent_change(h: &Harness) -> Option<Value> {
    items(h)
        .await
        .into_iter()
        .find(|item| item["kind"] == "agent_change")
}

async fn record(h: &Harness, query: &str) -> (StatusCode, Value) {
    h.socket(
        Method::POST,
        &format!("/api/v1/projects/{SLUG}/record{query}"),
        None,
    )
    .await
}

fn read(root: &Path, path: &str) -> String {
    std::fs::read_to_string(root.join(path)).unwrap()
}

/// The zsh slip of the incident: a loop whose redirect opened every word of a list as a file in the project folder, emptying the tracked README.md and adding 26 empty files.
fn slip(root: &Path) -> String {
    format!(
        "cd '{}' && : > README.md && for n in $(seq 1 26); do : > \"junk$n.txt\"; done\n",
        root.display()
    )
}

/// Acceptance 36, a worker's change of the project folder: a lead's Bash call empties the tracked README.md and adds 26 empty files in the project folder. fold does not offer them as the human's change: Needs you names the agent and the files, the main agent hears of it, `fld status` marks each file, and `fld record` asks first and records nothing. Only a human or the main agent lists or ignores it, and only a human restores it; the human's restore puts exactly those files into a stash entry and brings main's README.md back, and the item ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_36_a_workers_change_of_the_project_folder_waits_for_the_human_and_restores() {
    let fake = FakeClaude::new("bash_call").running_tool_scripts();
    let h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main(MAIN).await;
    let mut inbox = h.register_inbox(MAIN).await;
    let root = PathBuf::from(h.root());
    fake.set_tool_script(&slip(&root));
    let lead = start_lead(&h).await;

    let item = eventually(WAIT, async || match agent_change(&h).await {
        Some(item) if item["paths"].as_array().map_or(0, Vec::len) == 27 => Ok(item),
        other => Err(other),
    })
    .await;
    assert_eq!(item["agents"], json!([lead]));
    assert_eq!(item["agent"], json!(lead));
    assert_eq!(item["screen"], "repository");
    assert!(
        item["paths"]
            .as_array()
            .unwrap()
            .contains(&json!("README.md"))
    );
    assert!(
        item["reason"]
            .as_str()
            .unwrap()
            .contains("fld agent-changes restore")
    );
    assert!(
        items(&h)
            .await
            .iter()
            .all(|item| item["kind"] != "folder_held"),
        "the worker's files are not the human's waiting change"
    );
    let notice = notice_with(&mut inbox, "changed while").await;
    assert!(
        notice.contains(&format!(
            "the project folder changed while {lead} worked: 27 files"
        )),
        "{notice}"
    );

    let (status, refused) = record(&h, "").await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let details = &refused["error"]["details"];
    assert_eq!(refused["error"]["code"], "record_needs_confirmation");
    assert_eq!(details["emptied"], json!(["README.md"]));
    assert_eq!(details["empty_added_count"], 26);
    assert_eq!(details["agent_changed_count"], 27);
    assert_eq!(details["agents"], json!([lead]));
    assert_eq!(h.head().await.number.to_string(), "r1");

    let (status, folder) = h
        .socket(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/folder"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{folder}");
    let readme = folder["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|change| change["path"] == "README.md")
        .unwrap()
        .clone();
    assert_eq!(readme["agents"], json!([lead]));

    let listed = h
        .call(MAIN, "settle_agent_changes", json!({"choice": "list"}))
        .await;
    assert_eq!(listed["files"].as_array().unwrap().len(), 27);
    let denied = h
        .refused(
            "backend",
            "settle_agent_changes",
            json!({"choice": "ignore"}),
        )
        .await;
    assert_eq!(denied["code"], "main_agent_required");
    // Only a human restores: the main agent's tool lists and ignores.
    let denied = h
        .refused(MAIN, "settle_agent_changes", json!({"choice": "restore"}))
        .await;
    assert_eq!(denied["code"], "invalid_request", "{denied}");
    assert!(root.join("junk1.txt").exists());

    let (status, restored) = h
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/agent-changes/restore"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_eq!(restored["files"].as_array().unwrap().len(), 27);
    assert_eq!(read(&root, "README.md"), "# demo\n");
    assert!(!root.join("junk1.txt").exists());
    assert!(!root.join("junk26.txt").exists());
    let (_, stash) = h
        .socket(Method::GET, &format!("/api/v1/projects/{SLUG}/stash"), None)
        .await;
    assert_eq!(stash["entries"].as_array().unwrap().len(), 1, "{stash}");
    assert!(agent_change(&h).await.is_none());
    let (status, nothing) = record(&h, "").await;
    assert_eq!(status, StatusCode::OK, "{nothing}");
    assert_eq!(nothing["outcome"], "up_to_date");
}

/// Acceptance 36, the record's confirmation: one new empty file of the human's records at once, an emptied tracked file waits for a confirmation that names the folder's tree, and the record from the web goes on with it. A worker's file that the main agent ignores for the user counts as the human's change and records with no question.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_36_a_record_asks_before_an_emptied_file_and_an_ignored_workers_change_is_the_humans()
 {
    let fake = FakeClaude::new("bash_call").running_tool_scripts();
    let h = Harness::start_with_launcher(fake.launcher.clone()).await;
    h.make_main(MAIN).await;
    let root = PathBuf::from(h.root());

    std::fs::create_dir(root.join("pkg")).unwrap();
    std::fs::write(root.join("pkg/__init__.py"), "").unwrap();
    let (status, recorded) = record(&h, "").await;
    assert_eq!(status, StatusCode::OK, "{recorded}");
    assert_eq!(recorded["files"], json!(["pkg/__init__.py"]));

    std::fs::write(root.join("README.md"), "").unwrap();
    let (status, refused) = record(&h, "").await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let details = refused["error"]["details"].clone();
    assert_eq!(details["emptied"], json!(["README.md"]));
    assert_eq!(details["agent_changed_count"], 0);
    let (status, stale) = record(&h, &format!("?confirm={}", "0".repeat(64))).await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    let tree = details["tree"].as_str().unwrap();
    let (status, confirmed) = h
        .http(
            Method::POST,
            &format!("/api/v1/projects/{SLUG}/agent-changes/record?confirm={tree}"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["outcome"], "recorded");
    assert_eq!(confirmed["files"], json!(["README.md"]));

    fake.set_tool_script(&format!(
        "printf 'from the worker\\n' > '{}/notes.txt'\n",
        root.display()
    ));
    start_lead(&h).await;
    eventually(WAIT, async || agent_change(&h).await.ok_or(())).await;
    let ignored = h
        .call(MAIN, "settle_agent_changes", json!({"choice": "ignore"}))
        .await;
    assert_eq!(ignored["ignored"], json!(["notes.txt"]));
    assert!(agent_change(&h).await.is_none());
    let (status, recorded) = record(&h, "").await;
    assert_eq!(status, StatusCode::OK, "{recorded}");
    assert_eq!(recorded["files"], json!(["notes.txt"]));
}
