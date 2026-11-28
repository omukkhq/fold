#![cfg(test)]

use fold_core::WorkerState;
use serde_json::json;

use crate::acceptance_50_a_serial_lane_integrates_in_turn::{
    LEAD, integrated_in_order, migration, proposed_with, status,
};
use crate::acceptance_73_a_lane_refresh_goes_to_its_reviewer::{
    CHECKS, REFRESH, integration_runs, refresh_events,
};
use crate::auto_integrate::Running;
use crate::common::acceptance::assert_in_order;
use crate::worker_resume::{REVIEWER, TASK_WORKER, until_runs, until_state};
use crate::workers_support::FakeClaude;

/// Acceptance 74: two proposals of the lane `migrations`, whose `refresh` names the step to redo, each add a migration on `0001` and go to one reviewer as a review batch, which approves both. When the batch integrates, `P-1` merges first and changes a path of the lane, so `P-2` leaves the batch before its integration run: it goes to the reviewer that approved it, resumed in its own session with the lane's refresh text, the way a batch member that conflicts goes back alone. `P-1` integrates, `get_proposal` names the refresh, and no integration run starts for `P-2`. The author is never told or resumed.
#[tokio::test]
async fn acceptance_74_a_batch_member_after_an_earlier_member_of_its_lane_goes_to_its_reviewer_for_the_refresh()
 {
    let fake = FakeClaude::new("result");
    let mut harness = proposed_with(
        &fake,
        CHECKS,
        &["Add the users table", "Add the posts table"],
        &[
            vec![(
                "alembic/versions/0002_users.py",
                migration("0002_users", "0001"),
            )],
            vec![(
                "alembic/versions/0002_posts.py",
                migration("0002_posts", "0001"),
            )],
        ],
    )
    .await;
    let started = harness
        .call(
            LEAD,
            "start_worker",
            json!({"role": "reviewer", "proposals": ["P-1", "P-2"]}),
        )
        .await;
    assert_eq!(started["worker"]["name"], REVIEWER, "{started}");
    until_state(&harness, REVIEWER, WorkerState::Idle).await;
    harness.connect(REVIEWER).await;
    harness
        .call(
            REVIEWER,
            "submit_batch_review",
            json!({
                "id": "rb-1",
                "verdicts": [
                    {"proposal": "P-1", "verdict": "approve"},
                    {"proposal": "P-2", "verdict": "approve"},
                ],
            }),
        )
        .await;
    until_state(&harness, REVIEWER, WorkerState::Stopped).await;
    let author_runs = fake.runs_of(TASK_WORKER).len();

    let integrate = Running::start(&harness.state);
    let runs = until_runs(&fake, REVIEWER, 2).await;
    let turn = &runs[1].stdin[1];
    for wanted in [
        "fold resumed you because P-2 v1 for T-2, which you approved, needs its lane's refresh on main at r2 and was not integrated",
        "Lane migrations moved",
        &format!("The lane asks for a refresh: {REFRESH}"),
        "submit_review_fix",
    ] {
        assert!(turn.contains(wanted), "{wanted}: {turn}");
    }
    assert_eq!(integrated_in_order(&harness, 1).await, ["P-1"]);
    integrate.stop().await;

    let found = refresh_events(&harness).await;
    assert_eq!(found.len(), 1, "{found:?}");
    for (key, wanted) in [
        ("proposal", json!("P-2")),
        ("main_revision", json!("r2")),
        ("fixer", json!(REVIEWER)),
        ("lane", json!("migrations")),
        ("refresh", json!(REFRESH)),
        ("files", json!(0)),
    ] {
        assert_eq!(found[0][key], wanted, "{key}: {}", found[0]);
    }
    let view = harness
        .call(LEAD, "get_proposal", json!({"id": "P-2"}))
        .await;
    assert_eq!(view["conflicted_reason"]["kind"], "lane_refresh", "{view}");
    assert_eq!(view["conflicted_reason"]["refresh"], REFRESH, "{view}");
    assert_eq!(
        view["waits_for"],
        json!({"reason": "lane_refresh", "lane": "migrations"}),
        "{view}"
    );
    let (page, second_runs) = integration_runs(&harness, "P-2").await;
    assert!(second_runs.is_empty(), "no integration run for P-2: {page}");
    let first = harness
        .call(LEAD, "get_proposal", json!({"id": "P-1"}))
        .await;
    assert_eq!(
        first["integrated"],
        json!({"revision": "r2", "batch": "rb-1"}),
        "{first}"
    );
    assert_eq!(status(&harness, "T-1").await, "done");
    assert_eq!(status(&harness, "T-2").await, "review");
    assert_in_order(
        &harness.events().await,
        &[
            "review.batch_submitted",
            "proposal.integrated",
            "integration.conflicted",
        ],
    );
    assert_eq!(fake.runs_of(TASK_WORKER).len(), author_runs);
}
