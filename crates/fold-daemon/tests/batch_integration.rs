#![cfg(test)]

use std::time::Duration;

use crate::common::invariants::assert_consistent;
use crate::common::roles::{give_main_tag, give_role};
use crate::proposal_tools_support::{Fixture, human, t};
use fold_core::reviews::Verdict;
use fold_core::roles::Role;
use fold_core::{
    Actor, AgentActor, AgentName, FoldError, ProposalId, ReviewBatchId, RevisionId,
    TaskDescription, TaskStatus, TaskTitle, WorkerName, codes,
};
use fold_daemon::services::reviews::{
    BatchIntegration, BatchIntegrationView, BatchVerdict, IntegrationOutcome, MemberIntegration,
    ReviewError,
};
use fold_daemon::services::tasks::{CreateTask, TaskError};
use fold_store::reviews::ProposalState;
use fold_store::testing::eventually;
use serde_json::Value;

const WAIT: Duration = Duration::from_secs(30);

fn p(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
}

fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

fn refusal<T: std::fmt::Debug>(result: Result<T, ReviewError>) -> FoldError {
    match result {
        Err(ReviewError::Fold(error) | ReviewError::Task(TaskError::Fold(error))) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn reason(error: &FoldError) -> Option<String> {
    error
        .details()
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The areas beyond the fixture's two, each led by the agent of the same name, so five tasks have five authors.
const MORE_AREAS: [&str; 3] = ["docs", "extra", "tools"];

/// Five open proposals, P-n for T-n, each by its own author and each adding its own file `src/m<n>.rs`, so no two conflict. `review`, the main agent, makes the batch and gives the verdicts.
struct Scene {
    fx: Fixture,
    authors: Vec<Actor>,
    review: Actor,
}

impl Scene {
    async fn new() -> Self {
        Self::with(|_, _| {}).await
    }

    /// [`Scene::with`], then main gets a `fold-checks.toml` whose one integration check runs `run`: a change of the checks file waits for a human, so `ops` brings it in as P-6 of T-6, which the main agent approves and `tanim` integrates as `r2`.
    async fn with_checks(run: &str, extra: impl Fn(u32, &std::path::Path)) -> Self {
        let scene = Self::with(extra).await;
        let fx = &scene.fx;
        give_role(&fx.handle.store, "ops", Role::AreaLead, Some("ops")).await;
        fx.handle
            .tasks
            .create_task(
                &human("tanim"),
                CreateTask {
                    title: TaskTitle::parse("checks").unwrap(),
                    description: TaskDescription::new("the integration checks"),
                    labels: vec![fold_core::LabelName::parse("ops").unwrap()],
                },
            )
            .await
            .unwrap();
        let ops = fx.agent("ops").await;
        fx.workspaces().create_workspace(&ops, t(6)).await.unwrap();
        checks_file(&fx.project.paths().root().join(".fold/ws/t-6"), run);
        fx.reviews()
            .create_proposal(
                &ops,
                t(6),
                "Checks",
                "Make the change the task asks for.",
                Some("tests pass"),
            )
            .await
            .unwrap()
            .detail()
            .unwrap();
        fx.reviews()
            .submit_review(&scene.review, p(6), 1, Verdict::Approve, "ok", Vec::new())
            .await
            .unwrap();
        fx.reviews()
            .integrate_proposal(&human("tanim"), p(6), Some(1))
            .await
            .unwrap();
        eventually(WAIT, async || match scene.state(6).await {
            ProposalState::Integrated => Ok(()),
            other => Err(other),
        })
        .await;
        scene
    }

    /// [`Scene::new`] where `extra(task, dir)` may write more files into the workspace of each task before its proposal opens.
    async fn with(extra: impl Fn(u32, &std::path::Path)) -> Self {
        let fx = Fixture::new().await;
        for area in MORE_AREAS {
            give_role(&fx.handle.store, area, Role::AreaLead, Some(area)).await;
            fx.handle
                .tasks
                .create_task(
                    &human("tanim"),
                    CreateTask {
                        title: TaskTitle::parse(area).unwrap(),
                        description: TaskDescription::new("what to build"),
                        labels: vec![fold_core::LabelName::parse(area).unwrap()],
                    },
                )
                .await
                .unwrap();
        }
        let names = ["frontend", "backend", "docs", "extra", "tools"];
        let mut authors = Vec::new();
        for (task, name) in (1u32..).zip(names) {
            let author = fx.agent(name).await;
            fx.workspaces()
                .create_workspace(&author, t(task))
                .await
                .unwrap();
            fx.write(
                task,
                &format!("src/m{task}.rs"),
                &format!("member {task}\n"),
            );
            extra(
                task,
                &fx.project.paths().root().join(format!(".fold/ws/t-{task}")),
            );
            fx.reviews()
                .create_proposal(
                    &author,
                    t(task),
                    "Change",
                    &format!("Changes m{task}.rs; nothing else changes."),
                    Some("tests pass"),
                )
                .await
                .unwrap()
                .detail()
                .unwrap();
            authors.push(author);
        }
        fx.agent("review").await;
        give_main_tag(&fx.handle.store, "review").await;
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

    async fn batch(&self, proposals: &[u32]) -> ReviewBatchId {
        self.fx
            .reviews()
            .create_review_batch(&self.review, proposals.iter().copied().map(p).collect())
            .await
            .unwrap()
            .id
    }

    /// Submits the batch's review: `changes` get changes requested, every other open member is approved.
    async fn review(&self, batch: ReviewBatchId, members: &[u32], changes: &[u32]) {
        let verdicts = members
            .iter()
            .map(|&number| BatchVerdict {
                proposal: p(number),
                verdict: if changes.contains(&number) {
                    Verdict::RequestChanges
                } else {
                    Verdict::Approve
                },
                body: if changes.contains(&number) {
                    "rename the module".to_owned()
                } else {
                    String::new()
                },
            })
            .collect();
        self.fx
            .reviews()
            .submit_batch_review(&self.review, batch, verdicts, Vec::new())
            .await
            .unwrap();
    }

    async fn state(&self, proposal: u32) -> ProposalState {
        self.fx
            .reviews()
            .get_proposal(p(proposal), None)
            .await
            .unwrap()
            .summary
            .state
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql.to_owned()))
            .fetch_one(self.fx.handle.store.read())
            .await
            .unwrap()
    }

    /// The revision and the batch of each succeeded integration, in revision order, as `(proposal, revision, batch)`.
    async fn integrations(&self) -> Vec<(i64, i64, Option<i64>)> {
        sqlx::query_as::<_, (i64, i64, Option<i64>)>(
            "SELECT p.number, r.number, b.number FROM integrations i
             JOIN proposals p ON p.id = i.proposal_id
             JOIN revisions r ON r.id = i.revision_id
             LEFT JOIN review_batches b ON b.id = i.review_batch_id
             WHERE i.result = 'succeeded' ORDER BY r.number",
        )
        .fetch_all(self.fx.handle.store.read())
        .await
        .unwrap()
    }

    async fn events(&self, name: &str) -> Vec<fold_core::Event> {
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
        .collect()
    }

    async fn status(&self, task: u32) -> TaskStatus {
        self.fx
            .handle
            .tasks
            .get_view(t(task))
            .await
            .unwrap()
            .task
            .status
    }
}

/// A `fold-checks.toml` with one integration check that runs `run` on the merged tree.
fn checks_file(dir: &std::path::Path, run: &str) {
    std::fs::write(
        dir.join("fold-checks.toml"),
        format!("[[check]]\nname = \"suite\"\nrun = \"{run}\"\nstage = \"integration\"\n"),
    )
    .unwrap();
}

fn succeeded(members: &[MemberIntegration]) -> Vec<(ProposalId, RevisionId)> {
    members
        .iter()
        .filter_map(|member| match member.outcome {
            IntegrationOutcome::Succeeded { revision } => Some((member.proposal, revision)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn four_approved_members_of_a_five_member_batch_integrate_with_one_check_run() {
    let scene = Scene::with_checks("test -f src/lib.rs", |_, _| {}).await;
    let batch = scene.batch(&[1, 2, 3, 4, 5]).await;
    // P-5's author sends v2 before the review, so the batch review leaves it out and it is approved on its own later.
    scene.fx.write(5, "src/m5.rs", "member 5, second try\n");
    scene
        .fx
        .reviews()
        .update_proposal(
            &scene.authors[4],
            p(5),
            "Make the change the task asks for.",
            Some("tests pass"),
        )
        .await
        .unwrap();
    scene.review(batch, &[1, 2, 3, 4], &[1]).await;

    let none = scene.fx.reviews().integrate_ready().await.unwrap();
    assert!(none.is_empty(), "the batch waits for P-5: {none:?}");
    assert!(matches!(
        scene.fx.reviews().batch_integration_view(batch).await.unwrap(),
        BatchIntegrationView::Waiting { pending } if pending == [p(5)]
    ));

    scene
        .fx
        .reviews()
        .submit_review(&scene.review, p(5), 2, Verdict::Approve, "ok", Vec::new())
        .await
        .unwrap();
    let done = scene.fx.reviews().integrate_ready().await.unwrap();
    let revisions: Vec<(ProposalId, RevisionId)> = done
        .iter()
        .filter_map(|done| match done.outcome {
            IntegrationOutcome::Succeeded { revision } => Some((done.proposal, revision)),
            _ => None,
        })
        .collect();
    assert_eq!(
        revisions,
        [(p(2), r(3)), (p(3), r(4)), (p(4), r(5)), (p(5), r(6))]
    );
    assert_eq!(
        scene
            .count("SELECT COUNT(*) FROM check_runs WHERE stage = 'integration'")
            .await,
        2,
        "P-6's own run, then one run for the four members"
    );
    assert_eq!(
        scene
            .count(
                "SELECT COUNT(*) FROM check_runs WHERE stage = 'integration' AND state = 'passed'"
            )
            .await,
        2
    );
    assert_eq!(
        scene.integrations().await,
        [
            (6, 2, None),
            (2, 3, Some(1)),
            (3, 4, Some(1)),
            (4, 5, Some(1)),
            (5, 6, Some(1))
        ]
    );
    for task in 2..=5 {
        assert_eq!(scene.status(task).await, TaskStatus::Done);
    }
    // The one run names the batch and shows on the page of each member it checked, not on the member that went back.
    let mut shown = Vec::new();
    for member in 1..=5 {
        let view = scene
            .fx
            .reviews()
            .view_proposal(&human("tanim"), p(member), None)
            .await
            .unwrap();
        shown.push(
            view.checks
                .iter()
                .filter(|check| check.run.batch == Some(batch))
                .map(|check| check.run.id)
                .collect::<Vec<_>>(),
        );
    }
    assert!(shown[0].is_empty(), "P-1 went back: {shown:?}");
    assert!(
        shown[1..]
            .iter()
            .all(|runs| runs.len() == 1 && runs[0] == shown[4][0]),
        "{shown:?}"
    );
    assert_eq!(scene.state(1).await, ProposalState::Open);
    let batch_events = scene.events("review.batch_integrated").await;
    assert_eq!(batch_events.len(), 1);
    let data = &batch_events[0].data;
    assert_eq!(data["batch"], "rb-1");
    assert_eq!(data["outcome"], "together");
    let members: Vec<(String, String)> = data["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| {
            (
                member["proposal"].as_str().unwrap().to_owned(),
                member["revision"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        members,
        [
            ("P-2".to_owned(), "r3".to_owned()),
            ("P-3".to_owned(), "r4".to_owned()),
            ("P-4".to_owned(), "r5".to_owned()),
            ("P-5".to_owned(), "r6".to_owned()),
        ]
    );
    let integrated = scene.events("proposal.integrated").await;
    assert_eq!(integrated.len(), 5);
    assert!(
        integrated[1..]
            .iter()
            .all(|event| event.data["batch"] == "rb-1"),
        "each member's event names the batch"
    );
    assert!(matches!(
        scene.fx.reviews().batch_integration_view(batch).await.unwrap(),
        BatchIntegrationView::Ended { revisions, .. } if revisions.len() == 4
    ));
    // The sent-back member is out of the batch: its next version integrates on its own.
    let again = scene
        .fx
        .reviews()
        .integrate_review_batch(&human("tanim"), batch)
        .await;
    assert_eq!(reason(&refusal(again)).as_deref(), Some("batch_integrated"));
    assert_consistent(&scene.fx.project).await;
}

#[tokio::test]
async fn an_approved_member_of_an_open_batch_waits_for_the_batch_review() {
    let scene = Scene::new().await;
    scene.batch(&[1, 2]).await;
    scene
        .fx
        .reviews()
        .submit_review(&scene.review, p(1), 1, Verdict::Approve, "ok", Vec::new())
        .await
        .unwrap();
    assert!(
        scene
            .fx
            .reviews()
            .integrate_ready()
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(scene.state(1).await, ProposalState::Open);
    // A member alone is integrated on request.
    let alone = scene
        .fx
        .reviews()
        .integrate_proposal(&scene.review, p(1), Some(1))
        .await
        .unwrap();
    assert_eq!(alone, IntegrationOutcome::Succeeded { revision: r(2) });
    assert_eq!(scene.integrations().await, [(1, 2, None)]);
}

#[tokio::test]
async fn a_failed_check_run_falls_back_to_one_member_at_a_time_and_sends_back_only_the_failing_one()
{
    let scene = Scene::with_checks("test ! -f src/m2.rs", |_, _| {}).await;
    let batch = scene.batch(&[1, 2, 3]).await;
    scene.review(batch, &[1, 2, 3], &[]).await;

    let done = scene.fx.reviews().integrate_ready().await.unwrap();
    let outcomes: Vec<(ProposalId, bool)> = done
        .iter()
        .map(|done| {
            (
                done.proposal,
                matches!(done.outcome, IntegrationOutcome::Succeeded { .. }),
            )
        })
        .collect();
    assert_eq!(outcomes, [(p(1), true), (p(2), false), (p(3), true)]);
    assert!(matches!(
        done[1].outcome,
        IntegrationOutcome::ChecksFailed { .. }
    ));
    assert_eq!(
        scene
            .count("SELECT COUNT(*) FROM check_runs WHERE stage = 'integration'")
            .await,
        5,
        "P-6's own run, the batch's run, then one run for each member"
    );
    assert_eq!(
        scene.integrations().await,
        [(6, 2, None), (1, 3, None), (3, 4, None)]
    );
    let summary = scene
        .fx
        .reviews()
        .get_proposal(p(2), None)
        .await
        .unwrap()
        .summary;
    assert!(summary.checks_failed, "only P-2 went back");
    assert_eq!(scene.status(2).await, TaskStatus::InProgress);
    let ended = scene.events("review.batch_integrated").await;
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].data["outcome"], "one_at_a_time");
    assert_eq!(
        scene
            .count(
                "SELECT COUNT(*) FROM messages m JOIN message_recipients r ON r.message_id = m.id
                 JOIN workers w ON w.id = r.worker_id
                 WHERE m.sender_kind = 'fold' AND w.name = 'backend' AND m.body LIKE '%integration checks of P-2%'"
            )
            .await,
        1,
        "P-2's author hears why"
    );
    assert_eq!(
        scene
            .count(
                "SELECT COUNT(*) FROM messages m JOIN message_recipients r ON r.message_id = m.id
                 JOIN workers w ON w.id = r.worker_id
                 WHERE m.sender_kind = 'fold' AND w.name IN ('frontend', 'docs') AND m.body LIKE '%integration checks%'"
            )
            .await,
        0,
        "nobody else is sent back"
    );
    assert_consistent(&scene.fx.project).await;
}

#[tokio::test]
async fn a_member_that_conflicts_goes_back_alone_and_the_others_integrate_together() {
    // P-2 and P-4 both end README.md with their own line.
    let scene = Scene::with(|task, dir| match task {
        2 => std::fs::write(dir.join("README.md"), "# demo\nfrom two\n").unwrap(),
        4 => std::fs::write(dir.join("README.md"), "# demo\nfrom four\n").unwrap(),
        _ => {}
    })
    .await;
    let batch = scene.batch(&[2, 3, 4]).await;
    scene.review(batch, &[2, 3, 4], &[]).await;

    let outcome = scene
        .fx
        .reviews()
        .integrate_review_batch(&human("tanim"), batch)
        .await
        .unwrap();
    let BatchIntegration::Together { members } = outcome else {
        panic!("expected the members together, got {outcome:?}");
    };
    assert_eq!(succeeded(&members), [(p(2), r(2)), (p(3), r(3))]);
    assert!(matches!(
        members.last().unwrap(),
        MemberIntegration {
            proposal,
            outcome: IntegrationOutcome::Conflicted(_)
        } if *proposal == p(4)
    ));
    assert_eq!(
        scene.integrations().await,
        [(2, 2, Some(1)), (3, 3, Some(1))]
    );
    let summary = scene
        .fx
        .reviews()
        .get_proposal(p(4), None)
        .await
        .unwrap()
        .summary;
    assert!(summary.conflicted, "P-4 went back with its conflict report");
    assert_eq!(scene.events("integration.conflicted").await.len(), 1);
    assert_eq!(scene.status(4).await, TaskStatus::InProgress);
    assert_consistent(&scene.fx.project).await;
}

#[tokio::test]
async fn a_batch_whose_checks_fold_failed_to_run_waits_for_main_even_when_its_last_member_conflicts()
 {
    // P-4, the last ready member, conflicts with P-2, so the batch's run is stored on P-3.
    let scene = Scene::with_checks("test -f src/lib.rs", |task, dir| match task {
        2 => std::fs::write(dir.join("README.md"), "# demo\nfrom two\n").unwrap(),
        4 => std::fs::write(dir.join("README.md"), "# demo\nfrom four\n").unwrap(),
        _ => {}
    })
    .await;
    let batch = scene.batch(&[2, 3, 4]).await;
    scene.review(batch, &[2, 3, 4], &[]).await;
    // A file where fold's check directory belongs keeps fold from preparing it.
    let checks = scene.fx.project.paths().checks_dir();
    let parent = checks.parent().unwrap().to_path_buf();
    eventually(WAIT, async || {
        if checks.join("fold-checks.toml").is_file() {
            Ok(())
        } else {
            Err(())
        }
    })
    .await;
    std::fs::remove_dir_all(&parent).unwrap();
    std::fs::write(&parent, "").unwrap();

    assert!(
        scene
            .fx
            .reviews()
            .integrate_ready()
            .await
            .unwrap()
            .is_empty()
    );
    let runs = "SELECT COUNT(*) FROM check_runs WHERE stage = 'integration'";
    assert_eq!(scene.count(runs).await, 2, "P-6's run and the batch's");
    let stored = scene
        .count(
            "SELECT p.number FROM check_runs c JOIN proposals p ON p.id = c.proposal_id
             WHERE c.reason = 'fold_failed'",
        )
        .await;
    assert_eq!(stored, 3, "the run is on P-3, the last member that merged");
    for _ in 0..2 {
        assert!(
            scene
                .fx
                .reviews()
                .integrate_ready()
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(
        scene.count(runs).await,
        2,
        "fold does not run the batch again on the same main"
    );
    assert!(scene.integrations().await.iter().all(|row| row.0 == 6));
}

#[tokio::test]
async fn a_member_that_needs_the_sent_back_member_waits_and_integrates_on_its_own_later() {
    let scene = Scene::new().await;
    scene
        .fx
        .handle
        .tasks
        .add_dependency(&human("tanim"), t(3), t(1))
        .await
        .unwrap();
    let batch = scene.batch(&[1, 2, 3]).await;
    scene.review(batch, &[1, 2, 3], &[1]).await;

    let done = scene.fx.reviews().integrate_ready().await.unwrap();
    assert_eq!(done.len(), 1, "{done:?}");
    assert_eq!(done[0].proposal, p(2));
    assert_eq!(scene.integrations().await, [(2, 2, Some(1))]);
    assert_eq!(scene.state(3).await, ProposalState::Open);

    // P-1 comes back as v2 and is approved alone; once it is Done, P-3 follows on its own rules.
    scene.fx.write(1, "src/m1.rs", "member 1, renamed\n");
    scene
        .fx
        .reviews()
        .update_proposal(
            &scene.authors[0],
            p(1),
            "Make the change the task asks for.",
            Some("tests pass"),
        )
        .await
        .unwrap();
    scene
        .fx
        .reviews()
        .submit_review(&scene.review, p(1), 2, Verdict::Approve, "ok", Vec::new())
        .await
        .unwrap();
    let later = scene.fx.reviews().integrate_ready().await.unwrap();
    let order: Vec<ProposalId> = later.iter().map(|done| done.proposal).collect();
    assert_eq!(order, [p(1), p(3)]);
    assert_eq!(
        scene.integrations().await,
        [(2, 2, Some(1)), (1, 3, None), (3, 4, None)]
    );
    assert_consistent(&scene.fx.project).await;
}

#[tokio::test]
async fn a_deciding_agent_or_a_human_integrates_a_batch_with_one_command() {
    let scene = Scene::new().await;
    let batch = scene.batch(&[1, 2]).await;
    let open = scene
        .fx
        .reviews()
        .integrate_review_batch(&scene.review, batch)
        .await;
    assert_eq!(
        reason(&refusal(open)).as_deref(),
        Some("batch_not_submitted")
    );
    scene.review(batch, &[1, 2], &[]).await;
    let author = scene
        .fx
        .reviews()
        .integrate_review_batch(&scene.authors[1], batch)
        .await;
    assert_eq!(refusal(author).code(), codes::INTEGRATOR_REQUIRED);

    let outcome = scene
        .fx
        .reviews()
        .integrate_review_batch(&scene.review, batch)
        .await
        .unwrap();
    let BatchIntegration::Together { members } = outcome else {
        panic!("expected the members together, got {outcome:?}");
    };
    assert_eq!(succeeded(&members), [(p(1), r(2)), (p(2), r(3))]);
    // Each member is its own revision, so its commit has its own intent as the body.
    for (member, revision) in [(1, r(2)), (2, r(3))] {
        let row = fold_store::workspaces::get_by_number(scene.fx.handle.store.read(), revision)
            .await
            .unwrap();
        assert_eq!(
            fold_daemon::services::projects::commit_message(&row),
            format!(
                "Change\n\nChanges m{member}.rs; nothing else changes.\n\nFold-Proposal: P-{member}\nFold-Version: 1\nFold-Task: T-{member}\nFold-Revision: {revision}\n"
            )
        );
    }
    let integrated = scene.events("integration.succeeded").await;
    assert!(
        integrated
            .iter()
            .all(|event| event.actor.as_str() == "review" && event.data["batch"] == "rb-1")
    );
    assert_consistent(&scene.fx.project).await;
}

#[tokio::test]
async fn a_ready_batch_waits_while_an_update_of_a_member_runs_its_checks() {
    let scene = Scene::new().await;
    let batch = scene.batch(&[1, 2]).await;
    scene.review(batch, &[1, 2], &[]).await;
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    scene.fx.write(
        2,
        "fold-checks.toml",
        &format!(
            "[[check]]\nname = \"slow\"\nrun = \"until [ -e '{}' ]; do sleep 0.05; done; false\"\n",
            go.display()
        ),
    );
    let reviews = scene.fx.reviews();
    let fold_daemon::services::reviews::Proposed::Checking { run, .. } = reviews
        .update_proposal(
            &scene.authors[1],
            p(2),
            "Make the change the task asks for.",
            None,
        )
        .await
        .unwrap()
    else {
        panic!("expected a run");
    };
    assert!(reviews.integrate_ready().await.unwrap().is_empty());
    let updating = vec![(
        p(2),
        fold_daemon::services::reviews::checks::VersionRun {
            run,
            purpose: fold_core::reviews::checks::Purpose::UpdateProposal,
        },
    )];
    assert!(matches!(
        reviews.batch_integration_view(batch).await.unwrap(),
        BatchIntegrationView::Ready { updating: found, .. } if found == updating
    ));
    let held = refusal(reviews.integrate_review_batch(&scene.review, batch).await);
    assert_eq!(held.code(), codes::UPDATE_CHECKS_RUNNING);
    assert_eq!(held.details().unwrap()["purpose"], "update_proposal");
    assert_eq!(held.details().unwrap()["proposal"], "P-2");
    assert_eq!(held.details().unwrap()["run"], run.to_string());
    assert_eq!(scene.integrations().await, []);
    std::fs::write(&go, "").unwrap();
    crate::checks::ended(&reviews, run).await;
    let done = reviews.integrate_ready().await.unwrap();
    assert_eq!(
        done.iter().map(|done| done.proposal).collect::<Vec<_>>(),
        [p(1), p(2)]
    );
    assert_eq!(
        scene.integrations().await,
        [(1, 2, Some(1)), (2, 3, Some(1))]
    );
}

#[tokio::test]
async fn a_batch_waits_in_the_queue_when_the_project_has_integration_checks() {
    let scene = Scene::with_checks("test -f src/lib.rs", |_, _| {}).await;
    let batch = scene.batch(&[1, 2]).await;
    scene.review(batch, &[1, 2], &[]).await;
    let outcome = scene
        .fx
        .reviews()
        .integrate_review_batch(&human("tanim"), batch)
        .await
        .unwrap();
    assert_eq!(outcome, BatchIntegration::Queued);
    eventually(WAIT, async || {
        match scene
            .fx
            .reviews()
            .batch_integration_view(batch)
            .await
            .unwrap()
        {
            BatchIntegrationView::Ended { revisions, .. } if revisions.len() == 2 => Ok(()),
            other => Err(other),
        }
    })
    .await;
    assert_eq!(
        scene.integrations().await,
        [(6, 2, None), (1, 3, Some(1)), (2, 4, Some(1))]
    );
    assert_eq!(
        scene
            .count("SELECT COUNT(*) FROM check_runs WHERE stage = 'integration'")
            .await,
        2
    );
    eventually(WAIT, async || {
        match scene
            .count("SELECT COUNT(*) FROM review_batch_requests WHERE ended_at IS NULL")
            .await
        {
            0 => Ok(()),
            waiting => Err(waiting),
        }
    })
    .await;
    let queued = scene.events("integration.queued").await;
    assert_eq!(queued.len(), 2, "P-6's and the batch's");
    assert_eq!(queued[1].data["batch"], "rb-1");
}

#[cfg(feature = "failpoints")]
#[tokio::test]
async fn a_crash_inside_the_batch_transaction_publishes_none_of_its_revisions() {
    let scenario = fail::FailScenario::setup();
    let mut scene = Scene::new().await;
    let batch = scene.batch(&[1, 2, 3]).await;
    scene.review(batch, &[1, 2, 3], &[]).await;
    fail::cfg("batch_integration_in_transaction", "return").unwrap();
    let stopped = scene
        .fx
        .reviews()
        .integrate_review_batch(&human("tanim"), batch)
        .await;
    fail::remove("batch_integration_in_transaction");
    assert_eq!(refusal(stopped).code(), "failpoint");
    scene.fx.restart().await;

    assert_eq!(scene.count("SELECT COUNT(*) FROM revisions").await, 1);
    assert_eq!(scene.count("SELECT COUNT(*) FROM integrations").await, 0);
    assert_eq!(scene.count("SELECT COUNT(*) FROM completions").await, 0);
    assert_eq!(
        scene
            .count("SELECT COUNT(*) FROM review_batch_integrations")
            .await,
        0
    );
    for proposal in 1..=3 {
        assert_eq!(scene.state(proposal).await, ProposalState::Open);
    }
    assert!(scene.events("proposal.integrated").await.is_empty());
    assert_consistent(&scene.fx.project).await;

    let outcome = scene
        .fx
        .reviews()
        .integrate_review_batch(&human("tanim"), batch)
        .await
        .unwrap();
    let BatchIntegration::Together { members } = outcome else {
        panic!("expected the members together, got {outcome:?}");
    };
    assert_eq!(succeeded(&members).len(), 3);
    assert_eq!(scene.count("SELECT COUNT(*) FROM revisions").await, 4);
    assert_consistent(&scene.fx.project).await;
    scenario.teardown();
}

#[cfg(feature = "failpoints")]
#[tokio::test]
async fn a_stop_after_the_revision_objects_publishes_none_of_them_and_the_batch_integrates_again() {
    let scenario = fail::FailScenario::setup();
    let mut scene = Scene::new().await;
    let batch = scene.batch(&[1, 2, 3]).await;
    scene.review(batch, &[1, 2, 3], &[]).await;
    fail::cfg("batch_integration_after_objects", "return").unwrap();
    let stopped = scene
        .fx
        .reviews()
        .integrate_review_batch(&human("tanim"), batch)
        .await;
    fail::remove("batch_integration_after_objects");
    assert_eq!(refusal(stopped).code(), "failpoint");
    scene.fx.restart().await;

    // The three revision objects are written, but no row names them, so main and every member stay as they were.
    assert_eq!(scene.count("SELECT COUNT(*) FROM revisions").await, 1);
    assert_eq!(scene.count("SELECT COUNT(*) FROM integrations").await, 0);
    assert_eq!(scene.count("SELECT COUNT(*) FROM completions").await, 0);
    assert_eq!(
        scene
            .count("SELECT COUNT(*) FROM review_batch_integrations")
            .await,
        0
    );
    for proposal in 1..=3 {
        assert_eq!(scene.state(proposal).await, ProposalState::Open);
    }
    assert_consistent(&scene.fx.project).await;

    let outcome = scene
        .fx
        .reviews()
        .integrate_review_batch(&human("tanim"), batch)
        .await
        .unwrap();
    let BatchIntegration::Together { members } = outcome else {
        panic!("expected the members together, got {outcome:?}");
    };
    assert_eq!(
        succeeded(&members),
        [(p(1), r(2)), (p(2), r(3)), (p(3), r(4))]
    );
    assert_consistent(&scene.fx.project).await;
    scenario.teardown();
}

#[tokio::test]
async fn a_batch_with_no_member_ready_ends_its_turn_and_frees_the_rest() {
    let scene = Scene::new().await;
    scene
        .fx
        .handle
        .tasks
        .add_dependency(&human("tanim"), t(2), t(1))
        .await
        .unwrap();
    let batch = scene.batch(&[1, 2]).await;
    scene.review(batch, &[1, 2], &[1]).await;
    assert!(
        scene
            .fx
            .reviews()
            .integrate_ready()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        scene.fx.reviews().batch_integration_view(batch).await.unwrap(),
        BatchIntegrationView::Ended { revisions, .. } if revisions.is_empty()
    ));
    assert_eq!(
        scene.events("review.batch_integrated").await[0].data["outcome"],
        "none"
    );
}

#[tokio::test]
async fn a_member_with_only_a_comment_holds_the_batch_and_the_review_lead_hears_of_it_once() {
    let scene = Scene::new().await;
    give_role(&scene.fx.handle.store, "lead", Role::ReviewLead, None).await;
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = std::sync::Arc::clone(&heard);
    scene.fx.handle.push.register_stdin(
        &AgentName::parse("lead").unwrap(),
        std::sync::Arc::new(move |text: &str| {
            sink.lock().unwrap().push(text.to_owned());
            true
        }),
    );
    let batch = scene.batch(&[1, 2]).await;
    scene
        .fx
        .reviews()
        .submit_batch_review(
            &scene.review,
            batch,
            vec![
                BatchVerdict {
                    proposal: p(1),
                    verdict: Verdict::Approve,
                    body: String::new(),
                },
                BatchVerdict {
                    proposal: p(2),
                    verdict: Verdict::Comment,
                    body: "What happens on an empty list?".to_owned(),
                },
            ],
            Vec::new(),
        )
        .await
        .unwrap();

    assert!(
        scene
            .fx
            .reviews()
            .integrate_ready()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        scene.fx.reviews().batch_integration_view(batch).await.unwrap(),
        BatchIntegrationView::Waiting { pending } if pending == [p(2)]
    ));
    assert_eq!(scene.fx.reviews().note_batch_waits().await.unwrap(), 1);
    assert_eq!(scene.fx.reviews().note_batch_waits().await.unwrap(), 0);
    let waiting = scene.events("review.batch_waiting").await;
    assert_eq!(waiting.len(), 1, "one notice per member and version");
    assert_eq!(waiting[0].proposal, Some(p(2)));
    assert_eq!(waiting[0].data["batch"], "rb-1");
    assert_eq!(waiting[0].data["version"], 1);
    let notice = "fold: rb-1 waits for a verdict on P-2 v1: its reviewer must approve or request changes. Call get_review_batch.";
    eventually(WAIT, async || {
        let told: Vec<String> = heard
            .lock()
            .unwrap()
            .iter()
            .filter(|text| text.contains("waits for a verdict"))
            .cloned()
            .collect();
        if told.len() == 1 && told[0].contains(notice) {
            Ok(())
        } else {
            Err(told)
        }
    })
    .await;

    // A verdict that decides ends the wait: the approved member integrates and nothing more is told.
    scene
        .fx
        .reviews()
        .submit_review(&scene.review, p(2), 1, Verdict::Approve, "fine", Vec::new())
        .await
        .unwrap();
    assert_eq!(scene.fx.reviews().note_batch_waits().await.unwrap(), 0);
    let done = scene.fx.reviews().integrate_ready().await.unwrap();
    assert_eq!(done.len(), 2);
    assert_eq!(scene.events("review.batch_waiting").await.len(), 1);
}

/// The review lead hears every notice that contains `text`, collected through its session's input.
fn listen(scene: &Scene, text: &'static str) -> std::sync::Arc<std::sync::Mutex<Vec<String>>> {
    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = std::sync::Arc::clone(&heard);
    scene.fx.handle.push.register_stdin(
        &AgentName::parse("lead").unwrap(),
        std::sync::Arc::new(move |line: &str| {
            if line.contains(text) {
                sink.lock().unwrap().push(line.to_owned());
            }
            true
        }),
    );
    heard
}

#[tokio::test]
async fn an_hour_after_the_review_the_members_without_a_verdict_leave_and_the_approved_ones_integrate()
 {
    let scene = Scene::new().await;
    give_role(&scene.fx.handle.store, "lead", Role::ReviewLead, None).await;
    let heard = listen(&scene, "the wait of");
    let batch = scene.batch(&[1, 2, 3]).await;
    // P-3's author sends v2 before the review, so the batch review leaves it out and nobody reviews it yet.
    scene.fx.write(3, "src/m3.rs", "member 3, second try\n");
    scene
        .fx
        .reviews()
        .update_proposal(
            &scene.authors[2],
            p(3),
            "Make the change the task asks for.",
            Some("tests pass"),
        )
        .await
        .unwrap();
    scene.review(batch, &[1, 2], &[]).await;
    assert!(
        scene
            .fx
            .reviews()
            .integrate_ready()
            .await
            .unwrap()
            .is_empty()
    );

    scene
        .fx
        .project
        .clock()
        .advance(Duration::from_secs(59 * 60))
        .unwrap();
    assert_eq!(scene.fx.reviews().end_due_batch_waits().await.unwrap(), 0);
    assert!(scene.events("review.batch_wait_ended").await.is_empty());
    scene
        .fx
        .project
        .clock()
        .advance(Duration::from_secs(60))
        .unwrap();
    assert_eq!(scene.fx.reviews().end_due_batch_waits().await.unwrap(), 1);
    assert_eq!(scene.fx.reviews().end_due_batch_waits().await.unwrap(), 0);
    let ended = scene.events("review.batch_wait_ended").await;
    assert_eq!(ended.len(), 1, "the wait ends once");
    assert_eq!(ended[0].data["batch"], "rb-1");
    assert_eq!(ended[0].data["left"], serde_json::json!(["P-3"]));
    let notice = "fold: the wait of rb-1 ended: its approved members integrate together, and P-3 left the batch to be reviewed alone. Call get_review_batch.";
    eventually(WAIT, async || {
        let told = heard.lock().unwrap().clone();
        if told.len() == 1 && told[0].contains(notice) {
            Ok(())
        } else {
            Err(told)
        }
    })
    .await;
    assert!(matches!(
        scene.fx.reviews().batch_integration_view(batch).await.unwrap(),
        BatchIntegrationView::Ready { members, .. } if members == [p(1), p(2)]
    ));

    let done = scene.fx.reviews().integrate_ready().await.unwrap();
    assert_eq!(
        succeeded(&done_members(&done)),
        [(p(1), r(2)), (p(2), r(3))]
    );
    assert_eq!(
        scene.integrations().await,
        [(1, 2, Some(1)), (2, 3, Some(1))]
    );
    assert_eq!(scene.state(3).await, ProposalState::Open);
    // P-3 left the batch's turn, so its approval later integrates it on its own.
    scene
        .fx
        .reviews()
        .submit_review(&scene.review, p(3), 2, Verdict::Approve, "ok", Vec::new())
        .await
        .unwrap();
    let alone = scene.fx.reviews().integrate_ready().await.unwrap();
    assert_eq!(alone.len(), 1);
    assert_eq!(
        scene.integrations().await,
        [(1, 2, Some(1)), (2, 3, Some(1)), (3, 4, None)]
    );
    assert_consistent(&scene.fx.project).await;
}

fn done_members(
    done: &[fold_daemon::services::reviews::AutomaticIntegration],
) -> Vec<MemberIntegration> {
    done.iter()
        .map(|done| MemberIntegration {
            proposal: done.proposal,
            outcome: done.outcome.clone(),
        })
        .collect()
}

#[tokio::test]
async fn without_automatic_integration_a_batch_that_becomes_ready_later_is_told_once() {
    let scene = Scene::new().await;
    give_role(&scene.fx.handle.store, "lead", Role::ReviewLead, None).await;
    let heard = listen(&scene, "is ready");
    let waiting = scene.batch(&[1, 2]).await;
    scene
        .fx
        .reviews()
        .submit_batch_review(
            &scene.review,
            waiting,
            vec![
                BatchVerdict {
                    proposal: p(1),
                    verdict: Verdict::Approve,
                    body: String::new(),
                },
                BatchVerdict {
                    proposal: p(2),
                    verdict: Verdict::Comment,
                    body: "What happens on an empty list?".to_owned(),
                },
            ],
            Vec::new(),
        )
        .await
        .unwrap();
    // A batch ready at its submission is the holder's to integrate, so it is not told.
    let at_once = scene.batch(&[3, 4]).await;
    scene.review(at_once, &[3, 4], &[]).await;
    assert_eq!(
        scene.fx.reviews().settle_batches_by_hand().await.unwrap(),
        0
    );

    scene
        .fx
        .reviews()
        .submit_review(&scene.review, p(2), 1, Verdict::Approve, "fine", Vec::new())
        .await
        .unwrap();
    assert_eq!(
        scene.fx.reviews().settle_batches_by_hand().await.unwrap(),
        1
    );
    assert_eq!(
        scene.fx.reviews().settle_batches_by_hand().await.unwrap(),
        0
    );
    let ready = scene.events("review.batch_ready").await;
    assert_eq!(ready.len(), 1, "told once");
    assert_eq!(ready[0].data["batch"], "rb-1");
    assert_eq!(ready[0].data["members"], serde_json::json!(["P-1", "P-2"]));
    assert_eq!(ready[0].data["holder"], "review");
    let notice = "fold: rb-1 is ready: P-1, P-2 integrate together. Call integrate_review_batch.";
    eventually(WAIT, async || {
        let told = heard.lock().unwrap().clone();
        if told.len() == 1 && told[0].contains(notice) {
            Ok(())
        } else {
            Err(told)
        }
    })
    .await;
    assert!(
        scene.integrations().await.is_empty(),
        "nothing integrates by itself"
    );
}

#[tokio::test]
async fn without_automatic_integration_a_batch_with_no_member_left_ends_its_turn() {
    let scene = Scene::new().await;
    scene
        .fx
        .handle
        .tasks
        .add_dependency(&human("tanim"), t(2), t(1))
        .await
        .unwrap();
    let batch = scene.batch(&[1, 2]).await;
    scene.review(batch, &[1, 2], &[1]).await;
    assert_eq!(
        scene.fx.reviews().settle_batches_by_hand().await.unwrap(),
        1
    );
    assert!(matches!(
        scene.fx.reviews().batch_integration_view(batch).await.unwrap(),
        BatchIntegrationView::Ended { outcome: fold_core::reviews::batches::BatchOutcome::Nothing, revisions, .. } if revisions.is_empty()
    ));
    let ended = scene.events("review.batch_integrated").await;
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].data["outcome"], "none");
    assert!(scene.events("review.batch_ready").await.is_empty());
    assert_eq!(
        scene.fx.reviews().settle_batches_by_hand().await.unwrap(),
        0
    );
}
