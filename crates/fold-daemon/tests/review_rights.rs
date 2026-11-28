use crate::common::intents::Covering as _;
use fold_core::events::types;
use fold_core::reviews::Verdict;
use fold_core::roles::Role;
use fold_core::{
    Actor, AgentName, AreaName, EventFilter, LabelColor, LabelName, ProposalId, Tag,
    TaskDescription, TaskId, TaskTitle, codes,
};
use fold_daemon::services::reviews::{BatchVerdict, IntegrationOutcome, ReviewError, ReviewFix};
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

/// The refusals change nothing, so they share `P-1` of one scene; each verdict that decides gets a proposal of its own, because a rejection closes it.
#[tokio::test]
async fn a_verdict_that_decides_follows_the_role() {
    let fx = scene().await;
    for who in EVERYONE.into_iter().filter(|who| !who.decides()) {
        for verdict in [Verdict::Approve, Verdict::Reject] {
            let acting = actor(&fx, who).await;
            let result = fx
                .reviews()
                .submit_review(&acting, p(1), 1, verdict, "decided", Vec::new())
                .await;
            let expected = match who {
                Who::ReviewLead => codes::REVIEW_LEAD_DELEGATES,
                Who::Author => codes::AUTHOR_CANNOT_REVIEW,
                _ => codes::INTEGRATOR_REQUIRED,
            };
            assert_eq!(
                review_code(result.unwrap_err()),
                expected,
                "{who:?} {verdict}"
            );
            assert_eq!(reviews_of_p1(&fx).await, 0, "{who:?} {verdict}");
        }
    }
    for who in EVERYONE.into_iter().filter(|who| who.decides()) {
        for verdict in [Verdict::Approve, Verdict::Reject] {
            let proposal = fresh_proposal(&fx).await;
            let acting = actor(&fx, who).await;
            let result = fx
                .reviews()
                .submit_review(&acting, proposal, 1, verdict, "decided", Vec::new())
                .await;
            assert_eq!(result.unwrap().verdict, verdict, "{who:?} {verdict}");
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

#[cfg(test)]
fn batch_verdicts(members: [ProposalId; 2], verdict: Verdict) -> Vec<BatchVerdict> {
    members
        .map(|proposal| BatchVerdict {
            proposal,
            verdict,
            body: "decided".to_owned(),
        })
        .to_vec()
}

/// The refusals share `P-1` and `P-2` of one scene, the review lead's last because its batch holds them; each batch verdict that decides gets two proposals of its own.
#[tokio::test]
async fn a_batch_verdict_that_decides_follows_the_role() {
    let fx = scene().await;
    second_proposal(&fx).await;
    let shared = [p(1), p(2)];
    for who in [Who::Human, Who::Author, Who::AreaLead, Who::TaskWorker] {
        let acting = actor(&fx, who).await;
        let batch = fx
            .reviews()
            .create_review_batch(&acting, shared.to_vec())
            .await;
        let (expected, why) = match who {
            Who::Human => (
                codes::ACTOR_CANNOT_CLAIM,
                "a human reviews each proposal on its own",
            ),
            Who::Author => (codes::AUTHOR_CANNOT_REVIEW, "the author never reviews"),
            _ => (
                codes::INTEGRATOR_REQUIRED,
                "only a review role claims the members",
            ),
        };
        assert_eq!(review_code(batch.unwrap_err()), expected, "{who:?}: {why}");
    }
    let lead = actor(&fx, Who::ReviewLead).await;
    let batch = fx
        .reviews()
        .create_review_batch(&lead, shared.to_vec())
        .await;
    assert_eq!(
        review_code(batch.unwrap_err()),
        codes::REVIEW_LEAD_DELEGATES,
        "the review lead gives the group to a reviewer"
    );
    // A batch the review lead holds still never lets it check out or submit.
    let batch = give_batch(
        &fx.handle.store,
        "review-lead",
        &shared,
        fx.handle.clock.now(),
    )
    .await;
    let checkout = fx.reviews().create_batch_checkout(&lead, batch).await;
    assert_eq!(
        review_code(checkout.unwrap_err()),
        codes::REVIEW_LEAD_DELEGATES
    );
    for verdict in [Verdict::Approve, Verdict::Reject] {
        let submitted = fx
            .reviews()
            .submit_batch_review(&lead, batch, batch_verdicts(shared, verdict), Vec::new())
            .await;
        assert_eq!(
            review_code(submitted.unwrap_err()),
            codes::REVIEW_LEAD_DELEGATES,
            "{verdict}"
        );
    }
    assert_eq!(reviews_of_p1(&fx).await, 0);
    for who in [Who::MainAgent, Who::Reviewer] {
        for verdict in [Verdict::Approve, Verdict::Reject] {
            let members = [fresh_proposal(&fx).await, fresh_proposal(&fx).await];
            let acting = actor(&fx, who).await;
            let batch = fx
                .reviews()
                .create_review_batch(&acting, members.to_vec())
                .await;
            let batch = if who == Who::Reviewer {
                assert_eq!(
                    review_code(batch.unwrap_err()),
                    codes::WORKER_CLAIM_DENIED,
                    "a reviewer gets its batch from the review lead"
                );
                give_batch(
                    &fx.handle.store,
                    "reviewer-1",
                    &members,
                    fx.handle.clock.now(),
                )
                .await
            } else {
                batch.unwrap().id
            };
            let result = fx
                .reviews()
                .submit_batch_review(&acting, batch, batch_verdicts(members, verdict), Vec::new())
                .await;
            assert_eq!(result.unwrap().reviewed.len(), 2, "{who:?} {verdict}");
        }
    }
}

/// The refusals change nothing, so they share `P-1` of one scene; each role that ends up holding a review claim gets a proposal of its own.
#[tokio::test]
async fn only_the_main_agent_claims_a_review_and_the_author_is_refused_first() {
    let fx = scene().await;
    for who in EVERYONE {
        let proposal = match who {
            Who::ReviewLead | Who::MainAgent | Who::Reviewer => fresh_proposal(&fx).await,
            _ => p(1),
        };
        let acting = actor(&fx, who).await;
        let claimed = fx.reviews().claim_review(&acting, proposal).await;
        match who {
            Who::ReviewLead => {
                let ReviewError::Fold(error) = claimed.unwrap_err() else {
                    panic!("expected a refusal");
                };
                assert_eq!(error.code(), codes::REVIEW_LEAD_DELEGATES);
                assert!(error.hint().unwrap().contains("start_worker"));
                // A claim the review lead holds still never lets it check out or fix.
                give_review_claim(
                    &fx.handle.store,
                    "review-lead",
                    proposal,
                    fx.handle.clock.now(),
                )
                .await;
                let checkout = fx.reviews().create_review_checkout(&acting, proposal).await;
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
                    .submit_review_fix_covering(&acting, proposal, 1, &fixes, "cargo test: ok")
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
                    .create_review_checkout(&acting, proposal)
                    .await
                    .unwrap();
            }
            Who::Reviewer => {
                let ReviewError::Fold(error) = claimed.unwrap_err() else {
                    panic!("expected a refusal");
                };
                assert_eq!(error.code(), codes::WORKER_CLAIM_DENIED);
                assert!(error.hint().unwrap().contains("lead"));
                give_review_claim(
                    &fx.handle.store,
                    "reviewer-1",
                    proposal,
                    fx.handle.clock.now(),
                )
                .await;
                fx.reviews()
                    .create_review_checkout(&acting, proposal)
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
                let checkout = fx.reviews().create_review_checkout(&acting, proposal).await;
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

#[cfg(test)]
/// A new task of the author's area with the author's open proposal on it, which is the newest proposal. Its content names the task, so it still changes main after another proposal integrated.
async fn fresh_proposal(fx: &Fixture) -> ProposalId {
    let task = fx
        .handle
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
    let number = task.id.number();
    fx.proposal(&author, number, &format!("pub fn z{number}() {{}}\n"))
        .await;
    let newest: i64 = sqlx::query_scalar("SELECT MAX(number) FROM proposals")
        .fetch_one(fx.handle.store.read())
        .await
        .unwrap();
    p(u32::try_from(newest).unwrap())
}

/// Each role integrates an approved proposal of its own in one scene, made from main as it is after the integrations before it.
#[tokio::test]
async fn integration_follows_the_role() {
    let fx = scene().await;
    let reviewer = actor(&fx, Who::Reviewer).await;
    for who in EVERYONE {
        let proposal = fresh_proposal(&fx).await;
        fx.reviews()
            .submit_review(&reviewer, proposal, 1, Verdict::Approve, "good", Vec::new())
            .await
            .unwrap();
        let acting = actor(&fx, who).await;
        let result = fx
            .reviews()
            .integrate_proposal(&acting, proposal, None)
            .await;
        match who {
            _ if who.integrates() => {
                assert!(
                    matches!(result.unwrap(), IntegrationOutcome::Succeeded { .. }),
                    "{who:?}"
                );
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
/// Who removes a task that `web-worker` holds: the holder's agent, its area lead and a human.
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
/// `holder` holds the claim on `task`: given by its lead when fold started it as a task worker or reviewer, else claimed by itself.
async fn held_by(fx: &Fixture, holder: &str, task: TaskId) {
    let actor = agent_actor(fx, holder).await;
    match fx.handle.tasks.claim_task(&actor, task).await {
        Ok(_) => {}
        Err(TaskError::Fold(error)) if error.code() == codes::WORKER_CLAIM_DENIED => {
            give_claim(&fx.handle.store, holder, task, fx.handle.clock.now()).await;
        }
        Err(error) => panic!("{holder} cannot hold {task}: {error:?}"),
    }
}

#[cfg(test)]
/// A new task in the area of `T-2` that `web-worker` holds, for a remover that takes it away.
async fn fresh_held_task(fx: &Fixture) -> TaskId {
    let task = fx
        .handle
        .tasks
        .create_task(
            &human("tanim"),
            CreateTask {
                title: TaskTitle::parse("export").unwrap(),
                description: TaskDescription::new("what to build"),
                labels: crate::common::roles::work_labels_of("backend"),
            },
        )
        .await
        .unwrap()
        .id;
    held_by(fx, "web-worker", task).await;
    task
}

/// The refusals change nothing, so they share `T-2` of one scene; each remover that may archive gets a held task of its own.
#[tokio::test]
async fn a_held_task_is_archived_only_by_its_holder_its_lead_or_a_human() {
    let fx = scene().await;
    held_by(&fx, "web-worker", t(2)).await;
    for (name, allowed) in REMOVERS {
        let acting = remover(&fx, name).await;
        if allowed {
            let task = fresh_held_task(&fx).await;
            let result = fx.handle.tasks.archive_task(&acting, task).await;
            assert!(result.unwrap().archived_at.is_some(), "{name}");
        } else {
            let result = fx.handle.tasks.archive_task(&acting, t(2)).await;
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

/// The refusals change nothing, so they share `T-2` of one scene; each remover that may delete gets a held task of its own.
#[tokio::test]
async fn a_held_task_is_deleted_only_by_its_holder_its_lead_or_a_human() {
    let fx = scene().await;
    held_by(&fx, "web-worker", t(2)).await;
    for (name, allowed) in REMOVERS {
        let acting = remover(&fx, name).await;
        if allowed {
            let task = fresh_held_task(&fx).await;
            fx.handle.tasks.delete_task(&acting, task).await.unwrap();
            assert!(fx.handle.tasks.get(task).await.is_err(), "{name}");
        } else {
            let result = fx.handle.tasks.delete_task(&acting, t(2)).await;
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
    held_by(&fx, "reviewer-1", t(2)).await;
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
    held_by(&fx, "web-worker", t(2)).await;
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
