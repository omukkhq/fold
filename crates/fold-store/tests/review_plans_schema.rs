use crate::common::{count, exec, open};
use fold_core::reviews::plans::{PlanRelease, PlanState};
use fold_core::{AgentName, ProposalId, ReviewPlanId, TaskId, Timestamp};
use fold_store::reviews::plans;
use fold_store::{Store, StoreError};

/// The review lead, six tasks and an open first version of T-1 and T-2.
const SEED: &str = "
INSERT INTO agents (id, name, created_at) VALUES (1, 'review', 0), (2, 'reviewer', 0);
INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'review', 0), (2, 2, 'reviewer', 0);
INSERT INTO tasks (id, number, title, created_by, created_at, updated_at) VALUES (1, 1, 'one', 'review', 0, 0), (2, 2, 'two', 'review', 0, 0), (3, 3, 'three', 'review', 0, 0), (4, 4, 'four', 'review', 0, 0), (5, 5, 'five', 'review', 0, 0), (6, 6, 'six', 'review', 0, 0);
INSERT INTO revisions (id, number, hash, tree_hash, created_at) VALUES (1, 1, 'h1', 't1', 0);
INSERT INTO workspaces (id, number, task_id, worker_id, kind, path, base_revision_id, state, created_at) VALUES (1, 1, 1, 1, 'task', 'ws/t-1', 1, 'active', 0), (2, 2, 2, 1, 'task', 'ws/t-2', 1, 'active', 0);
INSERT INTO changesets (id, number, workspace_id, task_id, base_revision_id, tree_hash, message, created_by, created_at) VALUES (1, 1, 1, 1, 1, 't2', 'm', 'review', 0), (2, 2, 2, 2, 1, 't3', 'm', 'review', 0);
INSERT INTO proposals (id, number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 1, 'p', 0, 0), (2, 2, 2, 1, 'q', 0, 0);
INSERT INTO proposal_versions (id, proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at) VALUES (1, 1, 1, 1, 't2', 1, 'review', 0), (2, 2, 1, 1, 't3', 2, 'review', 0);
INSERT INTO review_plans (id, number, lead_agent_id, created_at, deadline_at) VALUES (1, 1, 1, 10, 100);
INSERT INTO review_plan_members (id, plan_id, task_id, proposal_id, version_id, added_at, joined_at) VALUES (1, 1, 1, 1, 1, 10, 10);
INSERT INTO review_plan_members (id, plan_id, task_id, added_at) VALUES (2, 1, 2, 10);
";

#[cfg(test)]
async fn seeded() -> (tempfile::TempDir, Store) {
    let (dir, _paths, store) = open().await;
    for statement in SEED.lines().filter(|line| !line.is_empty()) {
        exec(&store, statement).await.unwrap();
    }
    (dir, store)
}

#[cfg(test)]
async fn refused(store: &Store, sql: &str) -> String {
    exec(store, sql).await.unwrap_err().to_string()
}

#[cfg(test)]
fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).unwrap()
}

#[cfg(test)]
fn rp(number: u32) -> ReviewPlanId {
    ReviewPlanId::from_number(number).unwrap()
}

#[cfg(test)]
fn t(number: u32) -> TaskId {
    TaskId::from_number(number).unwrap()
}

#[tokio::test]
async fn a_task_and_a_proposal_are_in_one_live_plan_at_a_time() {
    let (_dir, store) = seeded().await;
    exec(
        &store,
        "INSERT INTO review_plans (id, number, lead_agent_id, created_at, deadline_at) VALUES (2, 2, 1, 10, 100)",
    )
    .await
    .unwrap();
    let again = "INSERT INTO review_plan_members (plan_id, task_id, added_at) VALUES (2, 2, 11)";
    assert!(
        refused(&store, again)
            .await
            .contains("review_plan_members.task_id")
    );
    exec(
        &store,
        "UPDATE review_plan_members SET released_at = 12, release_reason = 'blocker' WHERE id = 2",
    )
    .await
    .unwrap();
    exec(&store, again).await.unwrap();
}

#[tokio::test]
async fn a_plan_holds_at_most_five_live_members() {
    let (_dir, store) = seeded().await;
    for task in 3..=5 {
        exec(
            &store,
            &format!("INSERT INTO review_plan_members (plan_id, task_id, added_at) VALUES (1, {task}, 11)"),
        )
        .await
        .unwrap();
    }
    let sixth = "INSERT INTO review_plan_members (plan_id, task_id, added_at) VALUES (1, 6, 11)";
    assert!(
        refused(&store, sixth)
            .await
            .contains("review_plan_members_at_most_five")
    );
    exec(
        &store,
        "UPDATE review_plan_members SET released_at = 12, release_reason = 'removed' WHERE task_id = 5",
    )
    .await
    .unwrap();
    exec(&store, sixth).await.unwrap();
}

#[tokio::test]
async fn a_member_joins_with_its_own_tasks_proposal_and_changes_only_to_join_or_leave() {
    let (_dir, store) = seeded().await;
    let wrong_task = "UPDATE review_plan_members SET proposal_id = 1, version_id = 1, joined_at = 12 WHERE id = 2";
    assert!(
        refused(&store, wrong_task)
            .await
            .contains("review_plan_members_proposal_of_task")
    );
    let wrong_version = "UPDATE review_plan_members SET proposal_id = 2, version_id = 1, joined_at = 12 WHERE id = 2";
    assert!(
        refused(&store, wrong_version)
            .await
            .contains("review_plan_members_proposal_of_task")
    );
    let half = "UPDATE review_plan_members SET proposal_id = 2, version_id = 2 WHERE id = 2";
    assert!(refused(&store, half).await.contains("CHECK"));
    exec(
        &store,
        "UPDATE review_plan_members SET proposal_id = 2, version_id = 2, joined_at = 12 WHERE id = 2",
    )
    .await
    .unwrap();
    let rejoin = "UPDATE review_plan_members SET joined_at = 13 WHERE id = 2";
    assert!(
        refused(&store, rejoin)
            .await
            .contains("review_plan_members_join_or_release")
    );
    exec(
        &store,
        "UPDATE review_plan_members SET released_at = 14, release_reason = 'started' WHERE id = 2",
    )
    .await
    .unwrap();
    let again = "UPDATE review_plan_members SET released_at = 15 WHERE id = 2";
    assert!(
        refused(&store, again)
            .await
            .contains("review_plan_members_join_or_release")
    );
    let bad_reason =
        "UPDATE review_plan_members SET released_at = 15, release_reason = 'bored' WHERE id = 1";
    assert!(refused(&store, bad_reason).await.contains("CHECK"));
    assert!(
        refused(&store, "DELETE FROM review_plan_members")
            .await
            .contains("review_plan_members keeps every member")
    );
}

#[tokio::test]
async fn a_plan_moves_forward_only_and_takes_awaited_members_only_while_it_waits() {
    let (_dir, store) = seeded().await;
    let early = "INSERT INTO review_plans (number, lead_agent_id, created_at, deadline_at) VALUES (3, 1, 10, 10)";
    assert!(refused(&store, early).await.contains("CHECK"));
    exec(
        &store,
        "UPDATE review_plans SET wait_ended_at = 100 WHERE id = 1",
    )
    .await
    .unwrap();
    let late = "INSERT INTO review_plan_members (plan_id, task_id, added_at) VALUES (1, 3, 101)";
    assert!(
        refused(&store, late)
            .await
            .contains("review_plan_members_open_plan")
    );
    let move_deadline = "UPDATE review_plans SET deadline_at = 200 WHERE id = 1";
    assert!(
        refused(&store, move_deadline)
            .await
            .contains("review_plans_forward_only")
    );
    exec(
        &store,
        "UPDATE review_plans SET state = 'started', started_at = 101, started_agent_id = 2 WHERE id = 1",
    )
    .await
    .unwrap();
    for sql in [
        "UPDATE review_plans SET state = 'ended', ended_at = 102 WHERE id = 1",
        "UPDATE review_plans SET state = 'open', started_at = NULL, started_agent_id = NULL WHERE id = 1",
    ] {
        assert!(
            refused(&store, sql)
                .await
                .contains("review_plans_forward_only")
        );
    }
    let joined = "INSERT INTO review_plan_members (plan_id, task_id, proposal_id, version_id, added_at, joined_at) VALUES (1, 2, 2, 2, 103, 103)";
    assert!(
        refused(&store, joined)
            .await
            .contains("review_plan_members_open_plan")
    );
    assert!(
        refused(&store, "DELETE FROM review_plans")
            .await
            .contains("review_plans keeps every plan")
    );
}

#[tokio::test]
async fn the_plan_commands_read_back_what_they_wrote() {
    let (_dir, _paths, store) = open().await;
    for statement in SEED
        .lines()
        .filter(|line| !line.is_empty() && !line.contains("review_plan"))
    {
        exec(&store, statement).await.unwrap();
    }
    let lead = AgentName::parse("review").unwrap();
    let reviewer = AgentName::parse("reviewer").unwrap();
    let p1 = ProposalId::from_number(1).unwrap();
    let p2 = ProposalId::from_number(2).unwrap();
    store
        .write(async |tx| {
            plans::insert_plan(tx, rp(1), &lead, at(10), at(100)).await?;
            plans::insert_member(tx, rp(1), t(1), Some((p1, 1)), at(10)).await?;
            plans::insert_member(tx, rp(1), t(2), None, at(10)).await?;
            plans::insert_member(tx, rp(1), t(3), None, at(10)).await?;
            assert!(plans::join_member(tx, rp(1), t(2), p2, 1, at(20)).await?);
            assert!(!plans::join_member(tx, rp(1), t(2), p2, 1, at(21)).await?);
            assert!(plans::release_member(tx, rp(1), t(3), PlanRelease::Blocker, at(30)).await?);
            assert!(!plans::release_member(tx, rp(1), t(3), PlanRelease::Blocker, at(31)).await?);
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let read = store.read();
    assert_eq!(
        plans::live_plan_of_task(read, t(2)).await.unwrap(),
        Some(rp(1))
    );
    assert_eq!(plans::live_plan_of_task(read, t(3)).await.unwrap(), None);
    assert_eq!(
        plans::live_plan_of_proposal(read, p2).await.unwrap(),
        Some(rp(1))
    );
    assert_eq!(plans::due_plans(read, at(99)).await.unwrap(), []);
    assert_eq!(plans::due_plans(read, at(100)).await.unwrap(), [rp(1)]);
    let live = plans::live_members(read).await.unwrap();
    assert_eq!(
        live.iter()
            .map(|m| (m.task, m.proposal))
            .collect::<Vec<_>>(),
        [(t(1), Some(p1)), (t(2), Some(p2))]
    );
    let members = plans::members(read, rp(1)).await.unwrap();
    assert_eq!(members.len(), 3);
    assert_eq!(members[1].version, Some(1));
    assert_eq!(members[1].joined_at, Some(at(20)));
    assert_eq!(members[2].release, Some(PlanRelease::Blocker));
    assert!(!members[2].is_live() && !members[2].is_awaited());
    store
        .write(async |tx| {
            assert!(plans::end_wait(tx, rp(1), at(100)).await?);
            assert!(!plans::end_wait(tx, rp(1), at(101)).await?);
            assert!(plans::mark_started(tx, rp(1), &reviewer, None, at(102)).await?);
            assert!(!plans::mark_ended(tx, rp(1), at(103)).await?);
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    let plan = plans::get_plan(store.read(), rp(1)).await.unwrap().unwrap();
    assert_eq!(plan.state, PlanState::Started);
    assert_eq!(plan.lead, lead);
    assert_eq!(plan.reviewer, Some(reviewer));
    assert_eq!(plan.wait_ended_at, Some(at(100)));
    assert_eq!(plan.batch, None);
    assert!(
        plans::list_plans(store.read(), true)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        plans::list_plans(store.read(), false).await.unwrap().len(),
        1
    );
    assert_eq!(
        count(
            &store,
            "SELECT COUNT(*) FROM review_plan_members WHERE released_at IS NULL"
        )
        .await,
        2
    );
}
