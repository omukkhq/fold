#![cfg(test)]

use crate::common::roles::{give_review_claim, give_role};
use crate::proposal_tools_support::{Fixture, human, t};
use fold_core::reviews::{Side, Verdict};
use fold_core::roles::Role;
use fold_core::{
    Actor, AgentActor, AgentName, FoldError, ProposalId, ReviewBatchId, TaskDescription, TaskTitle,
    WorkerName, codes,
};
use fold_daemon::services::reviews::{
    BatchComment, BatchLines, BatchVerdict, CheckoutTarget, CommentLocation, LeftOutReason,
    ProposalSummary, ReviewError,
};
use fold_daemon::services::tasks::{CreateTask, TaskError};
use fold_daemon::services::workspaces::WorkspaceRef;
use fold_store::reviews::batches::{BatchState, MemberRelease};
use fold_store::workspaces::WorkspaceState;
use serde_json::{Value, json};

fn p(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
}

fn rb(number: u32) -> ReviewBatchId {
    ReviewBatchId::from_number(number).unwrap()
}

fn refusal<T: std::fmt::Debug>(result: Result<T, ReviewError>) -> FoldError {
    match result {
        Err(ReviewError::Fold(error) | ReviewError::Task(TaskError::Fold(error))) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn details(error: &FoldError) -> Value {
    error.details().cloned().map_or(Value::Null, Value::Object)
}

/// Four open proposals, each by its own agent, and `review`, the main agent, which makes a batch itself. `T-1` needs `T-3`, so the merge order of all four is P-2, P-3, P-1, P-4.
///
/// - P-1 (T-1): `README.md` gains a first line, and `src/one.rs` is new.
/// - P-2 (T-2): `README.md` gains a last line, and `src/two.rs` is new.
/// - P-3 (T-3): `src/three.rs` is new.
/// - P-4 (T-4): `README.md` gains another last line, where P-2 adds its own.
struct Scene {
    fx: Fixture,
    authors: Vec<Actor>,
    review: Actor,
}

impl Scene {
    async fn new() -> Self {
        let fx = Fixture::new().await;
        // Each author leads the area of its own task, as only a lead claims a task.
        for title in ["docs", "extra"] {
            give_role(&fx.handle.store, title, Role::AreaLead, Some(title)).await;
            fx.handle
                .tasks
                .create_task(
                    &human("tanim"),
                    CreateTask {
                        title: TaskTitle::parse(title).unwrap(),
                        description: TaskDescription::new("what to build"),
                        labels: vec![fold_core::LabelName::parse(title).unwrap()],
                    },
                )
                .await
                .unwrap();
        }
        let changes: [(&str, &[(&str, &str)]); 4] = [
            (
                "frontend",
                &[
                    ("README.md", "top one\n# demo\n"),
                    ("src/one.rs", "one a\none b\n"),
                ],
            ),
            (
                "backend",
                &[("README.md", "# demo\nfrom two\n"), ("src/two.rs", "two\n")],
            ),
            ("docs", &[("src/three.rs", "three\n")]),
            ("extra", &[("README.md", "# demo\nfrom four\n")]),
        ];
        let mut authors = Vec::new();
        for (task, (name, files)) in (1u32..).zip(changes) {
            let author = fx.agent(name).await;
            fx.workspaces()
                .create_workspace(&author, t(task))
                .await
                .unwrap();
            for (path, content) in files {
                fx.write(task, path, content);
            }
            fx.reviews()
                .create_proposal(&author, t(task), "Change", Some("tests pass"))
                .await
                .unwrap()
                .detail()
                .unwrap();
            authors.push(author);
        }
        fx.handle
            .tasks
            .add_dependency(&human("tanim"), t(1), t(3))
            .await
            .unwrap();
        fx.agent("review").await;
        // Only the main agent makes a batch itself; a reviewer gets its batch from the review lead's start.
        crate::common::roles::give_main_tag(&fx.handle.store, "review").await;
        let review = Actor::Agent(AgentActor {
            agent: AgentName::parse("review").unwrap(),
            worker: WorkerName::parse("review").unwrap(),
            tags: Vec::new(),
        });
        Self {
            fx,
            authors,
            review,
        }
    }

    async fn summary(&self, proposal: ProposalId) -> ProposalSummary {
        self.fx
            .reviews()
            .get_proposal(proposal, None)
            .await
            .unwrap()
            .summary
    }

    async fn batch(&self, proposals: &[u32]) -> ReviewBatchId {
        let proposals = proposals.iter().copied().map(p).collect();
        self.fx
            .reviews()
            .create_review_batch(&self.review, proposals)
            .await
            .unwrap()
            .id
    }

    fn read(&self, path: &str) -> String {
        std::fs::read_to_string(self.fx.project.paths().root().join(path)).unwrap()
    }

    async fn event_count(&self, name: &str) -> usize {
        fold_store::events::read_after(
            self.fx.handle.store.read(),
            0,
            &fold_core::EventFilter::default(),
            1000,
        )
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type.as_str() == name)
        .count()
    }
}

fn verdict(proposal: u32, verdict: Verdict, body: &str) -> BatchVerdict {
    BatchVerdict {
        proposal: p(proposal),
        verdict,
        body: body.to_owned(),
    }
}

fn on_lines(path: &str, start_line: u32, end_line: u32, body: &str) -> BatchComment {
    BatchComment {
        location: Some(BatchLines {
            path: path.to_owned(),
            start_line,
            end_line,
        }),
        members: Vec::new(),
        body: body.to_owned(),
    }
}

#[tokio::test]
async fn a_batch_takes_every_claim_and_its_checkout_merges_the_members_in_dependency_order() {
    let scene = Scene::new().await;
    let batch = scene
        .fx
        .reviews()
        .create_review_batch(&scene.review, vec![p(1), p(2), p(3)])
        .await
        .unwrap();
    assert_eq!(batch.id, rb(1));
    assert_eq!(batch.state, BatchState::Open);
    let order: Vec<(ProposalId, u32, u32)> = batch
        .members
        .iter()
        .map(|member| (member.proposal, member.position, member.version))
        .collect();
    assert_eq!(order, [(p(2), 1, 1), (p(3), 2, 1), (p(1), 3, 1)]);
    for proposal in [p(1), p(2), p(3)] {
        let claim = scene.summary(proposal).await.review_claim;
        assert_eq!(claim.as_ref().map(WorkerName::as_str), Some("review"));
    }
    assert_eq!(scene.event_count("review.batch_created").await, 1);

    let checkout = scene
        .fx
        .reviews()
        .create_batch_checkout(&scene.review, batch.id)
        .await
        .unwrap();
    assert_eq!(checkout.workspace.path, ".fold/ws/review-rb-1");
    assert_eq!(checkout.members, [(p(2), 1), (p(3), 1), (p(1), 1)]);
    assert_eq!(
        scene.read(".fold/ws/review-rb-1/README.md"),
        "top one\n# demo\nfrom two\n"
    );
    assert_eq!(
        scene.read(".fold/ws/review-rb-1/src/one.rs"),
        "one a\none b\n"
    );
    assert_eq!(scene.read(".fold/ws/review-rb-1/src/two.rs"), "two\n");
    assert_eq!(scene.read(".fold/ws/review-rb-1/src/three.rs"), "three\n");
    assert_eq!(
        scene.read(".fold/ws/review-rb-1/src/lib.rs"),
        "pub fn x() {}\n"
    );

    let again = scene
        .fx
        .reviews()
        .create_batch_checkout(&scene.review, batch.id)
        .await
        .unwrap();
    assert_eq!(again.workspace.id, checkout.workspace.id);
    let view = scene.fx.reviews().get_review_batch(batch.id).await.unwrap();
    assert_eq!(
        view.checkout.map(|workspace| workspace.id),
        Some(checkout.workspace.id)
    );
    let listed = scene.fx.reviews().list_review_batches(true).await.unwrap();
    assert_eq!(listed.len(), 1);
}

#[tokio::test]
async fn a_member_that_conflicts_is_refused_with_both_ids_until_it_is_removed() {
    let scene = Scene::new().await;
    let batch = scene.batch(&[1, 2, 3, 4]).await;

    let error = refusal(
        scene
            .fx
            .reviews()
            .create_batch_checkout(&scene.review, batch)
            .await,
    );
    assert_eq!(error.code(), codes::BATCH_CONFLICT);
    let found = details(&error);
    assert_eq!(found["proposal"], "P-4");
    assert_eq!(found["with"], "P-2");
    assert_eq!(found["paths"], json!(["README.md"]));
    assert!(
        !scene
            .fx
            .project
            .paths()
            .root()
            .join(".fold/ws/review-rb-1")
            .exists()
    );

    let view = scene
        .fx
        .reviews()
        .remove_from_batch(&scene.review, batch, p(4))
        .await
        .unwrap();
    let removed = view.members.iter().find(|member| member.proposal == p(4));
    assert_eq!(
        removed.and_then(|member| member.released),
        Some(MemberRelease::Removed)
    );
    assert_eq!(scene.summary(p(4)).await.review_claim, None);
    let checkout = scene
        .fx
        .reviews()
        .create_batch_checkout(&scene.review, batch)
        .await
        .unwrap();
    assert_eq!(checkout.members, [(p(2), 1), (p(3), 1), (p(1), 1)]);

    let again = refusal(
        scene
            .fx
            .reviews()
            .remove_from_batch(&scene.review, batch, p(4))
            .await,
    );
    assert_eq!(again.code(), codes::NOT_BATCH_MEMBER);
    scene
        .fx
        .reviews()
        .remove_from_batch(&scene.review, batch, p(3))
        .await
        .unwrap();
    let view = scene.fx.reviews().get_review_batch(batch).await.unwrap();
    assert_eq!(view.checkout, None, "a removal archives the checkout");
}

/// The project's checkout list shows a batch checkout next to the checkout of one proposal, with the members its files hold in merge order and each checkout's count of conflicted paths; a member that leaves archives the batch checkout, so the list drops it.
#[tokio::test]
async fn the_checkout_list_shows_a_batch_checkout_with_its_members() {
    let scene = Scene::new().await;
    let reviews = scene.fx.reviews();
    let batch = scene.batch(&[1, 2, 3]).await;
    let checkout = reviews
        .create_batch_checkout(&scene.review, batch)
        .await
        .unwrap();
    reviews.claim_review(&scene.review, p(4)).await.unwrap();
    let single = reviews
        .create_review_checkout(&scene.review, p(4))
        .await
        .unwrap();
    fold_store::testing::execute(
        scene.fx.project.store(),
        &format!(
            "INSERT INTO workspace_conflicts (workspace_id, path, kind)
             SELECT id, CAST('README.md' AS BLOB), 'both_modified_text' FROM workspaces WHERE number = {}",
            single.workspace.id.number()
        ),
    )
    .await;

    let listed = reviews.list_review_checkouts().await.unwrap();
    let rows: Vec<_> = listed
        .iter()
        .map(|row| {
            (
                row.workspace.id,
                row.workspace.conflicted,
                row.target.clone(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (
                checkout.workspace.id,
                0,
                CheckoutTarget::Batch {
                    batch,
                    members: vec![(p(2), 1), (p(3), 1), (p(1), 1)],
                },
            ),
            (
                single.workspace.id,
                1,
                CheckoutTarget::Proposal {
                    proposal: p(4),
                    version: 1,
                },
            ),
        ]
    );

    reviews
        .remove_from_batch(&scene.review, batch, p(3))
        .await
        .unwrap();
    let listed = reviews.list_review_checkouts().await.unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|row| row.workspace.id)
            .collect::<Vec<_>>(),
        [single.workspace.id]
    );
}

#[tokio::test]
async fn a_member_that_conflicts_with_main_is_refused_naming_main() {
    let scene = Scene::new().await;
    let reviews = scene.fx.reviews();
    reviews.claim_review(&scene.review, p(2)).await.unwrap();
    reviews
        .submit_review(&scene.review, p(2), 1, Verdict::Approve, "fine", Vec::new())
        .await
        .unwrap();
    reviews
        .integrate_proposal(&scene.review, p(2), Some(1))
        .await
        .unwrap();
    let batch = scene.batch(&[3, 4]).await;

    let error = refusal(reviews.create_batch_checkout(&scene.review, batch).await);
    assert_eq!(error.code(), codes::BATCH_CONFLICT);
    assert_eq!(details(&error)["proposal"], "P-4");
    assert_eq!(details(&error)["with"], "main");
}

#[tokio::test]
async fn one_call_reviews_every_member_and_line_comments_land_on_the_member_that_wrote_them() {
    let scene = Scene::new().await;
    let batch = scene.batch(&[1, 2, 3]).await;
    let reviews = scene.fx.reviews();
    reviews
        .create_batch_checkout(&scene.review, batch)
        .await
        .unwrap();

    let submitted = reviews
        .submit_batch_review(
            &scene.review,
            batch,
            vec![
                verdict(2, Verdict::Approve, ""),
                verdict(3, Verdict::Approve, "fine"),
                verdict(1, Verdict::RequestChanges, "rename the heading"),
            ],
            vec![
                on_lines("README.md", 3, 3, "a typo"),
                on_lines("README.md", 1, 1, "why at the top"),
                on_lines("src/one.rs", 2, 2, "unused"),
                BatchComment {
                    location: None,
                    members: vec![p(1), p(3)],
                    body: "together they break the build".to_owned(),
                },
            ],
        )
        .await
        .unwrap();
    let verdicts: Vec<(ProposalId, Verdict, usize)> = submitted
        .reviewed
        .iter()
        .map(|review| (review.proposal, review.verdict, review.comments.len()))
        .collect();
    assert_eq!(
        verdicts,
        [
            (p(2), Verdict::Approve, 1),
            (p(3), Verdict::Approve, 1),
            (p(1), Verdict::RequestChanges, 3),
        ]
    );
    assert!(submitted.left_out.is_empty());

    let on_two = reviews.list_comments(p(2), None).await.unwrap();
    assert_eq!(on_two.len(), 1);
    assert_eq!(on_two[0].version, 1);
    assert_eq!(on_two[0].verdict, Verdict::Approve);
    assert_eq!(
        on_two[0].location,
        Some(CommentLocation {
            path: "README.md".to_owned(),
            side: Side::New,
            start_line: 2,
            end_line: 2,
        })
    );
    let on_one = reviews.list_comments(p(1), None).await.unwrap();
    let located: Vec<Option<(String, u32)>> = on_one
        .iter()
        .map(|comment| {
            comment
                .location
                .as_ref()
                .map(|location| (location.path.clone(), location.start_line))
        })
        .collect();
    assert_eq!(
        located,
        [
            Some(("README.md".to_owned(), 1)),
            Some(("src/one.rs".to_owned(), 2)),
            None,
        ]
    );
    let on_three = reviews.list_comments(p(3), None).await.unwrap();
    assert_eq!(on_three.len(), 1);
    assert_eq!(on_three[0].body, "together they break the build");

    assert_eq!(
        scene.summary(p(1)).await.latest_verdict,
        Some(Verdict::RequestChanges)
    );
    assert_eq!(
        scene.summary(p(2)).await.latest_verdict,
        Some(Verdict::Approve)
    );
    assert_eq!(
        scene.summary(p(3)).await.latest_verdict,
        Some(Verdict::Approve)
    );
    let view = reviews.get_review_batch(batch).await.unwrap();
    assert_eq!(view.state, BatchState::Submitted);
    assert!(view.submitted_at.is_some());
    assert_eq!(view.checkout, None);
    assert_eq!(view.verdicts.len(), 3);
    assert!(
        view.members
            .iter()
            .all(|member| member.released == Some(MemberRelease::Submitted))
    );
    assert_eq!(scene.event_count("review.batch_submitted").await, 1);
    assert!(reviews.list_review_batches(true).await.unwrap().is_empty());

    let again = refusal(
        reviews
            .submit_batch_review(&scene.review, batch, Vec::new(), Vec::new())
            .await,
    );
    assert_eq!(again.code(), codes::REVIEW_BATCH_NOT_OPEN);
}

#[tokio::test]
async fn a_comment_on_lines_no_single_member_changed_is_refused() {
    let scene = Scene::new().await;
    let batch = scene.batch(&[1, 2]).await;
    let reviews = scene.fx.reviews();
    let verdicts = || {
        vec![
            verdict(1, Verdict::Approve, ""),
            verdict(2, Verdict::Approve, ""),
        ]
    };

    let early = refusal(
        reviews
            .submit_batch_review(
                &scene.review,
                batch,
                verdicts(),
                vec![on_lines("README.md", 1, 1, "x")],
            )
            .await,
    );
    assert_eq!(early.code(), codes::INVALID_REQUEST);
    assert_eq!(details(&early)["reason"], "no_batch_checkout");

    reviews
        .create_batch_checkout(&scene.review, batch)
        .await
        .unwrap();
    for (start, end) in [(2, 2), (1, 3), (9, 9)] {
        let error = refusal(
            reviews
                .submit_batch_review(
                    &scene.review,
                    batch,
                    verdicts(),
                    vec![on_lines("README.md", start, end, "x")],
                )
                .await,
        );
        assert_eq!(error.code(), codes::UNATTRIBUTED_LINES, "{start}-{end}");
    }
    let backwards = refusal(
        reviews
            .submit_batch_review(
                &scene.review,
                batch,
                verdicts(),
                vec![on_lines("README.md", 3, 1, "x")],
            )
            .await,
    );
    assert_eq!(backwards.code(), codes::INVALID_COMMENT_RANGE);
    let unnamed = refusal(
        reviews
            .submit_batch_review(
                &scene.review,
                batch,
                verdicts(),
                vec![BatchComment {
                    location: None,
                    members: Vec::new(),
                    body: "x".to_owned(),
                }],
            )
            .await,
    );
    assert_eq!(unnamed.code(), codes::INVALID_REQUEST);
}

#[tokio::test]
async fn a_member_that_gets_a_new_version_keeps_no_approval_from_the_batch() {
    let scene = Scene::new().await;
    let reviews = scene.fx.reviews();

    // A new version during the review: the batch leaves the member out.
    let batch = scene.batch(&[2, 3]).await;
    scene.fx.write(2, "README.md", "# demo\nfrom two, again\n");
    reviews
        .update_proposal(&scene.authors[1], p(2), None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    let submitted = reviews
        .submit_batch_review(
            &scene.review,
            batch,
            vec![
                verdict(2, Verdict::Approve, ""),
                verdict(3, Verdict::Approve, ""),
            ],
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(submitted.reviewed.len(), 1);
    assert_eq!(submitted.reviewed[0].proposal, p(3));
    assert_eq!(submitted.left_out.len(), 1);
    assert_eq!(submitted.left_out[0].proposal, p(2));
    assert_eq!(submitted.left_out[0].reason, LeftOutReason::NewVersion);
    let two = scene.summary(p(2)).await;
    assert_eq!(two.current_version, 2);
    assert_eq!(two.latest_verdict, None);

    // A new version after the review: the approval belongs to the version the batch reviewed.
    let batch = scene.batch(&[1, 4]).await;
    reviews
        .submit_batch_review(
            &scene.review,
            batch,
            vec![
                verdict(1, Verdict::Approve, ""),
                verdict(4, Verdict::Approve, ""),
            ],
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        scene.summary(p(1)).await.latest_verdict,
        Some(Verdict::Approve)
    );
    scene.fx.write(1, "src/one.rs", "one a\n");
    reviews
        .update_proposal(&scene.authors[0], p(1), None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    assert_eq!(scene.summary(p(1)).await.latest_verdict, None);
}

#[tokio::test]
async fn a_member_that_closed_or_lost_its_claim_is_left_out() {
    let scene = Scene::new().await;
    let reviews = scene.fx.reviews();

    let batch = scene.batch(&[2, 3]).await;
    reviews
        .close_proposal(&scene.authors[1], p(2), "not needed")
        .await
        .unwrap();
    let submitted = reviews
        .submit_batch_review(
            &scene.review,
            batch,
            vec![verdict(3, Verdict::Approve, "")],
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(submitted.left_out.len(), 1);
    assert_eq!(submitted.left_out[0].proposal, p(2));
    assert_eq!(submitted.left_out[0].reason, LeftOutReason::NotOpen);

    let batch = scene.batch(&[1, 4]).await;
    reviews
        .release_review(&scene.review, p(4), None)
        .await
        .unwrap();
    let submitted = reviews
        .submit_batch_review(
            &scene.review,
            batch,
            vec![
                verdict(1, Verdict::Approve, ""),
                verdict(4, Verdict::Approve, ""),
            ],
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(submitted.reviewed.len(), 1);
    assert_eq!(submitted.left_out[0].proposal, p(4));
    assert_eq!(submitted.left_out[0].reason, LeftOutReason::ClaimLost);
    assert_eq!(scene.summary(p(4)).await.latest_verdict, None);
}

#[tokio::test]
async fn batch_commands_refuse_what_the_rules_forbid() {
    let scene = Scene::new().await;
    let reviews = scene.fx.reviews();

    let one = refusal(reviews.create_review_batch(&scene.review, vec![p(1)]).await);
    assert_eq!(one.code(), codes::INVALID_REQUEST);
    let repeated = refusal(
        reviews
            .create_review_batch(&scene.review, vec![p(1), p(1)])
            .await,
    );
    assert_eq!(repeated.code(), codes::INVALID_REQUEST);
    let own = refusal(
        reviews
            .create_review_batch(&scene.authors[0], vec![p(1), p(2)])
            .await,
    );
    assert_eq!(own.code(), codes::AUTHOR_CANNOT_REVIEW);

    let batch = scene.batch(&[1, 2]).await;
    let twice = refusal(
        reviews
            .create_review_batch(&scene.review, vec![p(1), p(3)])
            .await,
    );
    assert_eq!(twice.code(), codes::ALREADY_IN_BATCH);
    assert_eq!(details(&twice)["batch"], "rb-1");
    let no_role = refusal(
        reviews
            .create_review_batch(&scene.authors[3], vec![p(2), p(3)])
            .await,
    );
    assert_eq!(no_role.code(), codes::INTEGRATOR_REQUIRED);
    give_role(&scene.fx.handle.store, "checker", Role::Reviewer, None).await;
    let checker = scene.fx.agent("checker").await;
    let worker = refusal(
        reviews
            .create_review_batch(&checker, vec![p(1), p(3)])
            .await,
    );
    assert_eq!(worker.code(), codes::WORKER_CLAIM_DENIED);
    give_review_claim(
        &scene.fx.handle.store,
        "checker",
        p(3),
        scene.fx.handle.clock.now(),
    )
    .await;
    let other = refusal(
        reviews
            .create_review_batch(&scene.review, vec![p(3), p(4)])
            .await,
    );
    assert_eq!(other.code(), codes::ALREADY_CLAIMED);
    assert_eq!(
        scene.summary(p(4)).await.review_claim,
        None,
        "a refused batch takes no claim"
    );

    let stranger = &scene.authors[2];
    let remove = refusal(reviews.remove_from_batch(stranger, batch, p(1)).await);
    assert_eq!(remove.code(), codes::NOT_CLAIM_HOLDER);
    let checkout = refusal(reviews.create_batch_checkout(stranger, batch).await);
    assert_eq!(checkout.code(), codes::NOT_CLAIM_HOLDER);
    let submit = refusal(
        reviews
            .submit_batch_review(stranger, batch, Vec::new(), Vec::new())
            .await,
    );
    assert_eq!(submit.code(), codes::NOT_CLAIM_HOLDER);

    let missing = refusal(
        reviews
            .submit_batch_review(
                &scene.review,
                batch,
                vec![verdict(1, Verdict::Approve, "")],
                Vec::new(),
            )
            .await,
    );
    assert_eq!(missing.code(), codes::INVALID_REQUEST);
    assert_eq!(details(&missing)["missing"], json!(["P-2"]));
    let outsider = refusal(
        reviews
            .submit_batch_review(
                &scene.review,
                batch,
                vec![verdict(3, Verdict::Approve, "")],
                Vec::new(),
            )
            .await,
    );
    assert_eq!(outsider.code(), codes::NOT_BATCH_MEMBER);
    let doubled = refusal(
        reviews
            .submit_batch_review(
                &scene.review,
                batch,
                vec![
                    verdict(1, Verdict::Approve, ""),
                    verdict(1, Verdict::Approve, ""),
                ],
                Vec::new(),
            )
            .await,
    );
    assert_eq!(doubled.code(), codes::INVALID_REQUEST);
    let silent = refusal(
        reviews
            .submit_batch_review(
                &scene.review,
                batch,
                vec![
                    verdict(1, Verdict::RequestChanges, ""),
                    verdict(2, Verdict::Approve, ""),
                ],
                Vec::new(),
            )
            .await,
    );
    assert_eq!(silent.code(), codes::EMPTY_REVIEW);
    let reject = refusal(
        reviews
            .submit_batch_review(
                &scene.review,
                batch,
                vec![
                    verdict(1, Verdict::Reject, " "),
                    verdict(2, Verdict::Approve, ""),
                ],
                Vec::new(),
            )
            .await,
    );
    assert_eq!(reject.code(), codes::EMPTY_REVIEW);

    let unknown = refusal(reviews.get_review_batch(rb(9)).await);
    assert_eq!(unknown.code(), codes::REVIEW_BATCH_NOT_FOUND);
}

#[tokio::test]
async fn a_reject_in_a_batch_closes_that_member() {
    let scene = Scene::new().await;
    let batch = scene.batch(&[1, 3]).await;
    let reviews = scene.fx.reviews();
    let submitted = reviews
        .submit_batch_review(
            &scene.review,
            batch,
            vec![
                verdict(1, Verdict::Reject, "not needed"),
                verdict(3, Verdict::Comment, "looks right"),
            ],
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(submitted.reviewed.len(), 2);
    let one = scene.summary(p(1)).await;
    assert_eq!(one.state.as_str(), "closed");
    assert_eq!(scene.summary(p(3)).await.latest_verdict, None);
}

#[tokio::test]
async fn a_comment_on_members_together_is_one_batch_comment_on_each_member() {
    let scene = Scene::new().await;
    let batch = scene.batch(&[1, 2, 3]).await;
    let reviews = scene.fx.reviews();
    let submitted = reviews
        .submit_batch_review(
            &scene.review,
            batch,
            vec![
                verdict(1, Verdict::RequestChanges, ""),
                verdict(2, Verdict::Approve, ""),
                verdict(3, Verdict::RequestChanges, ""),
            ],
            vec![BatchComment {
                location: None,
                members: vec![p(3), p(1)],
                body: " together they break the build ".to_owned(),
            }],
        )
        .await
        .unwrap();
    let counts: Vec<usize> = submitted
        .reviewed
        .iter()
        .map(|review| review.comments.len())
        .collect();
    assert_eq!(counts, [0, 1, 1], "P-2, P-3, P-1 in merge order");

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM review_batch_comments")
        .fetch_one(scene.fx.handle.store.read())
        .await
        .unwrap();
    assert_eq!(rows, 1);
    let linked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM review_comments WHERE batch_comment_id IS NOT NULL AND body = ''",
    )
    .fetch_one(scene.fx.handle.store.read())
    .await
    .unwrap();
    assert_eq!(linked, 2);

    let view = reviews.get_review_batch(batch).await.unwrap();
    assert_eq!(view.comments.len(), 1);
    assert_eq!(view.comments[0].number, 1);
    assert_eq!(view.comments[0].body, "together they break the build");
    assert_eq!(view.comments[0].members, [p(1), p(3)]);
    for member in [p(1), p(3)] {
        let comments = reviews.list_comments(member, None).await.unwrap();
        assert_eq!(comments.len(), 1, "{member}");
        assert_eq!(comments[0].body, "together they break the build");
        assert_eq!(comments[0].location, None);
        let link = comments[0].batch_comment.as_ref().unwrap();
        assert_eq!(link.batch, batch);
        assert_eq!(link.number, 1);
        assert_eq!(link.members, [p(1), p(3)]);
    }
    assert!(reviews.list_comments(p(2), None).await.unwrap().is_empty());
    let events = fold_store::events::read_after(
        scene.fx.handle.store.read(),
        0,
        &fold_core::EventFilter::default(),
        1000,
    )
    .await
    .unwrap();
    let linked_events = events
        .iter()
        .filter(|event| event.event_type.as_str() == "review.comment_added")
        .filter(|event| event.data["batch_comment"] == json!(1) && event.data["batch"] == "rb-1")
        .count();
    assert_eq!(linked_events, 2);
}

#[tokio::test]
async fn a_batch_whose_members_all_integrate_closes_and_archives_its_checkout() {
    let scene = Scene::new().await;
    let batch = scene.batch(&[2, 3]).await;
    let reviews = scene.fx.reviews();
    let checkout = reviews
        .create_batch_checkout(&scene.review, batch)
        .await
        .unwrap()
        .workspace
        .id;
    for proposal in [p(2), p(3)] {
        reviews
            .submit_review(
                &human("tanim"),
                proposal,
                1,
                Verdict::Approve,
                "fine",
                Vec::new(),
            )
            .await
            .unwrap();
    }

    reviews
        .integrate_proposal(&scene.review, p(2), None)
        .await
        .unwrap();
    let view = reviews.get_review_batch(batch).await.unwrap();
    assert_eq!(view.state, BatchState::Open, "P-3 is still to review");
    assert_eq!(view.checkout.map(|workspace| workspace.id), Some(checkout));

    reviews
        .integrate_proposal(&scene.review, p(3), None)
        .await
        .unwrap();
    let view = reviews.get_review_batch(batch).await.unwrap();
    assert_eq!(view.state, BatchState::Closed);
    assert!(view.closed_at.is_some());
    assert_eq!(view.submitted_at, None);
    assert_eq!(view.checkout, None);
    assert!(
        view.members
            .iter()
            .all(|member| member.released == Some(MemberRelease::Closed))
    );
    let archived = scene
        .fx
        .workspaces()
        .get(WorkspaceRef::Workspace(checkout))
        .await
        .unwrap();
    assert_eq!(archived.state, WorkspaceState::Archived);
    assert_eq!(scene.event_count("review.batch_closed").await, 1);
    let again = refusal(
        reviews
            .submit_batch_review(&scene.review, batch, Vec::new(), Vec::new())
            .await,
    );
    assert_eq!(again.code(), codes::REVIEW_BATCH_NOT_OPEN);
}

#[tokio::test]
async fn a_batch_whose_last_member_is_removed_or_closed_closes() {
    let scene = Scene::new().await;
    let reviews = scene.fx.reviews();
    let batch = scene.batch(&[2, 3]).await;
    reviews
        .remove_from_batch(&scene.review, batch, p(2))
        .await
        .unwrap();
    let view = reviews
        .remove_from_batch(&scene.review, batch, p(3))
        .await
        .unwrap();
    assert_eq!(view.state, BatchState::Closed);

    let batch = scene.batch(&[1, 4]).await;
    reviews
        .close_proposal(&scene.authors[0], p(1), "not needed")
        .await
        .unwrap();
    assert_eq!(
        reviews.get_review_batch(batch).await.unwrap().state,
        BatchState::Open
    );
    reviews
        .close_proposal(&scene.authors[3], p(4), "not needed")
        .await
        .unwrap();
    assert_eq!(
        reviews.get_review_batch(batch).await.unwrap().state,
        BatchState::Closed
    );
    assert_eq!(scene.event_count("review.batch_closed").await, 2);
}

#[tokio::test]
async fn a_batch_that_rejects_every_member_is_submitted_not_closed() {
    let scene = Scene::new().await;
    let reviews = scene.fx.reviews();
    let batch = scene.batch(&[2, 3]).await;
    reviews
        .submit_batch_review(
            &scene.review,
            batch,
            vec![
                verdict(2, Verdict::Reject, "out of scope"),
                verdict(3, Verdict::Reject, "out of scope"),
            ],
            Vec::new(),
        )
        .await
        .unwrap();
    let view = reviews.get_review_batch(batch).await.unwrap();
    assert_eq!(view.state, BatchState::Submitted);
    assert_eq!(scene.event_count("review.batch_closed").await, 0);
}
