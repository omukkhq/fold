#![cfg(test)]

use crate::common::intents::Covering as _;
use crate::common::intents::{covering_with, whole_change};
use fold_core::reviews::ProposalIntent;
use std::time::Duration;

use fold_core::policy::CHECKS_FILE_REASON;
use fold_core::reviews::Verdict;
use fold_core::reviews::checks::{RunReason, RunState, Stage};
use fold_core::roles::Role;
use fold_core::{CheckRunId, TaskDescription, TaskStatus, TaskTitle, codes};
use fold_daemon::services::reviews::Proposed;
use fold_daemon::services::tasks::CreateTask;
use fold_store::testing::eventually;

use crate::checks::{PASSING, ended, running, workspace_with_checks};
use crate::common::roles::give_role;
use crate::reviews_support::{Fixture, by, human, p, refusal, t};

const WAIT: Duration = Duration::from_secs(30);

fn checking(proposed: Proposed) -> CheckRunId {
    match proposed {
        Proposed::Checking { run, .. } => run,
        Proposed::Made(detail) => panic!("expected a run, got {}", detail.summary.id),
    }
}

#[cfg(test)]
async fn last_fold_message(fx: &Fixture) -> String {
    fx.text(
        "SELECT m.body FROM messages m
         JOIN message_recipients r ON r.message_id = m.id
         JOIN workers w ON w.id = r.worker_id
         WHERE m.sender_kind = 'fold' AND w.name = 'frontend'
         ORDER BY m.number DESC LIMIT 1",
    )
    .await
    .unwrap()
}

#[cfg(test)]
async fn proposals(fx: &Fixture) -> i64 {
    fx.count("SELECT COUNT(*) FROM proposals").await
}

/// The human review mark of `task` as `(reason, set_by)`, when it has one.
#[cfg(test)]
async fn mark(fx: &Fixture, task: u32) -> Option<(String, String)> {
    fx.tasks
        .get(t(task))
        .await
        .unwrap()
        .human_review
        .map(|mark| (mark.reason, mark.set_by.as_str().to_owned()))
}

#[tokio::test]
async fn create_proposal_waits_for_green_checks_and_then_opens_the_proposal() {
    let fx = Fixture::new().await;
    workspace_with_checks(&fx, PASSING).await;
    let author = by(&fx.frontend.worker);
    let run = checking(
        covering_with(
            ProposalIntent {
                summary: "Adds the checks file; the code stays as it is.".to_owned(),
                ..whole_change(&[])
            },
            async |intent| {
                fx.reviews
                    .create_proposal(&author, t(1), "Add checks", intent, Some("ran them"))
                    .await
            },
        )
        .await
        .unwrap(),
    );
    assert_eq!(ended(&fx.reviews, run).await.run.state, RunState::Passed);
    let detail = eventually(WAIT, async || {
        fx.reviews
            .get_proposal(p(1), None)
            .await
            .map_err(|_| "no proposal yet")
    })
    .await;
    assert_eq!(detail.summary.title, "Add checks");
    assert_eq!(detail.selected.evidence.as_deref(), Some("ran them"));
    assert_eq!(
        detail
            .selected
            .intent
            .as_ref()
            .map(|intent| intent.summary.as_str()),
        Some("Adds the checks file; the code stays as it is."),
        "the run carries the intent to the version it makes"
    );
    assert_eq!(fx.status(1).await, TaskStatus::Review);
    assert_eq!(
        last_fold_message(&fx).await,
        "fold made P-1 v1 for T-1: its checks passed (cr-1)."
    );
    let view = fx
        .reviews
        .view_proposal(&by(&fx.review.worker), p(1), None)
        .await
        .unwrap();
    assert_eq!(view.checks.len(), 1);
    assert_eq!(view.checks[0].run.id, run);
}

#[tokio::test]
async fn a_check_of_both_stages_runs_under_one_name_on_the_proposal_and_at_integration() {
    let fx = Fixture::new().await;
    give_role(fx.project.store(), "review", Role::Reviewer, None).await;
    workspace_with_checks(
        &fx,
        "[[check]]\nname = \"build\"\nrun = \"true\"\nstage = [\"proposal\", \"integration\"]\n",
    )
    .await;
    let author = by(&fx.frontend.worker);
    let run = checking(
        fx.reviews
            .create_proposal_covering(&author, t(1), "Add checks", None)
            .await
            .unwrap(),
    );
    ended(&fx.reviews, run).await;
    eventually(WAIT, async || {
        fx.reviews
            .get_proposal(p(1), None)
            .await
            .map(|_| ())
            .map_err(|_| "no proposal")
    })
    .await;
    assert_eq!(
        mark(&fx, 1).await,
        Some((CHECKS_FILE_REASON.to_owned(), "fold".to_owned())),
        "a proposal that adds the checks file waits for a human"
    );
    fx.reviews
        .submit_review(
            &by(&fx.review.worker),
            p(1),
            1,
            Verdict::Approve,
            "ok",
            Vec::new(),
        )
        .await
        .unwrap();
    fx.reviews
        .integrate_proposal(&human(), p(1), Some(1))
        .await
        .unwrap();
    let stages = eventually(WAIT, async || {
        let view = fx
            .reviews
            .view_proposal(&human(), p(1), None)
            .await
            .map_err(|_| "no view")?;
        let finished: Vec<_> = view
            .checks
            .iter()
            .filter(|run| run.run.state == RunState::Passed)
            .map(|run| (run.run.stage, run.results[0].check.clone()))
            .collect();
        if finished.len() == 2 {
            Ok(finished)
        } else {
            Err("the integration run has not ended")
        }
    })
    .await;
    for stage in [Stage::Proposal, Stage::Integration] {
        assert!(
            stages.contains(&(stage, Some("build".to_owned()))),
            "{stage:?} run missing: {stages:?}"
        );
    }
}

#[tokio::test]
async fn red_checks_make_no_proposal_and_tell_the_worker() {
    let fx = Fixture::new().await;
    workspace_with_checks(
        &fx,
        "[[check]]\nname = \"unit\"\nrun = \"echo assertion failed; exit 1\"\n",
    )
    .await;
    let author = by(&fx.frontend.worker);
    let run = checking(
        fx.reviews
            .create_proposal_covering(&author, t(1), "Add checks", None)
            .await
            .unwrap(),
    );
    assert_eq!(
        fx.status(1).await,
        TaskStatus::InProgress,
        "the task waits in progress"
    );
    let view = ended(&fx.reviews, run).await;
    assert_eq!(view.run.state, RunState::Failed);
    assert_eq!(view.run.reason, Some(RunReason::CheckFailed));
    assert_eq!(proposals(&fx).await, 0);
    assert_eq!(fx.status(1).await, TaskStatus::InProgress);
    let message = last_fold_message(&fx).await;
    assert!(message.contains("unit failed (exit 1)"), "{message}");
    assert!(message.contains("fold made no proposal"), "{message}");
    assert!(message.contains("assertion failed"), "{message}");
    assert_eq!(mark(&fx, 1).await, None, "no version, no mark");
}

#[tokio::test]
async fn a_green_run_of_the_same_files_is_reused_and_a_change_runs_again() {
    let fx = Fixture::new().await;
    workspace_with_checks(&fx, PASSING).await;
    let author = by(&fx.frontend.worker);
    let run = running(fx.reviews.run_checks(&author, t(1)).await.unwrap());
    ended(&fx.reviews, run).await;
    let made = fx
        .reviews
        .create_proposal_covering(&author, t(1), "Add checks", None)
        .await
        .unwrap()
        .detail()
        .expect("the passed run of the same files counts");
    assert_eq!(made.summary.id, p(1));
    assert_eq!(fx.count("SELECT COUNT(*) FROM check_runs").await, 1);

    fx.write("src/lib.rs", "pub fn changed() {}\n");
    let run = checking(
        fx.reviews
            .update_proposal_covering(&author, p(1), None)
            .await
            .unwrap(),
    );
    assert_eq!(ended(&fx.reviews, run).await.run.state, RunState::Passed);
    let detail = eventually(WAIT, async || {
        let detail = fx.reviews.get_proposal(p(1), None).await.unwrap();
        if detail.summary.current_version == 2 {
            Ok(detail)
        } else {
            Err(detail.summary.current_version)
        }
    })
    .await;
    assert_eq!(detail.selected.version, 2);
    assert_eq!(
        last_fold_message(&fx).await,
        "fold made P-1 v2 for T-1: its checks passed (cr-2)."
    );
    let unchanged = refusal(
        fx.reviews
            .update_proposal_covering(&author, p(1), None)
            .await,
    );
    assert_eq!(
        unchanged.code(),
        codes::VERSION_UNCHANGED,
        "nothing runs for no change"
    );
    assert_eq!(fx.count("SELECT COUNT(*) FROM check_runs").await, 2);
}

#[tokio::test]
async fn a_red_update_adds_no_version() {
    let fx = Fixture::new().await;
    workspace_with_checks(&fx, PASSING).await;
    let author = by(&fx.frontend.worker);
    let run = checking(
        fx.reviews
            .create_proposal_covering(&author, t(1), "Add checks", None)
            .await
            .unwrap(),
    );
    ended(&fx.reviews, run).await;
    eventually(WAIT, async || {
        fx.reviews
            .get_proposal(p(1), None)
            .await
            .map(|_| ())
            .map_err(|_| "no proposal")
    })
    .await;
    std::fs::remove_file(fx.task_dir().join("src/lib.rs")).unwrap();
    let run = checking(
        fx.reviews
            .update_proposal_covering(&author, p(1), None)
            .await
            .unwrap(),
    );
    assert_eq!(ended(&fx.reviews, run).await.run.state, RunState::Failed);
    let detail = fx.reviews.get_proposal(p(1), None).await.unwrap();
    assert_eq!(detail.summary.current_version, 1);
    let message = last_fold_message(&fx).await;
    assert!(
        message.contains("fold added no version to P-1"),
        "{message}"
    );
    assert!(message.contains("update_proposal again"), "{message}");
}

#[tokio::test]
async fn a_proposal_command_waits_its_turn_behind_a_running_check() {
    let fx = Fixture::new().await;
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    workspace_with_checks(
        &fx,
        &format!(
            "[[check]]\nname = \"slow\"\nrun = \"until [ -e '{}' ]; do sleep 0.05; done\"\n",
            go.display()
        ),
    )
    .await;
    let author = by(&fx.frontend.worker);
    let run = running(fx.reviews.run_checks(&author, t(1)).await.unwrap());
    let busy = refusal(
        fx.reviews
            .create_proposal_covering(&author, t(1), "Add checks", None)
            .await,
    );
    assert_eq!(busy.code(), codes::CHECKS_RUNNING);
    std::fs::write(&go, "").unwrap();
    ended(&fx.reviews, run).await;
}

#[tokio::test]
async fn a_green_run_whose_proposal_was_refused_is_reused() {
    let fx = Fixture::new().await;
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    workspace_with_checks(
        &fx,
        &format!(
            "[[check]]\nname = \"slow\"\nrun = \"until [ -e '{}' ]; do sleep 0.05; done\"\n",
            go.display()
        ),
    )
    .await;
    let author = by(&fx.frontend.worker);
    let run = checking(
        fx.reviews
            .create_proposal_covering(&author, t(1), "Add checks", None)
            .await
            .unwrap(),
    );
    fx.tasks.release_task(&author, t(1)).await.unwrap();
    std::fs::write(&go, "").unwrap();
    let view = ended(&fx.reviews, run).await;
    assert_eq!(view.run.state, RunState::Stopped);
    assert_eq!(view.run.reason, Some(RunReason::Refused));
    assert_eq!(proposals(&fx).await, 0);

    fx.tasks.claim_task(&author, t(1)).await.unwrap();
    let made = fx
        .reviews
        .create_proposal_covering(&author, t(1), "Add checks", None)
        .await
        .unwrap()
        .detail()
        .expect("the checks of the same files passed");
    assert_eq!(made.summary.id, p(1));
    assert_eq!(fx.count("SELECT COUNT(*) FROM check_runs").await, 1);
}

#[tokio::test]
async fn an_invalid_checks_file_refuses_the_proposal_at_once() {
    let fx = Fixture::new().await;
    workspace_with_checks(&fx, "[[check]]\nname = \"t\"\n").await;
    let refused = refusal(
        fx.reviews
            .create_proposal_covering(&by(&fx.frontend.worker), t(1), "Add checks", None)
            .await,
    );
    assert_eq!(refused.code(), codes::CHECKS_FILE_INVALID);
    assert_eq!(proposals(&fx).await, 0);
}

/// `T-1` adds `fold-checks.toml` through a green proposal that a reviewer approves and a human integrates, so main holds the file.
#[cfg(test)]
async fn main_with_checks(fx: &Fixture) {
    give_role(fx.project.store(), "review", Role::Reviewer, None).await;
    workspace_with_checks(fx, PASSING).await;
    let author = by(&fx.frontend.worker);
    let run = checking(
        fx.reviews
            .create_proposal_covering(&author, t(1), "Add checks", None)
            .await
            .unwrap(),
    );
    ended(&fx.reviews, run).await;
    eventually(WAIT, async || {
        fx.reviews
            .get_proposal(p(1), None)
            .await
            .map(|_| ())
            .map_err(|_| "no proposal")
    })
    .await;
    assert_eq!(
        mark(fx, 1).await,
        Some((CHECKS_FILE_REASON.to_owned(), "fold".to_owned())),
        "a proposal that adds the checks file waits for a human"
    );
    fx.reviews
        .submit_review(
            &by(&fx.review.worker),
            p(1),
            1,
            Verdict::Approve,
            "ok",
            Vec::new(),
        )
        .await
        .unwrap();
    let refused = refusal(
        fx.reviews
            .integrate_proposal(&by(&fx.review.worker), p(1), Some(1))
            .await,
    );
    assert_eq!(refused.code(), codes::HUMAN_APPROVAL_MISSING);
    fx.reviews
        .integrate_proposal(&human(), p(1), Some(1))
        .await
        .unwrap();
}

#[cfg(test)]
async fn new_task(fx: &Fixture) -> u32 {
    fx.tasks
        .create_task(
            &by(&fx.frontend.worker),
            CreateTask {
                title: TaskTitle::parse("more").unwrap(),
                description: TaskDescription::new("body"),
                labels: crate::common::roles::work_labels(),
            },
        )
        .await
        .unwrap();
    u32::try_from(fx.count("SELECT MAX(number) FROM tasks").await).unwrap()
}

/// `frontend` opens a proposal for `task` with `edit` applied to its fresh workspace, through its green checks, and returns the proposal's number.
#[cfg(test)]
async fn gated_proposal(fx: &Fixture, task: u32, edit: impl FnOnce(&std::path::Path)) -> u32 {
    let author = by(&fx.frontend.worker);
    fx.workspaces
        .create_workspace(&author, t(task))
        .await
        .unwrap();
    edit(&fx.project.paths().root().join(format!(".fold/ws/t-{task}")));
    let before = fx.count("SELECT COUNT(*) FROM proposals").await;
    match fx
        .reviews
        .create_proposal_covering(&author, t(task), "Change", None)
        .await
        .unwrap()
    {
        Proposed::Checking { run, .. } => {
            assert_eq!(ended(&fx.reviews, run).await.run.state, RunState::Passed);
        }
        Proposed::Made(_) => {}
    }
    eventually(WAIT, async || {
        let now = fx.count("SELECT COUNT(*) FROM proposals").await;
        if now > before { Ok(()) } else { Err(now) }
    })
    .await;
    u32::try_from(fx.count("SELECT MAX(number) FROM proposals").await).unwrap()
}

#[tokio::test]
async fn a_proposal_that_changes_or_removes_the_checks_file_needs_a_human() {
    let fx = Fixture::new().await;
    main_with_checks(&fx).await;

    let plain = new_task(&fx).await;
    gated_proposal(&fx, plain, |dir| {
        std::fs::write(dir.join("README.md"), "# changed\n").unwrap();
    })
    .await;
    assert_eq!(
        mark(&fx, plain).await,
        None,
        "the checks file is as main has it"
    );

    let weaker = new_task(&fx).await;
    gated_proposal(&fx, weaker, |dir| {
        std::fs::write(
            dir.join("fold-checks.toml"),
            "[[check]]\nname = \"test\"\nrun = \"true\"\nrequired = false\n",
        )
        .unwrap();
    })
    .await;
    assert_eq!(
        mark(&fx, weaker).await,
        Some((CHECKS_FILE_REASON.to_owned(), "fold".to_owned()))
    );

    let removed = new_task(&fx).await;
    let author = by(&fx.frontend.worker);
    fx.workspaces
        .create_workspace(&author, t(removed))
        .await
        .unwrap();
    std::fs::remove_file(
        fx.project
            .paths()
            .root()
            .join(format!(".fold/ws/t-{removed}/fold-checks.toml")),
    )
    .unwrap();
    let made = fx
        .reviews
        .create_proposal_covering(&author, t(removed), "Drop the checks", None)
        .await
        .unwrap()
        .detail()
        .expect("a tree without the file has no check to run");
    assert_eq!(made.task_status, TaskStatus::Review);
    assert_eq!(
        mark(&fx, removed).await,
        Some((CHECKS_FILE_REASON.to_owned(), "fold".to_owned())),
        "removing the checks file needs a human too"
    );
}

#[tokio::test]
async fn a_marked_task_keeps_its_own_mark() {
    let fx = Fixture::new().await;
    fx.reviews
        .require_human_review(&by(&fx.review.worker), t(1), None, "touches auth")
        .await
        .unwrap();
    workspace_with_checks(&fx, PASSING).await;
    let run = checking(
        fx.reviews
            .create_proposal_covering(&by(&fx.frontend.worker), t(1), "Add checks", None)
            .await
            .unwrap(),
    );
    ended(&fx.reviews, run).await;
    eventually(WAIT, async || {
        fx.reviews
            .get_proposal(p(1), None)
            .await
            .map(|_| ())
            .map_err(|_| "no proposal")
    })
    .await;
    assert_eq!(
        mark(&fx, 1).await,
        Some(("touches auth".to_owned(), "review".to_owned()))
    );
}

/// `frontend` opens `P-1` through green checks of `fold-checks.toml`, whose one check waits until `go` exists and then needs `src/lib.rs`; `go` exists until the call returns.
#[cfg(test)]
async fn proposal_behind_a_gate(fx: &Fixture, go: &std::path::Path) {
    give_role(fx.project.store(), "review", Role::Reviewer, None).await;
    std::fs::write(go, "").unwrap();
    workspace_with_checks(
        fx,
        &format!(
            "[[check]]\nname = \"slow\"\nrun = \"until [ -e '{}' ]; do sleep 0.05; done; test -f src/lib.rs\"\n",
            go.display()
        ),
    )
    .await;
    let run = checking(
        fx.reviews
            .create_proposal_covering(&by(&fx.frontend.worker), t(1), "Add checks", None)
            .await
            .unwrap(),
    );
    ended(&fx.reviews, run).await;
    eventually(WAIT, async || {
        fx.reviews
            .get_proposal(p(1), None)
            .await
            .map(|_| ())
            .map_err(|_| "no proposal")
    })
    .await;
}

#[tokio::test]
async fn an_approval_of_the_current_version_waits_while_an_update_runs_its_checks() {
    let fx = Fixture::new().await;
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    proposal_behind_a_gate(&fx, &go).await;
    std::fs::remove_file(&go).unwrap();
    fx.write("src/lib.rs", "pub fn z() {}\n");
    let update = checking(
        fx.reviews
            .update_proposal_covering(&by(&fx.frontend.worker), p(1), None)
            .await
            .unwrap(),
    );
    let view = fx
        .reviews
        .view_proposal(&human(), p(1), None)
        .await
        .unwrap();
    assert_eq!(view.update_run.map(|run| run.run), Some(update));
    assert_eq!(view.detail.summary.current_version, 1);
    let reviewer = by(&fx.review.worker);
    for actor in [&reviewer, &human()] {
        let held = refusal(
            fx.reviews
                .submit_review(actor, p(1), 1, Verdict::Approve, "ok", Vec::new())
                .await,
        );
        assert_eq!(held.code(), codes::UPDATE_CHECKS_RUNNING);
        assert_eq!(
            held.details().unwrap()["run"],
            serde_json::json!(update.to_string())
        );
        assert!(held.hint().unwrap().contains("wait_for_events"));
    }
    let integrate = refusal(fx.reviews.integrate_proposal(&human(), p(1), Some(1)).await);
    assert_eq!(integrate.code(), codes::UPDATE_CHECKS_RUNNING);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM reviews WHERE verdict = 'approve'")
            .await,
        0
    );
    fx.reviews
        .submit_review(&reviewer, p(1), 1, Verdict::Comment, "looking", Vec::new())
        .await
        .unwrap();
    fx.reviews
        .submit_review(
            &reviewer,
            p(1),
            1,
            Verdict::RequestChanges,
            "more",
            Vec::new(),
        )
        .await
        .unwrap();
    std::fs::write(&go, "").unwrap();
    assert_eq!(ended(&fx.reviews, update).await.run.state, RunState::Passed);
    let view = eventually(WAIT, async || {
        let view = fx
            .reviews
            .view_proposal(&human(), p(1), None)
            .await
            .unwrap();
        if view.detail.summary.current_version == 2 {
            Ok(view)
        } else {
            Err("no v2")
        }
    })
    .await;
    assert_eq!(view.update_run, None);
    fx.reviews
        .submit_review(&reviewer, p(1), 2, Verdict::Approve, "ok", Vec::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn an_approved_version_integrates_on_its_own_only_after_a_red_update_run_ends() {
    let fx = Fixture::new().await;
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    proposal_behind_a_gate(&fx, &go).await;
    for actor in [by(&fx.review.worker), human()] {
        fx.reviews
            .submit_review(&actor, p(1), 1, Verdict::Approve, "ok", Vec::new())
            .await
            .unwrap();
    }
    std::fs::remove_file(&go).unwrap();
    std::fs::remove_file(fx.task_dir().join("src/lib.rs")).unwrap();
    let update = checking(
        fx.reviews
            .update_proposal_covering(&by(&fx.frontend.worker), p(1), None)
            .await
            .unwrap(),
    );
    assert!(fx.reviews.integrate_ready().await.unwrap().is_empty());
    std::fs::write(&go, "").unwrap();
    assert_eq!(ended(&fx.reviews, update).await.run.state, RunState::Failed);
    let view = fx
        .reviews
        .view_proposal(&human(), p(1), None)
        .await
        .unwrap();
    assert_eq!(view.update_run, None);
    assert_eq!(view.detail.summary.current_version, 1);
    let done = fx.reviews.integrate_ready().await.unwrap();
    assert_eq!(
        done.iter().map(|done| done.proposal).collect::<Vec<_>>(),
        [p(1)]
    );
}
