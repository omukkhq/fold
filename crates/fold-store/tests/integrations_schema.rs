#![cfg(test)]

use crate::common::{count, exec, open};
use fold_core::object::{ObjectKind, hash_object};
use fold_core::revision::{Revision, StoredRevision};
use fold_core::{IntegrationId, ProposalId, RevisionId, Timestamp};
use fold_store::reviews::{IntegrationSubject, NewIntegration, insert_refused, insert_succeeded};
use fold_store::testing::assert_append_only;
use fold_store::workspaces::{IntegrationKind, create_initial_revision, initial_revision};
use fold_store::{Store, StoreError};

const FIXTURE: &str = r#"
INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'backend', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'backend', 0, 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, stats, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't2', 'm', '{"files_added":1,"files_modified":0,"files_deleted":0}', 'backend', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't2', 1, 'backend', 0);
"#;

fn now() -> Timestamp {
    Timestamp::from_millis(1_700_000_000_000).unwrap()
}

/// A project with `r1` and the open proposal `P-1` with its v1.
async fn seeded() -> (tempfile::TempDir, Store, StoredRevision) {
    let (dir, _paths, store) = open().await;
    let tree = hash_object(ObjectKind::Tree, b"r1");
    let first = StoredRevision::confirmed(initial_revision(tree, now()).unwrap());
    create_initial_revision(&store, &first, None).await.unwrap();
    exec(&store, FIXTURE).await.unwrap();
    (dir, store, first)
}

fn integration(number: u32) -> IntegrationId {
    IntegrationId::from_number(number).unwrap()
}

fn second(first: &StoredRevision, integration: IntegrationId) -> StoredRevision {
    StoredRevision::confirmed(Revision {
        tree: hash_object(ObjectKind::Tree, b"r2"),
        parent: Some(first.hash()),
        number: RevisionId::from_number(2).unwrap(),
        integration: Some(integration),
        created_at: now(),
    })
}

fn proposal_attempt(number: u32) -> NewIntegration<'static> {
    NewIntegration {
        number: integration(number),
        subject: IntegrationSubject::Proposal {
            proposal: ProposalId::from_number(1).unwrap(),
            version: 1,
            remote_commit: None,
            batch: None,
        },
        actor: "review",
        created_at: now(),
    }
}

fn constraint(error: StoreError) -> Option<String> {
    error.constraint()
}

#[tokio::test]
async fn integrations_refuse_update_and_delete() {
    let (_dir, store, _) = seeded().await;
    store
        .write(async |tx| {
            insert_refused(tx, &proposal_attempt(1), r#"{"code":"not_approved"}"#).await
        })
        .await
        .unwrap();

    assert_append_only(&store, "integrations").await;
}

#[tokio::test]
async fn kind_check_refuses_a_pull_with_a_proposal_and_a_proposal_without_one() {
    let (_dir, store, _) = seeded().await;
    let pull_with_proposal = exec(
        &store,
        "INSERT INTO integrations (number, kind, proposal_id, version_id, remote_commit, result, actor, created_at)
         VALUES (1, 'pull', 1, 1, 'abc', 'refused', 'human:ana', 0)",
    )
    .await
    .unwrap_err();
    let proposal_without_one = exec(
        &store,
        "INSERT INTO integrations (number, kind, result, actor, created_at)
         VALUES (2, 'proposal', 'refused', 'review', 0)",
    )
    .await
    .unwrap_err();
    let pull_without_commit = exec(
        &store,
        "INSERT INTO integrations (number, kind, result, actor, created_at)
         VALUES (3, 'pull', 'refused', 'human:ana', 0)",
    )
    .await
    .unwrap_err();

    for error in [
        pull_with_proposal,
        proposal_without_one,
        pull_without_commit,
    ] {
        assert_eq!(
            constraint(error).as_deref(),
            Some("integrations_columns_match_kind")
        );
    }
    assert_eq!(count(&store, "SELECT COUNT(*) FROM integrations").await, 0);
}

#[tokio::test]
async fn a_succeeded_pull_names_its_revision_and_the_revision_names_it() {
    let (_dir, store, first) = seeded().await;
    let attempt = NewIntegration {
        number: integration(4),
        subject: IntegrationSubject::Pull {
            remote_commit: "0123abcd",
        },
        actor: "human:ana",
        created_at: now(),
    };
    let stored = second(&first, integration(4));

    let row = store
        .write(async |tx| insert_succeeded(tx, &attempt, &stored).await)
        .await
        .unwrap();

    assert_eq!(row.number, RevisionId::from_number(2).unwrap());
    let info = row.integration.unwrap();
    assert_eq!(info.number, integration(4));
    assert_eq!(info.kind, IntegrationKind::Pull);
    assert_eq!(info.remote_commit.as_deref(), Some("0123abcd"));
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM integrations i JOIN revisions r ON r.id = i.revision_id
             WHERE i.number = 4 AND r.number = 2 AND r.integration_id = i.id
               AND i.result = 'succeeded' AND i.kind = 'pull' AND i.proposal_id IS NULL",
        )
        .await,
        1
    );
}

#[tokio::test]
async fn a_succeeded_proposal_records_its_proposal_and_version() {
    let (_dir, store, first) = seeded().await;
    let stored = second(&first, integration(1));

    store
        .write(async |tx| insert_succeeded(tx, &proposal_attempt(1), &stored).await)
        .await
        .unwrap();

    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM integrations
             WHERE number = 1 AND kind = 'proposal' AND proposal_id = 1 AND version_id = 1
               AND remote_commit IS NULL AND actor = 'review'",
        )
        .await,
        1
    );
}

#[tokio::test]
async fn a_sync_tasks_proposal_carries_its_remote_commit() {
    let (_dir, store, first) = seeded().await;
    let attempt = NewIntegration {
        subject: IntegrationSubject::Proposal {
            proposal: ProposalId::from_number(1).unwrap(),
            version: 1,
            remote_commit: Some("0123abcd"),
            batch: None,
        },
        ..proposal_attempt(2)
    };
    let stored = second(&first, integration(2));

    store
        .write(async |tx| insert_succeeded(tx, &attempt, &stored).await)
        .await
        .unwrap();

    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM integrations
             WHERE number = 2 AND kind = 'proposal' AND remote_commit = '0123abcd'",
        )
        .await,
        1
    );
}

#[tokio::test]
async fn a_revision_that_names_another_integration_is_refused() {
    let (_dir, store, first) = seeded().await;
    let stored = second(&first, integration(9));

    let error = store
        .write(async |tx| insert_succeeded(tx, &proposal_attempt(1), &stored).await)
        .await
        .unwrap_err();

    assert!(matches!(error, StoreError::Invalid(_)));
    assert_eq!(count(&store, "SELECT COUNT(*) FROM integrations").await, 0);
    assert_eq!(count(&store, "SELECT COUNT(*) FROM revisions").await, 1);
}

#[tokio::test]
async fn a_refused_row_keeps_its_report_and_names_no_revision() {
    let (_dir, store, _) = seeded().await;
    store
        .write(async |tx| {
            insert_refused(tx, &proposal_attempt(2), r#"{"code":"not_approved"}"#).await
        })
        .await
        .unwrap();

    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM integrations
             WHERE number = 2 AND result = 'refused' AND revision_id IS NULL
               AND json_extract(report, '$.code') = 'not_approved'",
        )
        .await,
        1
    );
}
