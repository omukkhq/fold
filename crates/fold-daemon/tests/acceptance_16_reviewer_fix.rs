#![cfg(test)]

use std::collections::BTreeMap;
use std::path::Path;

use crate::common::acceptance::{Harness, WAIT, assert_in_order, read_tree};
use fold_store::testing::eventually;
use serde_json::{Value, json};

const ENDPOINT: (&str, &str) = ("src/login.txt", "POST /login\n");

/// `README.md` of `r1` plus `files`, as a workspace or a checkout holds them.
fn tree_with(files: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
    let mut tree = BTreeMap::from([("README.md".to_owned(), b"# demo\n".to_vec())]);
    for (path, content) in files {
        tree.insert((*path).to_owned(), content.as_bytes().to_vec());
    }
    tree
}

async fn status(harness: &Harness, task: &str) -> Value {
    harness
        .call("review", "get_task", json!({"id": task}))
        .await["status"]
        .clone()
}

/// Acceptance 16: review finds that backend's endpoint misses its method check, fixes it in its own checkout and sends the fix as `v2` under its name, with the file, the reason and the checks. fold leaves backend's workspace files as they were, review approves its own `v2` and integrates it, and main and the project folder hold the fix.
#[tokio::test]
async fn acceptance_16_the_reviewer_fixes_the_proposal_in_its_checkout_and_integrates_its_own_version()
 {
    const FIXED: (&str, &str) = ("src/login.txt", "POST /login\nGET /login -> 405\n");
    let harness = Harness::start().await;
    let task = harness
        .create_task("backend", "Add the login endpoint")
        .await;
    let work = harness.start_work("backend", &task).await;
    work.write(&[ENDPOINT]);
    let proposal = harness
        .propose("backend", &work, "Add the login endpoint")
        .await;
    harness.give_review("review", &proposal).await;
    let checkout = harness
        .call("review", "create_review_checkout", json!({"id": proposal}))
        .await;
    let checkout_dir = Path::new(harness.root()).join(checkout["path"].as_str().unwrap());
    std::fs::write(checkout_dir.join(FIXED.0), FIXED.1).unwrap();

    let fixed = harness
        .call(
            "review",
            "submit_review_fix",
            json!({
                "id": proposal,
                "version": 1,
                "fixes": [{"path": FIXED.0, "reason": "GET was not refused; the endpoint now answers 405"}],
                "checks": "check-login: 2 passed",
            }),
        )
        .await;
    assert_eq!(fixed["version"], "v2");
    assert_eq!(fixed["from_version"], "v1");
    assert_eq!(fixed["checkout"], checkout["workspace"]);
    assert_eq!(status(&harness, &task).await, "review");
    let view = harness
        .call("backend", "get_proposal", json!({"id": proposal}))
        .await;
    assert_eq!(view["versions"][1]["created_by"], "review");
    assert_eq!(read_tree(&work.dir), tree_with(&[ENDPOINT]));

    let approved = harness
        .call(
            "review",
            "submit_review",
            json!({"id": proposal, "version": 2, "verdict": "approve", "body": "Fix: src/login.txt refuses GET with 405. check-login: 2 passed"}),
        )
        .await;
    assert_eq!(approved["verdict"], "approve");
    let integrated = harness
        .call(
            "review",
            "integrate_proposal",
            json!({"id": proposal, "expected_version": 2}),
        )
        .await;
    assert_eq!(integrated["result"], "succeeded");
    assert_eq!(status(&harness, &task).await, "done");
    let shown = harness.project.paths().root().join(FIXED.0);
    let content = eventually(WAIT, async || {
        std::fs::read_to_string(&shown)
            .map_err(|error| error.kind().to_string())
            .and_then(|text| {
                if text == FIXED.1 {
                    Ok(text)
                } else {
                    Err("not yet".to_owned())
                }
            })
    })
    .await;
    assert_eq!(content, FIXED.1);

    let events = harness.events().await;
    let from_opened: Vec<_> = events
        .iter()
        .skip_while(|event| event.kind != "proposal.opened")
        .cloned()
        .collect();
    assert_in_order(
        &from_opened,
        &[
            "proposal.opened",
            "proposal.reviewer_fixed",
            "review.submitted",
            "integration.succeeded",
        ],
    );
}
