#![cfg(test)]

use std::path::Path;
use std::time::Duration;

use fold_core::policy::SessionPeer;
use fold_core::reviews::Verdict;
use fold_core::reviews::checks::{RunState, Stage};
use fold_core::roles::Role;
use fold_core::{TaskDescription, TaskStatus, TaskTitle};
use fold_daemon::services::agents::peers;
use fold_daemon::services::reviews::{IntegrationOutcome, Turn};
use fold_daemon::services::tasks::{CreateTask, TaskPatch};
use fold_store::reviews::ProposalState;
use fold_store::testing::eventually;

use crate::common::roles::give_role;
use crate::reviews_support::{Fixture, by, human, p, t};

const WAIT: Duration = Duration::from_secs(30);

/// `frontend` opens a proposal for `task` in a fresh workspace after `edit`, and the review lead `review` approves its v1. Returns the proposal's number.
#[cfg(test)]
async fn approved(fx: &Fixture, task: u32, edit: impl FnOnce(&Path)) -> u32 {
    let author = by(&fx.frontend.worker);
    fx.workspaces
        .create_workspace(&author, t(task))
        .await
        .unwrap();
    edit(&fx.project.paths().root().join(format!(".fold/ws/t-{task}")));
    let detail = fx
        .reviews
        .create_proposal(&author, t(task), "Change", None)
        .await
        .unwrap()
        .detail()
        .expect("the file has no proposal check");
    let id = detail.summary.id;
    fx.reviews
        .submit_review(
            &by(&fx.review.worker),
            id,
            1,
            Verdict::Approve,
            "ok",
            Vec::new(),
        )
        .await
        .unwrap();
    id.number()
}

#[cfg(test)]
fn checks_file(dir: &Path, run: &str) {
    std::fs::write(
        dir.join("fold-checks.toml"),
        format!("[[check]]\nname = \"e2e\"\nrun = \"{run}\"\nstage = \"integration\"\n"),
    )
    .unwrap();
}

#[cfg(test)]
async fn state_of(fx: &Fixture, proposal: u32) -> ProposalState {
    fx.reviews
        .get_proposal(p(proposal), None)
        .await
        .unwrap()
        .summary
        .state
}

/// Moves `T-2` into the area that `frontend` leads, so that [`approved`] can let `frontend` work on it: a task has one area.
#[cfg(test)]
async fn second_task_to_frontend(fx: &Fixture) {
    fx.tasks
        .update_task(
            &human(),
            t(2),
            TaskPatch {
                labels: Some(crate::common::roles::work_labels_of("frontend")),
                ..TaskPatch::default()
            },
        )
        .await
        .unwrap();
}

#[cfg(test)]
async fn integrated(fx: &Fixture, proposal: u32) {
    eventually(WAIT, async || match state_of(fx, proposal).await {
        ProposalState::Integrated => Ok(()),
        other => Err(other),
    })
    .await;
}

#[cfg(test)]
async fn revision_of(fx: &Fixture, proposal: u32) -> i64 {
    fx.count(&format!(
        "SELECT r.number FROM integrations i JOIN revisions r ON r.id = i.revision_id
         JOIN proposals p ON p.id = i.proposal_id
         WHERE p.number = {proposal} AND i.result = 'succeeded'"
    ))
    .await
}

#[tokio::test]
async fn green_integration_checks_let_the_proposal_reach_main() {
    let fx = Fixture::new().await;
    give_role(fx.project.store(), "review", Role::ReviewLead, None).await;
    let proposal = approved(&fx, 1, |dir| {
        checks_file(dir, "test -f src/lib.rs && echo merged");
    })
    .await;
    let outcome = fx
        .reviews
        .integrate_proposal(&human(), p(proposal), Some(1))
        .await
        .unwrap();
    assert_eq!(outcome, IntegrationOutcome::Queued);
    integrated(&fx, proposal).await;
    assert_eq!(revision_of(&fx, proposal).await, 2);
    assert_eq!(fx.status(1).await, TaskStatus::Done);
    let view = fx
        .reviews
        .view_proposal(&human(), p(proposal), None)
        .await
        .unwrap();
    let run = view
        .checks
        .iter()
        .find(|view| view.run.stage == Stage::Integration)
        .expect("the integration run is shown on the version");
    assert_eq!(run.run.state, RunState::Passed);
    assert_eq!(run.run.version, Some(1));
    assert_eq!(run.results[0].check.as_deref(), Some("e2e"));
    let checked = fx.project.paths().checks_dir();
    assert!(
        checked.join("src/lib.rs").is_file(),
        "the check directory holds the merge"
    );
    assert!(checked.join("fold-checks.toml").is_file());
}

#[tokio::test]
async fn red_integration_checks_send_the_proposal_back_to_its_author() {
    let fx = Fixture::new().await;
    give_role(fx.project.store(), "review", Role::ReviewLead, None).await;
    let proposal = approved(&fx, 1, |dir| {
        checks_file(dir, "echo the suite broke; exit 3");
    })
    .await;
    assert_eq!(
        fx.reviews
            .integrate_proposal(&human(), p(proposal), Some(1))
            .await
            .unwrap(),
        IntegrationOutcome::Queued
    );
    let summary = eventually(WAIT, async || {
        let summary = fx
            .reviews
            .get_proposal(p(proposal), None)
            .await
            .unwrap()
            .summary;
        if summary.checks_failed {
            Ok(summary)
        } else {
            Err(summary.state)
        }
    })
    .await;
    assert_eq!(summary.state, ProposalState::Open);
    assert_eq!(summary.turn, Some(Turn::WithAuthor));
    assert_eq!(fx.status(1).await, TaskStatus::InProgress);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM revisions").await,
        1,
        "main did not move"
    );
    let body = fx
        .text(
            "SELECT m.body FROM messages m
             JOIN message_recipients r ON r.message_id = m.id
             JOIN workers w ON w.id = r.worker_id
             WHERE m.sender_kind = 'fold' AND w.name = 'frontend'
             ORDER BY m.number DESC LIMIT 1",
        )
        .await
        .unwrap();
    assert!(body.contains("e2e failed (exit 3)"), "{body}");
    assert!(body.contains("fold did not integrate it"), "{body}");
    assert!(body.contains("the suite broke"), "{body}");

    fx.write("README.md", "# next\n");
    let next = fx
        .reviews
        .update_proposal(&by(&fx.frontend.worker), p(proposal), None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    assert!(!next.summary.checks_failed, "a new version clears the flag");
}

#[tokio::test]
async fn a_version_whose_integration_checks_failed_is_refused_approval_and_integration_until_a_new_version()
 {
    let fx = Fixture::new().await;
    give_role(fx.project.store(), "review", Role::ReviewLead, None).await;
    let proposal = approved(&fx, 1, |dir| checks_file(dir, "exit 3")).await;
    fx.reviews
        .integrate_proposal(&human(), p(proposal), Some(1))
        .await
        .unwrap();
    eventually(WAIT, async || {
        let summary = fx
            .reviews
            .get_proposal(p(proposal), None)
            .await
            .unwrap()
            .summary;
        if summary.checks_failed {
            Ok(())
        } else {
            Err(summary.state)
        }
    })
    .await;
    for actor in [human(), by(&fx.review.worker)] {
        let refused = crate::reviews_support::refusal(
            fx.reviews
                .integrate_proposal(&actor, p(proposal), Some(1))
                .await,
        );
        assert_eq!(refused.code(), fold_core::codes::INVALID_REQUEST);
        assert_eq!(refused.details().unwrap()["reason"], "checks_failed");
        assert!(refused.hint().unwrap_or_default().contains("new version"));
        let approve = crate::reviews_support::refusal(
            fx.reviews
                .submit_review(&actor, p(proposal), 1, Verdict::Approve, "ok", Vec::new())
                .await,
        );
        assert_eq!(approve.code(), fold_core::codes::INVALID_REQUEST);
        assert_eq!(approve.details().unwrap()["reason"], "checks_failed");
    }
    fx.reviews
        .submit_review(
            &by(&fx.review.worker),
            p(proposal),
            1,
            Verdict::RequestChanges,
            "the e2e check fails",
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM check_runs WHERE stage = 'integration'")
            .await,
        1,
        "a refused integration runs no checks"
    );
}

#[tokio::test]
async fn queued_integrations_run_one_at_a_time_in_the_order_asked() {
    let fx = Fixture::new().await;
    give_role(fx.project.store(), "review", Role::ReviewLead, None).await;
    let outside = tempfile::tempdir().unwrap();
    let (hold, go) = (outside.path().join("hold"), outside.path().join("go"));
    let wait = format!(
        "if [ -e '{}' ]; then until [ -e '{}' ]; do sleep 0.05; done; fi",
        hold.display(),
        go.display()
    );
    let first = approved(&fx, 1, |dir| checks_file(dir, &wait)).await;
    fx.reviews
        .integrate_proposal(&human(), p(first), Some(1))
        .await
        .unwrap();
    integrated(&fx, first).await;

    fx.tasks
        .create_task(
            &by(&fx.frontend.worker),
            CreateTask {
                title: TaskTitle::parse("third").unwrap(),
                description: TaskDescription::new("body"),
                labels: crate::common::roles::work_labels(),
            },
        )
        .await
        .unwrap();
    second_task_to_frontend(&fx).await;
    let second = approved(&fx, 2, |dir| {
        std::fs::write(dir.join("two.txt"), "2\n").unwrap();
    })
    .await;
    let third = approved(&fx, 3, |dir| {
        std::fs::write(dir.join("three.txt"), "3\n").unwrap();
    })
    .await;
    std::fs::write(&hold, "").unwrap();
    let review = by(&fx.review.worker);
    for proposal in [second, third] {
        assert_eq!(
            fx.reviews
                .integrate_proposal(&review, p(proposal), Some(1))
                .await
                .unwrap(),
            IntegrationOutcome::Queued
        );
    }
    eventually(WAIT, async || {
        let running = fx
            .count("SELECT COUNT(*) FROM check_runs WHERE state = 'running'")
            .await;
        if running == 1 { Ok(()) } else { Err(running) }
    })
    .await;
    assert_eq!(state_of(&fx, second).await, ProposalState::Open);
    assert_eq!(state_of(&fx, third).await, ProposalState::Open);
    std::fs::write(&go, "").unwrap();
    integrated(&fx, second).await;
    integrated(&fx, third).await;
    assert_eq!(revision_of(&fx, second).await, 3);
    assert_eq!(revision_of(&fx, third).await, 4);
    assert!(
        fx.project.paths().checks_dir().join("three.txt").is_file(),
        "the check directory follows main"
    );
}

#[tokio::test]
async fn a_queued_integration_that_conflicts_is_told_to_the_integrating_agent() {
    let fx = Fixture::new().await;
    give_role(fx.project.store(), "review", Role::ReviewLead, None).await;
    let first = approved(&fx, 1, |dir| checks_file(dir, "true")).await;
    fx.reviews
        .integrate_proposal(&human(), p(first), Some(1))
        .await
        .unwrap();
    integrated(&fx, first).await;

    fx.tasks
        .create_task(
            &by(&fx.frontend.worker),
            CreateTask {
                title: TaskTitle::parse("third").unwrap(),
                description: TaskDescription::new("body"),
                labels: crate::common::roles::work_labels(),
            },
        )
        .await
        .unwrap();
    second_task_to_frontend(&fx).await;
    let second = approved(&fx, 2, |dir| {
        std::fs::write(dir.join("same.txt"), "two\n").unwrap();
    })
    .await;
    let third = approved(&fx, 3, |dir| {
        std::fs::write(dir.join("same.txt"), "three\n").unwrap();
    })
    .await;
    let review = by(&fx.review.worker);
    for proposal in [second, third] {
        assert_eq!(
            fx.reviews
                .integrate_proposal(&review, p(proposal), Some(1))
                .await
                .unwrap(),
            IntegrationOutcome::Queued
        );
    }
    integrated(&fx, second).await;
    let to_review = format!(
        "FROM messages m
         JOIN message_recipients r ON r.message_id = m.id
         JOIN workers w ON w.id = r.worker_id
         WHERE m.sender_kind = 'fold' AND w.name = 'review' AND m.body LIKE 'fold did not integrate P-{third}:%'"
    );
    eventually(WAIT, async || {
        let told = fx.count(&format!("SELECT COUNT(*) {to_review}")).await;
        if told == 1 { Ok(()) } else { Err(told) }
    })
    .await;
    let told = fx
        .text(&format!("SELECT m.body {to_review}"))
        .await
        .unwrap();
    assert!(told.contains("conflicts with main"), "{told}");
    assert_eq!(state_of(&fx, third).await, ProposalState::Open);
}

#[tokio::test]
async fn a_queued_integration_shows_on_the_proposal_and_a_second_request_is_refused() {
    let fx = Fixture::new().await;
    give_role(fx.project.store(), "review", Role::ReviewLead, None).await;
    let outside = tempfile::tempdir().unwrap();
    let go = outside.path().join("go");
    let wait = format!("until [ -e '{}' ]; do sleep 0.05; done", go.display());
    let proposal = approved(&fx, 1, |dir| checks_file(dir, &wait)).await;
    assert_eq!(
        fx.reviews
            .integrate_proposal(&human(), p(proposal), Some(1))
            .await
            .unwrap(),
        IntegrationOutcome::Queued
    );
    let summary = fx
        .reviews
        .get_proposal(p(proposal), None)
        .await
        .unwrap()
        .summary;
    assert!(summary.integration_queued);
    for expected in [Some(1), None] {
        let refused = crate::reviews_support::refusal(
            fx.reviews
                .integrate_proposal(&human(), p(proposal), expected)
                .await,
        );
        assert_eq!(refused.code(), fold_core::codes::INVALID_REQUEST);
        assert_eq!(refused.details().unwrap()["reason"], "integration_queued");
    }
    std::fs::write(&go, "").unwrap();
    integrated(&fx, proposal).await;
    eventually(WAIT, async || {
        let ended = fx
            .count("SELECT COUNT(*) FROM events WHERE type = 'integration.unqueued'")
            .await;
        if ended == 1 { Ok(()) } else { Err(ended) }
    })
    .await;
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM events WHERE type = 'integration.queued'")
            .await,
        1
    );
    assert!(
        !fx.reviews
            .get_proposal(p(proposal), None)
            .await
            .unwrap()
            .summary
            .integration_queued
    );
}

#[tokio::test]
async fn a_fault_of_folds_own_stops_the_run_sends_nothing_back_and_is_not_retried_on_its_own() {
    let fx = Fixture::new().await;
    give_role(fx.project.store(), "review", Role::ReviewLead, None).await;
    let first = approved(&fx, 1, |dir| checks_file(dir, "true")).await;
    fx.reviews
        .integrate_proposal(&human(), p(first), Some(1))
        .await
        .unwrap();
    integrated(&fx, first).await;
    second_task_to_frontend(&fx).await;
    let second = approved(&fx, 2, |dir| {
        std::fs::write(dir.join("two.txt"), "2\n").unwrap();
    })
    .await;
    // A file where fold's check directory belongs keeps fold from preparing it.
    let checks = fx.project.paths().checks_dir();
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
    let review = by(&fx.review.worker);
    assert_eq!(
        fx.reviews
            .integrate_proposal(&review, p(second), Some(1))
            .await
            .unwrap(),
        IntegrationOutcome::Queued
    );
    let to_review = format!(
        "FROM messages m
         JOIN message_recipients r ON r.message_id = m.id
         JOIN workers w ON w.id = r.worker_id
         WHERE m.sender_kind = 'fold' AND w.name = 'review' AND m.body LIKE 'fold did not integrate P-{second}:%'"
    );
    eventually(WAIT, async || {
        let told = fx.count(&format!("SELECT COUNT(*) {to_review}")).await;
        if told == 1 { Ok(()) } else { Err(told) }
    })
    .await;
    let told = fx
        .text(&format!("SELECT m.body {to_review}"))
        .await
        .unwrap();
    assert!(told.contains("integrate again"), "{told}");
    eventually(WAIT, async || {
        let summary = fx
            .reviews
            .get_proposal(p(second), None)
            .await
            .unwrap()
            .summary;
        if summary.integration_queued {
            Err(())
        } else {
            Ok(())
        }
    })
    .await;
    assert_eq!(
        fx.text("SELECT state || ' ' || reason FROM check_runs WHERE stage = 'integration' ORDER BY id DESC LIMIT 1")
            .await
            .as_deref(),
        Some("stopped fold_failed")
    );
    let summary = fx
        .reviews
        .get_proposal(p(second), None)
        .await
        .unwrap()
        .summary;
    assert!(!summary.checks_failed, "fold's own fault flags nothing");
    assert_eq!(summary.turn, Some(Turn::WithReviewer));
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM messages m
             JOIN message_recipients r ON r.message_id = m.id
             JOIN workers w ON w.id = r.worker_id
             WHERE m.sender_kind = 'fold' AND w.name = 'frontend' AND m.body LIKE '%failed on its merge%'"
        )
        .await,
        0,
        "the author is not sent back"
    );

    let runs = "SELECT COUNT(*) FROM check_runs WHERE stage = 'integration'";
    let before = fx.count(runs).await;
    fx.reviews.integrate_ready().await.unwrap();
    fx.reviews.integrate_ready().await.unwrap();
    assert_eq!(
        fx.count(runs).await,
        before,
        "fold does not retry on its own"
    );

    // A human who asks is told as well.
    assert_eq!(
        fx.reviews
            .integrate_proposal(&human(), p(second), Some(1))
            .await
            .unwrap(),
        IntegrationOutcome::Queued
    );
    let to_human = format!(
        "SELECT COUNT(*) FROM messages m JOIN message_recipients r ON r.message_id = m.id
         WHERE m.sender_kind = 'fold' AND r.human = 'tanim' AND m.body LIKE 'fold did not integrate P-{second}:%'"
    );
    eventually(WAIT, async || {
        let told = fx.count(&to_human).await;
        if told == 1 { Ok(()) } else { Err(told) }
    })
    .await;
    eventually(WAIT, async || {
        let summary = fx
            .reviews
            .get_proposal(p(second), None)
            .await
            .unwrap()
            .summary;
        if summary.integration_queued {
            Err(())
        } else {
            Ok(())
        }
    })
    .await;

    std::fs::remove_file(&parent).unwrap();
    assert_eq!(
        fx.reviews
            .integrate_proposal(&review, p(second), Some(1))
            .await
            .unwrap(),
        IntegrationOutcome::Queued
    );
    integrated(&fx, second).await;
}

#[tokio::test]
async fn a_project_without_integration_checks_integrates_at_once() {
    let fx = Fixture::new().await;
    give_role(fx.project.store(), "review", Role::ReviewLead, None).await;
    let proposal = approved(&fx, 1, |dir| {
        std::fs::write(dir.join("src/lib.rs"), "pub fn y() {}\n").unwrap();
    })
    .await;
    let outcome = fx
        .reviews
        .integrate_proposal(&by(&fx.review.worker), p(proposal), Some(1))
        .await
        .unwrap();
    assert!(matches!(outcome, IntegrationOutcome::Succeeded { .. }));
    assert!(!fx.project.paths().checks_dir().exists());
}

#[tokio::test]
async fn an_invalid_checks_file_of_the_merge_sends_the_proposal_back() {
    let fx = Fixture::new().await;
    give_role(fx.project.store(), "review", Role::ReviewLead, None).await;
    let proposal = approved(&fx, 1, |dir| checks_file(dir, "true")).await;
    // A human edit makes a version without running the proposal checks, so only the integration finds the file invalid.
    fx.reviews
        .edit_as_human(
            &human(),
            p(proposal),
            1,
            "fold-checks.toml",
            "[[check]]\nname = \"e2e\"\nrun = \"true\"\nstage = \"integration\"\nretries = 2\n",
        )
        .await
        .unwrap();
    assert_eq!(
        fx.reviews
            .integrate_proposal(&human(), p(proposal), Some(2))
            .await
            .unwrap(),
        IntegrationOutcome::Queued
    );
    let summary = eventually(WAIT, async || {
        let summary = fx
            .reviews
            .get_proposal(p(proposal), None)
            .await
            .unwrap()
            .summary;
        if summary.checks_failed {
            Ok(summary)
        } else {
            Err(summary.state)
        }
    })
    .await;
    assert_eq!(summary.state, ProposalState::Open);
    assert_eq!(
        fx.text("SELECT reason FROM check_runs WHERE stage = 'integration'")
            .await
            .as_deref(),
        Some("checks_file_invalid")
    );
    let body = fx
        .text(
            "SELECT m.body FROM messages m
             JOIN message_recipients r ON r.message_id = m.id
             JOIN workers w ON w.id = r.worker_id
             WHERE m.sender_kind = 'fold' AND w.name = 'frontend'
             ORDER BY m.number DESC LIMIT 1",
        )
        .await
        .unwrap();
    assert!(body.contains("fold-checks.toml is not valid"), "{body}");
    assert!(body.contains("line 5"), "{body}");
}

/// The worker as the session `fld claude` starts acts: with the `main` tag.
fn main_actor(worker: &fold_store::agents::WorkerRow) -> fold_core::Actor {
    fold_core::Actor::Agent(fold_core::AgentActor {
        agent: worker.name.agent(),
        worker: worker.name.clone(),
        tags: vec![fold_core::Tag::parse(fold_core::MAIN_TAG).unwrap()],
    })
}

/// The main agent `actor` writes `fold-checks.toml` with `text` straight onto main, as a direct change, which runs no checks.
async fn checks_file_on_main(fx: &Fixture, actor: &fold_core::Actor, text: &str) {
    let opened = fx.workspaces.open_main_workspace(actor).await.unwrap();
    let dir = fx.project.paths().root().join(&opened.workspace.path);
    std::fs::write(dir.join("fold-checks.toml"), text).unwrap();
    let changes = fx
        .workspaces
        .record_changes(actor, opened.workspace.id, "the checks")
        .await
        .unwrap();
    let outcome = fx
        .reviews
        .integrate_direct(
            actor,
            fold_daemon::services::reviews::DirectChange {
                changeset: changes.changeset,
                message: "Change the checks".to_owned(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        fold_daemon::services::reviews::DirectOutcome::Succeeded { .. }
    ));
}

async fn held_items(fx: &Fixture) -> Vec<fold_daemon::services::agents::NeedsYouItem> {
    fold_daemon::services::agents::needs_you(fx.project.store())
        .await
        .unwrap()
        .into_iter()
        .filter(|item| {
            item.kind == fold_daemon::services::agents::NeedsYouKind::ChecksFileInvalidOnMain
        })
        .collect()
}

#[tokio::test]
async fn an_invalid_checks_file_on_main_holds_the_integrations_until_main_fixes_it() {
    // `backend` is the main agent, which writes onto main directly.
    let fx = Fixture::with_reviewers().await;
    give_role(fx.project.store(), "review", Role::ReviewLead, None).await;
    let proposal = approved(&fx, 1, |dir| {
        std::fs::write(dir.join("one.txt"), "1\n").unwrap();
    })
    .await;
    // The file breaks on main after the proposal was made: a proposal made from a broken file is refused at its own checks.
    let main = main_actor(&fx.backend.worker);
    checks_file_on_main(
        &fx,
        &main,
        "[[check]]\nname = \"e2e\"\nrun = \"true\"\nstage = \"integration\"\nretries = 2\n",
    )
    .await;
    assert_eq!(
        fx.reviews
            .integrate_proposal(&human(), p(proposal), Some(1))
            .await
            .unwrap(),
        IntegrationOutcome::Queued
    );
    let held = "SELECT COUNT(*) FROM events WHERE type = 'integration.held'";
    eventually(WAIT, async || {
        let count = fx.count(held).await;
        if count == 1 { Ok(()) } else { Err(count) }
    })
    .await;
    let summary = fx
        .reviews
        .get_proposal(p(proposal), None)
        .await
        .unwrap()
        .summary;
    assert_eq!(summary.state, ProposalState::Open);
    assert!(
        !summary.checks_failed,
        "the fault is main's, not the proposal's"
    );
    assert!(summary.integration_queued, "the integration stays queued");
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM check_runs WHERE stage = 'integration'")
            .await,
        0
    );
    let items = held_items(&fx).await;
    assert_eq!(items.len(), 1);
    let title = items[0].title.as_deref().unwrap_or_default();
    assert!(title.starts_with("fold-checks.toml line 5: "), "{title}");
    assert_eq!(items[0].paths, ["fold-checks.toml"]);

    // The automatic integration holds it too, and the main agent hears of main's file once.
    fx.reviews.integrate_ready().await.unwrap();
    assert_eq!(fx.count(held).await, 1);

    checks_file_on_main(
        &fx,
        &main,
        "[[check]]\nname = \"e2e\"\nrun = \"true\"\nstage = \"integration\"\n",
    )
    .await;
    integrated(&fx, proposal).await;
    assert!(held_items(&fx).await.is_empty());
}

#[tokio::test]
async fn a_queued_integration_that_a_stop_cut_short_runs_again_at_the_next_start() {
    let mut fx = Fixture::new().await;
    give_role(fx.project.store(), "review", Role::ReviewLead, None).await;
    let outside = tempfile::tempdir().unwrap();
    let go = outside.path().join("go");
    let wait = format!("until [ -e '{}' ]; do sleep 0.05; done", go.display());
    let proposal = approved(&fx, 1, |dir| checks_file(dir, &wait)).await;
    let runner = fold_daemon::services::reviews::checks::CheckRunner::new(
        fold_daemon::services::reviews::checks::CheckPermits::new(1),
    );
    let reviews = fx.service().with_checks(runner.clone());
    assert_eq!(
        reviews
            .integrate_proposal(&human(), p(proposal), Some(1))
            .await
            .unwrap(),
        IntegrationOutcome::Queued
    );
    eventually(WAIT, async || {
        let running = fx
            .count("SELECT COUNT(*) FROM check_runs WHERE state = 'running'")
            .await;
        if running == 1 { Ok(()) } else { Err(running) }
    })
    .await;
    runner.shutdown().await;
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM integration_requests WHERE ended_at IS NULL")
            .await,
        1,
        "the stop left the request waiting"
    );

    fx.restart().await;
    let clock = fx.project.clock().clone();
    fold_daemon::services::reviews::checks::recover_check_runs(fx.project.store(), &fx.bus, &clock)
        .await
        .unwrap();
    std::fs::write(&go, "").unwrap();
    assert_eq!(fx.reviews.requeue_integrations().await.unwrap(), 1);
    integrated(&fx, proposal).await;
    eventually(WAIT, async || {
        let waiting = fx
            .count("SELECT COUNT(*) FROM integration_requests WHERE ended_at IS NULL")
            .await;
        if waiting == 0 { Ok(()) } else { Err(waiting) }
    })
    .await;
    assert_eq!(fx.reviews.requeue_integrations().await.unwrap(), 0);
}

#[tokio::test]
async fn a_process_of_an_integration_check_acts_as_no_agent() {
    let fx = Fixture::new().await;
    give_role(fx.project.store(), "review", Role::ReviewLead, None).await;
    let gate = tempfile::tempdir().unwrap();
    let (pids, go) = (gate.path().join("pids"), gate.path().join("go"));
    let run = format!(
        "sleep 60 & echo $$ $! > '{}'; until [ -e '{}' ]; do sleep 0.05; done",
        pids.display(),
        go.display()
    );
    let proposal = approved(&fx, 1, |dir| checks_file(dir, &run)).await;
    let outcome = fx
        .reviews
        .integrate_proposal(&human(), p(proposal), Some(1))
        .await
        .unwrap();
    assert_eq!(outcome, IntegrationOutcome::Queued);
    let line = eventually(WAIT, async || match std::fs::read_to_string(&pids) {
        Ok(line) if line.ends_with('\n') => Ok(line),
        other => Err(format!("{other:?}")),
    })
    .await;
    let found: Vec<u32> = line
        .split_whitespace()
        .map(|pid| pid.parse().unwrap())
        .collect();
    let chain = peers::process_chain(found[1]).unwrap();
    let peer = peers::check_peer(&chain);
    assert_eq!(peer.as_ref().and_then(|peer| peer.agent.clone()), None);
    assert_eq!(peers::session_peer(peer, ""), SessionPeer::Check);
    std::fs::write(&go, "").unwrap();
    integrated(&fx, proposal).await;
    assert_eq!(peers::check_peer(&[found[0], std::process::id()]), None);
}
