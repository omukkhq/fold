#![cfg(test)]

use crate::common::intents::Covering as _;
use crate::common::roles::{give_review_claim, give_role};
use crate::reviews_support::{Fixture, LIB, README, by, human, names, p, refusal, t, ws};
use fold_core::events::types;
use fold_core::merge::{Conflict, ConflictReport, ReportEntryKind, ReportKind};
use fold_core::object::ObjectKind;
use fold_core::reviews::Verdict;
use fold_core::roles::Role;
use fold_core::tree::{EntryKind, FlatEntry, FlatTree};
use fold_core::{Actor, RevisionId, TaskStatus, codes};
use fold_daemon::services::reviews::{
    IntegrationOutcome, ProposalDetail, PullIntegration, PullOutcome, Turn,
};
use fold_store::reviews::ProposalState;
use fold_store::workspaces::{self, RevisionRow};
use fold_vcs::objects::ObjectStore;
use fold_vcs::revision_store::write_initial_tree;
use serde_json::json;

const COMMIT: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const MAIN_LIB: &str = "pub fn main_side() {}\n";
const RESOLVED_LIB: &str = "pub fn y() {}\npub fn main_side() {}\n";

fn r(number: u32) -> RevisionId {
    RevisionId::from_number(number).unwrap()
}

async fn head(fx: &Fixture) -> RevisionRow {
    workspaces::head(fx.project.store().read())
        .await
        .unwrap()
        .unwrap()
}

/// Moves main to the next revision holding exactly `files`, as a pull of a remote commit based on `r1` brings them in.
async fn move_main(fx: &Fixture, files: &[(&str, &[u8])]) {
    let objects = ObjectStore::open(&fx.project.paths().fold_dir()).unwrap();
    let mut tree = FlatTree::new();
    for (path, content) in files {
        tree.insert(
            path.as_bytes().to_vec(),
            FlatEntry {
                kind: EntryKind::File,
                hash: objects.write(ObjectKind::Blob, content).unwrap(),
            },
        );
    }
    let pull = PullIntegration {
        remote_commit: COMMIT.to_owned(),
        remote: "origin".to_owned(),
        branch: "main".to_owned(),
        base_tree: workspaces::get_by_number(fx.project.store().read(), r(1))
            .await
            .unwrap()
            .tree_hash,
        remote_tree: write_initial_tree(&objects, tree).unwrap(),
    };
    let outcome = fx.reviews.integrate_pull(&human(), pull).await.unwrap();
    assert!(matches!(outcome, PullOutcome::Succeeded { .. }));
    fx.checks.wait_idle().await;
}

/// `review` is a reviewer and acts with it.
async fn reviewer(fx: &Fixture) -> Actor {
    give_role(fx.project.store(), "review", Role::Reviewer, None).await;
    fx.agents.actor(&fx.review.ctx).await.unwrap()
}

async fn approve(fx: &Fixture, actor: &Actor) {
    let version = fx.reviews.get_proposal(p(1), None).await.unwrap();
    let version = version.summary.current_version;
    fx.reviews
        .submit_review(
            actor,
            p(1),
            version,
            Verdict::Approve,
            "looks right",
            Vec::new(),
        )
        .await
        .unwrap();
}

/// `P-1` approved by the reviewer `review`, which holds its review claim, with main moved to `r2` so that its `src/lib.rs` conflicts. main moves, and its test-merge of the queue ends, before the approval, so the integration the test calls is the one that finds the conflict.
async fn conflicting(fx: &Fixture) -> Actor {
    fx.proposal().await;
    let review = reviewer(fx).await;
    claim_for_review(fx).await;
    move_main(
        fx,
        &[("src/lib.rs", MAIN_LIB.as_bytes()), ("README.md", README)],
    )
    .await;
    approve(fx, &review).await;
    review
}

/// Registers `review-lead` as the project's review lead.
async fn review_lead(fx: &Fixture) {
    crate::reviews_support::register(&fx.agents, "review-lead", &[]).await;
    give_role(fx.project.store(), "review-lead", Role::ReviewLead, None).await;
}

async fn claim_for_review(fx: &Fixture) {
    use fold_core::Clock as _;
    give_review_claim(fx.project.store(), "review", p(1), fx.project.clock().now()).await;
}

async fn integrate_conflicted(fx: &Fixture, review: &Actor) -> Vec<Conflict> {
    match fx
        .reviews
        .integrate_proposal(review, p(1), None)
        .await
        .unwrap()
    {
        IntegrationOutcome::Conflicted(conflicts) => conflicts,
        IntegrationOutcome::Succeeded { revision } => panic!("integrated as {revision}"),
        other => panic!("expected a conflict, got {other:?}"),
    }
}

async fn detail(fx: &Fixture, version: Option<u32>) -> ProposalDetail {
    fx.reviews.get_proposal(p(1), version).await.unwrap()
}

/// The author syncs `ws-1`, resolves `src/lib.rs` and sends `v2`.
async fn resolve_and_update(fx: &Fixture) -> ProposalDetail {
    let author = by(&fx.frontend.worker);
    fx.workspaces.sync_workspace(&author, ws(1)).await.unwrap();
    fx.write("src/lib.rs", RESOLVED_LIB);
    fx.reviews
        .update_proposal_covering(&author, p(1), None)
        .await
        .unwrap()
        .detail()
        .unwrap()
}

#[tokio::test]
async fn conflicting_integration_publishes_nothing() {
    let fx = Fixture::new().await;
    let review = conflicting(&fx).await;
    let before = head(&fx).await;
    let revisions = fx.count("SELECT COUNT(*) FROM revisions").await;

    let conflicts = integrate_conflicted(&fx, &review).await;

    assert_eq!(conflicts.len(), 1);
    let after = head(&fx).await;
    assert_eq!((after.number, after.hash), (before.number, before.hash));
    assert_eq!(fx.count("SELECT COUNT(*) FROM revisions").await, revisions);
    assert_eq!(fx.count("SELECT COUNT(*) FROM completions").await, 0);
    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM integrations WHERE kind = 'proposal' AND result = 'succeeded'"
        )
        .await,
        0
    );
    assert_eq!(detail(&fx, None).await.summary.state, ProposalState::Open);
}

#[tokio::test]
async fn conflict_marks_the_proposal_and_keeps_the_task_in_review_with_the_reviewer() {
    let fx = Fixture::new().await;
    let review = conflicting(&fx).await;
    assert_eq!(fx.status(1).await, TaskStatus::Review);

    integrate_conflicted(&fx, &review).await;

    let summary = detail(&fx, None).await.summary;
    assert!(summary.conflicted);
    assert_eq!(summary.turn, Some(Turn::WithReviewer));
    assert_eq!(summary.current_version, 1);
    assert_eq!(fx.status(1).await, TaskStatus::Review);
}

#[tokio::test]
async fn conflict_writes_one_integrations_row_with_the_report() {
    let fx = Fixture::new().await;
    let review = conflicting(&fx).await;
    let main = head(&fx).await;
    let seq = fx.last_seq().await;

    integrate_conflicted(&fx, &review).await;

    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM integrations i
             JOIN proposals p ON p.id = i.proposal_id
             JOIN proposal_versions v ON v.id = i.version_id
             WHERE i.result = 'conflicted' AND i.kind = 'proposal' AND i.number = 2
               AND i.revision_id IS NULL AND i.actor = 'review'
               AND p.number = 1 AND v.version = 1",
        )
        .await,
        1
    );
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM integrations WHERE result = 'conflicted'")
            .await,
        1
    );
    let stored = fx
        .text("SELECT report FROM integrations WHERE result = 'conflicted'")
        .await
        .unwrap();
    let report: ConflictReport = serde_json::from_str(&stored).unwrap();
    let v1 = detail(&fx, None).await.selected;
    let r1 = workspaces::get_by_number(fx.project.store().read(), r(1))
        .await
        .unwrap();
    assert_eq!((report.proposal, report.version), (p(1), 1));
    assert_eq!((report.base_revision, report.main_revision), (r(1), r(2)));
    assert_eq!(report.trees.base, r1.tree_hash);
    assert_eq!(report.trees.main, main.tree_hash);
    assert_eq!(report.trees.version, v1.tree);
    assert_eq!(report.files.len(), 1);
    let file = &report.files[0];
    assert_eq!(
        (file.path.as_str(), file.kind),
        ("src/lib.rs", ReportKind::Text)
    );
    assert_eq!(file.hunks.len(), 1);
    let hunk = &file.hunks[0];
    assert_eq!(hunk.base.lines, [String::from_utf8(LIB.to_vec()).unwrap()]);
    assert_eq!(hunk.main.lines, [MAIN_LIB]);
    assert_eq!(hunk.version.lines, ["pub fn y() {}\n"]);
    assert_eq!(
        (hunk.base.start, hunk.main.start, hunk.version.start),
        (1, 1, 1)
    );

    let events = fx.events_after(seq).await;
    assert_eq!(names(&events), [types::INTEGRATION_CONFLICTED.as_str()]);
    let conflicted = &events[0];
    assert_eq!(conflicted.task, Some(t(1)));
    assert_eq!(conflicted.proposal, Some(p(1)));
    assert_eq!(conflicted.revision, Some(r(2)));
    assert_eq!(
        conflicted.data,
        json!({
            "integration": "i-2",
            "proposal": "P-1",
            "version": 1,
            "main_revision": "r2",
            "files": 1,
            "fixer": "review",
            "lane": null,
            "before_turn": false,
        })
    );
    assert!(
        events
            .iter()
            .all(|event| event.created_at == conflicted.created_at)
    );
}

#[tokio::test]
async fn the_approving_reviewer_takes_the_conflict_and_nobody_gets_a_message() {
    let fx = Fixture::new().await;
    review_lead(&fx).await;
    let review = conflicting(&fx).await;

    integrate_conflicted(&fx, &review).await;

    assert_eq!(fx.count("SELECT COUNT(*) FROM messages").await, 0);
}

/// A reviewer that approved the version but lost its review claim cannot fix the conflict, so the review lead gets a notice from fold with the paths and no content; the author gets nothing.
#[tokio::test]
async fn without_the_review_claim_the_review_lead_gets_a_notice_with_paths_only() {
    let fx = Fixture::new().await;
    review_lead(&fx).await;
    fx.proposal().await;
    let review = reviewer(&fx).await;
    approve(&fx, &review).await;
    move_main(
        &fx,
        &[("src/lib.rs", MAIN_LIB.as_bytes()), ("README.md", README)],
    )
    .await;

    integrate_conflicted(&fx, &review).await;

    assert_eq!(
        fx.count(
            "SELECT COUNT(*) FROM messages m
             JOIN tasks t ON t.id = m.task_id
             JOIN proposals p ON p.id = m.proposal_id
             JOIN message_recipients mr ON mr.message_id = m.id
             JOIN workers w ON w.id = mr.worker_id
             WHERE m.sender_kind = 'fold' AND m.sender_worker_id IS NULL
               AND t.number = 1 AND p.number = 1
               AND w.name = 'review-lead' AND mr.addressed_as = 'agent'",
        )
        .await,
        1
    );
    assert_eq!(fx.count("SELECT COUNT(*) FROM message_recipients").await, 1);
    let body = fx.text("SELECT body FROM messages").await.unwrap();
    for wanted in [
        "P-1 v1 for T-1 conflicts with main at r2",
        "No reviewer that approved v1 holds the review claim of P-1",
        "(1): src/lib.rs.",
        "start_worker",
        "create_review_checkout",
    ] {
        assert!(body.contains(wanted), "{wanted:?} missing from {body:?}");
    }
    for content in ["main_side", "pub fn", "<<<<<<<"] {
        assert!(!body.contains(content), "{content:?} in {body:?}");
    }
}

#[tokio::test]
async fn get_proposal_returns_the_conflict_for_the_current_version_only() {
    let fx = Fixture::new().await;
    let review = conflicting(&fx).await;
    assert_eq!(detail(&fx, None).await.conflict, None);

    integrate_conflicted(&fx, &review).await;
    let report = detail(&fx, None).await.conflict.unwrap();
    assert_eq!((report.proposal, report.version), (p(1), 1));
    assert_eq!(report.files[0].path, "src/lib.rs");

    resolve_and_update(&fx).await;
    assert_eq!(detail(&fx, None).await.conflict, None);
    assert_eq!(detail(&fx, Some(2)).await.conflict, None);
    assert_eq!(detail(&fx, Some(1)).await.conflict, Some(report));
}

#[tokio::test]
async fn sync_resolve_and_update_proposal_return_the_task_to_review() {
    let fx = Fixture::new().await;
    let review = conflicting(&fx).await;
    integrate_conflicted(&fx, &review).await;

    let updated = resolve_and_update(&fx).await;

    assert_eq!(updated.summary.current_version, 2);
    assert!(!updated.summary.conflicted);
    assert_eq!(updated.summary.turn, Some(Turn::WithReviewer));
    assert_eq!(updated.selected.base, r(2));
    assert_eq!(updated.versions.len(), 2);
    assert_eq!(fx.status(1).await, TaskStatus::Review);
    assert_eq!(fx.count("SELECT COUNT(*) FROM reviews").await, 1);
}

#[tokio::test]
async fn update_proposal_refuses_while_markers_remain() {
    let fx = Fixture::new().await;
    let review = conflicting(&fx).await;
    integrate_conflicted(&fx, &review).await;
    let author = by(&fx.frontend.worker);

    let synced = fx.workspaces.sync_workspace(&author, ws(1)).await.unwrap();
    assert_eq!(synced.conflicted, ["src/lib.rs"]);
    let marked = std::fs::read_to_string(fx.task_dir().join("src/lib.rs")).unwrap();
    assert!(marked.contains("<<<<<<< workspace"));

    let error = refusal(
        fx.reviews
            .update_proposal_covering(&author, p(1), None)
            .await,
    );

    assert_eq!(error.code(), codes::UNRESOLVED_CONFLICTS);
    let summary = detail(&fx, None).await.summary;
    assert_eq!(summary.current_version, 1);
    assert!(summary.conflicted);
    assert_eq!(fx.status(1).await, TaskStatus::Review);
}

#[tokio::test]
async fn a_new_version_needs_a_new_approval() {
    let fx = Fixture::new().await;
    let review = conflicting(&fx).await;
    integrate_conflicted(&fx, &review).await;
    resolve_and_update(&fx).await;

    let error = refusal(fx.reviews.integrate_proposal(&review, p(1), Some(2)).await);
    assert_eq!(error.code(), codes::NOT_APPROVED);
    assert_eq!(fx.status(1).await, TaskStatus::Review);

    approve(&fx, &review).await;
    let outcome = fx
        .reviews
        .integrate_proposal(&review, p(1), Some(2))
        .await
        .unwrap();

    assert_eq!(outcome, IntegrationOutcome::Succeeded { revision: r(3) });
    assert_eq!(fx.status(1).await, TaskStatus::Done);
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM integrations WHERE result = 'conflicted'")
            .await,
        1
    );
    assert!(detail(&fx, Some(1)).await.conflict.is_some());
}

#[tokio::test]
async fn binary_and_delete_versus_edit_conflicts_are_reported_by_kind() {
    let fx = Fixture::new().await;
    let author = by(&fx.frontend.worker);
    fx.workspaces.create_workspace(&author, t(1)).await.unwrap();
    fx.write("src/lib.rs", "pub fn y() {}\n");
    fx.write("src/new.rs", "pub fn version() {}\n");
    std::fs::write(fx.task_dir().join("README.md"), b"\0version").unwrap();
    fx.reviews
        .create_proposal_covering(&author, t(1), "Change every file", None)
        .await
        .unwrap()
        .detail()
        .unwrap();
    let review = reviewer(&fx).await;
    claim_for_review(&fx).await;
    approve(&fx, &review).await;
    move_main(
        &fx,
        &[
            ("README.md", b"\0main".as_slice()),
            ("src/new.rs", b"pub fn main() {}\n".as_slice()),
        ],
    )
    .await;

    let conflicts = integrate_conflicted(&fx, &review).await;

    assert_eq!(conflicts.len(), 3);
    let report = detail(&fx, None).await.conflict.unwrap();
    let kinds: Vec<(&str, ReportKind)> = report
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("README.md", ReportKind::BothModifiedBinary),
            ("src/lib.rs", ReportKind::ModifyDelete),
            ("src/new.rs", ReportKind::AddAdd),
        ]
    );
    let sides = |index: usize| {
        let file = &report.files[index];
        (
            file.base.map(|entry| entry.kind),
            file.main.map(|entry| entry.kind),
            file.version.map(|entry| entry.kind),
        )
    };
    let file = Some(ReportEntryKind::File);
    assert_eq!(sides(0), (file, file, file));
    assert_eq!(sides(1), (file, None, file));
    assert_eq!(sides(2), (None, file, file));
    assert!(report.files.iter().all(|file| file.hunks.is_empty()));
    assert_eq!(fx.count("SELECT COUNT(*) FROM messages").await, 0);
    assert_eq!(fx.status(1).await, TaskStatus::Review);
}

/// A proposal closed after the merge found the conflict and before it was recorded makes the record step write nothing and the procedure start again, whose open gate then refuses. No conflicted row, flag, message or event is left behind, and main stays where it was.
#[cfg(feature = "failpoints")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proposal_closed_before_the_conflict_is_recorded_records_nothing() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    const FAILPOINT: &str = "integration_before_conflict_record";
    let fx = Fixture::new().await;
    let review = conflicting(&fx).await;
    let before = head(&fx).await;
    let scenario = fail::FailScenario::setup();
    let reached = Arc::new(AtomicBool::new(false));
    let open = Arc::new(AtomicBool::new(false));
    let (held_reached, held_open) = (reached.clone(), open.clone());
    fail::cfg_callback(FAILPOINT, move || {
        held_reached.store(true, Ordering::SeqCst);
        while !held_open.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
    })
    .unwrap();
    let reviews = fx.reviews.clone();
    let integrate =
        tokio::spawn(async move { reviews.integrate_proposal(&review, p(1), None).await });
    tokio::time::timeout(Duration::from_secs(20), async {
        while !reached.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    fx.reviews
        .close_proposal(&by(&fx.frontend.worker), p(1), "starting over")
        .await
        .unwrap();
    let seq = fx.last_seq().await;
    open.store(true, Ordering::SeqCst);
    let error = refusal(integrate.await.unwrap());
    fail::remove(FAILPOINT);
    scenario.teardown();

    assert_eq!(error.code(), codes::PROPOSAL_NOT_OPEN);
    assert_eq!(fx.last_seq().await, seq);
    let after = head(&fx).await;
    assert_eq!((after.number, after.hash), (before.number, before.hash));
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM integrations WHERE result = 'conflicted'")
            .await,
        0
    );
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM proposals WHERE conflicted = 1")
            .await,
        0
    );
    assert_eq!(fx.count("SELECT COUNT(*) FROM messages").await, 0);
    assert_eq!(detail(&fx, None).await.conflict, None);
}

/// Changes requested after the merge found the conflict and before it was recorded make the record step, which reads the gates again, write nothing, and the procedure starts again, whose gate step then refuses with `changes_requested`. No conflicted row, flag, message or event is left behind, and main stays where it was.
#[cfg(feature = "failpoints")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changes_requested_before_the_conflict_is_recorded_record_nothing() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    const FAILPOINT: &str = "integration_before_conflict_record";
    let fx = Fixture::new().await;
    let review = conflicting(&fx).await;
    let before = head(&fx).await;
    let scenario = fail::FailScenario::setup();
    let reached = Arc::new(AtomicBool::new(false));
    let open = Arc::new(AtomicBool::new(false));
    let (held_reached, held_open) = (reached.clone(), open.clone());
    fail::cfg_callback(FAILPOINT, move || {
        held_reached.store(true, Ordering::SeqCst);
        while !held_open.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
    })
    .unwrap();
    let reviews = fx.reviews.clone();
    let reviewer = review.clone();
    let integrate =
        tokio::spawn(async move { reviews.integrate_proposal(&review, p(1), None).await });
    tokio::time::timeout(Duration::from_secs(20), async {
        while !reached.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    fx.reviews
        .submit_review(
            &reviewer,
            p(1),
            1,
            Verdict::RequestChanges,
            "one more change",
            Vec::new(),
        )
        .await
        .unwrap();
    let seq = fx.last_seq().await;
    open.store(true, Ordering::SeqCst);
    let error = refusal(integrate.await.unwrap());
    fail::remove(FAILPOINT);
    scenario.teardown();

    assert_eq!(error.code(), codes::CHANGES_REQUESTED);
    assert_eq!(fx.last_seq().await, seq + 1, "only the refusal is recorded");
    let last: String = sqlx::query_scalar("SELECT type FROM events ORDER BY seq DESC LIMIT 1")
        .fetch_one(fx.project.store().read())
        .await
        .unwrap();
    assert_eq!(last, "integration.refused");
    let after = head(&fx).await;
    assert_eq!((after.number, after.hash), (before.number, before.hash));
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM integrations WHERE result = 'conflicted'")
            .await,
        0
    );
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM proposals WHERE conflicted = 1")
            .await,
        0
    );
    assert_eq!(fx.count("SELECT COUNT(*) FROM messages").await, 0);
    assert_eq!(detail(&fx, None).await.conflict, None);
}

/// `P-1` is approved before main moves, so the test-merge of the queue after the move finds its conflict before its turn and records it as fold, with `before_turn`; the integration a caller then asks for answers with the same conflict and records nothing more.
#[tokio::test]
async fn a_conflict_found_before_the_turn_is_recorded_once() {
    let fx = Fixture::new().await;
    fx.proposal().await;
    let review = reviewer(&fx).await;
    claim_for_review(&fx).await;
    approve(&fx, &review).await;
    let seq = fx.last_seq().await;
    move_main(
        &fx,
        &[("src/lib.rs", MAIN_LIB.as_bytes()), ("README.md", README)],
    )
    .await;

    let conflicted = || async {
        fx.events_after(seq)
            .await
            .into_iter()
            .filter(|event| event.event_type == types::INTEGRATION_CONFLICTED)
            .collect::<Vec<_>>()
    };
    let found = conflicted().await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].data["before_turn"], true);
    assert_eq!(found[0].data["fixer"], "review");
    assert_eq!(found[0].data["main_revision"], "r2");
    let summary = detail(&fx, None).await;
    assert!(summary.summary.conflicted);
    let report = summary.conflict.unwrap();
    let hunk = &report.files[0].hunks[0];
    assert_eq!(hunk.main.lines, [MAIN_LIB]);
    assert_eq!(hunk.version.lines, ["pub fn y() {}\n"]);

    let conflicts = integrate_conflicted(&fx, &review).await;
    assert_eq!(conflicts.len(), 1);
    assert_eq!(
        conflicted().await.len(),
        1,
        "the integration records nothing more"
    );
    assert_eq!(
        fx.count("SELECT COUNT(*) FROM integrations WHERE result = 'conflicted'")
            .await,
        1
    );
}

/// A test-merge asked for onto a main that moved on records nothing; onto the current main it finds the approved version's conflict.
#[tokio::test]
async fn the_test_merge_onto_a_stale_main_records_nothing() {
    let fx = Fixture::new().await;
    let review = conflicting(&fx).await;
    let seq = fx.last_seq().await;

    let stale = fx.reviews.find_queue_conflicts(r(1)).await.unwrap();
    assert!(stale.is_empty());
    assert!(fx.events_after(seq).await.is_empty());
    assert!(!detail(&fx, None).await.summary.conflicted);

    let found = fx.reviews.find_queue_conflicts(r(2)).await.unwrap();
    assert_eq!(found, [p(1)]);
    assert_eq!(
        names(&fx.events_after(seq).await),
        [types::INTEGRATION_CONFLICTED.as_str()]
    );
    let again = fx.reviews.find_queue_conflicts(r(2)).await.unwrap();
    assert!(again.is_empty(), "a flagged proposal left the queue");
    drop(review);
}
