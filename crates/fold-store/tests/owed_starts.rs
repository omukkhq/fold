use fold_store::agents;

use crate::common::{exec, open};

/// Five stopped reviewers that each approved, or waits to review, one proposal whose unreleased review claim it holds: `rv-conflict` an open proposal whose integration conflicted after the reviewer started, `rv-clean` an open proposal without a conflict, `rv-closed` and `rv-integrated` proposals that conflicted and then left `open`, `rv-review` an open version that came after its start and has no verdict, and `rv-other` the claim on an open conflicted proposal that a human approved, so its conflict is not the reviewer's. Each reviewer started at 10.
#[cfg(test)]
const ROWS: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'author', 0), (2, 'rv-conflict', 0), (3, 'rv-clean', 0), (4, 'rv-closed', 0), (5, 'rv-integrated', 0), (6, 'rv-review', 0), (7, 'rv-other', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'author', 0), (2, 2, 'rv-conflict', 0), (3, 3, 'rv-clean', 0), (4, 4, 'rv-closed', 0), (5, 5, 'rv-integrated', 0), (6, 6, 'rv-review', 0), (7, 7, 'rv-other', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'fold', 0, 0), (2, 2, 'two', 'fold', 0, 0), (3, 3, 'three', 'fold', 0, 0), (4, 4, 'four', 'fold', 0, 0), (5, 5, 'five', 'fold', 0, 0), (6, 6, 'six', 'fold', 0, 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0), (2, 2, 2, 1, 'task', 'ws/t-2', 1, 'active', 0), (3, 3, 3, 1, 'task', 'ws/t-3', 1, 'active', 0), (4, 4, 4, 1, 'task', 'ws/t-4', 1, 'active', 0), (5, 5, 5, 1, 'task', 'ws/t-5', 1, 'active', 0), (6, 6, 6, 1, 'task', 'ws/t-6', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't2', 'm', 'author', 0), (2, 2, 2, 2, 1, 't2', 'm', 'author', 0), (3, 3, 3, 3, 1, 't2', 'm', 'author', 0), (4, 4, 4, 4, 1, 't2', 'm', 'author', 0), (5, 5, 5, 5, 1, 't2', 'm', 'author', 0), (6, 6, 6, 6, 1, 't2', 'm', 'author', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, state, close_reason, conflicted, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 'open', NULL, 1, 0, 0), (2, 2, 2, 1, 'p', 'open', NULL, 0, 0, 0), (3, 3, 3, 1, 'p', 'closed', 'withdrawn', 1, 0, 0), (4, 4, 4, 1, 'p', 'integrated', NULL, 1, 0, 0), (5, 5, 5, 1, 'p', 'open', NULL, 0, 0, 0), (6, 6, 6, 1, 'p', 'open', NULL, 1, 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't2', 1, 'author', 5), (2, 2, 1, 1, 't2', 2, 'author', 5), (3, 3, 1, 1, 't2', 3, 'author', 5), (4, 4, 1, 1, 't2', 4, 'author', 5), (5, 5, 1, 1, 't2', 5, 'author', 30), (6, 6, 1, 1, 't2', 6, 'author', 5);
INSERT INTO reviews (id, version_id, reviewer_worker_id, verdict, body, created_at) VALUES (1, 1, 2, 'approve', 'b', 15), (2, 2, 3, 'approve', 'b', 15), (3, 3, 4, 'approve', 'b', 15), (4, 4, 5, 'approve', 'b', 15);
INSERT INTO reviews (id, version_id, reviewer_human, verdict, body, created_at) VALUES (6, 6, 'ana', 'approve', 'b', 15);
INSERT INTO integrations (id, number, kind, proposal_id, version_id, result, actor, created_at) VALUES (1, 1, 'proposal', 1, 1, 'conflicted', 'fold', 20), (2, 2, 'proposal', 3, 3, 'conflicted', 'fold', 20), (3, 3, 'proposal', 4, 4, 'conflicted', 'fold', 20), (5, 5, 'proposal', 6, 6, 'conflicted', 'fold', 20);
INSERT INTO claims (kind, proposal_id, worker_id, acquired_at, renewed_at) VALUES ('review', 1, 2, 0, 0), ('review', 2, 3, 0, 0), ('review', 3, 4, 0, 0), ('review', 4, 5, 0, 0), ('review', 5, 6, 0, 0), ('review', 6, 7, 0, 0);
INSERT INTO worker_processes (id, agent_id, role, session_id, state, started_by, started_at, stopped_at) VALUES (2, 2, 'reviewer', 's2', 'stopped', 'review-lead', 10, 16), (3, 3, 'reviewer', 's3', 'stopped', 'review-lead', 10, 16), (4, 4, 'reviewer', 's4', 'stopped', 'review-lead', 10, 16), (5, 5, 'reviewer', 's5', 'stopped', 'review-lead', 10, 16), (6, 6, 'reviewer', 's6', 'stopped', 'review-lead', 10, 16), (7, 7, 'reviewer', 's7', 'stopped', 'review-lead', 10, 16);
INSERT INTO worker_proposals (worker_process_id, proposal_id) VALUES (2, 1), (3, 2), (4, 3), (5, 4), (6, 5), (7, 6);
";

#[cfg(test)]
async fn owed(store: &fold_store::Store) -> Vec<(String, Option<String>, i64)> {
    agents::owed_starts(store.read())
        .await
        .unwrap()
        .into_iter()
        .map(|owed| {
            (
                owed.resume.agent.as_str().to_owned(),
                owed.resume.proposal.map(|proposal| proposal.to_string()),
                owed.resume.since.as_millis(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_stopped_approving_reviewer_is_owed_the_conflict_of_its_open_proposal_in_time_order() {
    let (_dir, _paths, store) = open().await;
    for statement in ROWS.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    assert_eq!(
        owed(&store).await,
        [
            ("rv-conflict".to_owned(), Some("P-1".to_owned()), 20),
            ("rv-review".to_owned(), Some("P-5".to_owned()), 30),
        ],
        "only the open conflicted proposal that the reviewer approved is owed, dated by its conflict and ranked with the owed review by time"
    );

    exec(
        &store,
        "UPDATE worker_processes SET state = 'failed', failed_reason = 'session_lost' WHERE id = 2",
    )
    .await
    .unwrap();
    assert_eq!(
        owed(&store).await,
        [("rv-review".to_owned(), Some("P-5".to_owned()), 30)],
        "a reviewer whose session is gone is owed nothing: the review lead was told"
    );

    exec(
        &store,
        "UPDATE worker_processes SET state = 'starting', failed_reason = NULL, started_at = 25, stopped_at = NULL WHERE id = 2",
    )
    .await
    .unwrap();
    exec(
        &store,
        "UPDATE worker_processes SET state = 'stopped', stopped_at = 26 WHERE id = 2",
    )
    .await
    .unwrap();
    assert_eq!(
        owed(&store).await,
        [("rv-review".to_owned(), Some("P-5".to_owned()), 30)],
        "a start after the conflict served it"
    );

    exec(
        &store,
        "INSERT INTO integrations (id, number, kind, proposal_id, version_id, result, actor, created_at) VALUES (4, 4, 'proposal', 1, 1, 'conflicted', 'fold', 27)",
    )
    .await
    .unwrap();
    exec(
        &store,
        "INSERT INTO reviews (id, version_id, reviewer_worker_id, verdict, body, created_at) VALUES (5, 1, 2, 'request_changes', 'b', 28)",
    )
    .await
    .unwrap();
    assert_eq!(
        owed(&store).await,
        [("rv-review".to_owned(), Some("P-5".to_owned()), 30)],
        "a conflict sent back to its author is not the reviewer's"
    );
}
