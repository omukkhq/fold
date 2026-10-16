#![cfg(test)]

use crate::reviews_support::{
    Fixture, LEASE_MINUTES, agent, by, details, human, names, p, refusal, t,
};
use fold_core::events::types;
use fold_core::{EventActor, TaskStatus, codes};
use fold_store::tasks::claims;
use serde_json::json;

#[tokio::test]
async fn claim_review_is_atomic_and_the_loser_gets_already_claimed() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    let before = fx.last_seq().await;
    let (backend, review) = (by(&fx.backend.worker), by(&fx.review.worker));
    let (first, second) = (fx.service(), fx.service());

    let (left, right) = tokio::join!(
        first.claim_review(&backend, p(1)),
        second.claim_review(&review, p(1))
    );

    let (winner, loser, lost) = match (left, right) {
        (Ok(view), lost @ Err(_)) => (view.holder, "review", refusal(lost)),
        (lost @ Err(_), Ok(view)) => (view.holder, "backend", refusal(lost)),
        other => panic!("exactly one claim must win, got {other:?}"),
    };
    assert_ne!(winner.as_str(), loser);
    assert_eq!(lost.code(), codes::ALREADY_CLAIMED);
    assert_eq!(
        details(&lost),
        json!({"proposal": "P-1", "holder": winner.as_str()})
    );
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM claims WHERE kind = 'review' AND released_at IS NULL")
            .await,
        1
    );
    let loser_actor = agent(loser);
    let error = refusal(fx.reviews.claim_review(&loser_actor, p(1)).await);
    assert_eq!(error.code(), codes::ALREADY_CLAIMED);
    assert_eq!(
        details(&error),
        json!({"proposal": "P-1", "holder": winner.as_str()})
    );
    let summary = fx.reviews.get_proposal(p(1), None).await.unwrap().summary;
    assert_eq!(summary.review_claim, Some(winner.clone()));

    fx.advance(1);
    let again = fx
        .reviews
        .claim_review(&agent(winner.as_str()), p(1))
        .await
        .unwrap();
    assert_eq!(again.holder, winner);
    assert_eq!(again.renewed_at, fx.project.now());
    assert!(again.acquired_at < again.renewed_at);
    assert!(!again.stale);

    let appended = fx.events_after(before).await;
    assert_eq!(names(&appended), ["claim.acquired"]);
    let acquired = &appended[0];
    assert_eq!(acquired.actor, EventActor::worker(&winner));
    assert_eq!(acquired.task, Some(t(1)));
    assert_eq!(acquired.proposal, Some(p(1)));
    assert_eq!(
        acquired.data,
        json!({"kind": "review", "proposal": "P-1", "worker": winner.as_str()})
    );
}

#[tokio::test]
async fn review_claim_leaves_the_implementation_claim_and_status_alone() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    let read = fx.project.store().read();
    let implementation = claims::unreleased_for_task(read, t(1)).await.unwrap();
    assert_eq!(fx.status(1).await, TaskStatus::Review);
    let before = fx.last_seq().await;

    fx.advance(1);
    fx.reviews
        .claim_review(&by(&fx.backend.worker), p(1))
        .await
        .unwrap();
    fx.recompute(1).await;

    assert_eq!(
        claims::unreleased_for_task(read, t(1)).await.unwrap(),
        implementation
    );
    assert_eq!(
        implementation.as_ref().map(|claim| claim.worker.clone()),
        Some(fx.frontend.worker.name.clone())
    );
    assert_eq!(fx.status(1).await, TaskStatus::Review);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM claims WHERE released_at IS NULL")
            .await,
        2
    );
    assert_eq!(names(&fx.events_after(before).await), ["claim.acquired"]);

    fx.reviews
        .release_review(&by(&fx.backend.worker), p(1), None)
        .await
        .unwrap();
    assert_eq!(
        claims::unreleased_for_task(read, t(1)).await.unwrap(),
        implementation
    );
    assert_eq!(fx.status(1).await, TaskStatus::Review);
}

#[tokio::test]
async fn author_agent_cannot_claim_its_own_review() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    let sub = fx.subagent(&fx.frontend, "checker").await;

    for actor in [by(&fx.frontend.worker), by(&sub)] {
        let error = refusal(fx.reviews.claim_review(&actor, p(1)).await);
        assert_eq!(error.code(), codes::AUTHOR_CANNOT_REVIEW);
        assert_eq!(
            details(&error),
            json!({"proposal": "P-1", "author": "frontend"})
        );
    }
    let error = refusal(fx.reviews.claim_review(&human(), p(1)).await);
    assert_eq!(error.code(), codes::ACTOR_CANNOT_CLAIM);
    let error = refusal(fx.reviews.claim_review(&by(&fx.backend.worker), p(2)).await);
    assert_eq!(error.code(), codes::PROPOSAL_NOT_FOUND);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM claims WHERE kind = 'review'")
            .await,
        0
    );
}

#[tokio::test]
async fn stale_review_claim_is_taken_over_by_the_next_claim() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    let first = fx
        .reviews
        .claim_review(&by(&fx.backend.worker), p(1))
        .await
        .unwrap();

    fx.advance(LEASE_MINUTES - 1);
    let error = refusal(fx.reviews.claim_review(&by(&fx.review.worker), p(1)).await);
    assert_eq!(error.code(), codes::ALREADY_CLAIMED);

    fx.advance(1);
    let before = fx.last_seq().await;
    let taken = fx
        .reviews
        .claim_review(&by(&fx.review.worker), p(1))
        .await
        .unwrap();

    assert_eq!(taken.holder, fx.review.worker.name);
    assert_eq!(taken.acquired_at, fx.project.now());
    assert!(first.acquired_at < taken.acquired_at);
    assert_eq!(
        fx.text(
            "SELECT release_reason FROM claims c JOIN workers w ON w.id = c.worker_id
             WHERE c.kind = 'review' AND w.name = 'backend'"
        )
        .await
        .as_deref(),
        Some("taken_over")
    );
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM claims WHERE kind = 'review' AND released_at IS NULL")
            .await,
        1
    );
    let summary = fx.reviews.get_proposal(p(1), None).await.unwrap().summary;
    assert_eq!(summary.review_claim, Some(fx.review.worker.name.clone()));
    let appended = fx.events_after(before).await;
    assert_eq!(names(&appended), ["claim.taken_over", "claim.acquired"]);
    assert_eq!(
        appended[0].actor,
        EventActor::worker(&fx.review.worker.name)
    );
    assert_eq!(appended[0].proposal, Some(p(1)));
    assert_eq!(
        appended[0].data,
        json!({"kind": "review", "proposal": "P-1", "from": "backend", "to": "review"})
    );
    assert_eq!(
        appended[1].data,
        json!({"kind": "review", "proposal": "P-1", "worker": "review"})
    );
    assert_eq!(fx.status(1).await, TaskStatus::Review);
}

#[tokio::test]
async fn any_tool_call_renews_a_review_claim() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    let sub = fx.subagent(&fx.backend, "r1").await;
    fx.reviews.claim_review(&by(&sub), p(1)).await.unwrap();
    fx.advance(LEASE_MINUTES - 1);
    fx.execute("UPDATE claims SET stale_at = 1 WHERE kind = 'review'")
        .await;

    fx.agents
        .record_call(&fx.backend.ctx, &fx.backend.worker, "list_proposals", None)
        .await
        .unwrap();

    let renewed = fx
        .count("SELECT renewed_at FROM claims WHERE kind = 'review'")
        .await;
    assert_eq!(renewed, fx.project.now().as_millis());
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM claims WHERE kind = 'review' AND stale_at IS NULL")
            .await,
        1
    );
    fx.advance(LEASE_MINUTES - 1);
    let error = refusal(fx.reviews.claim_review(&by(&fx.review.worker), p(1)).await);
    assert_eq!(error.code(), codes::ALREADY_CLAIMED);
    assert_eq!(details(&error)["holder"], json!("backend/r1"));
}

#[tokio::test]
async fn closing_the_proposal_releases_the_review_claim() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    fx.reviews
        .claim_review(&by(&fx.backend.worker), p(1))
        .await
        .unwrap();

    fx.reviews
        .close_proposal(&by(&fx.frontend.worker), p(1), "superseded")
        .await
        .unwrap();

    let summary = fx.reviews.get_proposal(p(1), None).await.unwrap().summary;
    assert_eq!(summary.review_claim, None);
    assert_eq!(
        fx.text("SELECT release_reason FROM claims WHERE kind = 'review'")
            .await
            .as_deref(),
        Some("closed")
    );
    let error = refusal(
        fx.reviews
            .release_review(&by(&fx.backend.worker), p(1), None)
            .await,
    );
    assert_eq!(error.code(), codes::CLAIM_NOT_FOUND);
    let error = refusal(fx.reviews.claim_review(&by(&fx.review.worker), p(1)).await);
    assert_eq!(error.code(), codes::PROPOSAL_NOT_OPEN);
}

#[tokio::test]
async fn release_review_is_for_the_holder_agent_and_frees_the_proposal() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    let sub = fx.subagent(&fx.backend, "r1").await;
    fx.reviews.claim_review(&by(&sub), p(1)).await.unwrap();

    for actor in [by(&fx.review.worker), by(&fx.frontend.worker), human()] {
        let error = refusal(fx.reviews.release_review(&actor, p(1), None).await);
        assert_eq!(error.code(), codes::NOT_CLAIM_HOLDER);
        assert_eq!(
            details(&error),
            json!({"proposal": "P-1", "holder": "backend/r1"})
        );
    }
    let long = "x".repeat(501);
    let error = refusal(
        fx.reviews
            .release_review(&by(&sub), p(1), Some(&long))
            .await,
    );
    assert_eq!(error.code(), codes::INVALID_REQUEST);
    let error = refusal(fx.reviews.release_review(&by(&sub), p(2), None).await);
    assert_eq!(error.code(), codes::PROPOSAL_NOT_FOUND);
    let before = fx.last_seq().await;

    fx.reviews
        .release_review(&by(&fx.backend.worker), p(1), Some("  handing over  "))
        .await
        .unwrap();

    assert_eq!(
        fx.text("SELECT release_reason FROM claims WHERE kind = 'review'")
            .await
            .as_deref(),
        Some("handing over")
    );
    let appended = fx.events_after(before).await;
    assert_eq!(names(&appended), ["claim.released"]);
    assert_eq!(appended[0].event_type, types::CLAIM_RELEASED);
    assert_eq!(
        appended[0].actor,
        EventActor::worker(&fx.backend.worker.name)
    );
    assert_eq!(appended[0].proposal, Some(p(1)));
    assert_eq!(
        appended[0].data,
        json!({"kind": "review", "proposal": "P-1", "worker": "backend/r1", "reason": "handing over"})
    );
    let listed = fx
        .reviews
        .list_proposals(fold_daemon::services::reviews::ProposalFilter {
            state: None,
            unclaimed: true,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);

    let view = fx
        .reviews
        .claim_review(&by(&fx.review.worker), p(1))
        .await
        .unwrap();
    assert_eq!(view.holder, fx.review.worker.name);
    fx.reviews
        .release_review(&by(&fx.review.worker), p(1), Some(" "))
        .await
        .unwrap();
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM claims WHERE kind = 'review' AND release_reason = 'released'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn the_lease_is_read_from_the_project_configuration() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    let config = fx.project.paths().config();
    std::fs::write(&config, "lease_minutes = 2\n").unwrap();
    fx.configured_service()
        .claim_review(&by(&fx.backend.worker), p(1))
        .await
        .unwrap();
    fx.advance(2);

    std::fs::write(&config, "lease_minutes = 0\n").unwrap();
    let broken = fx.configured_service();
    let error = refusal(broken.claim_review(&by(&fx.review.worker), p(1)).await);
    assert_eq!(error.code(), codes::INVALID_CONFIG);
    assert_eq!(error.hint(), Some("ask a human to fix .fold/config.toml"));
    assert_eq!(
        broken.get_proposal(p(1), None).await.unwrap().summary.id,
        p(1)
    );

    std::fs::write(&config, "lease_minutes = 2\n").unwrap();
    let taken = fx
        .configured_service()
        .claim_review(&by(&fx.review.worker), p(1))
        .await
        .unwrap();
    assert_eq!(taken.holder, fx.review.worker.name);
}
