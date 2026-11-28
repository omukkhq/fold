#![cfg(test)]

use std::path::Path;

use axum::http::{Method, StatusCode};
use fold_core::WorkerState;
use serde_json::{Value, json};

use crate::acceptance_50_a_serial_lane_integrates_in_turn::{
    LEAD, SECOND_REVIEWER, approve, integrated_in_order, migration, proposed_with, status,
};
use crate::auto_integrate::Running;
use crate::common::acceptance::{HUMAN, Harness, SLUG};
use crate::worker_resume::{REVIEWER, TASK_WORKER, until_runs, until_state};
use crate::workers_support::FakeClaude;

const REFRESH: &str = "put the migration on the current head";

/// main's checks: the lane `migrations` with a `refresh`, and an integration check that refuses two migrations on the same parent.
const CHECKS: &str = r#"
[[serial]]
name    = "migrations"
paths   = ["alembic/versions/*.py"]
refresh = "put the migration on the current head"

[[check]]
name  = "one-head"
run   = "test -z \"$(cat alembic/versions/*.py | grep '^down' | sort | uniq -d)\""
stage = "integration"
"#;

async fn refresh_events(harness: &Harness) -> Vec<Value> {
    harness
        .events()
        .await
        .into_iter()
        .filter(|event| event.kind == "integration.conflicted")
        .map(|event| event.data)
        .collect()
}

/// The integration check runs of `proposal` as the HTTP proposal detail lists them.
async fn integration_runs(harness: &Harness, proposal: &str) -> (Value, Vec<Value>) {
    let (code, page) = harness
        .http(
            Method::GET,
            &format!("/api/v1/projects/{SLUG}/proposals/{proposal}"),
            None,
            HUMAN,
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{page}");
    let runs = page["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|run| run["stage"] == "integration")
        .cloned()
        .collect();
    (page, runs)
}

/// Acceptance 73: two approved members of the lane `migrations`, whose `refresh` names the step to redo, each add a migration on `0001`. `P-1` is the front, its base is main, and it integrates without a refresh. Then main has changed a path of the lane since `P-2`'s base, so fold starts no integration run for `P-2`: it goes to the reviewer that approved it, resumed in its own session with the lane's refresh text. `get_proposal`, the HTTP detail and the queue name the refresh, and the event carries the lane, the text and no conflicting file. The reviewer redoes the migration on the new main and approves the fix, whose base is main, and fold integrates it. The author is never told or resumed.
#[tokio::test]
async fn acceptance_73_a_moved_lane_with_refresh_sends_its_front_member_to_its_reviewer() {
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
    approve(&mut harness, "P-1", REVIEWER).await;
    approve(&mut harness, "P-2", SECOND_REVIEWER).await;
    let author_runs = fake.runs_of(TASK_WORKER).len();

    let integrate = Running::start(&harness.state);
    let runs = until_runs(&fake, SECOND_REVIEWER, 2).await;
    let turn = &runs[1].stdin[1];
    for wanted in [
        "fold resumed you because P-2 v1 for T-2, which you approved, needs its lane's refresh on main at r2 and was not integrated",
        "Lane migrations moved",
        &format!("The lane asks for a refresh: {REFRESH}"),
        "create_review_checkout for P-2",
        "submit_review_fix",
    ] {
        assert!(turn.contains(wanted), "{wanted}: {turn}");
    }
    assert_eq!(integrated_in_order(&harness, 1).await, ["P-1"]);
    let (_, first_runs) = integration_runs(&harness, "P-1").await;
    assert_eq!(first_runs.len(), 1, "P-1 integrated after its own run");

    let found = refresh_events(&harness).await;
    assert_eq!(found.len(), 1, "{found:?}");
    for (key, wanted) in [
        ("proposal", json!("P-2")),
        ("main_revision", json!("r2")),
        ("fixer", json!(SECOND_REVIEWER)),
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
        view["conflicted_reason"]["fixer"], SECOND_REVIEWER,
        "{view}"
    );
    assert_eq!(
        view["waits_for"],
        json!({"reason": "lane_refresh", "lane": "migrations"}),
        "{view}"
    );
    let (page, second_runs) = integration_runs(&harness, "P-2").await;
    assert!(second_runs.is_empty(), "no integration run for P-2: {page}");
    assert_eq!(
        page["conflicted_reason"],
        json!({"kind": "lane_refresh", "lane": "migrations", "refresh": REFRESH, "fixer": SECOND_REVIEWER}),
        "{page}"
    );
    assert_eq!(
        page["waits_for"],
        json!({"reason": "lane_refresh", "lane": "migrations"}),
        "{page}"
    );
    assert_eq!(status(&harness, "T-2").await, "review");

    until_state(&harness, SECOND_REVIEWER, WorkerState::Idle).await;
    let checkout = harness
        .call(
            SECOND_REVIEWER,
            "create_review_checkout",
            json!({"id": "P-2"}),
        )
        .await;
    let dir = Path::new(harness.root()).join(checkout["path"].as_str().unwrap());
    assert!(
        dir.join("alembic/versions/0002_users.py").exists(),
        "the checkout holds the main that P-1 made"
    );
    std::fs::write(
        dir.join("alembic/versions/0002_posts.py"),
        migration("0002_posts", "0002_users"),
    )
    .unwrap();
    let fixed = harness
        .call(
            SECOND_REVIEWER,
            "submit_review_fix",
            json!({
                "intent": "Make the change the task asks for.",
                "id": "P-2",
                "version": 1,
                "fixes": [{"path": "alembic/versions/0002_posts.py", "reason": REFRESH}],
                "checks": "read the migration chain",
            }),
        )
        .await;
    assert_eq!(fixed["version"], "v2", "{fixed}");
    harness
        .call(
            SECOND_REVIEWER,
            "submit_review",
            json!({"id": "P-2", "version": 2, "verdict": "approve", "body": "One head again."}),
        )
        .await;

    assert_eq!(integrated_in_order(&harness, 2).await, ["P-1", "P-2"]);
    integrate.stop().await;
    for task in ["T-1", "T-2"] {
        assert_eq!(status(&harness, task).await, "done", "{task}");
    }
    assert_eq!(
        refresh_events(&harness).await.len(),
        1,
        "v2 needs no refresh"
    );
    assert_eq!(fake.runs_of(TASK_WORKER).len(), author_runs);
}
