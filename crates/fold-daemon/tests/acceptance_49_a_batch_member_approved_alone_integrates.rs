#![cfg(test)]

use std::path::Path;

use fold_store::testing::eventually;
use serde_json::{Value, json};

use crate::auto_integrate::Running;
use crate::common::acceptance::{Harness, WAIT, assert_in_order};

/// Whether `member` is still in the batch of the `get_review_batch` result `batch`.
fn in_batch(batch: &Value, member: &str) -> bool {
    batch["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|line| line["proposal"] == member)
        .expect("the batch lists the member")["in_batch"]
        == true
}

async fn proposal(h: &Harness, id: &str) -> Value {
    h.call("review", "get_proposal", json!({"id": id})).await
}

/// Acceptance 49: a member of an open review batch that gets its verdict alone is never held by the batch. The reviewer holds two proposals as one batch and finds a defect in one of them. It fixes that member in its own review checkout with `submit_review_fix` and approves the fixed version with `submit_review`. The verdict takes the member out of the batch, and fold integrates it automatically while the other member stays in the open batch, which still waits for its review.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acceptance_49_a_batch_member_fixed_and_approved_alone_integrates_automatically() {
    const FIXED: (&str, &str) = ("src/login.txt", "POST /login\nGET /login -> 405\n");
    let h = Harness::start().await;
    let login = h.create_task("backend", "Add the login endpoint").await;
    let footer = h.create_task("frontend", "Add the page footer").await;
    let login_work = h.start_work("backend", &login).await;
    login_work.write(&[("src/login.txt", "POST /login\n")]);
    let first = h
        .propose("backend", &login_work, "Add the login endpoint")
        .await;
    let footer_work = h.start_work("frontend", &footer).await;
    footer_work.write(&[("src/footer.txt", "<footer>\n")]);
    let second = h
        .propose("frontend", &footer_work, "Add the page footer")
        .await;
    let batch = h.give_batch("review", &[&first, &second]).await;
    let integrate = Running::start(&h.state);

    let checkout = h
        .call("review", "create_review_checkout", json!({"id": first}))
        .await;
    let dir = Path::new(h.root()).join(checkout["path"].as_str().unwrap());
    std::fs::write(dir.join(FIXED.0), FIXED.1).unwrap();
    let fixed = h
        .call(
            "review",
            "submit_review_fix",
            json!({
                "intent": "Make the change the task asks for.",
                "id": first,
                "version": 1,
                "fixes": [{"path": FIXED.0, "reason": "GET was not refused; the endpoint now answers 405"}],
                "checks": "check-login: 2 passed",
            }),
        )
        .await;
    assert_eq!(fixed["version"], "v2", "{fixed}");
    // The fixed member is still in the batch, at the version it joined at, until it gets a verdict.
    let before = h
        .call("review", "get_review_batch", json!({"id": batch}))
        .await;
    assert!(in_batch(&before, &first), "{before}");

    h.call(
        "review",
        "submit_review",
        json!({"id": first, "version": 2, "verdict": "approve", "body": "Fix: GET answers 405. check-login: 2 passed"}),
    )
    .await;
    let integrated = eventually(WAIT, async || {
        let read = proposal(&h, &first).await;
        if read["state"] == "integrated" {
            Ok(read)
        } else {
            Err(read["state"].clone())
        }
    })
    .await;
    assert_eq!(
        integrated["integrated"]["batch"],
        Value::Null,
        "{integrated}"
    );
    assert_eq!(h.completions(&login).await, 1);

    let after = h
        .call("review", "get_review_batch", json!({"id": batch}))
        .await;
    assert_eq!(after["state"], "open", "{after}");
    assert!(!in_batch(&after, &first), "{after}");
    assert!(in_batch(&after, &second), "{after}");
    assert_eq!(proposal(&h, &second).await["state"], "open");
    integrate.stop().await;

    let events = h.events().await;
    let released = events
        .iter()
        .find(|event| event.kind == "review.submitted")
        .expect("the verdict is recorded");
    assert_eq!(released.data["released_from_batch"], batch.as_str());
    assert_in_order(
        &events,
        &[
            "proposal.reviewer_fixed",
            "review.submitted",
            "integration.succeeded",
            "proposal.integrated",
        ],
    );
}
