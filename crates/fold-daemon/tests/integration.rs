#![cfg(test)]

use crate::common::roles::give_role;
use fold_core::roles::Role;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::reviews_support::{Fixture, LIB, by, details, human, names, p, refusal, t, ws};
use fold_core::object::{ObjectHash, ObjectKind};
use fold_core::reviews::Verdict;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree, Tree, flatten};
use fold_core::{Actor, FoldError, RevisionId, TaskStatus, codes};
use fold_daemon::services::reviews::{
    IntegrationOutcome, PullIntegration, PullOutcome, ReviewError,
};
use fold_store::workspaces;
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::write_initial_tree;

const COMMIT: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

/// What a refused or conflicted integration must leave alone.
#[derive(Debug, PartialEq, Eq)]
struct Untouched {
    head: RevisionId,
    revisions: i64,
    completions: i64,
    status: TaskStatus,
}

async fn untouched(fx: &Fixture) -> Untouched {
    Untouched {
        head: head(fx).await,
        revisions: fx.count("SELECT COUNT(*) FROM revisions").await,
        completions: fx.count("SELECT COUNT(*) FROM completions").await,
        status: fx.status(1).await,
    }
}

async fn head(fx: &Fixture) -> RevisionId {
    workspaces::head(fx.project.store().read())
        .await
        .unwrap()
        .unwrap()
        .number
}

/// Makes the object store's scratch folder read-only, so the next object write fails. `None` when this process can still write there, as root can, so the failure cannot be produced and the test has nothing to check.
fn block_object_writes(fx: &Fixture) -> Option<PathBuf> {
    let scratch = fx.project.paths().fold_dir().join("objects/tmp");
    std::fs::set_permissions(&scratch, Permissions::from_mode(0o500)).unwrap();
    let probe = scratch.join("probe");
    if std::fs::File::create(&probe).is_ok() {
        std::fs::remove_file(&probe).unwrap();
        unblock_object_writes(&scratch);
        eprintln!("skipped: a read-only folder does not stop this user from writing");
        return None;
    }
    Some(scratch)
}

fn unblock_object_writes(scratch: &Path) {
    std::fs::set_permissions(scratch, Permissions::from_mode(0o700)).unwrap();
}

fn objects(fx: &Fixture) -> ObjectStore {
    ObjectStore::open(&fx.project.paths().fold_dir()).unwrap()
}

/// The files of revision `revision` with their content.
async fn files_of(fx: &Fixture, revision: RevisionId) -> Vec<(String, String)> {
    let row = workspaces::get_by_number(fx.project.store().read(), revision)
        .await
        .unwrap();
    let objects = objects(fx);
    let load = |hash: &ObjectHash| -> Result<Tree, FoldError> {
        let object = objects.read_expecting(hash, ObjectKind::Tree)?;
        Ok(Tree::decode(&object.payload)?)
    };
    flatten(&row.tree_hash, &load)
        .unwrap()
        .into_iter()
        .filter(|(_, entry)| entry.kind == EntryKind::File)
        .map(|(path, entry)| {
            let content = objects
                .read_expecting(&entry.hash, ObjectKind::Blob)
                .unwrap()
                .payload;
            (
                String::from_utf8(path).unwrap(),
                String::from_utf8(content).unwrap(),
            )
        })
        .collect()
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> Option<&'a str> {
    files
        .iter()
        .find(|(name, _)| name == path)
        .map(|(_, content)| content.as_str())
}

/// A tree like `r1` with `README.md` replaced, as a pull brings it in.
fn remote_tree(fx: &Fixture, readme: &str) -> ObjectHash {
    let objects = objects(fx);
    let mut tree = FlatTree::new();
    for (path, content) in [("src/lib.rs", LIB), ("README.md", readme.as_bytes())] {
        tree.insert(
            path.as_bytes().to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash: objects.write(ObjectKind::Blob, content).unwrap(),
            },
        );
    }
    write_initial_tree(&objects, tree).unwrap()
}

/// A pull whose sync base is `r1`'s tree.
fn pull(fx: &Fixture, readme: &str) -> PullIntegration {
    PullIntegration {
        remote_commit: COMMIT.to_owned(),
        remote: "origin".to_owned(),
        branch: "main".to_owned(),
        base_tree: remote_tree(fx, "# demo\n"),
        remote_tree: remote_tree(fx, readme),
    }
}

/// `review` is the review lead and acts with it.
async fn review_lead(fx: &Fixture) -> Actor {
    give_role(fx.project.store(), "review", Role::ReviewLead, None).await;
    fx.agents.actor(&fx.review.ctx, None).await.unwrap()
}

async fn approve(fx: &Fixture, actor: &Actor, proposal: u32) {
    let read = fx.reviews.get_proposal(p(proposal), None).await.unwrap();
    fx.reviews
        .submit_review(
            actor,
            p(proposal),
            read.summary.current_version,
            Verdict::Approve,
            "looks right",
            Vec::new(),
        )
        .await
        .unwrap();
}

/// `P-1` approved by the review lead `review`, which acts with the returned actor.
async fn approved(fx: &Fixture) -> Actor {
    fx.proposal().await;
    let review = review_lead(fx).await;
    approve(fx, &review, 1).await;
    review
}

/// `backend` takes `T-2`, adds `docs/second.md` and opens `P-2`.
async fn second_proposal(fx: &Fixture) {
    let author = by(&fx.backend.worker);
    fx.workspaces.create_workspace(&author, t(2)).await.unwrap();
    let path = fx
        .project
        .paths()
        .root()
        .join(".fold/ws/t-2/docs/second.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "second\n").unwrap();
    fx.reviews
        .create_proposal(&author, t(2), "Add the second page", None)
        .await
        .unwrap();
}

fn succeeded(outcome: IntegrationOutcome) -> RevisionId {
    match outcome {
        IntegrationOutcome::Succeeded { revision } => revision,
        IntegrationOutcome::Conflicted(conflicts) => panic!("conflicted: {conflicts:?}"),
    }
}

fn code<T: std::fmt::Debug>(result: Result<T, ReviewError>) -> &'static str {
    match result {
        Err(error) => error.code().unwrap(),
        Ok(value) => panic!("expected an error, got {value:?}"),
    }
}

#[tokio::test]
async fn integrate_moves_main_and_completes_the_task_in_one_transaction() {
    let fx = Fixture::new().await;
    let review = approved(&fx).await;
    let seq = fx.last_seq().await;

    let revision = succeeded(
        fx.reviews
            .integrate_proposal(&review, p(1), Some(1))
            .await
            .unwrap(),
    );

    assert_eq!(revision, r(2));
    assert_eq!(head(&fx).await, r(2));
    let files = files_of(&fx, r(2)).await;
    assert_eq!(file(&files, "src/lib.rs"), Some("pub fn y() {}\n"));
    assert_eq!(file(&files, "src/new.rs"), Some("pub fn z() {}\n"));
    assert_eq!(fx.status(1).await, TaskStatus::Done);
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM completions c JOIN integrations i ON i.id = c.integration_id
             JOIN revisions r ON r.integration_id = i.id
             WHERE c.kind = 'integration' AND i.number = 1 AND i.result = 'succeeded'
               AND i.kind = 'proposal' AND i.actor = 'review' AND r.number = 2
               AND i.revision_id = r.id",
        )
        .await,
        1
    );
    assert_eq!(
        fx.text("SELECT state FROM proposals WHERE number = 1")
            .await
            .as_deref(),
        Some("integrated")
    );
    let events = fx.events_after(seq).await;
    let types = names(&events);
    for expected in [
        "integration.succeeded",
        "main.advanced",
        "proposal.integrated",
        "workspace.archived",
        "claim.released",
        "task.status_changed",
    ] {
        assert!(
            types.contains(&expected),
            "{expected} missing from {types:?}"
        );
    }
    let advanced = events
        .iter()
        .find(|event| event.event_type.as_str() == "main.advanced")
        .unwrap();
    assert_eq!(advanced.revision, Some(r(2)));
    assert_eq!(advanced.data["previous"], "r1");
    assert_eq!(advanced.data["revision"], "r2");
    // One transaction: every event of the integration has the same time.
    assert!(
        events
            .iter()
            .all(|event| event.created_at == events[0].created_at)
    );
}

#[tokio::test]
async fn integrate_archives_the_workspace_and_releases_claims() {
    let fx = Fixture::new().await;
    let review = approved(&fx).await;
    fx.reviews.claim_review(&review, p(1)).await.unwrap();
    let checkout = fx
        .reviews
        .create_review_checkout(&review, p(1))
        .await
        .unwrap();
    let checkout = checkout.workspace.id;

    succeeded(
        fx.reviews
            .integrate_proposal(&review, p(1), None)
            .await
            .unwrap(),
    );

    assert_eq!(
        fx.text("SELECT state FROM workspaces WHERE number = 1")
            .await
            .as_deref(),
        Some("archived")
    );
    let checkout_state = fx
        .text(&format!(
            "SELECT state FROM workspaces WHERE number = {}",
            checkout.number()
        ))
        .await;
    assert_eq!(checkout_state.as_deref(), Some("archived"));
    assert_ne!(checkout, ws(1));
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM claims WHERE released_at IS NULL")
            .await,
        0
    );
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM claims
             WHERE kind = 'implementation' AND release_reason = 'integrated'"
        )
        .await,
        1
    );
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM claims WHERE kind = 'review' AND release_reason = 'integrated'"
        )
        .await,
        1
    );
    assert!(fx.task_dir().join("src/new.rs").exists());
}

#[tokio::test]
async fn human_integrates_without_a_tag_and_the_approval_is_recorded() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    let seq = fx.last_seq().await;

    succeeded(
        fx.reviews
            .integrate_proposal(&human(), p(1), Some(1))
            .await
            .unwrap(),
    );

    assert_eq!(fx.status(1).await, TaskStatus::Done);
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM reviews
             WHERE verdict = 'approve' AND reviewer_human = 'tanim' AND body <> ''"
        )
        .await,
        1
    );
    assert_eq!(
        fx.text("SELECT actor FROM integrations").await.as_deref(),
        Some("human:tanim")
    );
    let events = fx.events_after(seq).await;
    let submitted = events
        .iter()
        .find(|event| event.event_type.as_str() == "review.submitted")
        .unwrap();
    assert_eq!(submitted.actor.as_str(), "human:tanim");
    assert_eq!(submitted.data["verdict"], "approve");
}

#[tokio::test]
async fn human_integrates_over_an_earlier_request_changes() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    let review = review_lead(&fx).await;
    fx.reviews
        .submit_review(
            &review,
            p(1),
            1,
            Verdict::RequestChanges,
            "rename it",
            Vec::new(),
        )
        .await
        .unwrap();

    succeeded(
        fx.reviews
            .integrate_proposal(&human(), p(1), None)
            .await
            .unwrap(),
    );

    assert_eq!(fx.status(1).await, TaskStatus::Done);
    assert_eq!(
        fx.text("SELECT verdict FROM reviews ORDER BY id DESC LIMIT 1")
            .await
            .as_deref(),
        Some("approve")
    );
}

#[tokio::test]
async fn a_human_who_already_approved_last_adds_no_second_approval() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    approve(&fx, &human(), 1).await;

    succeeded(
        fx.reviews
            .integrate_proposal(&human(), p(1), None)
            .await
            .unwrap(),
    );

    assert_eq!(fx.count("SELECT COUNT(*) FROM reviews").await, 1);
}

#[tokio::test]
async fn human_integration_is_refused_by_a_stale_expected_version() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    let before = untouched(&fx).await;

    let error = refusal(fx.reviews.integrate_proposal(&human(), p(1), Some(2)).await);

    assert_eq!(error.code(), codes::STALE_VERSION);
    assert_eq!(details(&error)["current_version"], 1);
    assert_eq!(untouched(&fx).await, before);
    assert_eq!(fx.count("SELECT COUNT(*) FROM integrations").await, 0);
    assert_eq!(fx.count("SELECT COUNT(*) FROM reviews").await, 0);
}

#[tokio::test]
async fn agent_cannot_integrate_its_own_proposal() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    give_role(fx.project.store(), "frontend", Role::ReviewLead, None).await;
    let author = fx.agents.actor(&fx.frontend.ctx, None).await.unwrap();
    let before = untouched(&fx).await;

    let error = refusal(fx.reviews.integrate_proposal(&author, p(1), None).await);

    assert_eq!(error.code(), codes::AUTHOR_CANNOT_INTEGRATE);
    assert_eq!(untouched(&fx).await, before);
    assert_eq!(fx.count("SELECT COUNT(*) FROM integrations").await, 0);
}

#[tokio::test]
async fn integrate_makes_dependents_todo() {
    let fx = Fixture::new().await;
    fx.tasks.add_dependency(&human(), t(2), t(1)).await.unwrap();
    let review = approved(&fx).await;
    assert_eq!(fx.status(2).await, TaskStatus::Backlog);

    succeeded(
        fx.reviews
            .integrate_proposal(&review, p(1), None)
            .await
            .unwrap(),
    );

    assert_eq!(fx.status(1).await, TaskStatus::Done);
    assert_eq!(fx.status(2).await, TaskStatus::Todo);
}

#[tokio::test]
async fn moved_main_with_a_clean_merge_integrates_without_new_review() {
    let fx = Fixture::new().await;
    let review = approved(&fx).await;
    let moved = fx
        .reviews
        .integrate_pull(&human(), pull(&fx, "# demo from the remote\n"))
        .await
        .unwrap();
    assert_eq!(moved, PullOutcome::Succeeded { revision: r(2) });

    let revision = succeeded(
        fx.reviews
            .integrate_proposal(&review, p(1), None)
            .await
            .unwrap(),
    );

    assert_eq!(revision, r(3));
    let files = files_of(&fx, r(3)).await;
    assert_eq!(file(&files, "README.md"), Some("# demo from the remote\n"));
    assert_eq!(file(&files, "src/lib.rs"), Some("pub fn y() {}\n"));
    assert_eq!(file(&files, "src/new.rs"), Some("pub fn z() {}\n"));
    assert_eq!(fx.count("SELECT COUNT(*) FROM reviews").await, 1);
    assert_eq!(fx.status(1).await, TaskStatus::Done);
}

#[tokio::test]
async fn conflicting_proposal_returns_conflicts_and_publishes_nothing() {
    let fx = Fixture::new().await;
    let review = approved(&fx).await;
    let mut moved = pull(&fx, "# demo\n");
    let objects = objects(&fx);
    let mut tree = FlatTree::new();
    for (path, content) in [
        ("src/lib.rs", "pub fn main_side() {}\n"),
        ("README.md", "# demo\n"),
    ] {
        tree.insert(
            path.as_bytes().to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash: objects.write(ObjectKind::Blob, content.as_bytes()).unwrap(),
            },
        );
    }
    moved.remote_tree = write_initial_tree(&objects, tree).unwrap();
    fx.reviews.integrate_pull(&human(), moved).await.unwrap();
    let before = untouched(&fx).await;

    let outcome = fx
        .reviews
        .integrate_proposal(&review, p(1), None)
        .await
        .unwrap();

    let IntegrationOutcome::Conflicted(conflicts) = outcome else {
        panic!("expected a conflict");
    };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, b"src/lib.rs");
    assert_eq!(
        untouched(&fx).await,
        Untouched {
            status: TaskStatus::InProgress,
            ..before
        }
    );
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM integrations WHERE result = 'conflicted'")
            .await,
        1
    );
}

/// Sets up `P-1` for one gate case and returns the caller; the case refuses with `code` and records a row when `recorded`.
struct GateCase {
    code: &'static str,
    recorded: bool,
}

async fn gate_case(fx: &Fixture, code: &str) -> (Actor, Option<u32>) {
    fx.proposal().await;
    let review = review_lead(fx).await;
    match code {
        codes::NOT_APPROVED => (review, None),
        codes::CHANGES_REQUESTED => {
            fx.reviews
                .submit_review(&review, p(1), 1, Verdict::RequestChanges, "no", Vec::new())
                .await
                .unwrap();
            (review, None)
        }
        codes::HUMAN_APPROVAL_MISSING => {
            approve(fx, &review, 1).await;
            fx.reviews
                .require_human_review(&review, t(1), None, "touches auth")
                .await
                .unwrap();
            (review, None)
        }
        codes::PREREQUISITES_NOT_DONE => {
            approve(fx, &review, 1).await;
            fx.tasks.add_dependency(&human(), t(1), t(2)).await.unwrap();
            (review, None)
        }
        codes::PROPOSAL_NOT_OPEN => {
            fx.reviews
                .close_proposal(&human(), p(1), "not needed")
                .await
                .unwrap();
            (review, None)
        }
        codes::STALE_VERSION => {
            approve(fx, &review, 1).await;
            (review, Some(3))
        }
        codes::INTEGRATOR_REQUIRED => {
            give_role(fx.project.store(), "backend", Role::TaskWorker, None).await;
            (by(&fx.backend.worker), None)
        }
        codes::AUTHOR_CANNOT_INTEGRATE => (by(&fx.frontend.worker), None),
        other => panic!("no case for {other}"),
    }
}

#[tokio::test]
async fn gate_refusals_record_a_refused_row_and_change_nothing() {
    let cases = [
        GateCase {
            code: codes::NOT_APPROVED,
            recorded: true,
        },
        GateCase {
            code: codes::CHANGES_REQUESTED,
            recorded: true,
        },
        GateCase {
            code: codes::HUMAN_APPROVAL_MISSING,
            recorded: true,
        },
        GateCase {
            code: codes::PREREQUISITES_NOT_DONE,
            recorded: true,
        },
        GateCase {
            code: codes::PROPOSAL_NOT_OPEN,
            recorded: false,
        },
        GateCase {
            code: codes::STALE_VERSION,
            recorded: false,
        },
        GateCase {
            code: codes::INTEGRATOR_REQUIRED,
            recorded: false,
        },
        GateCase {
            code: codes::AUTHOR_CANNOT_INTEGRATE,
            recorded: false,
        },
    ];
    for case in cases {
        let fx = Fixture::new().await;
        let (caller, expected) = gate_case(&fx, case.code).await;
        let before = untouched(&fx).await;
        let seq = fx.last_seq().await;

        let error = refusal(fx.reviews.integrate_proposal(&caller, p(1), expected).await);

        assert_eq!(error.code(), case.code);
        assert_eq!(untouched(&fx).await, before, "{}", case.code);
        let events = fx.events_after(seq).await;
        if case.recorded {
            assert_eq!(
                fx.count(&format!(
                    "SELECT COUNT(*) FROM integrations
                     WHERE result = 'refused' AND kind = 'proposal' AND revision_id IS NULL
                       AND json_extract(report, '$.code') = '{}'
                       AND json_extract(report, '$.proposal') = 'P-1'",
                    case.code
                ))
                .await,
                1,
                "{}",
                case.code
            );
            assert_eq!(names(&events), vec!["integration.refused"], "{}", case.code);
            assert_eq!(events[0].data["code"], case.code);
        } else {
            assert_eq!(
                fx.count("SELECT COUNT(*) FROM integrations").await,
                0,
                "{}",
                case.code
            );
            assert_eq!(events, Vec::new(), "{}", case.code);
        }
    }
}

#[tokio::test]
async fn head_cas_failure_retries_then_fails_with_head_moved() {
    let fx = Fixture::new().await;
    let review = approved(&fx).await;
    fx.reviews
        .integrate_pull(&human(), pull(&fx, "# moved\n"))
        .await
        .unwrap();
    // Every integration number taken after the merge moves main between r1 and r2, as another writer would.
    fx.execute(
        "CREATE TRIGGER move_main AFTER UPDATE ON counters WHEN NEW.kind = 'integration'
         BEGIN
             UPDATE refs SET revision_id = CASE revision_id
                 WHEN (SELECT id FROM revisions WHERE number = 1) THEN (SELECT id FROM revisions WHERE number = 2)
                 ELSE (SELECT id FROM revisions WHERE number = 1) END
             WHERE name = 'main';
         END",
    )
    .await;
    let revisions = fx.count("SELECT COUNT(*) FROM revisions").await;

    let error = refusal(fx.reviews.integrate_proposal(&review, p(1), None).await);

    assert_eq!(error.code(), codes::HEAD_MOVED);
    assert_eq!(fx.count("SELECT COUNT(*) FROM revisions").await, revisions);
    assert_eq!(fx.count("SELECT COUNT(*) FROM completions").await, 0);
    assert_eq!(fx.status(1).await, TaskStatus::Review);
    assert_eq!(fx.count("SELECT COUNT(*) FROM integrations").await, 1);
    // The pull took i-1 and the three attempts took i-2 to i-4.
    assert_eq!(
        fx.count("SELECT next FROM counters WHERE kind = 'integration'")
            .await,
        5
    );
}

#[tokio::test]
async fn two_integrations_run_one_at_a_time() {
    let fx = Fixture::new().await;
    let review = approved(&fx).await;
    second_proposal(&fx).await;
    approve(&fx, &review, 2).await;

    let held = fx.integration.lock().await;
    let service = fx.service();
    let actor = review.clone();
    let mut waiting =
        tokio::spawn(async move { service.integrate_proposal(&actor, p(1), None).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut waiting)
            .await
            .is_err()
    );
    assert_eq!(head(&fx).await, r(1));
    drop(held);
    succeeded(waiting.await.unwrap().unwrap());

    // Two calls for P-2 through two services: the second runs after the first committed and finds it integrated.
    let (one, two) = (fx.service(), fx.service());
    let (a, b) = tokio::join!(
        one.integrate_proposal(&review, p(2), None),
        two.integrate_proposal(&review, p(2), None),
    );
    let outcomes = [a, b];
    let landed = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
    assert_eq!(landed, 1);
    let refused: Vec<&str> = outcomes
        .iter()
        .filter_map(|outcome| outcome.as_ref().err().and_then(ReviewError::code))
        .collect();
    assert_eq!(refused, vec![codes::PROPOSAL_NOT_OPEN]);
    assert_eq!(head(&fx).await, r(3));
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM integrations WHERE result = 'succeeded'")
            .await,
        2
    );
    assert_eq!(fx.status(2).await, TaskStatus::Done);
}

#[tokio::test]
async fn failed_integration_never_completes_a_task() {
    let fx = Fixture::new().await;
    let review = approved(&fx).await;
    let before = untouched(&fx).await;
    let Some(scratch) = block_object_writes(&fx) else {
        return;
    };

    let result = fx.reviews.integrate_proposal(&review, p(1), None).await;

    unblock_object_writes(&scratch);
    let error = refusal(result);
    assert_eq!(error.code(), codes::INTEGRATION_FAILED);
    assert_eq!(untouched(&fx).await, before);
    assert_eq!(fx.count("SELECT COUNT(*) FROM integrations").await, 0);
    assert_eq!(
        fx.text("SELECT state FROM proposals WHERE number = 1")
            .await
            .as_deref(),
        Some("open")
    );
}

#[tokio::test]
async fn integration_number_is_not_reused_after_a_failure() {
    let fx = Fixture::new().await;
    let review = approved(&fx).await;
    let Some(scratch) = block_object_writes(&fx) else {
        return;
    };
    let failed = fx.reviews.integrate_proposal(&review, p(1), None).await;
    unblock_object_writes(&scratch);
    assert_eq!(code(failed), codes::INTEGRATION_FAILED);

    succeeded(
        fx.reviews
            .integrate_proposal(&review, p(1), None)
            .await
            .unwrap(),
    );

    assert_eq!(
        fx.count(
            "SELECT i.number FROM integrations i JOIN revisions r ON r.integration_id = i.id
             WHERE r.number = 2"
        )
        .await,
        2
    );
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM integrations WHERE number = 1")
            .await,
        0
    );
}

#[tokio::test]
async fn pull_integration_moves_main_without_a_proposal_task_or_review() {
    let fx = Fixture::new().await;
    let request = pull(&fx, "# pulled\n");
    let tree = request.remote_tree;
    let statuses = (fx.status(1).await, fx.status(2).await);

    let outcome = fx.reviews.integrate_pull(&human(), request).await.unwrap();

    assert_eq!(outcome, PullOutcome::Succeeded { revision: r(2) });
    assert_eq!(head(&fx).await, r(2));
    let row = workspaces::get_by_number(fx.project.store().read(), r(2))
        .await
        .unwrap();
    assert_eq!(row.tree_hash, tree);
    assert_eq!(
        file(&files_of(&fx, r(2)).await, "README.md"),
        Some("# pulled\n")
    );
    assert_eq!(fx.count("SELECT COUNT(*) FROM completions").await, 0);
    assert_eq!(fx.count("SELECT COUNT(*) FROM reviews").await, 0);
    assert_eq!(fx.count("SELECT COUNT(*) FROM proposals").await, 0);
    assert_eq!((fx.status(1).await, fx.status(2).await), statuses);
}

#[tokio::test]
async fn pull_integration_records_kind_pull_the_remote_commit_and_remote_pulled() {
    let fx = Fixture::new().await;
    let seq = fx.last_seq().await;

    fx.reviews
        .integrate_pull(&human(), pull(&fx, "# pulled\n"))
        .await
        .unwrap();

    assert_eq!(
        fx.count(&format!(
            "SELECT COUNT(*) FROM integrations i JOIN revisions r ON r.id = i.revision_id
             WHERE i.kind = 'pull' AND i.result = 'succeeded' AND i.remote_commit = '{COMMIT}'
               AND i.proposal_id IS NULL AND i.version_id IS NULL AND i.actor = 'human:tanim'
               AND r.number = 2 AND r.integration_id = i.id"
        ))
        .await,
        1
    );
    let events = fx.events_after(seq).await;
    assert_eq!(
        names(&events),
        vec!["integration.succeeded", "main.advanced", "remote.pulled"]
    );
    assert_eq!(
        events[2].data,
        serde_json::json!({"remote": "origin", "branch": "main", "remote_commit": COMMIT, "revision": "r2"})
    );
    assert_eq!(events[2].revision, Some(r(2)));
    assert_eq!(events[1].data["previous"], "r1");
}

#[tokio::test]
async fn pull_integration_is_refused_for_an_agent_with_human_required() {
    let fx = Fixture::new().await;
    let review = review_lead(&fx).await;
    let request = pull(&fx, "# pulled\n");

    let error = refusal(fx.reviews.integrate_pull(&review, request).await);

    assert_eq!(error.code(), codes::HUMAN_REQUIRED);
    assert_eq!(head(&fx).await, r(1));
    assert_eq!(fx.count("SELECT COUNT(*) FROM integrations").await, 0);
}

#[tokio::test]
async fn pull_integration_refuses_a_remote_commit_that_is_not_an_object_id() {
    let fx = Fixture::new().await;
    let mut request = pull(&fx, "# pulled\n");
    request.remote_commit = "-x".to_owned();

    let error = refusal(fx.reviews.integrate_pull(&human(), request).await);

    assert_eq!(error.code(), codes::INVALID_REQUEST);
    assert_eq!(fx.count("SELECT COUNT(*) FROM integrations").await, 0);
}

#[tokio::test]
async fn conflicting_pull_integration_returns_conflicts_and_writes_nothing() {
    let fx = Fixture::new().await;
    fx.reviews
        .integrate_pull(&human(), pull(&fx, "# first\n"))
        .await
        .unwrap();
    let before = untouched(&fx).await;
    let seq = fx.last_seq().await;

    let outcome = fx
        .reviews
        .integrate_pull(&human(), pull(&fx, "# second\n"))
        .await
        .unwrap();

    let PullOutcome::Conflicted(conflicts) = outcome else {
        panic!("expected a conflict");
    };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, b"README.md");
    assert_eq!(untouched(&fx).await, before);
    assert_eq!(fx.count("SELECT COUNT(*) FROM integrations").await, 1);
    assert_eq!(fx.events_after(seq).await, Vec::new());
}

#[tokio::test]
async fn pull_and_proposal_integrations_run_one_at_a_time() {
    let fx = Fixture::new().await;
    let review = approved(&fx).await;
    let request = pull(&fx, "# pulled\n");
    let (proposals, pulls) = (fx.service(), fx.service());
    let tanim = human();

    let (proposal, pulled) = tokio::join!(
        proposals.integrate_proposal(&review, p(1), None),
        pulls.integrate_pull(&tanim, request),
    );

    succeeded(proposal.unwrap());
    assert!(matches!(pulled.unwrap(), PullOutcome::Succeeded { .. }));
    assert_eq!(head(&fx).await, r(3));
    let files = files_of(&fx, r(3)).await;
    assert_eq!(file(&files, "README.md"), Some("# pulled\n"));
    assert_eq!(file(&files, "src/new.rs"), Some("pub fn z() {}\n"));
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM integrations WHERE result = 'succeeded'")
            .await,
        2
    );
}
