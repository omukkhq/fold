use crate::common::intents::Covering as _;
use fold_core::events::types;
use fold_core::reviews::Verdict;
use fold_core::roles::Role;
use fold_core::{
    Actor, AgentName, AreaName, EventFilter, LabelColor, LabelName, ProposalId, Tag,
    TaskDescription, TaskTitle, codes,
};
use fold_daemon::services::reviews::{BatchVerdict, ReviewError, ReviewFix};
use fold_daemon::services::tasks::{CreateTask, TaskError, TaskPatch};
use fold_store::{StoreError, events};

use crate::common::roles::{give_batch, give_claim, give_review_claim, give_role};
use crate::proposal_tools_support::{Fixture, human, t};

#[cfg(test)]
fn p(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
}

#[cfg(test)]
/// Who acts, each with its standing in [`scene`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Who {
    /// `main`, with the `main` tag.
    MainAgent,
    /// `review-lead`.
    ReviewLead,
    /// `reviewer-1`, a reviewer the review lead started.
    Reviewer,
    /// The human `tanim`.
    Human,
    /// `web-lead`, the lead of area `web`.
    AreaLead,
    /// `web-worker`, a task worker of area `web`.
    TaskWorker,
    /// `backend`, which wrote `P-1` and leads its area, so its authorship refuses it first.
    Author,
}

#[cfg(test)]
const EVERYONE: [Who; 7] = [
    Who::MainAgent,
    Who::ReviewLead,
    Who::Reviewer,
    Who::Human,
    Who::AreaLead,
    Who::TaskWorker,
    Who::Author,
];

#[cfg(test)]
impl Who {
    fn agent(self) -> Option<&'static str> {
        match self {
            Who::MainAgent => Some("main"),
            Who::ReviewLead => Some("review-lead"),
            Who::Reviewer => Some("reviewer-1"),
            Who::Human => None,
            Who::AreaLead => Some("web-lead"),
            Who::TaskWorker => Some("web-worker"),
            Who::Author => Some("backend"),
        }
    }

    /// Approves and rejects: the review lead never reviews.
    fn decides(self) -> bool {
        matches!(self, Who::MainAgent | Who::Reviewer | Who::Human)
    }

    /// Integrates and closes: the review lead acts on its reviewers' verdicts.
    fn integrates(self) -> bool {
        self.decides() || self == Who::ReviewLead
    }
}

#[cfg(test)]
/// Registers `name` with `tags` and returns its main worker's actor.
async fn register(fx: &Fixture, name: &str, tags: &[&str]) -> Actor {
    let agents = fx.agents();
    let mut session = agents.start_session(1).await.unwrap();
    let tags: Vec<Tag> = tags.iter().map(|tag| Tag::parse(tag).unwrap()).collect();
    agents
        .register_agent(&mut session, &AgentName::parse(name).unwrap(), &tags)
        .await
        .unwrap();
    agents.actor(&session).await.unwrap()
}

#[cfg(test)]
/// The main agent, the area `web` over label `ui` with its lead, two task workers of the area, the review lead, a reviewer and the author `backend`, whose `P-1` on `T-1` is open.
async fn scene() -> Fixture {
    let fx = Fixture::new().await;
    register(&fx, "main", &["main"]).await;
    fx.handle
        .tasks
        .create_label(
            &human("tanim"),
            LabelName::parse("ui").unwrap(),
            LabelColor::Sage,
        )
        .await
        .unwrap();
    fx.agents()
        .define_area(
            &human("tanim"),
            &AreaName::parse("web").unwrap(),
            &LabelName::parse("ui").unwrap(),
            &fold_core::roles::AreaProfile::default(),
        )
        .await
        .unwrap();
    let store = &fx.handle.store;
    for (name, role, area) in [
        ("web-lead", Role::AreaLead, Some("web")),
        ("web-worker", Role::TaskWorker, Some("web")),
        ("web-worker-2", Role::TaskWorker, Some("web")),
        ("review-lead", Role::ReviewLead, None),
        ("reviewer-1", Role::Reviewer, None),
    ] {
        register(&fx, name, &[]).await;
        give_role(store, name, role, area).await;
    }
    // The author leads the area of the task it works on, and a task has one area.
    fx.handle
        .tasks
        .update_task(
            &human("tanim"),
            t(1),
            TaskPatch {
                labels: Some(crate::common::roles::work_labels_of("backend")),
                ..TaskPatch::default()
            },
        )
        .await
        .unwrap();
    let author = actor(&fx, Who::Author).await;
    fx.proposal(&author, 1, "pub fn y() {}\n").await;
    fx
}

#[cfg(test)]
/// The actor of `who`, read through the agent service as a call would.
async fn actor(fx: &Fixture, who: Who) -> Actor {
    match who.agent() {
        Some(name) => agent_actor(fx, name).await,
        None => human("tanim"),
    }
}

#[cfg(test)]
async fn agent_actor(fx: &Fixture, name: &str) -> Actor {
    let agents = fx.agents();
    let mut session = agents.start_session(1).await.unwrap();
    agents
        .register_agent(&mut session, &AgentName::parse(name).unwrap(), &[])
        .await
        .unwrap();
    agents.actor(&session).await.unwrap()
}

#[cfg(test)]
fn review_code(error: ReviewError) -> String {
    match error {
        ReviewError::Fold(error) => error.code().to_owned(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[cfg(test)]
fn task_code(error: TaskError) -> String {
    match error {
        TaskError::Fold(error) => error.code().to_owned(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[cfg(test)]
async fn reviews_of_p1(fx: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM reviews")
        .fetch_one(fx.handle.store.read())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_verdict_that_decides_follows_the_role() {
    for who in EVERYONE {
        for verdict in [Verdict::Approve, Verdict::Reject] {
            let fx = scene().await;
            let acting = actor(&fx, who).await;
            let result = fx
                .reviews()
                .submit_review(&acting, p(1), 1, verdict, "decided", Vec::new())
                .await;
            match who {
                _ if who.decides() => {
                    assert_eq!(result.unwrap().verdict, verdict, "{who:?} {verdict}");
                }
                Who::ReviewLead => {
                    assert_eq!(
                        review_code(result.unwrap_err()),
                        codes::REVIEW_LEAD_DELEGATES,
                        "{verdict}"
                    );
                    assert_eq!(reviews_of_p1(&fx).await, 0);
                }
                Who::Author => {
                    assert_eq!(
                        review_code(result.unwrap_err()),
                        codes::AUTHOR_CANNOT_REVIEW
                    );
                }
                _ => {
                    assert_eq!(
                        review_code(result.unwrap_err()),
                        codes::INTEGRATOR_REQUIRED,
                        "{who:?} {verdict}"
                    );
                    assert_eq!(reviews_of_p1(&fx).await, 0);
                }
            }
        }
    }
}

#[tokio::test]
async fn any_reviewer_but_the_author_comments_and_asks_for_changes() {
    let fx = scene().await;
    for who in [Who::AreaLead, Who::TaskWorker, Who::Reviewer, Who::Human] {
        let acting = actor(&fx, who).await;
        fx.reviews()
            .submit_review(&acting, p(1), 1, Verdict::Comment, "a note", Vec::new())
            .await
            .unwrap();
    }
    let lead = actor(&fx, Who::ReviewLead).await;
    for verdict in [Verdict::Comment, Verdict::RequestChanges] {
        let error = fx
            .reviews()
            .submit_review(&lead, p(1), 1, verdict, "a note", Vec::new())
            .await
            .unwrap_err();
        assert_eq!(
            review_code(error),
            codes::REVIEW_LEAD_DELEGATES,
            "{verdict}"
        );
    }
    let author = actor(&fx, Who::Author).await;
    let error = fx
        .reviews()
        .submit_review(
            &author,
            p(1),
            1,
            Verdict::RequestChanges,
            "mine",
            Vec::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(review_code(error), codes::AUTHOR_CANNOT_REVIEW);
}

#[tokio::test]
async fn a_batch_verdict_that_decides_follows_the_role() {
    for who in EVERYONE {
        for verdict in [Verdict::Approve, Verdict::Reject] {
            let fx = scene().await;
            second_proposal(&fx).await;
            let acting = actor(&fx, who).await;
            let batch = fx
                .reviews()
                .create_review_batch(&acting, vec![p(1), p(2)])
                .await;
            let batch = match who {
                Who::Human => {
                    assert_eq!(
                        review_code(batch.unwrap_err()),
                        codes::ACTOR_CANNOT_CLAIM,
                        "a human reviews each proposal on its own"
                    );
                    continue;
                }
                Who::Author => {
                    assert_eq!(review_code(batch.unwrap_err()), codes::AUTHOR_CANNOT_REVIEW);
                    continue;
                }
                Who::ReviewLead => {
                    assert_eq!(
                        review_code(batch.unwrap_err()),
                        codes::REVIEW_LEAD_DELEGATES,
                        "the review lead gives the group to a reviewer"
                    );
                    // A batch the review lead holds still never lets it check out or submit.
                    let batch = give_batch(
                        &fx.handle.store,
                        "review-lead",
                        &[p(1), p(2)],
                        fx.handle.clock.now(),
                    )
                    .await;
                    let checkout = fx.reviews().create_batch_checkout(&acting, batch).await;
                    assert_eq!(
                        review_code(checkout.unwrap_err()),
                        codes::REVIEW_LEAD_DELEGATES
                    );
                    let verdicts = [p(1), p(2)]
                        .map(|proposal| BatchVerdict {
                            proposal,
                            verdict,
                            body: "decided".to_owned(),
                        })
                        .to_vec();
                    let submitted = fx
                        .reviews()
                        .submit_batch_review(&acting, batch, verdicts, Vec::new())
                        .await;
                    assert_eq!(
                        review_code(submitted.unwrap_err()),
                        codes::REVIEW_LEAD_DELEGATES
                    );
                    assert_eq!(reviews_of_p1(&fx).await, 0);
                    continue;
                }
                Who::AreaLead | Who::TaskWorker => {
                    assert_eq!(
                        review_code(batch.unwrap_err()),
                        codes::INTEGRATOR_REQUIRED,
                        "only a review role claims the members"
                    );
                    continue;
                }
                Who::Reviewer => {
                    assert_eq!(
                        review_code(batch.unwrap_err()),
                        codes::WORKER_CLAIM_DENIED,
                        "a reviewer gets its batch from the review lead"
                    );
                    give_batch(
                        &fx.handle.store,
                        "reviewer-1",
                        &[p(1), p(2)],
                        fx.handle.clock.now(),
                    )
                    .await
                }
                _ => batch.unwrap().id,
            };
            let verdicts = [p(1), p(2)]
                .map(|proposal| BatchVerdict {
                    proposal,
                    verdict,
                    body: "decided".to_owned(),
                })
                .to_vec();
            let result = fx
                .reviews()
                .submit_batch_review(&acting, batch, verdicts, Vec::new())
                .await;
            assert_eq!(result.unwrap().reviewed.len(), 2, "{who:?} {verdict}");
        }
    }
}

#[tokio::test]
async fn only_the_main_agent_claims_a_review_and_the_author_is_refused_first() {
    for who in EVERYONE {
        let fx = scene().await;
        let acting = actor(&fx, who).await;
        let claimed = fx.reviews().claim_review(&acting, p(1)).await;
        match who {
            Who::ReviewLead => {
                let ReviewError::Fold(error) = claimed.unwrap_err() else {
                    panic!("expected a refusal");
                };
                assert_eq!(error.code(), codes::REVIEW_LEAD_DELEGATES);
                assert!(error.hint().unwrap().contains("start_worker"));
                // A claim the review lead holds still never lets it check out or fix.
                give_review_claim(&fx.handle.store, "review-lead", p(1), fx.handle.clock.now())
                    .await;
                let checkout = fx.reviews().create_review_checkout(&acting, p(1)).await;
                assert_eq!(
                    review_code(checkout.unwrap_err()),
                    codes::REVIEW_LEAD_DELEGATES
                );
                let fixes = [ReviewFix {
                    path: "src/lib.rs".to_owned(),
                    reason: "a fix".to_owned(),
                }];
                let fixed = fx
                    .reviews()
                    .submit_review_fix_covering(&acting, p(1), 1, &fixes, "cargo test: ok")
                    .await;
                assert_eq!(
                    review_code(fixed.unwrap_err()),
                    codes::REVIEW_LEAD_DELEGATES
                );
            }
            Who::MainAgent => {
                assert_eq!(
                    claimed.unwrap().holder.as_str(),
                    who.agent().unwrap(),
                    "{who:?}"
                );
                fx.reviews()
                    .create_review_checkout(&acting, p(1))
                    .await
                    .unwrap();
            }
            Who::Reviewer => {
                let ReviewError::Fold(error) = claimed.unwrap_err() else {
                    panic!("expected a refusal");
                };
                assert_eq!(error.code(), codes::WORKER_CLAIM_DENIED);
                assert!(error.hint().unwrap().contains("lead"));
                give_review_claim(&fx.handle.store, "reviewer-1", p(1), fx.handle.clock.now())
                    .await;
                fx.reviews()
                    .create_review_checkout(&acting, p(1))
                    .await
                    .unwrap();
            }
            Who::Human => {
                assert_eq!(review_code(claimed.unwrap_err()), codes::ACTOR_CANNOT_CLAIM);
            }
            Who::Author => {
                assert_eq!(
                    review_code(claimed.unwrap_err()),
                    codes::AUTHOR_CANNOT_REVIEW,
                    "the author is a reviewer, and its authorship refuses it first"
                );
            }
            Who::AreaLead | Who::TaskWorker => {
                let ReviewError::Fold(error) = claimed.unwrap_err() else {
                    panic!("expected a refusal");
                };
                assert_eq!(error.code(), codes::INTEGRATOR_REQUIRED, "{who:?}");
                assert!(error.hint().unwrap().contains("review lead"), "{who:?}");
                let checkout = fx.reviews().create_review_checkout(&acting, p(1)).await;
                assert_eq!(
                    review_code(checkout.unwrap_err()),
                    codes::REVIEW_CLAIM_REQUIRED,
                    "{who:?}"
                );
            }
        }
    }
}

#[cfg(test)]
/// The author's `P-2` on a new task `T-3`, so a batch has two members.
async fn second_proposal(fx: &Fixture) {
    fx.handle
        .tasks
        .create_task(
            &human("tanim"),
            CreateTask {
                title: TaskTitle::parse("import").unwrap(),
                description: TaskDescription::new("what to build"),
                labels: crate::common::roles::work_labels_of("backend"),
            },
        )
        .await
        .unwrap();
    let author = actor(fx, Who::Author).await;
    fx.proposal(&author, 3, "pub fn z() {}\n").await;
}

#[tokio::test]
async fn integration_follows_the_role() {
    for who in EVERYONE {
        let fx = scene().await;
        let reviewer = actor(&fx, Who::Reviewer).await;
        fx.reviews()
            .submit_review(&reviewer, p(1), 1, Verdict::Approve, "good", Vec::new())
            .await
            .unwrap();
        let acting = actor(&fx, who).await;
        let result = fx.reviews().integrate_proposal(&acting, p(1), None).await;
        match who {
            _ if who.integrates() => {
                result.unwrap();
            }
            Who::Author => {
                assert_eq!(
                    review_code(result.unwrap_err()),
                    codes::AUTHOR_CANNOT_INTEGRATE
                );
            }
            _ => {
                assert_eq!(
                    review_code(result.unwrap_err()),
                    codes::INTEGRATOR_REQUIRED,
                    "{who:?}"
                );
            }
        }
    }
}

/// fold refuses every verdict of the review lead, but an approval that a project stored for it earlier still counts: the main agent integrates on it.
#[tokio::test]
async fn a_stored_approval_of_the_review_lead_still_counts() {
    let fx = scene().await;
    let now = fx.handle.clock.now();
    fx.handle
        .store
        .write(async |tx| {
            let version: i64 = sqlx::query_scalar(
                "SELECT v.id FROM proposal_versions v JOIN proposals p ON p.id = v.proposal_id
                 WHERE p.number = 1 AND v.version = 1",
            )
            .fetch_one(tx.conn())
            .await?;
            let worker: i64 =
                sqlx::query_scalar("SELECT id FROM workers WHERE name = 'review-lead'")
                    .fetch_one(tx.conn())
                    .await?;
            fold_store::reviews::insert_review(
                tx,
                version,
                fold_store::reviews::ReviewerRef::Worker(worker),
                Verdict::Approve,
                "approved before the review lead only assigned reviews",
                now,
            )
            .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let main = actor(&fx, Who::MainAgent).await;
    fx.reviews()
        .integrate_proposal(&main, p(1), None)
        .await
        .unwrap();
}

#[cfg(test)]
/// Who removes `T-2` while `web-worker` holds it: the holder's agent, its area lead and a human.
const REMOVERS: [(&str, bool); 8] = [
    ("web-worker", true),
    ("web-lead", true),
    ("human", true),
    ("main", false),
    ("review-lead", false),
    ("reviewer-1", false),
    ("web-worker-2", false),
    ("backend", false),
];

#[cfg(test)]
async fn remover(fx: &Fixture, name: &str) -> Actor {
    match name {
        "human" => human("tanim"),
        name => agent_actor(fx, name).await,
    }
}

#[cfg(test)]
/// `holder` holds the claim on `T-2`: given by its lead when fold started it as a task worker or reviewer, else claimed by itself.
async fn held_by(fx: &Fixture, holder: &str) {
    let actor = agent_actor(fx, holder).await;
    match fx.handle.tasks.claim_task(&actor, t(2)).await {
        Ok(_) => {}
        Err(TaskError::Fold(error)) if error.code() == codes::WORKER_CLAIM_DENIED => {
            give_claim(&fx.handle.store, holder, t(2), fx.handle.clock.now()).await;
        }
        Err(error) => panic!("{holder} cannot hold T-2: {error:?}"),
    }
}

#[tokio::test]
async fn a_held_task_is_archived_only_by_its_holder_its_lead_or_a_human() {
    for (name, allowed) in REMOVERS {
        let fx = scene().await;
        held_by(&fx, "web-worker").await;
        let acting = remover(&fx, name).await;
        let result = fx.handle.tasks.archive_task(&acting, t(2)).await;
        if allowed {
            assert!(result.unwrap().archived_at.is_some(), "{name}");
        } else {
            assert_eq!(
                task_code(result.unwrap_err()),
                codes::NOT_CLAIM_HOLDER,
                "{name}"
            );
            let claim =
                fold_store::tasks::claims::unreleased_for_task(fx.handle.store.read(), t(2))
                    .await
                    .unwrap();
            assert_eq!(claim.unwrap().worker.as_str(), "web-worker", "{name}");
        }
    }
}

#[tokio::test]
async fn a_held_task_is_deleted_only_by_its_holder_its_lead_or_a_human() {
    for (name, allowed) in REMOVERS {
        let fx = scene().await;
        held_by(&fx, "web-worker").await;
        let acting = remover(&fx, name).await;
        let result = fx.handle.tasks.delete_task(&acting, t(2)).await;
        if allowed {
            result.unwrap();
        } else {
            assert_eq!(
                task_code(result.unwrap_err()),
                codes::NOT_CLAIM_HOLDER,
                "{name}"
            );
            assert!(fx.handle.tasks.get(t(2)).await.is_ok(), "{name}");
        }
    }
}

#[tokio::test]
async fn the_review_lead_removes_a_task_its_reviewer_holds() {
    let fx = scene().await;
    held_by(&fx, "reviewer-1").await;
    let area_lead = agent_actor(&fx, "web-lead").await;
    let error = fx
        .handle
        .tasks
        .archive_task(&area_lead, t(2))
        .await
        .unwrap_err();
    assert_eq!(task_code(error), codes::NOT_CLAIM_HOLDER);
    let review_lead = agent_actor(&fx, "review-lead").await;
    fx.handle
        .tasks
        .archive_task(&review_lead, t(2))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_task_nobody_holds_is_removed_by_any_agent() {
    let fx = scene().await;
    let task_worker = agent_actor(&fx, "web-worker-2").await;
    fx.handle
        .tasks
        .archive_task(&task_worker, t(2))
        .await
        .unwrap();
}

#[tokio::test]
async fn deleting_a_held_task_releases_its_claim_with_an_event() {
    let fx = scene().await;
    held_by(&fx, "web-worker").await;
    let lead = agent_actor(&fx, "web-lead").await;
    fx.handle.tasks.delete_task(&lead, t(2)).await.unwrap();
    let released: Vec<_> =
        events::read_after(fx.handle.store.read(), 0, &EventFilter::default(), 1000)
            .await
            .unwrap()
            .into_iter()
            .filter(|event| event.event_type == types::CLAIM_RELEASED)
            .collect();
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].actor.as_str(), "web-lead");
    assert_eq!(released[0].task, Some(t(2)));
    assert_eq!(
        released[0].data,
        serde_json::json!({"kind": "implementation", "worker": "web-worker", "reason": "task_deleted"})
    );
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM claims")
        .fetch_one(fx.handle.store.read())
        .await
        .map_err(StoreError::from)
        .unwrap();
    assert_eq!(rows, 1, "only the author's claim on T-1 stays");
}
