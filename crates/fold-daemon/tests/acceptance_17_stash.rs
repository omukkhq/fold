#![cfg(test)]

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{HUMAN, Harness, WAIT};
use crate::folder_stash::wait_for;
use crate::human_changes::{branch_born, git, integrate};

/// A `fld` command as the human runs it: a request over the socket.
async fn fld(h: &Harness, method: Method, action: &str) -> (StatusCode, Value) {
    h.socket(method, &format!("/api/v1/projects/demo/{action}"), None)
        .await
}

async fn fld_ok(h: &Harness, method: Method, action: &str) -> Value {
    let (status, body) = fld(h, method, action).await;
    assert_eq!(status, StatusCode::OK, "{action}: {body}");
    body
}

async fn needs_you_kinds(h: &Harness) -> Vec<String> {
    let (status, body) = h
        .http(Method::GET, "/api/v1/projects/demo/needs-you", None, HUMAN)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["kind"].as_str().unwrap().to_owned())
        .collect()
}

fn write(root: &Path, path: &str, text: &str) {
    std::fs::write(root.join(path), text).unwrap();
}

fn read(root: &Path, path: &str) -> String {
    std::fs::read_to_string(root.join(path)).unwrap()
}

/// Acceptance 17, the human's stash: the human has unfinished changes in the project folder while an agent's integration changes the same file, so the folder waits. `fld stash` puts the changes aside and the folder follows main at once; `fld unstash` merges them back cleanly and `fld record` records them. In a second round the stashed change conflicts with main: `fld unstash` writes conflict markers and keeps the entry, Needs you lists it, `fld record` refuses the marked file, and after the human fixes it `fld record` works and the entry is dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_17_the_human_stashes_unfinished_changes_while_agents_integrate() {
    let h = Harness::start().await;
    branch_born(&h).await;
    let root = PathBuf::from(h.root());
    integrate(&h, "frontend", &[("docs.md", "a\nb\nc\n")]).await;
    wait_for(&root, "docs.md", "a\nb\nc\n").await;

    // The human's unfinished edit holds the folder when an agent changes the same file.
    write(&root, "docs.md", "a\nb\nmine\n");
    write(&root, "notes.md", "my notes\n");
    let revision = integrate(&h, "frontend", &[("docs.md", "main\nb\nc\n")]).await;
    eventually(WAIT, async || {
        let kinds = needs_you_kinds(&h).await;
        if kinds.contains(&"folder_held".to_owned()) {
            Ok(())
        } else {
            Err(kinds)
        }
    })
    .await;
    assert_eq!(read(&root, "docs.md"), "a\nb\nmine\n");

    let stashed = fld_ok(&h, Method::POST, "stash").await;
    assert_eq!(
        stashed,
        json!({"outcome": "stashed", "revision": revision, "files": ["docs.md", "notes.md"], "entries": 1, "skipped": [], "unstash": null})
    );
    assert_eq!(read(&root, "docs.md"), "main\nb\nc\n");
    assert!(!root.join("notes.md").exists());
    assert!(
        !needs_you_kinds(&h)
            .await
            .contains(&"folder_held".to_owned())
    );
    let listed = fld_ok(&h, Method::GET, "stash").await;
    assert_eq!(listed["entries"][0]["stash"], "stash@{0}");
    assert_eq!(
        listed["entries"][0]["files"],
        json!(["docs.md", "notes.md"])
    );

    let applied = fld_ok(&h, Method::POST, "unstash").await;
    assert_eq!(applied["outcome"], "applied", "{applied}");
    assert_eq!(read(&root, "docs.md"), "main\nb\nmine\n");
    assert_eq!(read(&root, "notes.md"), "my notes\n");
    let recorded = fld_ok(&h, Method::POST, "record").await;
    assert_eq!(recorded["outcome"], "recorded", "{recorded}");
    assert_eq!(recorded["files"], json!(["docs.md", "notes.md"]));
    assert_eq!(fld_ok(&h, Method::GET, "stash").await["entries"], json!([]));

    // A second round: the stashed change conflicts with what main brings meanwhile.
    write(&root, "README.md", "# mine\n");
    fld_ok(&h, Method::POST, "stash").await;
    let revision = integrate(&h, "frontend", &[("README.md", "# fold\n")]).await;
    wait_for(&root, "README.md", "# fold\n").await;
    let conflicted = fld_ok(&h, Method::POST, "unstash").await;
    assert_eq!(conflicted["outcome"], "conflicted", "{conflicted}");
    assert_eq!(conflicted["revision"], revision);
    assert_eq!(
        conflicted["conflicted"],
        json!([{"path": "README.md", "kind": "both_modified_text", "sidecar": null}])
    );
    assert_eq!(
        read(&root, "README.md"),
        format!("<<<<<<< main ({revision})\n# fold\n=======\n# mine\n>>>>>>> stash@{{0}}\n")
    );
    assert_eq!(
        fld_ok(&h, Method::GET, "stash").await["entries"][0]["conflicted"],
        json!(["README.md"])
    );
    // The unstash conflict stands for the held project folder, so Needs you shows it and no folder item.
    let kinds = needs_you_kinds(&h).await;
    assert!(
        kinds.contains(&"unstash_conflicted".to_owned()),
        "{kinds:?}"
    );
    assert!(!kinds.contains(&"folder_held".to_owned()), "{kinds:?}");
    let (status, refused) = fld(&h, Method::POST, "record").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["error"]["details"]["reason"], "stash_markers");
    assert_eq!(refused["error"]["details"]["paths"], json!(["README.md"]));

    write(&root, "README.md", "# fold, and mine\n");
    let recorded = fld_ok(&h, Method::POST, "record").await;
    assert_eq!(recorded["outcome"], "recorded", "{recorded}");
    assert_eq!(recorded["files"], json!(["README.md"]));
    assert_eq!(fld_ok(&h, Method::GET, "stash").await["entries"], json!([]));
    assert!(
        !needs_you_kinds(&h)
            .await
            .contains(&"unstash_conflicted".to_owned())
    );
    // The folder follows main and its branch holds the record: Git shows nothing of the human's left.
    eventually(WAIT, async || {
        let status = git(
            &root,
            &[
                "--no-optional-locks",
                "status",
                "--porcelain",
                "--untracked-files=no",
            ],
        );
        if status.is_empty() {
            Ok(())
        } else {
            Err(status)
        }
    })
    .await;
}
