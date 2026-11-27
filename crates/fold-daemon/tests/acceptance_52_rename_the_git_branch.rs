#![cfg(test)]

use std::path::{Path, PathBuf};

use axum::http::{Method, StatusCode};
use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::common::acceptance::{Harness, WAIT};
use crate::human_changes::{git, integrate};

async fn socket(
    h: &Harness,
    method: Method,
    route: &str,
    body: Option<&Value>,
) -> (StatusCode, Value) {
    h.socket(method, &format!("/api/v1/projects/demo/{route}"), body)
        .await
}

async fn rename(h: &Harness, to: &str) -> (StatusCode, Value) {
    socket(h, Method::POST, "git/branch", Some(&json!({"to": to}))).await
}

fn add_remote(root: &Path, remote: &Path) {
    let config = root.join(".fold/config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str(&format!(
        "\n[remotes.origin]\nurl = {:?}\nbranch = \"dev\"\npush_on_integrate = false\n",
        remote.to_str().unwrap()
    ));
    std::fs::write(config, text).unwrap();
}

fn branch_ref(root: &Path, branch: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
}

/// Acceptance 52, the local Git branch takes the name of the remote branch: a project whose remote `origin` takes main on its branch `dev` shows agents both names in `get_repository_state`, the local `main` and `origin` → `dev`. `fld git branch dev` renames the local branch and the `[git] branch` of the config together under the Git locks, so the folder is on `dev`, the next integration commits to `dev` and no `head_moved` attention comes up. A rename to a branch that exists is refused, the same rename again changes nothing, and after a rename by hand the hint never names the branch that is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance_52_the_local_git_branch_is_renamed_to_match_the_remote() {
    let source = tempfile::TempDir::new().unwrap();
    std::fs::write(source.path().join("README.md"), "# demo\n").unwrap();
    let (mut h, _) = Harness::start_initialized(source.path()).await;
    crate::common::roles::give_role(
        &h.handle.store,
        "review",
        fold_core::roles::Role::Reviewer,
        None,
    )
    .await;
    h.lead_the_work_area().await;
    let root = PathBuf::from(h.root());

    // 1. Without Git sync the state has no Git part.
    let state = h.call("review", "get_repository_state", json!({})).await;
    assert_eq!(state["git"], Value::Null, "{state}");

    // 2. Git sync on and a remote on `dev`: agents see the local branch and the remote branch apart.
    let (status, body) = socket(
        &h,
        Method::POST,
        "git",
        Some(&json!({"git_name": "Ana Lima", "git_email": "ana@example.com"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let remote = tempfile::TempDir::new().unwrap();
    git(
        remote.path(),
        &["init", "--quiet", "--bare", "--initial-branch=dev"],
    );
    add_remote(&root, remote.path());
    let state = h.call("review", "get_repository_state", json!({})).await;
    assert_eq!(
        state["git"],
        json!({"branch": "main", "remotes": [{"name": "origin", "branch": "dev", "push_on_integrate": false}]}),
        "{state}"
    );
    let (status, body) = socket(&h, Method::GET, "repository", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["git"]["branch"], "main", "{body}");
    assert_eq!(body["git"]["remotes"][0]["branch"], "dev", "{body}");

    // 3. A rename to a branch that exists is refused and changes nothing.
    git(&root, &["branch", "taken"]);
    let (status, body) = rename(&h, "taken").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["error"]["details"]["reason"], "branch_exists",
        "{body}"
    );
    assert_eq!(
        git(&root, &["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/main"
    );
    git(&root, &["branch", "-D", "taken"]);

    // 4. The rename: the folder is on `dev`, the config says `dev`, and `main` is gone from Git.
    let before = branch_ref(&root, "main").unwrap();
    let (status, body) = rename(&h, "dev").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"from": "main", "to": "dev", "changed": true}));
    assert_eq!(
        git(&root, &["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/dev"
    );
    assert_eq!(branch_ref(&root, "dev").unwrap(), before);
    assert_eq!(branch_ref(&root, "main"), None);
    let config = std::fs::read_to_string(root.join(".fold/config.toml")).unwrap();
    let parsed = fold_core::ProjectConfig::from_toml(&config).unwrap();
    assert_eq!(parsed.git.branch, "dev");
    assert_eq!(parsed.remotes["origin"].branch, "dev");
    let state = h.call("review", "get_repository_state", json!({})).await;
    assert_eq!(state["git"]["branch"], "dev", "{state}");
    let (status, body) = rename(&h, "dev").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"], false, "{body}");

    // 5. The next integration commits to `dev`, and the branch never counts as moved.
    let revision = integrate(&h, "frontend", &[("src.txt", "agent\n")]).await;
    eventually(WAIT, async || {
        let shown = h
            .call("review", "get_revision", json!({"r": revision}))
            .await;
        match shown["commit"].as_str() {
            Some(commit) if branch_ref(&root, "dev").as_deref() == Some(commit) => Ok(()),
            _ => Err(shown),
        }
    })
    .await;
    assert_eq!(branch_ref(&root, "main"), None);
    assert!(h.handle.git_attention.kinds().is_empty());
    assert_eq!(git(&root, &["status", "--porcelain"]), "");

    // 6. A rename by hand leaves the configured branch gone: the hint names the rename command, never `git switch dev`.
    git(&root, &["branch", "-m", "dev", "trunk"]);
    let (status, body) = socket(&h, Method::GET, "folder", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let next = body["blockers"][0]["next"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(next.contains("`fld git branch trunk`"), "{body}");
    assert!(!next.contains("git switch dev"), "{body}");
    let (status, body) = rename(&h, "trunk").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"from": "dev", "to": "trunk", "changed": true}));
    let (status, body) = socket(&h, Method::GET, "folder", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["blockers"], json!([]), "{body}");
}
