#![cfg(test)]

use std::fs::{self, OpenOptions};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::daemon_support::{Opened, agent, t};
use axum::http::StatusCode;
use fold_core::{Address, HumanName, MessageBody, RevisionId};
use fold_daemon::mcp::{FoldServer, FooterPart, FooterSource};
use fold_daemon::services::agents::SessionContext;
use fold_daemon::services::events::EventService;
use fold_daemon::services::messages::{MessageService, SendMessage};
use fold_daemon::services::workspaces::StateRef;
use rmcp::model::CallToolRequestParams;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

/// The rows of a file list as `(path, previous path, change, added, removed)`.
type Row = (String, Option<String>, String, u64, u64);

#[cfg(test)]
fn rows(files: &Value) -> Vec<Row> {
    files
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap().to_owned(),
                file["previous_path"].as_str().map(str::to_owned),
                file["change"].as_str().unwrap().to_owned(),
                file["added_lines"].as_u64().unwrap(),
                file["removed_lines"].as_u64().unwrap(),
            )
        })
        .collect()
}

#[cfg(test)]
fn row(path: &str, previous: Option<&str>, change: &str, added: u64, removed: u64) -> Row {
    (
        path.to_owned(),
        previous.map(str::to_owned),
        change.to_owned(),
        added,
        removed,
    )
}

#[cfg(test)]
fn code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap()
}

#[cfg(test)]
fn edit(dir: &std::path::Path) {
    fs::write(dir.join("README.md"), "# demo\nmore\n").unwrap();
    fs::write(dir.join("src/new.rs"), "pub fn y() {}\n").unwrap();
    fs::remove_file(dir.join("src/lib.rs")).unwrap();
    fs::rename(dir.join("docs/old.md"), dir.join("docs/new.md")).unwrap();
}

#[cfg(test)]
fn edited() -> Vec<Row> {
    vec![
        row("README.md", None, "modified", 1, 0),
        row("docs/new.md", Some("docs/old.md"), "renamed", 0, 0),
        row("src/lib.rs", None, "deleted", 0, 1),
        row("src/new.rs", None, "added", 1, 0),
    ]
}

#[tokio::test]
async fn the_workspace_routes_list_one_entry_with_its_attention() {
    let fx = Opened::new().await;
    let (id, _) = fx.create(1).await;
    let tanim = HumanName::parse("tanim").unwrap();
    let now = fx.project.now();
    fx.project
        .registry()
        .write(async |tx| fold_store::projects::insert_human(tx, &tanim, now).await)
        .await
        .unwrap();
    MessageService::new(
        fx.handle.store.clone(),
        fx.state.registry.clone(),
        fx.handle.bus.clone(),
        fx.handle.clock.clone(),
    )
    .send(
        &agent("frontend"),
        SendMessage {
            to: Address::parse_all(&["human:tanim"]).unwrap(),
            body: MessageBody::parse("which one?").unwrap(),
            task: Some(t(1)),
            proposal: None,
            reply_to: None,
        },
    )
    .await
    .unwrap();

    let (status, list) = fx.get("/workspaces").await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let entries = list["workspaces"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry["id"], id.to_string());
    assert_eq!(entry["task"], "T-1");
    assert_eq!(entry["task_title"], "task");
    assert_eq!(entry["owner"], "frontend");
    assert_eq!(entry["kind"], "task");
    assert_eq!(entry["base"], "r1");
    assert_eq!(entry["state"], "active");
    assert_eq!(entry["attention"], json!([{"kind": "question"}]));
    assert_eq!(entry["live"]["workspace"], id.to_string());
    assert_eq!(entry["live"]["summary"], "no changes");
    assert_eq!(entry["watch"], "watching");

    let (status, one) = fx.get(&format!("/workspaces/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&one, entry);

    let (status, unknown) = fx.get("/workspaces/ws-9").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&unknown), "workspace_not_found");
    let (status, bad) = fx.get("/workspaces/T-1").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&bad), "invalid_id");
    let (status, unknown) = fx.get("/workspaces/ws-9/changes").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&unknown), "workspace_not_found");
}

#[tokio::test]
async fn changes_and_the_diff_list_each_edit_against_the_workspace_base() {
    let fx = Opened::new().await;
    let (id, dir) = fx.create(1).await;
    edit(&dir);
    let main = fx.move_main(&[("README.md", b"# demo\nfrom main\n")]).await;
    assert_eq!(main, RevisionId::from_number(2).unwrap());

    let (status, changes) = fx.get(&format!("/workspaces/{id}/changes")).await;
    assert_eq!(status, StatusCode::OK, "{changes}");
    assert_eq!(changes["source"], "working");
    assert_eq!(changes["base"], "r1");
    assert_eq!(rows(&changes["files"]), edited());
    assert_eq!(
        changes["summary"],
        "4 files: 1 modified, 1 added, 1 deleted, 1 renamed (+2 -1)"
    );
    assert_eq!(changes["watch"], "watching");
    assert_eq!(changes["warnings"], json!([]));
    assert_eq!(changes["unfinished_write"], Value::Null);
    assert_eq!(changes["checkpoints"], json!([]));
    assert_eq!(changes["proposal"], Value::Null);

    let (status, diff) = fx.get(&format!("/diff?from=r1&to={id}")).await;
    assert_eq!(status, StatusCode::OK, "{diff}");
    assert_eq!(diff["view"], "files");
    assert_eq!(diff["source"], "working");
    assert_eq!(diff["from"]["kind"], "revision");
    assert_eq!(diff["to"]["kind"], "workspace");
    assert_eq!(diff["to"]["base"], "r1");
    assert_eq!(diff["to"]["worker"], "frontend");
    assert_eq!(rows(&diff["files"]), edited());
    assert_eq!(
        diff["totals"],
        json!({"files": 4, "added_lines": 2, "removed_lines": 1})
    );

    let (status, readme) = fx
        .get(&format!("/diff?from=r1&to={id}&path=README.md"))
        .await;
    assert_eq!(status, StatusCode::OK, "{readme}");
    assert_eq!(readme["view"], "file");
    assert_eq!(readme["source"], "working");
    assert_eq!(readme["change"], "modified");
    assert_eq!(readme["binary"], false);
    assert_eq!(
        readme["hunks"],
        json!([{
            "old_start": 1, "old_lines": 1, "new_start": 1, "new_lines": 2,
            "lines": [
                {"kind": "context", "old_no": 1, "new_no": 1, "text": "# demo\n", "no_newline": false},
                {"kind": "added", "old_no": null, "new_no": 2, "text": "more\n", "no_newline": false},
            ],
        }])
    );
    let (_, renamed) = fx
        .get(&format!("/diff?from=r1&to={id}&path=docs/new.md"))
        .await;
    assert_eq!(renamed["change"], "renamed");
    assert_eq!(renamed["previous_path"], "docs/old.md");
    assert_eq!(renamed["hunks"], json!([]));
    let (_, deleted) = fx
        .get(&format!("/diff?from=r1&to={id}&path=src/lib.rs"))
        .await;
    assert_eq!(deleted["change"], "deleted");
    assert_eq!(deleted["new_hash"], Value::Null);
    assert_eq!(deleted["hunks"][0]["lines"][0]["kind"], "removed");
}

#[tokio::test]
async fn a_binary_file_answers_with_its_hashes_and_no_hunks() {
    let fx = Opened::new().await;
    let (id, dir) = fx.create(1).await;
    fs::write(dir.join("logo.png"), b"\x89PNG\0\x02\x03").unwrap();
    let (status, list) = fx.get(&format!("/diff?from=r1&to={id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["files"][0]["binary"], true);
    let (status, logo) = fx
        .get(&format!("/diff?from=r1&to={id}&path=logo.png"))
        .await;
    assert_eq!(status, StatusCode::OK, "{logo}");
    assert_eq!(logo["binary"], true);
    assert_eq!(logo["hunks"], json!([]));
    let (old, new) = (
        logo["old_hash"].as_str().unwrap(),
        logo["new_hash"].as_str().unwrap(),
    );
    assert_ne!(old, new);
    assert_eq!(list["files"][0]["old_hash"], old);
    assert_eq!(list["files"][0]["new_hash"], new);
}

#[tokio::test]
async fn a_file_rewritten_during_the_read_keeps_its_base_entry_with_a_warning() {
    let fx = Opened::new().await;
    let (id, dir) = fx.create(1).await;
    fs::write(dir.join("README.md"), "# demo\nsteady\n").unwrap();
    let file = OpenOptions::new()
        .write(true)
        .open(dir.join("data/noisy.bin"))
        .unwrap();
    file.set_len(8 << 20).unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let writer = {
        let running = Arc::clone(&running);
        std::thread::spawn(move || {
            let mut step = 0u64;
            while running.load(Ordering::Relaxed) {
                step += 1;
                file.set_len((8 << 20) + step % 2).unwrap();
            }
        })
    };
    let unstable = json!([{"path": "data/noisy.bin", "reason": "unstable"}]);
    // A writer thread the scheduler holds back for three whole reads lets one scan see the file steady, so each route is asked until a scan sees it change.
    let until_unstable = async |path: String| {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let (status, body) = fx.get(&path).await;
                if status != StatusCode::OK || body["warnings"] == unstable {
                    return (status, body);
                }
            }
        })
        .await
        .expect("no scan saw the file change")
    };
    let diff = until_unstable(format!("/diff?from=r1&to={id}")).await;
    let changes = until_unstable(format!("/workspaces/{id}/changes")).await;
    running.store(false, Ordering::Relaxed);
    writer.join().unwrap();
    let steady = vec![row("README.md", None, "modified", 1, 0)];
    let (status, diff) = diff;
    assert_eq!(status, StatusCode::OK, "{diff}");
    assert_eq!(rows(&diff["files"]), steady);
    assert_eq!(diff["warnings"], unstable);
    let (status, changes) = changes;
    assert_eq!(status, StatusCode::OK, "{changes}");
    assert_eq!(rows(&changes["files"]), steady);
    assert_eq!(changes["warnings"], unstable);
}

#[tokio::test]
async fn recorded_states_list_the_files_the_comparison_module_does() {
    let fx = Opened::new().await;
    let (id, dir) = fx.create(1).await;
    edit(&dir);
    let recorded = fx
        .workspaces()
        .record_changes(&agent("frontend"), id, "edit the files")
        .await
        .unwrap();
    fx.move_main(&[("README.md", b"# demo\nfrom main\n"), ("extra.txt", b"x\n")])
        .await;
    let changeset = recorded.changeset;
    for (from, to) in [
        ("r1".to_owned(), "r2".to_owned()),
        ("r1".to_owned(), changeset.to_string()),
        (changeset.to_string(), "r2".to_owned()),
    ] {
        let (status, diff) = fx.get(&format!("/diff?from={from}&to={to}")).await;
        assert_eq!(status, StatusCode::OK, "{diff}");
        assert_eq!(diff["source"], "recorded");
        let expected = fx
            .workspaces()
            .compare_states(
                StateRef::parse(&from).unwrap(),
                StateRef::parse(&to).unwrap(),
            )
            .await
            .unwrap();
        let expected: Vec<Row> = expected
            .files
            .iter()
            .map(|file| {
                (
                    file.path.to_string(),
                    file.old_path.as_ref().map(ToString::to_string),
                    file.change.as_str().to_owned(),
                    u64::from(file.added_lines),
                    u64::from(file.removed_lines),
                )
            })
            .collect();
        assert!(!expected.is_empty());
        assert_eq!(rows(&diff["files"]), expected, "{from}..{to}");
    }
    let (_, against_main) = fx.get(&format!("/diff?from=r1&to={changeset}")).await;
    assert_eq!(rows(&against_main["files"]), edited());
}

#[tokio::test]
async fn the_workspace_tree_lists_the_working_files_of_one_directory() {
    let fx = Opened::new().await;
    let (id, dir) = fx.create(1).await;
    edit(&dir);

    let (status, root) = fx.get(&format!("/workspaces/{id}/tree")).await;
    assert_eq!(status, StatusCode::OK, "{root}");
    let listed = |tree: &Value| -> Vec<(String, String)> {
        tree["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e["path"].as_str().unwrap().to_owned(),
                    e["kind"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    let top = listed(&root);
    assert!(top.contains(&("src".to_owned(), "directory".to_owned())));
    assert!(top.contains(&("README.md".to_owned(), "file".to_owned())));

    let (_, src) = fx.get(&format!("/workspaces/{id}/tree?path=src")).await;
    assert_eq!(src["path"], "src");
    assert_eq!(listed(&src), [("src/new.rs".to_owned(), "file".to_owned())]);

    for (path, status, expected) in [
        ("path=nope", StatusCode::NOT_FOUND, "not_found"),
        ("path=README.md", StatusCode::NOT_FOUND, "not_found"),
        ("path=../x", StatusCode::BAD_REQUEST, "invalid_path"),
    ] {
        let (got, body) = fx.get(&format!("/workspaces/{id}/tree?{path}")).await;
        assert_eq!((got, code(&body)), (status, expected), "{path}");
    }
    let (status, body) = fx.get("/workspaces/ws-99/tree").await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::NOT_FOUND, "workspace_not_found")
    );

    fx.workspaces()
        .abandon_workspace(&agent("frontend"), id, "the approach does not work")
        .await
        .unwrap();
    let (status, body) = fx.get(&format!("/workspaces/{id}/tree")).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, "workspace_not_active")
    );
}

#[tokio::test]
async fn bad_references_unknown_ids_and_bad_paths_give_their_codes() {
    let fx = Opened::new().await;
    let (id, _) = fx.create(1).await;
    let cases = [
        (
            "/diff?from=x9&to=r1",
            StatusCode::BAD_REQUEST,
            "invalid_state_ref",
        ),
        (
            "/diff?from=r1&to=T-1",
            StatusCode::BAD_REQUEST,
            "invalid_state_ref",
        ),
        ("/diff?from=r1", StatusCode::BAD_REQUEST, "invalid_request"),
        (
            "/diff?from=r99&to=r1",
            StatusCode::NOT_FOUND,
            "revision_not_found",
        ),
        (
            "/diff?from=r1&to=ws-99",
            StatusCode::NOT_FOUND,
            "workspace_not_found",
        ),
        ("/diff?from=r1&to=C-99", StatusCode::NOT_FOUND, "not_found"),
        ("/diff?from=r1&to=ck-99", StatusCode::NOT_FOUND, "not_found"),
        (
            "/diff?from=r1&to=r1&path=../etc/passwd",
            StatusCode::BAD_REQUEST,
            "invalid_path",
        ),
        (
            "/diff?from=r1&to=r1&path=src/../lib.rs",
            StatusCode::BAD_REQUEST,
            "invalid_path",
        ),
        (
            "/diff?from=r1&to=r1&path=/etc/passwd",
            StatusCode::BAD_REQUEST,
            "invalid_path",
        ),
        (
            "/diff?from=r1&to=r1&path=README.md",
            StatusCode::NOT_FOUND,
            "path_not_in_comparison",
        ),
    ];
    for (path, status, expected) in cases {
        let (got, body) = fx.get(path).await;
        assert_eq!((got, code(&body)), (status, expected), "{path}");
    }
    let (status, escaped) = fx
        .get(&format!(
            "/diff?from=r1&to={id}&path=..%2F..%2Fetc%2Fpasswd"
        ))
        .await;
    assert_eq!(
        (status, code(&escaped)),
        (StatusCode::BAD_REQUEST, "invalid_path")
    );
}

struct OneUnread;

impl FooterSource for OneUnread {
    fn part(&self) -> FooterPart {
        FooterPart::Unread
    }

    fn count<'a>(
        &'a self,
        _session: &'a SessionContext,
    ) -> Pin<Box<dyn Future<Output = u64> + Send + 'a>> {
        Box::pin(async { 1 })
    }
}

#[tokio::test]
async fn the_tool_answers_the_same_files_and_counts_as_the_route() {
    let fx = Opened::new().await;
    let (id, dir) = fx.create(1).await;
    edit(&dir);
    let agents = fx.handle.agents();
    let session = agents.start_session(9).await.unwrap();
    let server = FoldServer::new(
        agents,
        EventService::new(fx.handle.store.clone(), fx.handle.bus.clone()),
        fx.workspaces(),
        fx.handle.tasks.clone(),
        MessageService::new(
            fx.handle.store.clone(),
            fx.state.registry.clone(),
            fx.handle.bus.clone(),
            fx.handle.clock.clone(),
        ),
        session,
    )
    .with_footer_source(Arc::new(OneUnread));
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_end).await {
            let _ = running.waiting().await;
        }
    });
    let client: rmcp::service::RunningService<RoleClient, ()> = ().serve(client_end).await.unwrap();
    let call = async |tool: &str, arguments: Value| {
        let Value::Object(arguments) = arguments else {
            panic!("arguments must be an object");
        };
        client
            .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
            .await
            .unwrap()
    };
    call("register_agent", json!({"name": "frontend"})).await;
    let result = call("get_workspace_changes", json!({"ws": id.to_string()})).await;
    assert_eq!(result.is_error, Some(false), "{result:?}");
    assert_eq!(result.content.len(), 2);
    assert!(
        result.content[1]
            .as_text()
            .unwrap()
            .text
            .starts_with("fold: 1 unread")
    );
    let tool: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    let (_, route) = fx.get(&format!("/workspaces/{id}/changes")).await;
    let from_tool: Vec<Row> = tool["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap().to_owned(),
                file["old_path"].as_str().map(str::to_owned),
                file["change"].as_str().unwrap().to_owned(),
                file["added"].as_u64().unwrap(),
                file["removed"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(from_tool, rows(&route["files"]));
    assert_eq!(tool["summary"], route["summary"]);
    assert_eq!(tool["base"], "r1");
    assert_eq!(tool["last_activity_at"], route["last_activity_at"]);
    assert_eq!(tool["checkpoints"], 0);
    assert_eq!(tool["proposal"], Value::Null);
    assert!(tool.get("unfinished_write").is_none());

    let missing = call("get_workspace_changes", json!({"ws": "ws-9"})).await;
    assert_eq!(missing.is_error, Some(true));
    let error: Value = serde_json::from_str(&missing.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(error["error"]["code"], "workspace_not_found");
}
