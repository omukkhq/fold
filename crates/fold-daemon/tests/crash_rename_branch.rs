#![cfg(test)]

use std::path::PathBuf;

use axum::http::{Method, StatusCode};
use serde_json::json;

use crate::common::acceptance::Harness;
use crate::human_changes::git;

/// The rename stops after `git branch -m` and before the config names the new branch: the folder's `HEAD` is on `dev` and the config still says `main`. The same rename again finishes it, writing only the config.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rename_cut_off_before_the_config_is_finished_by_the_same_call() {
    let scenario = fail::FailScenario::setup();
    let source = tempfile::TempDir::new().unwrap();
    std::fs::write(source.path().join("README.md"), "# demo\n").unwrap();
    let (h, _) = Harness::start_initialized(source.path()).await;
    let root = PathBuf::from(h.root());
    let (status, body) = h
        .socket(
            Method::POST,
            "/api/v1/projects/demo/git",
            Some(&json!({"git_name": "Ana Lima", "git_email": "ana@example.com"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let config = root.join(".fold/config.toml");
    let before = std::fs::read_to_string(&config).unwrap();

    fail::cfg("rename_branch_before_config", "return").unwrap();
    let rename = json!({"to": "dev"});
    let (status, body) = h
        .socket(
            Method::POST,
            "/api/v1/projects/demo/git/branch",
            Some(&rename),
        )
        .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(
        git(&root, &["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/dev"
    );
    assert_eq!(std::fs::read_to_string(&config).unwrap(), before);

    fail::remove("rename_branch_before_config");
    let (status, body) = h
        .socket(
            Method::POST,
            "/api/v1/projects/demo/git/branch",
            Some(&rename),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"from": "main", "to": "dev", "changed": true}));
    let parsed =
        fold_core::ProjectConfig::from_toml(&std::fs::read_to_string(&config).unwrap()).unwrap();
    assert_eq!(parsed.git.branch, "dev");
    assert_eq!(
        git(&root, &["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/dev"
    );
    scenario.teardown();
}
