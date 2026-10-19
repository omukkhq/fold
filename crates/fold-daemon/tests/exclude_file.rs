#![cfg(test)]

use std::path::PathBuf;

use fold_core::codes;
use fold_daemon::services::projects::RecordOutcome;
use serde_json::json;

use fold_store::testing::eventually;

use crate::common::acceptance::{Harness, WAIT};
use crate::human_changes::{branch_born, file_of, git, record_as_ana};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_the_main_agent_adds_patterns_and_git_and_fold_leave_the_paths_out() {
    let mut h = Harness::start().await;
    branch_born(&h).await;
    h.connect_tagged("main", &["main"]).await;
    let root = PathBuf::from(h.root());
    for (agent, code) in [
        ("frontend", codes::MAIN_AGENT_REQUIRED),
        ("review", codes::MAIN_AGENT_REQUIRED),
    ] {
        let refused = h
            .refused(
                agent,
                "add_exclude_patterns",
                json!({"patterns": ["*.log"]}),
            )
            .await;
        assert_eq!(refused["code"], code, "{agent}: {refused}");
    }
    let bad = h
        .refused(
            "main",
            "add_exclude_patterns",
            json!({"patterns": ["# a comment"]}),
        )
        .await;
    assert_eq!(bad["code"], codes::INVALID_REQUEST, "{bad}");
    let added = h
        .call(
            "main",
            "add_exclude_patterns",
            json!({"patterns": ["local/", "*.log", "local/"]}),
        )
        .await;
    assert_eq!(added["added"], json!(["local/", "*.log"]), "{added}");
    let file = std::fs::read_to_string(root.join(".fold/exclude")).unwrap();
    assert!(
        file.contains("/.fold/\n") && file.ends_with("local/\n*.log\n"),
        "{file}"
    );
    assert_eq!(
        std::fs::read_link(root.join(".git/info/exclude")).unwrap(),
        std::path::Path::new("../../.fold/exclude")
    );

    // The human's own files that the rules exclude reach neither main nor Git.
    std::fs::create_dir_all(root.join("local")).unwrap();
    std::fs::write(root.join("local/notes.md"), "mine\n").unwrap();
    std::fs::write(root.join("debug.log"), "noise\n").unwrap();
    std::fs::write(root.join("plan.md"), "the plan\n").unwrap();
    let status = git(&root, &["status", "--porcelain", "--untracked-files=all"]);
    assert_eq!(status, "?? plan.md\n");
    let outcome = record_as_ana(&h).await.unwrap();
    let RecordOutcome::Recorded { files, .. } = outcome else {
        panic!("a clean record: {outcome:?}");
    };
    assert_eq!(files, ["plan.md"]);
    assert_eq!(file_of(&h, "r2", "debug.log").await, None);

    // A workspace follows the same file: an excluded file there is never recorded or proposed.
    let task = h.create_task("frontend", "Work").await;
    let work = h.start_work("frontend", &task).await;
    work.write(&[("src.txt", "code\n"), ("build.log", "noise\n")]);
    let recorded = h
        .call(
            "frontend",
            "record_changes",
            json!({"ws": work.workspace, "message": "work"}),
        )
        .await;
    let changeset = recorded["changeset"].as_str().unwrap().to_owned();
    let compared = h
        .call(
            "frontend",
            "compare_revisions",
            json!({"from": "r2", "to": changeset}),
        )
        .await;
    let paths: Vec<&str> = compared["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["src.txt"]);

    // A pattern the main agent adds takes effect in the live state of every workspace at once.
    work.write(&[("trace.out", "noise\n")]);
    let ws: fold_core::WorkspaceId = work.workspace.parse().unwrap();
    let lists_trace = || {
        h.handle.live.get(ws).is_some_and(|live| {
            live.view
                .files
                .iter()
                .any(|file| file.path.as_str() == "trace.out")
        })
    };
    eventually(WAIT, async || {
        if lists_trace() {
            Ok(())
        } else {
            Err("not yet")
        }
    })
    .await;
    h.call(
        "main",
        "add_exclude_patterns",
        json!({"patterns": ["*.out"]}),
    )
    .await;
    assert!(!lists_trace());
}
