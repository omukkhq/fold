#![cfg(test)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::common::roles::give_role;
use crate::proposal_tools_support::{Fixture, human, t};
use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fold_core::policy::{Gate, GateStatus};
use fold_core::reviews::Verdict;
use fold_core::reviews::checks::{Purpose, RunReason, RunState};
use fold_core::roles::Role;
use fold_core::{Actor, EventFilter, FoldError, ProposalId, TaskStatus, WorkspaceId, codes};
use fold_daemon::http::host::CLIENT_HEADER;
use fold_daemon::http::socket_router;
use fold_daemon::services::reviews::checks::VersionRun;
use fold_daemon::services::reviews::{
    BatchVerdict, CheckoutTarget, FixedVersion, IntegrationOutcome, LeftOutReason, ReviewError,
    ReviewFix,
};
use fold_daemon::services::tasks::TaskError;
use fold_daemon::services::workspaces::WorkspaceError;
use fold_store::events;
use fold_store::workspaces::{self, CheckpointReason};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const FIXED: &str = "pub fn y() -> u8 { 1 }\n";
const TEST: &str = "#[test]\nfn y_is_one() { assert_eq!(super::y(), 1); }\n";

fn p(number: u32) -> ProposalId {
    ProposalId::from_number(number).unwrap()
}

fn ws(number: u32) -> WorkspaceId {
    WorkspaceId::from_number(number).unwrap()
}

fn fix(path: &str, reason: &str) -> ReviewFix {
    ReviewFix {
        path: path.to_owned(),
        reason: reason.to_owned(),
    }
}

fn both_fixes() -> Vec<ReviewFix> {
    vec![
        fix(
            "src/lib.rs",
            "y returned nothing; it returns 1 as the task asks",
        ),
        fix("tests/y.rs", "regression test for y"),
    ]
}

fn refusal<T: std::fmt::Debug>(result: Result<T, ReviewError>) -> FoldError {
    match result {
        Err(ReviewError::Fold(error) | ReviewError::Task(TaskError::Fold(error))) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn workspace_refusal<T: std::fmt::Debug>(result: Result<T, WorkspaceError>) -> FoldError {
    match result {
        Err(WorkspaceError::Fold(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn details(error: &FoldError) -> Value {
    error.details().cloned().map_or(Value::Null, Value::Object)
}

/// `frontend` proposes `P-1` for `T-1`; `review`, a reviewer, gets its claim and checks it out as `ws-2`.
struct Scene {
    fx: Fixture,
    frontend: Actor,
    review: Actor,
    checkout: PathBuf,
}

impl Scene {
    async fn new() -> Self {
        let fx = Fixture::new().await;
        let frontend = fx.agent("frontend").await;
        let review = fx.agent("review").await;
        give_role(&fx.handle.store, "review", Role::Reviewer, None).await;
        fx.proposal(&frontend, 1, "pub fn y() {}\n").await;
        let reviews = fx.reviews();
        crate::common::roles::give_review_claim(
            &fx.handle.store,
            "review",
            p(1),
            fx.handle.clock.now(),
        )
        .await;
        let checkout = reviews.create_review_checkout(&review, p(1)).await.unwrap();
        assert_eq!(checkout.workspace.id, ws(2));
        let checkout = fx.project.paths().root().join(&checkout.workspace.path);
        Self {
            fx,
            frontend,
            review,
            checkout,
        }
    }

    fn edit(&self, path: &str, content: &str) {
        let file = self.checkout.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }

    fn task_file(&self, path: &str) -> String {
        std::fs::read_to_string(
            self.fx
                .project
                .paths()
                .root()
                .join(".fold/ws/t-1")
                .join(path),
        )
        .unwrap()
    }

    async fn fix(&self, version: u32, fixes: &[ReviewFix]) -> Result<FixedVersion, ReviewError> {
        self.fx
            .reviews()
            .submit_review_fix(
                &self.review,
                p(1),
                version,
                fixes,
                "cargo test: 2 passed\nmake check: ok",
                "Makes y return 1 as the task asks, with a regression test; nothing else changes.",
            )
            .await
            .map(|outcome| outcome.fixed().unwrap())
    }

    async fn last_seq(&self) -> u64 {
        self.events_after(0)
            .await
            .last()
            .map_or(0, |event| event.seq)
    }

    async fn events_after(&self, seq: u64) -> Vec<fold_core::Event> {
        events::read_after(
            self.fx.handle.store.read(),
            seq,
            &EventFilter::default(),
            1000,
        )
        .await
        .unwrap()
    }

    async fn approve(&self, actor: &Actor, version: u32) {
        self.fx
            .reviews()
            .submit_review(
                actor,
                p(1),
                version,
                Verdict::Approve,
                "Fixes: src/lib.rs (y returns 1; tests/y.rs). cargo test: 2 passed",
                Vec::new(),
            )
            .await
            .unwrap();
    }

    async fn gate(&self, gate: Gate) -> GateStatus {
        self.fx
            .reviews()
            .view_proposal(&self.review, p(1), None)
            .await
            .unwrap()
            .gates
            .into_iter()
            .find(|state| state.gate == gate)
            .unwrap()
            .status
    }
}

#[tokio::test]
async fn a_fix_in_the_checkout_becomes_the_reviewers_next_version_and_waits_in_the_task_workspace()
{
    let scene = Scene::new().await;
    let mode = std::fs::metadata(scene.checkout.join("src/lib.rs"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o200, 0, "a review checkout is writable");
    scene.edit("src/lib.rs", FIXED);
    scene.edit("tests/y.rs", TEST);
    let before = scene.last_seq().await;

    let fixed = scene.fix(1, &both_fixes()).await.unwrap();

    assert_eq!((fixed.version, fixed.from_version), (2, 1));
    assert_eq!(fixed.checkout, ws(2));
    let detail = &fixed.detail;
    let v1 = &detail.versions[0];
    let v2 = &detail.versions[1];
    assert_eq!(v2.created_by, "review");
    assert_eq!(v2.fixed_in_checkout, Some(ws(2)));
    assert_eq!(v1.fixed_in_checkout, None);
    assert_eq!(v2.base, v1.base);
    assert!(!v2.human_edit);
    assert_eq!(
        v2.intent.as_deref(),
        Some("Makes y return 1 as the task asks, with a regression test; nothing else changes."),
        "the fix carries the reviewer's own intent"
    );
    assert_eq!(
        v2.evidence.as_deref(),
        Some(
            "Review fixes by review on v1:\n- src/lib.rs: y returned nothing; it returns 1 as the task asks\n- tests/y.rs: regression test for y\nChecks:\ncargo test: 2 passed\nmake check: ok"
        )
    );
    assert_eq!(detail.summary.author.as_str(), "frontend");
    assert_eq!(
        detail.summary.review_claim.as_ref().map(|w| w.as_str()),
        Some("review")
    );
    assert_eq!(
        detail.summary.latest_verdict, None,
        "a fix starts unapproved"
    );
    assert_eq!(detail.task_status, TaskStatus::Review);

    let appended = scene.events_after(before).await;
    let kinds: Vec<&str> = appended.iter().map(|e| e.event_type.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "changeset.recorded",
            "checkpoint.recorded",
            "proposal.reviewer_fixed"
        ]
    );
    let event = &appended[2];
    assert_eq!(event.actor.as_str(), "review");
    assert_eq!(event.task, Some(t(1)));
    assert_eq!(event.proposal, Some(p(1)));
    assert_eq!(event.workspace, Some(ws(1)));
    assert_eq!(
        event.data,
        json!({
            "version": 2,
            "from_version": 1,
            "changeset": fixed.changeset.to_string(),
            "checkpoint": fixed.checkpoint.to_string(),
            "checkout": "ws-2",
            "files": 2,
        })
    );

    // fold leaves the task workspace's files alone and marks the fix as its pending write.
    assert_eq!(scene.task_file("src/lib.rs"), "pub fn y() {}\n");
    let store = scene.fx.handle.store.read();
    let pending = workspaces::sync_checkpoint(store, ws(1)).await.unwrap();
    assert_eq!(pending, Some(fixed.checkpoint));
    let checkpoint = workspaces::get_checkpoint(store, fixed.checkpoint)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checkpoint.reason, CheckpointReason::ReviewFix);
    assert_eq!(checkpoint.tree, v2.tree);
    let workspaces = scene.fx.workspaces();
    let error = workspace_refusal(
        workspaces
            .record_changes(&scene.frontend, ws(1), "more")
            .await,
    );
    assert_eq!(error.code(), codes::WORKSPACE_WRITE_UNFINISHED);
    assert!(error.hint().unwrap().contains("reviewer's version"));

    // The checkout holds the new version, so a repeat checkout returns it and a second fix goes on from it before the worker syncs.
    let listed = scene.fx.reviews().list_review_checkouts().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].target,
        CheckoutTarget::Proposal {
            proposal: p(1),
            version: 2
        }
    );
    let again = scene
        .fx
        .reviews()
        .create_review_checkout(&scene.review, p(1))
        .await
        .unwrap();
    assert_eq!((again.workspace.id, again.version), (ws(2), 2));
    scene.edit("README.md", "# demo\n\nUse y.\n");
    let second = scene
        .fix(2, &[fix("README.md", "the readme named x")])
        .await
        .unwrap();
    assert_eq!(second.version, 3);

    // The worker's sync writes the newest version into its files, and it continues from there.
    workspaces
        .sync_workspace(&scene.frontend, ws(1))
        .await
        .unwrap();
    assert_eq!(scene.task_file("src/lib.rs"), FIXED);
    assert_eq!(scene.task_file("tests/y.rs"), TEST);
    assert_eq!(scene.task_file("README.md"), "# demo\n\nUse y.\n");
    assert_eq!(
        workspaces::sync_checkpoint(store, ws(1)).await.unwrap(),
        None
    );
    scene.fx.write(1, "src/more.rs", "pub fn more() {}\n");
    workspaces
        .record_changes(&scene.frontend, ws(1), "more")
        .await
        .unwrap();
}

#[tokio::test]
async fn the_reviewer_approves_its_own_fix_and_fold_integrates_it() {
    let scene = Scene::new().await;
    scene.edit("src/lib.rs", FIXED);
    scene.edit("tests/y.rs", TEST);
    scene.fix(1, &both_fixes()).await.unwrap();
    assert!(matches!(
        scene.gate(Gate::Approval).await,
        GateStatus::Unmet(_)
    ));

    scene.approve(&scene.review, 2).await;

    assert_eq!(scene.gate(Gate::Approval).await, GateStatus::Met);
    let outcome = scene
        .fx
        .reviews()
        .integrate_proposal(&scene.review, p(1), Some(2))
        .await
        .unwrap();
    assert!(matches!(outcome, IntegrationOutcome::Succeeded { .. }));
    assert_eq!(
        scene.fx.handle.tasks.get(t(1)).await.unwrap().status,
        TaskStatus::Done
    );
}

#[tokio::test]
async fn a_marked_task_still_waits_for_the_human_after_the_reviewer_approves_its_fix() {
    let scene = Scene::new().await;
    scene
        .fx
        .reviews()
        .require_human_review(&human("tanim"), t(1), None, "touches the API")
        .await
        .unwrap();
    scene.edit("src/lib.rs", FIXED);
    scene.edit("tests/y.rs", TEST);
    scene.fix(1, &both_fixes()).await.unwrap();

    scene.approve(&scene.review, 2).await;

    assert_eq!(scene.gate(Gate::Approval).await, GateStatus::Met);
    assert!(matches!(
        scene.gate(Gate::HumanReview).await,
        GateStatus::Unmet(_)
    ));
    let error = refusal(
        scene
            .fx
            .reviews()
            .integrate_proposal(&scene.review, p(1), Some(2))
            .await,
    );
    assert_eq!(error.code(), codes::HUMAN_APPROVAL_MISSING);
    scene.approve(&human("tanim"), 2).await;
    assert_eq!(scene.gate(Gate::HumanReview).await, GateStatus::Met);
}

#[tokio::test]
async fn only_the_review_claim_holder_fixes_and_never_on_a_stale_version() {
    let scene = Scene::new().await;
    let reviews = scene.fx.reviews();
    let backend = scene.fx.agent("backend").await;
    scene.edit("src/lib.rs", FIXED);
    let one = [fix("src/lib.rs", "y returns 1")];
    for (actor, code) in [
        (&backend, codes::REVIEW_CLAIM_REQUIRED),
        (&scene.frontend, codes::AUTHOR_CANNOT_REVIEW),
        (&human("tanim"), codes::REVIEW_CLAIM_REQUIRED),
        (&Actor::System, codes::REVIEW_CLAIM_REQUIRED),
    ] {
        let error = refusal(
            reviews
                .submit_review_fix(
                    actor,
                    p(1),
                    1,
                    &one,
                    "ok",
                    "Make the change the task asks for.",
                )
                .await,
        );
        assert_eq!(error.code(), code, "{actor:?}");
    }
    let error = refusal(
        reviews
            .submit_review_fix(
                &backend,
                p(1),
                1,
                &one,
                "ok",
                "Make the change the task asks for.",
            )
            .await,
    );
    assert_eq!(
        details(&error),
        json!({"proposal": "P-1", "holder": "review"})
    );

    // The author adds v2 while the reviewer fixes v1: nothing is written and the checkout keeps v1.
    scene.fx.write(1, "src/lib.rs", "pub fn y() -> u8 { 2 }\n");
    reviews
        .update_proposal(
            &scene.frontend,
            p(1),
            "Make the change the task asks for.",
            None,
        )
        .await
        .unwrap();
    let error = refusal(scene.fix(1, &one).await);
    assert_eq!(error.code(), codes::STALE_VERSION);
    let detail = reviews.get_proposal(p(1), None).await.unwrap();
    assert_eq!(detail.versions.len(), 2);
    let listed = reviews.list_review_checkouts().await.unwrap();
    assert_eq!(
        listed[0].target,
        CheckoutTarget::Proposal {
            proposal: p(1),
            version: 1
        }
    );
    // v2 has no checkout of the reviewer yet.
    let error = refusal(scene.fix(2, &one).await);
    assert_eq!(error.code(), codes::WORKSPACE_NOT_FOUND);
    assert_eq!(details(&error)["reason"], "no_checkout");
    assert!(error.hint().unwrap().contains("create_review_checkout"));

    // A released claim takes the right away.
    reviews
        .release_review(&scene.review, p(1), None)
        .await
        .unwrap();
    let error = refusal(scene.fix(2, &one).await);
    assert_eq!(error.code(), codes::REVIEW_CLAIM_REQUIRED);
}

#[tokio::test]
async fn the_fixes_name_exactly_the_files_that_changed() {
    let scene = Scene::new().await;
    let error = refusal(scene.fix(1, &[fix("src/lib.rs", "nothing")]).await);
    assert_eq!(error.code(), codes::VERSION_UNCHANGED);

    scene.edit("src/lib.rs", FIXED);
    scene.edit("tests/y.rs", TEST);
    let error = refusal(scene.fix(1, &[fix("src/lib.rs", "y returns 1")]).await);
    assert_eq!(error.code(), codes::INVALID_REQUEST);
    assert_eq!(
        details(&error),
        json!({
            "reason": "fixes_mismatch",
            "unlisted": ["tests/y.rs"],
            "unlisted_count": 1,
            "unchanged": [],
            "unchanged_count": 0,
        })
    );
    let mut extra = both_fixes();
    extra.push(fix("README.md", "not changed"));
    let error = refusal(scene.fix(1, &extra).await);
    assert_eq!(details(&error)["unchanged"], json!(["README.md"]));
    let error = refusal(
        scene
            .fx
            .reviews()
            .submit_review_fix(
                &scene.review,
                p(1),
                1,
                &both_fixes(),
                " ",
                "Make the change the task asks for.",
            )
            .await,
    );
    assert_eq!(details(&error)["field"], "checks");
    let error = refusal(
        scene
            .fx
            .reviews()
            .submit_review_fix(&scene.review, p(1), 1, &both_fixes(), "ok", "\n")
            .await,
    );
    assert_eq!(error.code(), codes::INVALID_INTENT);
    assert!(
        error
            .hint()
            .unwrap()
            .starts_with("call submit_review_fix again")
    );
    let versions = scene.fx.reviews().get_proposal(p(1), None).await.unwrap();
    assert_eq!(versions.versions.len(), 1, "a refusal writes no version");
}

#[tokio::test]
async fn a_batch_member_is_fixed_in_its_own_checkout_and_left_out_of_the_batch_review() {
    let scene = Scene::new().await;
    let reviews = scene.fx.reviews();
    let backend = scene.fx.agent("backend").await;
    scene
        .fx
        .workspaces()
        .create_workspace(&backend, t(2))
        .await
        .unwrap();
    scene.fx.write(2, "README.md", "# demo\nmore\n");
    reviews
        .create_proposal(
            &backend,
            t(2),
            "Extend the readme",
            "Make the change the task asks for.",
            Some("read it"),
        )
        .await
        .unwrap();
    reviews
        .release_review(&scene.review, p(1), None)
        .await
        .unwrap();
    let batch = crate::common::roles::give_batch(
        &scene.fx.handle.store,
        "review",
        &[p(1), p(2)],
        scene.fx.handle.clock.now(),
    )
    .await;
    let checkout = reviews
        .create_batch_checkout(&scene.review, batch)
        .await
        .unwrap();
    let batch_dir = scene
        .fx
        .project
        .paths()
        .root()
        .join(&checkout.workspace.path);
    let mode = std::fs::metadata(batch_dir.join("src/lib.rs"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o222, 0, "a batch checkout stays read-only");
    let error = refusal(
        reviews
            .submit_review_fix(
                &scene.review,
                p(2),
                1,
                &[fix("README.md", "r")],
                "ok",
                "Make the change the task asks for.",
            )
            .await,
    );
    assert_eq!(error.code(), codes::WORKSPACE_NOT_FOUND);
    assert!(error.hint().unwrap().contains("create_review_checkout"));

    // The member's own checkout (ws-2, made before the batch) holds v1 of P-1.
    scene.edit("src/lib.rs", FIXED);
    scene.edit("tests/y.rs", TEST);
    let fixed = scene.fix(1, &both_fixes()).await.unwrap();
    assert_eq!(fixed.version, 2);
    let submitted = reviews
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
                    verdict: Verdict::Approve,
                    body: String::new(),
                },
            ],
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(submitted.left_out.len(), 1);
    assert_eq!(submitted.left_out[0].proposal, p(1));
    assert_eq!(submitted.left_out[0].reason, LeftOutReason::NewVersion);
    assert_eq!(submitted.reviewed[0].proposal, p(2));
    scene.approve(&scene.review, 2).await;
}

async fn post(router: Router, body: &Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .header(CLIENT_HEADER, "test")
        .method(Method::POST)
        .uri("/api/v1/hooks/edit")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn the_edit_hook_admits_only_the_claim_holders_own_checkout_of_one_proposal() {
    let scene = Scene::new().await;
    let fx = &scene.fx;
    let root = fx.project.paths().root().to_string_lossy().into_owned();
    let row = fold_store::projects::ProjectRow {
        slug: "demo".to_owned(),
        name: "Demo".to_owned(),
        root: root.clone(),
    };
    fx.project
        .registry()
        .write(async |tx| fold_store::projects::insert_project(tx, &row, fx.project.now()).await)
        .await
        .unwrap();
    let backend = fx.agent("backend").await;
    let socket = socket_router(fx.state.clone());
    let ask =
        |agent: &str, workspace: &str| json!({"cwd": root, "agent": agent, "workspace": workspace});
    let allowed = |answer: &(StatusCode, Value)| {
        assert_eq!(answer.0, StatusCode::OK, "{answer:?}");
        answer.1["allowed"].as_bool().unwrap()
    };

    let own = post(socket.clone(), &ask("review", ".fold/ws/review-p-1")).await;
    assert!(allowed(&own));
    assert_eq!(own.1["reason"], Value::Null);
    let other = post(socket.clone(), &ask("backend", ".fold/ws/review-p-1")).await;
    assert!(!allowed(&other));
    assert!(
        other.1["reason"]
            .as_str()
            .unwrap()
            .contains("another reviewer")
    );
    let task = post(socket.clone(), &ask("review", ".fold/ws/t-1")).await;
    assert!(!allowed(&task));

    let reviews = fx.reviews();
    reviews
        .release_review(&scene.review, p(1), None)
        .await
        .unwrap();
    let unclaimed = post(socket.clone(), &ask("review", ".fold/ws/review-p-1")).await;
    assert!(!allowed(&unclaimed));
    assert!(
        unclaimed.1["reason"]
            .as_str()
            .unwrap()
            .contains("claim_review")
    );

    fx.workspaces()
        .create_workspace(&backend, t(2))
        .await
        .unwrap();
    fx.write(2, "README.md", "# demo\nmore\n");
    reviews
        .create_proposal(
            &backend,
            t(2),
            "Extend the readme",
            "Make the change the task asks for.",
            None,
        )
        .await
        .unwrap();
    let batch = crate::common::roles::give_batch(
        &fx.handle.store,
        "review",
        &[p(1), p(2)],
        fx.handle.clock.now(),
    )
    .await;
    let checkout = reviews
        .create_batch_checkout(&scene.review, batch)
        .await
        .unwrap();
    let batched = post(socket.clone(), &ask("review", &checkout.workspace.path)).await;
    assert!(!allowed(&batched));
    assert!(
        batched.1["reason"]
            .as_str()
            .unwrap()
            .contains("batch checkout")
    );

    // Closing the proposal archives its checkout.
    reviews
        .close_proposal(&scene.frontend, p(1), "not needed")
        .await
        .unwrap();
    let archived = post(socket.clone(), &ask("review", ".fold/ws/review-p-1")).await;
    assert!(!allowed(&archived));

    let (status, answer) = post(socket, &ask("Not A Name", ".fold/ws/review-p-1")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(answer["error"]["code"], codes::INVALID_NAME);
}

/// `frontend` proposes `P-1` with a `fold-checks.toml` whose check needs `fn y` in `src/lib.rs`, through its green checks; `review`, a reviewer, gets its claim and checks it out.
#[cfg(test)]
async fn scene_with_checks() -> Scene {
    let fx = Fixture::new().await;
    let frontend = fx.agent("frontend").await;
    let review = fx.agent("review").await;
    give_role(&fx.handle.store, "review", Role::Reviewer, None).await;
    fx.workspaces()
        .create_workspace(&frontend, t(1))
        .await
        .unwrap();
    fx.write(1, "src/lib.rs", "pub fn y() {}\n");
    fx.write(
        1,
        "fold-checks.toml",
        "[[check]]\nname = \"lib\"\nrun = \"grep -q 'fn y' src/lib.rs\"\n",
    );
    let reviews = fx.reviews();
    let proposed = reviews
        .create_proposal(
            &frontend,
            t(1),
            "Add y",
            "Make the change the task asks for.",
            None,
        )
        .await
        .unwrap();
    assert!(matches!(
        proposed,
        fold_daemon::services::reviews::Proposed::Checking { .. }
    ));
    fold_store::testing::eventually(std::time::Duration::from_secs(30), async || {
        reviews
            .get_proposal(p(1), None)
            .await
            .map(|_| ())
            .map_err(|_| "no proposal")
    })
    .await;
    crate::common::roles::give_review_claim(
        &fx.handle.store,
        "review",
        p(1),
        fx.handle.clock.now(),
    )
    .await;
    let checkout = reviews.create_review_checkout(&review, p(1)).await.unwrap();
    let checkout = fx.project.paths().root().join(&checkout.workspace.path);
    Scene {
        fx,
        frontend,
        review,
        checkout,
    }
}

#[cfg(test)]
async fn checked_fix(scene: &Scene, fixes: &[ReviewFix]) -> fold_core::CheckRunId {
    let outcome = scene
        .fx
        .reviews()
        .submit_review_fix(
            &scene.review,
            p(1),
            1,
            fixes,
            "grep: ok",
            "Makes y return 1, checked by the proposal checks.",
        )
        .await
        .unwrap();
    let fold_daemon::services::reviews::FixOutcome::Checking { run, checkout, .. } = outcome else {
        panic!("expected a check run of the fix");
    };
    assert_eq!(checkout, ws(2));
    fold_store::testing::eventually(std::time::Duration::from_secs(30), async || {
        let view = scene.fx.reviews().get_check_run(run, false).await.unwrap();
        if view.run.state == fold_core::reviews::checks::RunState::Running {
            Err(view.run.state)
        } else {
            Ok(())
        }
    })
    .await;
    run
}

#[cfg(test)]
async fn last_message_to_review(scene: &Scene) -> String {
    sqlx::query_scalar(
        "SELECT m.body FROM messages m
         JOIN message_recipients r ON r.message_id = m.id
         JOIN workers w ON w.id = r.worker_id
         WHERE m.sender_kind = 'fold' AND w.name = 'review'
         ORDER BY m.number DESC LIMIT 1",
    )
    .fetch_one(scene.fx.handle.store.read())
    .await
    .unwrap()
}

#[tokio::test]
async fn a_fix_becomes_a_version_only_after_its_proposal_checks_pass() {
    let scene = scene_with_checks().await;
    scene.edit("src/lib.rs", "pub fn y() -> u8 { 1 }\n");
    let run = checked_fix(
        &scene,
        &[fix("src/lib.rs", "y returned nothing; it returns 1")],
    )
    .await;
    let view = scene.fx.reviews().get_check_run(run, false).await.unwrap();
    assert_eq!(view.run.state, fold_core::reviews::checks::RunState::Passed);
    let detail = fold_store::testing::eventually(std::time::Duration::from_secs(30), async || {
        let detail = scene.fx.reviews().get_proposal(p(1), None).await.unwrap();
        if detail.summary.current_version == 2 {
            Ok(detail)
        } else {
            Err(detail.summary.current_version)
        }
    })
    .await;
    assert_eq!(detail.versions[1].created_by, "review");
    assert_eq!(detail.versions[1].fixed_in_checkout, Some(ws(2)));
    assert_eq!(
        detail.versions[1].intent.as_deref(),
        Some("Makes y return 1, checked by the proposal checks."),
        "the run carries the fix's intent to its version"
    );
    assert!(
        last_message_to_review(&scene)
            .await
            .starts_with("fold made P-1 v2 for T-1: the checks of your fix passed")
    );
}

#[tokio::test]
async fn a_red_fix_adds_no_version_and_tells_the_reviewer() {
    let scene = scene_with_checks().await;
    scene.edit("src/lib.rs", "pub fn z() {}\n");
    let run = checked_fix(&scene, &[fix("src/lib.rs", "rename")]).await;
    let view = scene.fx.reviews().get_check_run(run, false).await.unwrap();
    assert_eq!(view.run.state, fold_core::reviews::checks::RunState::Failed);
    let detail = scene.fx.reviews().get_proposal(p(1), None).await.unwrap();
    assert_eq!(detail.summary.current_version, 1);
    let message = last_message_to_review(&scene).await;
    assert!(message.contains("lib failed (exit 1)"), "{message}");
    assert!(
        message.contains("fold added no fix version to P-1"),
        "{message}"
    );
    assert!(message.contains("submit_review_fix again"), "{message}");
}

/// `frontend` opens `P-1` behind a check that waits until `go` exists; `review` has the reviewer role.
#[cfg(test)]
async fn gated_scene(go: &std::path::Path) -> (Fixture, Actor) {
    let fx = Fixture::new().await;
    let frontend = fx.agent("frontend").await;
    let review = fx.agent("review").await;
    give_role(&fx.handle.store, "review", Role::Reviewer, None).await;
    fx.gated_proposal(&frontend, go).await;
    (fx, review)
}

#[tokio::test]
async fn a_running_fix_holds_approve_and_integrate_of_the_fixed_version_and_the_reviewer_approves_its_fix()
 {
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    let (fx, review) = gated_scene(&go).await;
    let (run, _) = fx.fix_p1(&review).await;
    let reviews = fx.reviews();
    let view = reviews.view_proposal(&review, p(1), None).await.unwrap();
    assert_eq!(
        view.update_run,
        Some(VersionRun {
            run,
            purpose: Purpose::ReviewFix
        })
    );
    for actor in [&review, &human("tanim")] {
        let held = refusal(
            reviews
                .submit_review(actor, p(1), 1, Verdict::Approve, "ok", Vec::new())
                .await,
        );
        assert_eq!(held.code(), codes::UPDATE_CHECKS_RUNNING);
        assert_eq!(details(&held)["run"], json!(run.to_string()));
        assert_eq!(details(&held)["purpose"], "review_fix");
        assert!(held.message().contains("reviewer's fix"), "{held:?}");
    }
    let integrate = refusal(
        reviews
            .integrate_proposal(&human("tanim"), p(1), Some(1))
            .await,
    );
    assert_eq!(integrate.code(), codes::UPDATE_CHECKS_RUNNING);
    assert_eq!(details(&integrate)["purpose"], "review_fix");
    reviews
        .submit_review(
            &human("tanim"),
            p(1),
            1,
            Verdict::Comment,
            "waiting",
            Vec::new(),
        )
        .await
        .unwrap();

    std::fs::write(&go, "").unwrap();
    let view = fold_store::testing::eventually(std::time::Duration::from_secs(30), async || {
        let view = reviews.view_proposal(&review, p(1), None).await.unwrap();
        if view.detail.summary.current_version == 2 {
            Ok(view)
        } else {
            Err(view.detail.summary.current_version)
        }
    })
    .await;
    assert_eq!(view.update_run, None);
    reviews
        .submit_review(
            &review,
            p(1),
            2,
            Verdict::Approve,
            "Fixes: src/lib.rs (y returns 1). cargo test: ok",
            Vec::new(),
        )
        .await
        .unwrap();
    // The checks file the proposal adds gives the task the human review mark, so the human integrates after the reviewer's approval.
    let outcome = reviews
        .integrate_proposal(&human("tanim"), p(1), Some(2))
        .await
        .unwrap();
    assert!(matches!(outcome, IntegrationOutcome::Succeeded { .. }));
}

#[tokio::test]
async fn a_version_approved_before_a_fix_integrates_on_its_own_once_the_fix_run_ends_without_a_version()
 {
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    let (fx, review) = gated_scene(&go).await;
    let reviews = fx.reviews();
    for actor in [&review, &human("tanim")] {
        reviews
            .submit_review(actor, p(1), 1, Verdict::Approve, "ok", Vec::new())
            .await
            .unwrap();
    }
    let (run, file) = fx.fix_p1(&review).await;
    assert!(reviews.integrate_ready().await.unwrap().is_empty());
    std::fs::write(&file, "pub fn y() -> u8 { 2 }\n").unwrap();
    std::fs::write(&go, "").unwrap();
    let ended = crate::checks::ended(&reviews, run).await;
    assert_eq!(ended.run.state, RunState::Failed);
    assert_eq!(ended.run.reason, Some(RunReason::FilesChanged));
    let view = reviews.view_proposal(&review, p(1), None).await.unwrap();
    assert_eq!(view.update_run, None);
    assert_eq!(view.detail.summary.current_version, 1);
    let done = reviews.integrate_ready().await.unwrap();
    assert_eq!(
        done.iter().map(|done| done.proposal).collect::<Vec<_>>(),
        [p(1)]
    );
}

#[tokio::test]
async fn a_batch_approval_of_a_member_waits_while_a_fix_of_it_runs_its_checks() {
    let gate = tempfile::tempdir().unwrap();
    let go = gate.path().join("go");
    let (fx, review) = gated_scene(&go).await;
    let reviews = fx.reviews();
    let backend = fx.agent("backend").await;
    fx.workspaces()
        .create_workspace(&backend, t(2))
        .await
        .unwrap();
    fx.write(2, "README.md", "# demo\nmore\n");
    reviews
        .create_proposal(
            &backend,
            t(2),
            "Extend the readme",
            "Make the change the task asks for.",
            Some("read it"),
        )
        .await
        .unwrap();
    let (run, _) = fx.fix_p1(&review).await;
    let batch = crate::common::roles::give_batch(
        &fx.handle.store,
        "review",
        &[p(1), p(2)],
        fx.handle.clock.now(),
    )
    .await;
    let approve = |proposal| BatchVerdict {
        proposal,
        verdict: Verdict::Approve,
        body: String::new(),
    };
    let held = refusal(
        reviews
            .submit_batch_review(
                &review,
                batch,
                vec![approve(p(1)), approve(p(2))],
                Vec::new(),
            )
            .await,
    );
    assert_eq!(held.code(), codes::UPDATE_CHECKS_RUNNING);
    assert_eq!(details(&held)["proposal"], "P-1");
    assert_eq!(details(&held)["purpose"], "review_fix");
    std::fs::write(&go, "").unwrap();
    crate::checks::ended(&reviews, run).await;
}
