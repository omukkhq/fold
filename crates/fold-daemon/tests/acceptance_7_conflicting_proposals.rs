#![cfg(test)]

use std::collections::BTreeMap;

use crate::common::acceptance::{Harness, WAIT, Work, read_tree};
use fold_store::testing::eventually;
use fold_store::workspaces::RevisionRow;
use serde_json::{Value, json};

/// Both workers rewrite the one line of `README.md` in `r1`.
const FRONTEND_README: &str = "# demo: the login screen\n";
const BACKEND_README: &str = "# demo: the login endpoint\n";
/// The resolution keeps both changes.
const RESOLVED: &str = "# demo: the login screen and the login endpoint\n";

/// Two proposals that change the same line from `r1`, both approved, after the frontend one integrated as `r2` and the backend one was refused with a conflict.
struct Conflicted {
    harness: Harness,
    second: Work,
    second_proposal: String,
    /// The head of main after the first integration, read before the second `integrate_proposal`.
    before: RevisionRow,
}

async fn status(harness: &Harness, task: &str) -> Value {
    harness
        .call("review", "get_task", json!({"id": task}))
        .await["status"]
        .clone()
}

/// Waits until the project folder holds only `README.md` with `readme`.
async fn main_shows(harness: &Harness, readme: &str) {
    let main = harness.project.paths().root();
    let wanted = BTreeMap::from([("README.md".to_owned(), readme.as_bytes().to_vec())]);
    eventually(WAIT, async || {
        let shown = read_tree(main);
        if shown == wanted { Ok(()) } else { Err(shown) }
    })
    .await;
}

async fn conflicted() -> (Conflicted, Value) {
    let harness = Harness::start().await;
    let screen = harness
        .create_task("review", "Show the product name on the login screen")
        .await;
    let endpoint = harness
        .create_task("review", "Name the login endpoint in the readme")
        .await;
    let first = harness.start_work("frontend", &screen).await;
    let second = harness.start_work("backend", &endpoint).await;
    for work in [&first, &second] {
        let workspace = harness
            .call("review", "get_workspace", json!({"ws": work.workspace}))
            .await;
        assert_eq!(workspace["base"], "r1");
    }
    first.write(&[("README.md", FRONTEND_README)]);
    second.write(&[("README.md", BACKEND_README)]);
    let first_proposal = harness
        .propose("frontend", &first, "Name the login screen")
        .await;
    let second_proposal = harness
        .propose("backend", &second, "Name the login endpoint")
        .await;
    harness.approve(&first_proposal).await;
    harness.approve(&second_proposal).await;

    let integrated = harness
        .call(
            "review",
            "integrate_proposal",
            json!({"id": first_proposal}),
        )
        .await;
    assert_eq!(integrated["revision"], "r2");
    main_shows(&harness, FRONTEND_README).await;
    let before = harness.head().await;
    let error = harness
        .refused(
            "review",
            "integrate_proposal",
            json!({"id": second_proposal}),
        )
        .await;
    (
        Conflicted {
            harness,
            second,
            second_proposal,
            before,
        },
        error,
    )
}

/// Acceptance 7: the second of two proposals over the same line is refused with a conflict report. Main keeps the first integration byte for byte, the second proposal stays open with the report, its task returns to In Progress, its author is told, and nothing completes it.
#[tokio::test]
async fn acceptance_7_second_proposal_is_refused_with_a_conflict_report_and_main_is_intact() {
    let (state, error) = conflicted().await;
    let Conflicted {
        harness,
        second,
        second_proposal,
        before,
    } = &state;

    assert_eq!(error["code"], "integration_conflicted", "{error}");
    assert_eq!(
        error["details"],
        json!({"proposal": second_proposal, "files": [{"path": "README.md", "kind": "text"}]})
    );

    let head = harness.head().await;
    assert_eq!(head.number.to_string(), "r2");
    // The Git job may write r2's commit at any moment, so the row is compared without it.
    assert_eq!(
        (head.hash, head.tree_hash, &head.integration),
        (before.hash, before.tree_hash, &before.integration)
    );
    main_shows(harness, FRONTEND_README).await;
    let r2 = harness
        .call("review", "get_revision", json!({"r": "r2"}))
        .await;
    assert_eq!(r2["tree"], head.tree_hash.to_string());
    let r1 = harness
        .call("review", "get_revision", json!({"r": "r1"}))
        .await;
    let repository = harness
        .call("review", "get_repository_state", json!({}))
        .await;
    assert_eq!(repository["head"], "r2");
    assert_eq!(repository["recent"].as_array().unwrap().len(), 2);

    let view = harness
        .call("backend", "get_proposal", json!({"id": second_proposal}))
        .await;
    assert_eq!(view["state"], "open");
    assert_eq!(view["turn"], "with_author");
    let report = &view["conflict"];
    assert_eq!(report["proposal"], second_proposal.as_str());
    assert_eq!(report["base_revision"], "r1");
    assert_eq!(report["main_revision"], "r2");
    assert_eq!(report["trees"]["base"], r1["tree"]);
    assert_eq!(report["trees"]["main"], r2["tree"]);
    let version_tree = report["trees"]["version"].as_str().unwrap();
    assert_ne!(Some(version_tree), r1["tree"].as_str());
    assert_ne!(Some(version_tree), r2["tree"].as_str());
    let files = report["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{report}");
    assert_eq!(files[0]["path"], "README.md");
    assert_eq!(files[0]["kind"], "text");
    let hunk = &files[0]["hunks"][0];
    assert_eq!(hunk["main"]["lines"], json!([FRONTEND_README]));
    assert_eq!(hunk["version"]["lines"], json!([BACKEND_README]));

    assert_eq!(status(harness, &second.task).await, "in_progress");
    assert_eq!(harness.completions(&second.task).await, 0);

    let inbox = harness.call("backend", "get_inbox", json!({})).await;
    let messages = inbox["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "{inbox}");
    assert_eq!(messages[0]["from"], "fold");
    assert!(
        messages[0]["body"].as_str().unwrap().contains("README.md"),
        "{inbox}"
    );
}

/// Acceptance 7, continued: the author syncs, resolves the markers, records and sends `v2`; `review` approves it and it integrates as `r3` with both changes.
#[tokio::test]
async fn acceptance_7_conflicted_proposal_integrates_after_sync_and_a_new_version() {
    let (state, error) = conflicted().await;
    assert_eq!(error["code"], "integration_conflicted", "{error}");
    let Conflicted {
        harness,
        second,
        second_proposal,
        ..
    } = &state;

    let synced = harness
        .call("backend", "sync_workspace", json!({"ws": second.workspace}))
        .await;
    assert_eq!(synced["base"], "r2");
    assert_eq!(synced["conflicted"], json!(["README.md"]));
    let marked = std::fs::read_to_string(second.dir.join("README.md")).unwrap();
    assert!(marked.contains("<<<<<<<"), "{marked}");
    assert!(marked.contains(FRONTEND_README.trim_end()), "{marked}");
    assert!(marked.contains(BACKEND_README.trim_end()), "{marked}");

    second.write(&[("README.md", RESOLVED)]);
    let recorded = harness
        .call(
            "backend",
            "record_changes",
            json!({"ws": second.workspace, "message": "Keep both names"}),
        )
        .await;
    assert!(recorded["changeset"].as_str().is_some(), "{recorded}");
    let updated = harness
        .call("backend", "update_proposal", json!({"id": second_proposal}))
        .await;
    assert_eq!(updated["version"], "v2");
    assert_eq!(updated["state"], "open");
    let view = harness
        .call("review", "get_proposal", json!({"id": second_proposal}))
        .await;
    assert_eq!(view["base"], "r2");

    harness.approve(second_proposal).await;
    let integrated = harness
        .call(
            "review",
            "integrate_proposal",
            json!({"id": second_proposal, "expected_version": 2}),
        )
        .await;
    assert_eq!(integrated["revision"], "r3");
    assert_eq!(harness.head().await.number.to_string(), "r3");
    assert_eq!(status(harness, &second.task).await, "done");
    assert_eq!(harness.completions(&second.task).await, 1);

    let compared = harness
        .call(
            "review",
            "compare_revisions",
            json!({"from": "r1", "to": "r3", "path": "README.md"}),
        )
        .await;
    let text = compared.to_string();
    assert!(
        text.contains("the login screen and the login endpoint"),
        "{compared}"
    );
    main_shows(harness, RESOLVED).await;
}
