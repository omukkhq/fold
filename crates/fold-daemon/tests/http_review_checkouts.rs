#![cfg(test)]

use crate::daemon_support::{Opened, agent, t};
use axum::http::StatusCode;
use fold_core::{AgentName, ProposalId};
use fold_daemon::services::agents::AgentService;
use fold_daemon::services::reviews::ReviewService;
use serde_json::{Value, json};

fn p(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
}

async fn register(opened: &Opened, name: &str) {
    let agents = AgentService::new(
        opened.handle.store.clone(),
        opened.handle.bus.clone(),
        opened.handle.clock.clone(),
    );
    let mut session = agents.start_session(2).await.unwrap();
    agents
        .register_agent(&mut session, &AgentName::parse(name).unwrap(), &[])
        .await
        .unwrap();
}

fn reviews(opened: &Opened) -> ReviewService {
    let handle = &opened.handle;
    ReviewService::new(
        handle.store.clone(),
        handle.bus.clone(),
        handle.clock.clone(),
        opened.workspaces(),
        handle.integration.clone(),
    )
}

/// `frontend` proposes a change to `T-1` as `P-1`, and `review` claims its review.
async fn proposed() -> Opened {
    let opened = Opened::new().await;
    register(&opened, "review").await;
    let (_, dir) = opened.create(1).await;
    std::fs::write(dir.join("src/lib.rs"), "pub fn y() {}\n").unwrap();
    let reviews = reviews(&opened);
    reviews
        .create_proposal(&agent("frontend"), t(1), "Change x", None)
        .await
        .unwrap();
    reviews.claim_review(&agent("review"), p(1)).await.unwrap();
    opened
}

#[tokio::test]
async fn no_checkout_lists_nothing() {
    let opened = proposed().await;

    let (status, body) = opened.get("/reviews/checkouts").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "checkouts": [] }));
}

#[tokio::test]
async fn an_active_checkout_lists_its_worker_path_and_version() {
    let opened = proposed().await;
    let checkout = reviews(&opened)
        .create_review_checkout(&agent("review"), p(1))
        .await
        .unwrap();

    let (status, body) = opened.get("/reviews/checkouts").await;

    assert_eq!(status, StatusCode::OK);
    let rows = body["checkouts"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["workspace"], checkout.workspace.id.to_string());
    assert_eq!(row["path"], ".fold/ws/review-p-1");
    assert_eq!(row["proposal"], "P-1");
    assert_eq!(row["version"], 1);
    assert_eq!(row["worker"], "review");
    assert_eq!(row["base"], "r1");
    assert_eq!(row["created_at"], opened.project.now().as_millis());
    assert_eq!(
        row.get("last_command"),
        None::<&Value>,
        "the daemon records no command run in a checkout"
    );
}
