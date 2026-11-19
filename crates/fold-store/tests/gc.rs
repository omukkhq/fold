#![cfg(test)]

use crate::seed;

use std::collections::HashSet;
use std::time::Duration;

use fold_core::object::ObjectHash;
use fold_core::{AgentName, Role};
use fold_core::{Timestamp, WorkspaceId};
use fold_store::agents::{ended_short_lived_workers, open_session_folders};
use fold_store::testing::{TestProject, execute};
use fold_store::workspaces::{
    ChangesetStats, CheckpointReason, OwnerHold, closed_checkpoints_due, closed_directories,
    closed_directories_due, gc_roots, insert_changeset, insert_checkpoint, mark_directory_removed,
    prune_auto, removed_directories_at, reopen_directory_removal, start_directory_removal,
    tree_recorded, workspaces_over_auto_limit,
};
use fold_store::{Store, StoreError};

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

async fn checkpoint(
    store: &Store,
    workspace: WorkspaceId,
    tree: ObjectHash,
    reason: CheckpointReason,
) {
    store
        .write(async |tx| {
            insert_checkpoint(
                tx,
                workspace,
                tree,
                reason.kind(),
                reason,
                None,
                Timestamp::EPOCH,
            )
            .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn roots_name_every_referenced_hash_once_and_skip_pruned_checkpoints() {
    let project = TestProject::new().await;
    let store = project.store();
    assert!(gc_roots(store.read()).await.unwrap().is_empty());
    let one = seed::workspace(store, 1).await;
    seed::workspace(store, 2).await;
    checkpoint(
        store,
        one.workspace,
        seed::tree("pruned"),
        CheckpointReason::Quiet,
    )
    .await;
    store
        .write(async |tx| {
            prune_auto(tx, one.workspace, 0, Timestamp::EPOCH).await?;
            insert_changeset(
                tx,
                one.workspace,
                Some(one.task),
                one.revision,
                seed::tree("changeset"),
                "record",
                ChangesetStats::default(),
                "frontend",
                Timestamp::EPOCH,
            )
            .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    checkpoint(
        store,
        one.workspace,
        seed::tree("explicit"),
        CheckpointReason::Explicit,
    )
    .await;
    execute(
        store,
        &format!(
            "INSERT INTO agents (id, name, created_at) VALUES (1, 'frontend', 0);
             INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'frontend', 0);
             INSERT INTO proposals (number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 'p', 0, 0);
             INSERT INTO proposal_versions (proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at)
             VALUES (1, 1, 1, '{}', 1, 'frontend', 0);",
            seed::tree("proposal")
        ),
    )
    .await;

    let roots = gc_roots(store.read()).await.unwrap();
    let unique: HashSet<_> = roots.iter().copied().collect();
    assert_eq!(unique.len(), roots.len());
    let expected: HashSet<_> = [
        seed::tree("revision 1"),
        seed::tree("revision 2"),
        seed::tree("revision tree"),
        seed::tree("changeset"),
        seed::tree("explicit"),
        seed::tree("proposal"),
    ]
    .into();
    assert_eq!(unique, expected);
}

#[tokio::test]
async fn only_workspaces_over_the_limit_need_auto_pruning() {
    let project = TestProject::new().await;
    let store = project.store();
    let one = seed::workspace(store, 1).await.workspace;
    let two = seed::workspace(store, 2).await.workspace;
    for n in 0..3 {
        checkpoint(
            store,
            one,
            seed::tree(&format!("one {n}")),
            CheckpointReason::Quiet,
        )
        .await;
    }
    checkpoint(
        store,
        one,
        seed::tree("one explicit"),
        CheckpointReason::Explicit,
    )
    .await;
    checkpoint(store, two, seed::tree("two"), CheckpointReason::BeforeSync).await;
    assert_eq!(
        workspaces_over_auto_limit(store.read(), 2).await.unwrap(),
        vec![one]
    );
    assert_eq!(
        workspaces_over_auto_limit(store.read(), 0).await.unwrap(),
        vec![one, two]
    );
    assert!(
        workspaces_over_auto_limit(store.read(), 3)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn every_closed_workspace_directory_is_due_at_once_until_it_is_removed() {
    let project = TestProject::new().await;
    let store = project.store();
    for n in 1..=5 {
        seed::workspace(store, n).await;
    }
    let now = project.now();
    let ago = |days: u32| now.checked_sub(DAY * days).unwrap().as_millis();
    execute(
        store,
        &format!(
            "UPDATE workspaces SET state = 'archived', closed_at = {} WHERE number = 1;
             UPDATE workspaces SET state = 'archived', kind = 'review', path = 'ws/review-p-1', closed_at = {} WHERE number = 2;
             UPDATE workspaces SET state = 'abandoned', closed_at = {} WHERE number = 3;
             UPDATE workspaces SET closed_at = {} WHERE number = 4;
             UPDATE workspaces SET state = 'missing' WHERE number = 5;",
            now.as_millis(),
            ago(15),
            ago(1),
            ago(30),
        ),
    )
    .await;

    let due = closed_directories_due(store.read(), None, None)
        .await
        .unwrap();
    let ids: Vec<u32> = due.iter().map(|closed| closed.id.number()).collect();
    assert_eq!(ids, vec![1, 2, 3]);
    assert_eq!(due[1].path, "ws/review-p-1");
    assert_eq!(due[1].closed_at.as_millis(), ago(15));
    let only = |n: u32| async move {
        closed_directories_due(
            store.read(),
            Some(WorkspaceId::from_number(n).unwrap()),
            None,
        )
        .await
        .unwrap()
        .len()
    };
    assert_eq!((only(3).await, only(4).await, only(5).await), (1, 0, 0));

    let mark = |workspace| async move {
        store
            .write(async |tx| mark_directory_removed(tx, workspace, now).await)
            .await
            .unwrap()
    };
    let four = WorkspaceId::from_number(4).unwrap();
    let one = WorkspaceId::from_number(1).unwrap();
    let start = |workspace| async move {
        store
            .write(async |tx| start_directory_removal(tx, workspace, now).await)
            .await
            .unwrap()
    };
    assert!(!due[0].removal_started);
    assert!(!start(four).await, "a live workspace does not start");
    assert!(start(one).await);
    assert!(!start(one).await, "the first start stays");
    let due = closed_directories_due(store.read(), Some(one), None)
        .await
        .unwrap();
    assert!(due[0].removal_started);
    assert!(!mark(four).await, "a live workspace is not marked");
    assert!(mark(one).await);
    assert!(!mark(one).await, "the first mark stays");
    let ids: Vec<u32> = closed_directories_due(store.read(), None, None)
        .await
        .unwrap()
        .iter()
        .map(|closed| closed.id.number())
        .collect();
    assert_eq!(ids, vec![2, 3]);
}

#[tokio::test]
async fn a_closed_workspace_waits_while_a_worker_process_of_its_agent_runs() {
    let project = TestProject::new().await;
    let store = project.store();
    for n in 1..=2 {
        seed::workspace(store, n).await;
    }
    execute(
        store,
        "INSERT INTO agents (id, name, created_at) VALUES (1, 'coder', 0), (2, 'other', 0);
         INSERT INTO workers (id, agent_id, name, parent_worker_id, created_at) VALUES (1, 1, 'coder', NULL, 0), (2, 1, 'coder/sub', 1, 0), (3, 2, 'other', NULL, 0);
         INSERT INTO worker_processes (agent_id, role, session_id, state, started_by, started_at) VALUES (1, 'review_lead', 's1', 'working', 'main', 0);
         UPDATE workspaces SET state = 'archived', closed_at = 0, worker_id = 2 WHERE number = 1;
         UPDATE workspaces SET state = 'abandoned', closed_at = 0, worker_id = 3 WHERE number = 2;",
    )
    .await;
    let due = || async {
        closed_directories_due(store.read(), None, None)
            .await
            .unwrap()
            .iter()
            .map(|closed| closed.id.number())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        due().await,
        vec![2],
        "a workspace of a row named `agent/name` waits for its agent's process"
    );
    for state in ["starting", "idle", "stopping"] {
        execute(
            store,
            &format!(
                "UPDATE worker_processes SET state = '{state}', idle_since = CASE WHEN '{state}' = 'idle' THEN 1 END WHERE agent_id = 1"
            ),
        )
        .await;
        assert_eq!(due().await, vec![2], "{state}");
    }
    for state in ["stopped", "failed"] {
        execute(
            store,
            &format!(
                "UPDATE worker_processes SET state = '{state}', idle_since = NULL, stopped_at = 1 WHERE agent_id = 1"
            ),
        )
        .await;
        assert_eq!(due().await, vec![1, 2], "{state}");
        execute(
            store,
            "UPDATE worker_processes SET state = 'stopping', stopped_at = NULL WHERE agent_id = 1",
        )
        .await;
    }
}

#[tokio::test]
async fn a_closed_workspace_waits_while_its_agent_has_an_open_session() {
    let project = TestProject::new().await;
    let store = project.store();
    for n in 1..=2 {
        seed::workspace(store, n).await;
    }
    execute(
        store,
        "INSERT INTO agents (id, name, created_at) VALUES (1, 'main', 0), (2, 'other', 0);
         INSERT INTO workers (id, agent_id, name, parent_worker_id, created_at) VALUES (1, 1, 'main', NULL, 0), (2, 2, 'other', NULL, 0);
         INSERT INTO sessions (id, agent_id, pid, started_at) VALUES (1, 1, 10, 0), (2, NULL, 11, 0);
         UPDATE workspaces SET state = 'archived', closed_at = 0, worker_id = 1 WHERE number = 1;
         UPDATE workspaces SET state = 'abandoned', closed_at = 0, worker_id = 2 WHERE number = 2;",
    )
    .await;
    let due = || async {
        closed_directories_due(store.read(), None, None)
            .await
            .unwrap()
            .iter()
            .map(|closed| closed.id.number())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        due().await,
        vec![2],
        "an agent without a worker process waits for its session; an unbound session holds nothing"
    );
    execute(store, "UPDATE sessions SET ended_at = 1 WHERE id = 1").await;
    assert_eq!(due().await, vec![1, 2]);
}

#[tokio::test]
async fn a_closed_workspace_of_the_supervised_main_is_due_while_it_is_idle_between_turns() {
    let project = TestProject::new().await;
    let store = project.store();
    for n in 1..=2 {
        seed::workspace(store, n).await;
    }
    execute(
        store,
        "INSERT INTO agents (id, name, created_at) VALUES (1, 'main', 0), (2, 'web-lead', 0);
         INSERT INTO workers (id, agent_id, name, parent_worker_id, created_at) VALUES (1, 1, 'main', NULL, 0), (2, 2, 'web-lead', NULL, 0);
         INSERT INTO worker_processes (agent_id, role, session_id, state, started_by, started_at, idle_since) VALUES (1, 'main', 's1', 'working', 'fold', 0, NULL), (2, 'review_lead', 's2', 'idle', 'main', 0, 1);
         INSERT INTO sessions (id, agent_id, pid, started_at) VALUES (1, 1, 10, 0), (2, 2, 11, 0);
         UPDATE workspaces SET state = 'archived', closed_at = 0, worker_id = 1 WHERE number = 1;
         UPDATE workspaces SET state = 'abandoned', closed_at = 0, worker_id = 2 WHERE number = 2;",
    )
    .await;
    let due = || async {
        closed_directories_due(store.read(), None, None)
            .await
            .unwrap()
            .iter()
            .map(|closed| closed.id.number())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        due().await,
        Vec::<u32>::new(),
        "a working main and an idle lead keep their closed workspaces"
    );
    for state in ["starting", "stopping"] {
        execute(
            store,
            &format!("UPDATE worker_processes SET state = '{state}' WHERE agent_id = 1"),
        )
        .await;
        assert_eq!(due().await, Vec::<u32>::new(), "{state}");
    }
    execute(
        store,
        "UPDATE worker_processes SET state = 'idle', idle_since = 2 WHERE agent_id = 1",
    )
    .await;
    assert_eq!(
        due().await,
        vec![1],
        "the idle main and its own adapter's session hold nothing back"
    );
}

#[tokio::test]
async fn checkpoints_of_a_closed_workspace_are_due_once_its_close_and_removal_are_old() {
    let project = TestProject::new().await;
    let store = project.store();
    for n in 1..=5 {
        let seeded = seed::workspace(store, n).await;
        checkpoint(
            store,
            seeded.workspace,
            seed::tree(&format!("checkpoint {n}")),
            CheckpointReason::Explicit,
        )
        .await;
    }
    let now = project.now();
    let ago = |days: u32| now.checked_sub(DAY * days).unwrap().as_millis();
    execute(
        store,
        &format!(
            "UPDATE workspaces SET state = 'archived', closed_at = {old}, directory_removed_at = {old} WHERE number = 1;
             UPDATE workspaces SET state = 'abandoned', closed_at = {old}, directory_removed_at = {recent} WHERE number = 2;
             UPDATE workspaces SET state = 'archived', closed_at = {old} WHERE number = 3;
             UPDATE workspaces SET closed_at = {old}, directory_removed_at = {old} WHERE number = 4;
             UPDATE workspaces SET state = 'archived', closed_at = {old}, directory_removed_at = {old} WHERE number = 5;
             UPDATE checkpoints SET pruned_at = 0 WHERE workspace_id = 5;",
            old = ago(15),
            recent = ago(13),
        ),
    )
    .await;
    let due = |days| async move {
        closed_checkpoints_due(store.read(), now, days)
            .await
            .unwrap()
            .iter()
            .map(|workspace| workspace.number())
            .collect::<Vec<_>>()
    };
    assert_eq!(due(14).await, vec![1]);
    assert_eq!(due(12).await, vec![1, 2]);
    assert!(due(16).await.is_empty());
    assert!(due(u64::MAX).await.is_empty());
}

#[tokio::test]
async fn a_tree_is_recorded_when_a_row_of_the_workspace_or_its_task_holds_it() {
    let project = TestProject::new().await;
    let store = project.store();
    let one = seed::workspace(store, 1).await;
    let two = seed::workspace(store, 2).await;
    checkpoint(
        store,
        one.workspace,
        seed::tree("pruned"),
        CheckpointReason::Quiet,
    )
    .await;
    checkpoint(
        store,
        one.workspace,
        seed::tree("kept"),
        CheckpointReason::Quiet,
    )
    .await;
    checkpoint(
        store,
        one.workspace,
        seed::tree("explicit"),
        CheckpointReason::Explicit,
    )
    .await;
    store
        .write(async |tx| {
            prune_auto(tx, one.workspace, 1, Timestamp::EPOCH).await?;
            insert_changeset(
                tx,
                one.workspace,
                Some(one.task),
                one.revision,
                seed::tree("changeset"),
                "record",
                ChangesetStats::default(),
                "frontend",
                Timestamp::EPOCH,
            )
            .await?;
            Ok::<_, StoreError>(())
        })
        .await
        .unwrap();
    execute(
        store,
        &format!(
            "INSERT INTO agents (id, name, created_at) VALUES (1, 'frontend', 0);
             INSERT INTO workers (id, agent_id, name, created_at) VALUES (1, 1, 'frontend', 0);
             INSERT INTO proposals (number, task_id, author_worker_id, title, created_at, updated_at) VALUES (1, 1, 1, 'p', 0, 0);
             INSERT INTO proposal_versions (proposal_id, version, base_revision_id, tree_hash, head_changeset_id, created_by, created_at)
             VALUES (1, 1, 1, '{}', 1, 'frontend', 0);",
            seed::tree("version")
        ),
    )
    .await;
    let recorded = |workspace, label: &str| {
        let tree = seed::tree(label);
        async move { tree_recorded(store.read(), workspace, &tree).await.unwrap() }
    };
    for label in ["revision tree", "explicit", "changeset", "version"] {
        assert!(recorded(one.workspace, label).await, "{label}");
    }
    // An automatic checkpoint is no record: pruning beyond the kept number can remove it later.
    for label in ["pruned", "kept", "other"] {
        assert!(!recorded(one.workspace, label).await, "{label}");
    }
    assert!(recorded(two.workspace, "revision tree").await);
    for label in ["explicit", "changeset", "version"] {
        assert!(!recorded(two.workspace, label).await, "{label}");
    }
}

#[tokio::test]
async fn a_review_checkout_of_a_lead_that_never_stops_is_due_once_the_window_has_passed() {
    let project = TestProject::new().await;
    let store = project.store();
    for n in 1..=3 {
        seed::workspace(store, n).await;
    }
    let now = project.now();
    let ago = |days: u32| now.checked_sub(DAY * days).unwrap();
    // As fold kept the review checkouts that the review lead itself opened: its process is long-lived and never ends.
    execute(
        store,
        &format!(
            "INSERT INTO agents (id, name, created_at) VALUES (1, 'review-lead', 0), (2, 'main', 0);
             INSERT INTO workers (id, agent_id, name, parent_worker_id, created_at) VALUES (1, 1, 'review-lead', NULL, 0), (2, 2, 'main', NULL, 0);
             INSERT INTO worker_processes (agent_id, role, session_id, state, started_by, started_at) VALUES (1, 'review_lead', 's1', 'working', 'main', 0);
             INSERT INTO sessions (id, agent_id, pid, started_at) VALUES (1, 2, 10, 0);
             UPDATE workspaces SET state = 'archived', kind = 'review', path = '.fold/ws/review-p-4', closed_at = {old}, worker_id = 1 WHERE number = 1;
             UPDATE workspaces SET state = 'archived', kind = 'review', path = '.fold/ws/review-p-9', closed_at = {recent}, worker_id = 1 WHERE number = 2;
             UPDATE workspaces SET state = 'archived', closed_at = {recent}, worker_id = 2 WHERE number = 3;",
            old = ago(29).as_millis(),
            recent = ago(13).as_millis(),
        ),
    )
    .await;
    let cutoff = ago(14);
    let closed = closed_directories(store.read(), None, Some(cutoff))
        .await
        .unwrap();
    let seen: Vec<_> = closed
        .iter()
        .map(|closed| {
            (
                closed.id.number(),
                closed.owner.as_deref(),
                closed.hold,
                closed.past_window,
                closed.due(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        vec![
            (1, Some("review-lead"), Some(OwnerHold::Process), true, true),
            (
                2,
                Some("review-lead"),
                Some(OwnerHold::Process),
                false,
                false
            ),
            (3, Some("main"), Some(OwnerHold::Session), false, false),
        ]
    );
    let due = |cutoff| async move {
        closed_directories_due(store.read(), None, cutoff)
            .await
            .unwrap()
            .iter()
            .map(|closed| closed.id.number())
            .collect::<Vec<_>>()
    };
    assert_eq!(due(Some(cutoff)).await, vec![1]);
    assert_eq!(
        due(None).await,
        Vec::<u32>::new(),
        "without a window the owner rule alone decides"
    );
    assert_eq!(due(Some(ago(12))).await, vec![1, 2, 3]);
}

#[tokio::test]
async fn a_removal_mark_is_found_by_path_and_reopened_from_its_start() {
    let project = TestProject::new().await;
    let store = project.store();
    for n in 1..=3 {
        seed::workspace(store, n).await;
    }
    execute(
        store,
        "UPDATE workspaces SET state = 'archived', path = '.fold/ws/t-517', closed_at = 5, removal_started_at = 6, directory_removed_at = 7 WHERE number = 1;
         UPDATE workspaces SET state = 'archived', path = '.fold/ws/t-2', closed_at = 5 WHERE number = 2;
         UPDATE workspaces SET path = '.fold/ws/t-3', directory_removed_at = 7 WHERE number = 3;",
    )
    .await;
    let paths: Vec<String> = [
        ".fold/ws/t-517",
        ".fold/ws/t-2",
        ".fold/ws/t-3",
        ".fold/ws/other",
    ]
    .map(str::to_owned)
    .to_vec();
    let found = removed_directories_at(store.read(), &paths).await.unwrap();
    assert_eq!(
        found.iter().map(|id| id.number()).collect::<Vec<_>>(),
        vec![1],
        "a closed workspace marked removed; one not removed and a live one are not"
    );
    assert!(
        removed_directories_at(store.read(), &[])
            .await
            .unwrap()
            .is_empty()
    );
    let reopen = |workspace| async move {
        store
            .write(async |tx| reopen_directory_removal(tx, workspace).await)
            .await
            .unwrap()
    };
    assert!(reopen(found[0]).await);
    assert!(!reopen(found[0]).await, "a second reopen finds no mark");
    assert!(
        !reopen(WorkspaceId::from_number(3).unwrap()).await,
        "a live workspace keeps its row"
    );
    let closed = closed_directories(store.read(), Some(found[0]), None)
        .await
        .unwrap();
    assert_eq!(closed.len(), 1);
    assert!(
        !closed[0].removal_started,
        "the removal starts again, with its checkpoint"
    );
}

#[tokio::test]
async fn ended_task_workers_and_reviewers_are_listed_with_what_ties_them_to_work() {
    let project = TestProject::new().await;
    let store = project.store();
    seed::workspace(store, 1).await;
    let now = project.now();
    let old = now.checked_sub(DAY * 20).unwrap().as_millis();
    let recent = now.checked_sub(DAY).unwrap().as_millis();
    execute(
        store,
        &format!(
            "INSERT INTO labels (id, name, color, created_at) VALUES (11, 'ui', 'sage', 0);
             INSERT INTO areas (id, name, label_id, created_by, created_at) VALUES (21, 'web', 11, 'main', 0);
             INSERT INTO agents (id, name, created_at) VALUES (1, 'done', 0), (2, 'claimed', 0), (3, 'talking', 0), (4, 'parked', 0), (5, 'waiting', 0), (6, 'recent', 0), (7, 'lead', 0), (8, 'running', 0);
             INSERT INTO workers (id, agent_id, name, parent_worker_id, created_at) VALUES (1, 1, 'done', NULL, 0), (2, 2, 'claimed', NULL, 0), (3, 2, 'claimed/sub', 2, 0), (4, 3, 'talking', NULL, 0), (5, 4, 'parked', NULL, 0), (6, 5, 'waiting', NULL, 0), (7, 6, 'recent', NULL, 0), (8, 7, 'lead', NULL, 0), (9, 8, 'running', NULL, 0);
             INSERT INTO worker_processes (agent_id, role, session_id, state, started_by, started_at, stopped_at, area_id) VALUES
               (1, 'reviewer', 's1', 'stopped', 'fold', 0, {old}, NULL),
               (2, 'task_worker', 's2', 'failed', 'fold', 0, {old}, 21),
               (3, 'task_worker', 's3', 'stopped', 'fold', 0, {old}, 21),
               (4, 'task_worker', 's4', 'stopped', 'fold', 0, {old}, 21),
               (5, 'reviewer', 's5', 'stopped', 'fold', 0, {old}, NULL),
               (6, 'task_worker', 's6', 'stopped', 'fold', 0, {recent}, 21),
               (7, 'area_lead', 's7', 'stopped', 'fold', 0, {old}, 21),
               (8, 'task_worker', 's8', 'working', 'fold', 0, NULL, 21);
             UPDATE worker_processes SET parked_by = 7, parked_for = 'the go', parked_at = 1 WHERE agent_id = 4;
             UPDATE worker_processes SET wait_reason = 'network', wait_since = 1, wait_tried_at = 1 WHERE agent_id = 5;
             INSERT INTO sessions (id, agent_id, pid, started_at, cwd) VALUES (1, 3, 10, 0, '/srv/demo/.fold/workers/talking/start'), (2, NULL, 11, 0, '/srv/demo'), (3, 1, 12, 0, '/srv/demo');
             UPDATE sessions SET ended_at = 1 WHERE id = 3;
             INSERT INTO claims (kind, task_id, worker_id, acquired_at, renewed_at) VALUES ('implementation', 1, 3, 0, 0);",
        ),
    )
    .await;
    let cutoff = now.checked_sub(DAY * 14).unwrap();
    let ended = ended_short_lived_workers(store.read(), cutoff, None)
        .await
        .unwrap();
    let seen: Vec<_> = ended
        .iter()
        .map(|worker| {
            (
                worker.agent.as_str(),
                worker.role,
                worker.claim,
                worker.session,
                worker.parked,
                worker.waiting,
            )
        })
        .collect();
    assert_eq!(
        seen,
        vec![
            ("claimed", Role::TaskWorker, true, false, false, false),
            ("done", Role::Reviewer, false, false, false, false),
            ("parked", Role::TaskWorker, false, false, true, false),
            ("talking", Role::TaskWorker, false, true, false, false),
            ("waiting", Role::Reviewer, false, false, false, true),
        ],
        "a recent stop, a lead and a running worker are not listed"
    );
    assert_eq!(ended[1].stopped_at.as_millis(), old);
    let done = AgentName::parse("done").unwrap();
    let only = ended_short_lived_workers(store.read(), cutoff, Some(&done))
        .await
        .unwrap();
    assert_eq!(only.len(), 1);
    assert_eq!(
        open_session_folders(store.read()).await.unwrap(),
        vec!["/srv/demo/.fold/workers/talking/start", "/srv/demo"]
    );
}
