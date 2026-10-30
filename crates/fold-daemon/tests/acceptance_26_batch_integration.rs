#![cfg(test)]

use serde_json::json;

use crate::common::acceptance::{Harness, assert_in_order};

/// Acceptance 26: a review batch is reviewed and integrated end to end. Three related proposals go to one reviewer as a batch. The reviewer sends one back and approves the other two, and while the one sent back has no new version the batch is ready: the reviewer integrates it with one command, which publishes the two approved members together, one revision each, and the proposals show the batch they integrated with. The member that went back is out of the batch: its next version is reviewed and integrated on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acceptance_26_the_approved_members_of_a_batch_integrate_together() {
    let h = Harness::start().await;
    let header = h.create_task("frontend", "Add the page header").await;
    let api = h
        .create_task_for("frontend", "backend", "Add the header API")
        .await;
    let footer = h.create_task("frontend", "Add the page footer").await;

    let header_work = h.start_work("frontend", &header).await;
    header_work.write(&[("src/header.txt", "<header>\n")]);
    let first = h
        .propose("frontend", &header_work, "Add the page header")
        .await;
    let api_work = h.start_work("backend", &api).await;
    api_work.write(&[("src/header_api.txt", "GET /header\n")]);
    let second = h.propose("backend", &api_work, "Add the header API").await;
    let footer_work = h.start_work("frontend", &footer).await;
    footer_work.write(&[("src/footer.txt", "<footer>\n")]);
    let third = h
        .propose("frontend", &footer_work, "Add the page footer")
        .await;

    let batch = h.give_batch("review", &[&first, &second, &third]).await;
    let early = h
        .refused("review", "integrate_review_batch", json!({"id": batch}))
        .await;
    assert_eq!(early["details"]["reason"], "batch_not_submitted", "{early}");
    h.call("review", "create_batch_checkout", json!({"id": batch}))
        .await;
    h.call(
        "review",
        "submit_batch_review",
        json!({"id": batch, "verdicts": [
            {"proposal": first, "verdict": "request_changes", "body": "Use the shared header style."},
            {"proposal": second, "verdict": "approve"},
            {"proposal": third, "verdict": "approve"},
        ]}),
    )
    .await;

    let ready = h
        .call("review", "get_review_batch", json!({"id": batch}))
        .await;
    assert_eq!(ready["integration"]["state"], "ready", "{ready}");
    assert_eq!(ready["integration"]["members"], json!([second, third]));
    let integrated = h
        .call("review", "integrate_review_batch", json!({"id": batch}))
        .await;
    assert_eq!(integrated["result"], "together", "{integrated}");
    assert_eq!(
        integrated["members"],
        json!([
            {"proposal": second, "result": "succeeded", "revision": "r2"},
            {"proposal": third, "result": "succeeded", "revision": "r3"},
        ])
    );
    assert_eq!(h.head().await.number.to_string(), "r3");
    for (proposal, revision) in [(&second, "r2"), (&third, "r3")] {
        let view = h
            .call("frontend", "get_proposal", json!({"id": proposal}))
            .await;
        assert_eq!(view["state"], "integrated");
        assert_eq!(
            view["integrated"],
            json!({"revision": revision, "batch": batch}),
            "{view}"
        );
    }
    for task in [&api, &footer] {
        assert_eq!(h.completions(task).await, 1);
    }
    let ended = h
        .call("review", "get_review_batch", json!({"id": batch}))
        .await;
    assert_eq!(ended["integration"]["state"], "ended");
    assert_eq!(ended["integration"]["outcome"], "together");
    let again = h
        .refused("review", "integrate_review_batch", json!({"id": batch}))
        .await;
    assert_eq!(again["details"]["reason"], "batch_integrated");

    h.call(
        "frontend",
        "sync_workspace",
        json!({"ws": header_work.workspace}),
    )
    .await;
    header_work.write(&[("src/header.txt", "<header class=shared>\n")]);
    h.call(
        "frontend",
        "record_changes",
        json!({"ws": header_work.workspace, "message": "Use the shared header style"}),
    )
    .await;
    let updated = h
        .call(
            "frontend",
            "update_proposal",
            json!({"intent": "Make the change the task asks for.", "id": first}),
        )
        .await;
    assert_eq!(updated["version"], "v2", "{updated}");
    h.approve(&first).await;
    let alone = h
        .call(
            "review",
            "integrate_proposal",
            json!({"id": first, "expected_version": 2}),
        )
        .await;
    assert_eq!(alone["revision"], "r4", "{alone}");
    let view = h
        .call("frontend", "get_proposal", json!({"id": first}))
        .await;
    assert_eq!(
        view["integrated"],
        json!({"revision": "r4", "batch": null}),
        "{view}"
    );
    assert_in_order(
        &h.events().await,
        &[
            "review.batch_submitted",
            "integration.succeeded",
            "proposal.integrated",
            "integration.succeeded",
            "proposal.integrated",
            "review.batch_integrated",
            "proposal.version_added",
            "integration.succeeded",
        ],
    );
}
