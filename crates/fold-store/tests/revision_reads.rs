#[cfg(test)]
mod revisions_support;

use fold_core::codes;
use fold_core::revision::Revision;
use fold_core::{ProposalId, TaskId};
use fold_store::testing::TestProject;
use fold_store::workspaces::{self, IntegrationKind};

const PARENTS: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'backend', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'worker-a', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 3, 'three', 'backend', 0, 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-3', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't', 'm', 'backend', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 5, 1, 1, 'Add the thing', 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't', 1, 'frontend', 0);
";

use revisions_support::{append, child, count, exec, hash, id, init, integration};

#[cfg(test)]
mod chain_support {
    use super::*;

    pub async fn chain(project: &TestProject, length: u32) -> Vec<Revision> {
        let mut all = vec![init(project).await];
        for n in 2..=length {
            let next = child(&all[all.len() - 1], n as u8);
            append(project, &next).await.unwrap();
            all.push(next);
        }
        all
    }
}
use chain_support::chain;

#[tokio::test]
async fn reads_return_the_written_rows() {
    let project = TestProject::new().await;
    let all = chain(&project, 4).await;
    let pool = project.store().read();

    let by_number = workspaces::get_by_number(pool, id(3)).await.unwrap();
    assert_eq!(by_number.hash, all[2].hash());
    assert_eq!(by_number.parent, Some(id(2)));
    assert_eq!(by_number.tree_hash, all[2].tree);
    assert_eq!(by_number.created_at, all[2].created_at);

    let by_hash = workspaces::get_by_hash(pool, &all[1].hash()).await.unwrap();
    assert_eq!(
        by_hash,
        Some(workspaces::get_by_number(pool, id(2)).await.unwrap())
    );
    assert_eq!(
        workspaces::get_by_hash(pool, &hash(200)).await.unwrap(),
        None
    );

    let missing = workspaces::get_by_number(pool, id(9)).await.unwrap_err();
    assert_eq!(missing.code(), Some(codes::REVISION_NOT_FOUND));

    let head = workspaces::head(pool).await.unwrap().unwrap();
    assert_eq!(head.number, id(1), "main stays at r1 until it advances");

    let numbers = |rows: Vec<workspaces::RevisionRow>| -> Vec<u32> {
        rows.iter().map(|row| row.number.number()).collect()
    };
    let ancestors = workspaces::ancestors(pool, id(4), 2).await.unwrap();
    assert_eq!(numbers(ancestors), [3, 2]);
    let all_ancestors = workspaces::ancestors(pool, id(4), 50).await.unwrap();
    assert_eq!(numbers(all_ancestors), [3, 2, 1]);
    let unknown = workspaces::ancestors(pool, id(9), 2).await.unwrap_err();
    assert_eq!(unknown.code(), Some(codes::REVISION_NOT_FOUND));
}

#[tokio::test]
async fn lists_page_newest_first_and_oldest_first() {
    let project = TestProject::new().await;
    chain(&project, 5).await;
    let pool = project.store().read();
    let numbers = |rows: Vec<workspaces::RevisionRow>| -> Vec<u32> {
        rows.iter().map(|row| row.number.number()).collect()
    };
    assert_eq!(
        numbers(workspaces::list_recent(pool, 2, None).await.unwrap()),
        [5, 4]
    );
    assert_eq!(
        numbers(workspaces::list_recent(pool, 2, Some(id(4))).await.unwrap()),
        [3, 2]
    );
    assert_eq!(
        numbers(
            workspaces::list_recent(pool, 10, Some(id(2)))
                .await
                .unwrap()
        ),
        [1]
    );
    assert_eq!(
        numbers(workspaces::list_after(pool, id(3)).await.unwrap()),
        [4, 5]
    );
    assert!(
        workspaces::list_after(pool, id(5))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        workspaces::list_recent(pool, 0, None)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_proposal_integration_carries_its_proposal_task_author_and_actor() {
    let project = TestProject::new().await;
    let r1 = init(&project).await;
    exec(project.store(), PARENTS).await.unwrap();
    exec(
        project.store(),
        "INSERT INTO integrations (id, number, kind, proposal_id, version_id, result, actor, created_at) VALUES (40, 7, 'proposal', 1, 1, 'succeeded', 'review-bot', 0)",
    )
    .await
    .unwrap();
    assert_eq!(
        count(project.store(), "SELECT COUNT(*) FROM integrations").await,
        1
    );
    let integrated = Revision {
        integration: integration(7),
        ..child(&r1, 2)
    };
    append(&project, &integrated).await.unwrap();

    let row = workspaces::get_by_number(project.store().read(), id(2))
        .await
        .unwrap();
    let info = row.integration.unwrap();
    assert_eq!(info.number.to_string(), "i-7");
    assert_eq!(info.kind, IntegrationKind::Proposal);
    assert_eq!(info.actor, "review-bot");
    assert_eq!(info.remote_commit, None);
    let proposal = info.proposal.unwrap();
    assert_eq!(proposal.number, ProposalId::from_number(5).unwrap());
    assert_eq!(proposal.title, "Add the thing");
    assert_eq!(proposal.task, TaskId::from_number(3).unwrap());
    assert_eq!(proposal.agent, "backend");
    assert_eq!(proposal.worker, "worker-a");
}

#[tokio::test]
async fn a_pull_integration_carries_its_commit_and_no_proposal() {
    let project = TestProject::new().await;
    let r1 = init(&project).await;
    exec(
        project.store(),
        "INSERT INTO integrations (id, number, kind, remote_commit, result, actor, created_at) VALUES (9, 1, 'pull', 'abc123', 'succeeded', 'human', 0)",
    )
    .await
    .unwrap();
    let pulled = Revision {
        integration: integration(1),
        ..child(&r1, 2)
    };
    append(&project, &pulled).await.unwrap();

    let row = workspaces::get_by_number(project.store().read(), id(2))
        .await
        .unwrap();
    let info = row.integration.unwrap();
    assert_eq!(info.kind, IntegrationKind::Pull);
    assert_eq!(info.actor, "human");
    assert_eq!(info.remote_commit.as_deref(), Some("abc123"));
    assert_eq!(info.proposal, None);
    let first = workspaces::get_by_number(project.store().read(), id(1))
        .await
        .unwrap();
    assert_eq!(first.integration, None);
}
